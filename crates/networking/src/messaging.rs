use std::any::TypeId;

use bevy::{
    ecs::system::SystemParam,
    prelude::{
        trace, warn, App, EventReader, EventWriter, IntoSystemDescriptor, Local, Plugin, Res,
        ResMut, Resource, SystemLabel,
    },
    utils::{HashMap, HashSet},
};
use bevy_renet::{
    renet::{
        ChannelConfig, ReliableChannelConfig, RenetClient, RenetServer, UnreliableChannelConfig,
    },
    run_if_client_connected,
};
use bincode::Options;
use bytes::{BufMut, Bytes};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::{ConnectionId, NetworkManager, NetworkSystem, Players};

/// Serialize data once and allow it to be shared in multiple places without reallocating.
pub(crate) fn serialize_once<T: Serialize>(data: &T) -> Bytes {
    let options = bincode::options();
    let mut writer =
        bytes::BytesMut::with_capacity(options.serialized_size(data).unwrap() as usize).writer();
    options.serialize_into(&mut writer, data).unwrap();
    writer.into_inner().freeze()
}

pub(crate) fn deserialize<T>(data: &[u8]) -> bincode::Result<T>
where
    T: for<'a> Deserialize<'a>,
{
    let options = bincode::options();
    options.deserialize(data)
}

/// Assigns packet numbers to types uniquely and allows to lookup the id for a specific type.
/// Used in packet registration, serialization and deserialization.
#[derive(Default, Resource)]
pub struct MessageTypes {
    last_type: u16,
    types: HashMap<TypeId, u16>,
}

impl MessageTypes {
    fn register<T: 'static>(&mut self) -> u16 {
        let type_id = self.last_type + 1;
        self.last_type = type_id;

        self.types.insert(TypeId::of::<T>(), type_id);
        trace!(type_id = ?TypeId::of::<T>(), message_id = type_id, "Registered message type {}", std::any::type_name::<T>());

        type_id
    }
}

enum MessageKind {
    Reliable,
    Unreliable,
}

/// A message received from a peer
struct IncomingMessage {
    connection: ConnectionId,
    type_id: u16,
    content: Bytes,
}

/// Specifies to which peers a message should be sent
pub enum MessageReceivers {
    /// Send to all authenticated players
    AllPlayers,
    /// Send to a list of players
    Set(HashSet<ConnectionId>),
    /// Send to a single player
    Single(ConnectionId),
    /// Send to the server (panics when not on client)
    Server,
}

/// A message that will be sent to a single or multiple peers
pub struct OutboundMessage {
    type_id: u16,
    content: Bytes,
    receivers: MessageReceivers,
    kind: MessageKind,
    priority: i16,
}

/// The actual data being serialized over the network
#[derive(Serialize, Deserialize, Debug, Clone)]
struct NetworkMessage {
    /// The id registered in [`MessageTypes`]
    type_id: u16,
    // TODO: Use serde_bytes for optimization
    /// The serialized content of the message
    content: Bytes,
}

impl From<OutboundMessage> for NetworkMessage {
    fn from(outbound: OutboundMessage) -> Self {
        Self {
            type_id: outbound.type_id,
            content: outbound.content,
        }
    }
}

/// A new-type struct to mark this network message to be sent over an unreliable channel
#[derive(Serialize, Deserialize, Debug, Clone)]
struct UnreliableNetworkMessage(pub NetworkMessage);

// A typed event sent for every received message
#[derive(Clone, Copy)]
pub struct MessageEvent<T> {
    pub message: T,
    pub connection: ConnectionId,
}

// This should be private, but the SystemParam implementation prevents this.
#[derive(Resource)]
pub struct InternalSenderRes {
    sender: flume::Sender<OutboundMessage>,
}

pub trait AppExt {
    fn add_network_message<T>(&mut self) -> &mut Self
    where
        T: 'static + Serialize + DeserializeOwned + Send + Sync;
}

impl AppExt for App {
    /// Registers a message type which can be sent over the network.
    ///
    /// Messages can be read from an [`EventReader<MessageEvent<T>>`] and sent using a [`MessageSender`].
    fn add_network_message<T>(&mut self) -> &mut Self
    where
        T: 'static + Serialize + DeserializeOwned + Send + Sync,
    {
        let mut types = self.world.get_resource_mut::<MessageTypes>().unwrap();
        let type_id = types.register::<T>();

        let packet_reader =
            move |mut raw_events: EventReader<IncomingMessage>,
                  mut events: EventWriter<MessageEvent<T>>| {
                for event in raw_events.iter() {
                    if event.type_id != type_id {
                        continue;
                    }

                    let message: T = match bincode::deserialize(&event.content) {
                        Ok(m) => m,
                        Err(_) => {
                            // TODO: Disconnect after X invalid packets?
                            warn!(
                                "Received malformed packet from connection={} message_id={}",
                                event.connection, event.type_id
                            );
                            continue;
                        }
                    };
                    events.send(MessageEvent {
                        message,
                        connection: event.connection,
                    });
                }
            };

        self.add_event::<MessageEvent<T>>().add_system(
            packet_reader
                .label(NetworkSystem::ReadNetworkMessages)
                .after(MessagingSystem::ReadRaw),
        )
    }
}

#[derive(SystemParam)]
pub struct MessageSender<'w, 's> {
    sender_res: Res<'w, InternalSenderRes>,
    message_sender: Local<'s, Option<flume::Sender<OutboundMessage>>>,
    types: Res<'w, MessageTypes>,
}

impl<'w, 's> MessageSender<'w, 's> {
    pub fn send<T>(&mut self, message: &T, receivers: MessageReceivers)
    where
        T: 'static + Serialize + Send + Sync,
    {
        self.send_internal(message, receivers, MessageKind::Reliable, 0);
    }

    pub fn send_with_priority<T>(&mut self, message: &T, receivers: MessageReceivers, priority: i16)
    where
        T: 'static + Serialize + Send + Sync,
    {
        self.send_internal(message, receivers, MessageKind::Reliable, priority);
    }

    pub fn send_to_server<T>(&mut self, message: &T)
    where
        T: 'static + Serialize + Send + Sync,
    {
        self.send(message, MessageReceivers::Server);
    }

    pub fn send_unreliable<T>(&mut self, message: &T, receivers: MessageReceivers)
    where
        T: 'static + Serialize + Send + Sync,
    {
        self.send_internal(message, receivers, MessageKind::Unreliable, 0);
    }

    fn send_internal<T>(
        &mut self,
        message: &T,
        receivers: MessageReceivers,
        kind: MessageKind,
        priority: i16,
    ) where
        T: 'static + Serialize + Send + Sync,
    {
        let type_id = self
            .types
            .types
            .get(&TypeId::of::<T>())
            .expect("Tried to send unregistered message type");
        let event = OutboundMessage {
            type_id: *type_id,
            content: bincode::serialize(message)
                .expect("Unable to serialize message")
                .into(),
            receivers,
            kind,
            priority,
        };
        self.get_sender().send(event).unwrap();
    }

    fn get_sender(&mut self) -> &mut flume::Sender<OutboundMessage> {
        self.message_sender
            .get_or_insert_with(|| self.sender_res.sender.clone())
    }
}

pub(crate) enum Channel {
    Default,
    DefaultUnreliable,
    Timing,
    Transforms,
}

impl Channel {
    pub fn id(&self) -> u8 {
        match self {
            Self::Default => 0,
            Self::DefaultUnreliable => 1,
            Self::Timing => 2,
            Self::Transforms => 3,
        }
    }

    pub fn channels_config() -> Vec<ChannelConfig> {
        vec![
            ChannelConfig::Reliable(ReliableChannelConfig {
                channel_id: Self::Default.id(),
                message_send_queue_size: 8192,
                message_receive_queue_size: 8192,
                ordered: true,
                ..Default::default()
            }),
            ChannelConfig::Unreliable(UnreliableChannelConfig {
                channel_id: Self::DefaultUnreliable.id(),
                ..Default::default()
            }),
            ChannelConfig::Unreliable(UnreliableChannelConfig {
                channel_id: Self::Timing.id(),
                ..Default::default()
            }),
            ChannelConfig::Unreliable(UnreliableChannelConfig {
                channel_id: Self::Transforms.id(),
                ..Default::default()
            }),
        ]
    }
}

/// Reads from the network channels and sends message events
fn read_channel_server(mut events: EventWriter<IncomingMessage>, mut server: ResMut<RenetServer>) {
    'clients: for client_id in server.clients_id().into_iter() {
        for channel_id in [Channel::Default.id(), Channel::DefaultUnreliable.id()] {
            while let Some(message) = server.receive_message(client_id, channel_id) {
                let message: NetworkMessage = match bincode::deserialize(&message) {
                    Ok(m) => m,
                    Err(_) => {
                        warn!(client_id, "Invalid message from client");
                        continue 'clients;
                    }
                };
                events.send(IncomingMessage {
                    type_id: message.type_id,
                    content: message.content,
                    connection: ConnectionId(client_id),
                });
            }
        }
    }
}

fn read_channel_client(mut events: EventWriter<IncomingMessage>, mut client: ResMut<RenetClient>) {
    for channel_id in [Channel::Default.id(), Channel::DefaultUnreliable.id()] {
        while let Some(message) = client.receive_message(channel_id) {
            let message: NetworkMessage = match bincode::deserialize(&message) {
                Ok(m) => m,
                Err(_) => {
                    warn!("Invalid message from server");
                    continue;
                }
            };
            events.send(IncomingMessage {
                type_id: message.type_id,
                content: message.content,
                // TODO: Client should not have any connection id field for server?
                // Using 0 as a placeholder here
                connection: ConnectionId(0),
            });
        }
    }
}

fn send_outbound_messages_server(
    receiver: &flume::Receiver<OutboundMessage>,
    mut server: ResMut<RenetServer>,
    players: Res<Players>,
    mut message_buffer: Local<Vec<OutboundMessage>>,
) {
    // Read messages from outbound channel
    message_buffer.extend(receiver.try_iter());
    // Sort current messages by priority
    message_buffer.sort_unstable_by(|a, b| b.priority.cmp(&a.priority));

    for outbound in message_buffer.drain(..) {
        let message = NetworkMessage {
            type_id: outbound.type_id,
            content: outbound.content,
        };
        match outbound.receivers {
            MessageReceivers::AllPlayers => {
                send_message_to(
                    &mut server,
                    message,
                    outbound.kind,
                    players.players.iter().map(|(id, _)| id).copied(),
                );
            }
            MessageReceivers::Set(connections) => {
                send_message_to(&mut server, message, outbound.kind, connections.into_iter());
            }
            MessageReceivers::Server => {
                panic!("Trying to send to server from server");
            }
            MessageReceivers::Single(id) => {
                send_message_to(&mut server, message, outbound.kind, std::iter::once(id));
            }
        }
    }

    message_buffer.clear();
}

fn send_message_to(
    server: &mut RenetServer,
    message: NetworkMessage,
    kind: MessageKind,
    receivers: impl Iterator<Item = ConnectionId>,
) {
    let serialized: Bytes = bincode::serialize(&message).unwrap().into();
    let channel = match kind {
        MessageKind::Reliable => Channel::Default,
        MessageKind::Unreliable => Channel::DefaultUnreliable,
    };
    for id in receivers {
        server.send_message(id.0, channel.id(), serialized.clone());
    }
}

fn send_outbound_messages_client(
    receiver: &flume::Receiver<OutboundMessage>,
    mut client: ResMut<RenetClient>,
) {
    for outbound in receiver.try_iter() {
        let channel = match outbound.kind {
            MessageKind::Reliable => Channel::Default,
            MessageKind::Unreliable => Channel::DefaultUnreliable,
        };

        let message: NetworkMessage = outbound.into();
        client.send_message(channel.id(), bincode::serialize(&message).unwrap());
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, SystemLabel)]
pub(crate) enum MessagingSystem {
    ReadRaw,
    SendOutbound,
}

pub(crate) struct MessagingPlugin;

impl Plugin for MessagingPlugin {
    fn build(&self, app: &mut App) {
        let (tx, rx) = flume::unbounded();

        app.init_resource::<MessageTypes>()
            .insert_resource(InternalSenderRes { sender: tx })
            .add_event::<IncomingMessage>();

        if app
            .world
            .get_resource::<NetworkManager>()
            .unwrap()
            .is_client()
        {
            let outbound = move |client: ResMut<RenetClient>| {
                send_outbound_messages_client(&rx, client);
            };
            app.add_system(outbound.with_run_criteria(run_if_client_connected))
                .add_system(
                    read_channel_client
                        .label(MessagingSystem::ReadRaw)
                        .with_run_criteria(run_if_client_connected),
                );
        } else {
            let outbound = move |server: ResMut<RenetServer>,
                                 players: Res<Players>,
                                 buffer: Local<Vec<OutboundMessage>>| {
                send_outbound_messages_server(&rx, server, players, buffer);
            };
            app.add_system(outbound.label(MessagingSystem::SendOutbound))
                .add_system(read_channel_server.label(MessagingSystem::ReadRaw));
        }
    }
}
