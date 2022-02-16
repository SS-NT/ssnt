use std::{any::TypeId, time::Duration};

use bevy::{utils::{HashMap, HashSet}, prelude::{App, EventReader, EventWriter, warn, Res, ResMut, Plugin, ParallelSystemDescriptorCoercion, CoreStage, SystemLabel}, ecs::system::SystemParam};
use bevy_networking_turbulence::{MessageChannelSettings, MessageChannelMode, ReliableChannelSettings, NetworkResource, ConnectionChannelsBuilder};
use serde::{Serialize, Deserialize, de::DeserializeOwned};

use crate::{ConnectionId, Players, NetworkSystem, NetworkManager, transform::TransformMessage};


/// Assigns packet numbers to types uniquely and allows to lookup the id for a specific type.
/// Used in packet registration, serialization and deserialization.
#[derive(Default)]
pub struct MessageTypes {
    last_type: u16,
    types: HashMap<TypeId, u16>,
}

impl MessageTypes {
    fn register<T: 'static>(&mut self) -> u16 {
        let type_id = self.last_type + 1;
        self.last_type = type_id;

        self.types.insert(TypeId::of::<T>(), type_id);

        type_id
    }
}

/// A message received from a peer
struct IncomingMessage {
    connection: ConnectionId,
    type_id: u16,
    content: Vec<u8>,
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
    content: Vec<u8>,
    receivers: MessageReceivers,
}

/// The actual data being serialized over the network
#[derive(Serialize, Deserialize, Debug, Clone)]
struct NetworkMessage {
    /// The id registered in [`MessageTypes`]
    type_id: u16,
    /// The serialized content of the message
    content: Vec<u8>,
}

impl From<&OutboundMessage> for NetworkMessage {
    fn from(outbound: &OutboundMessage) -> Self {
        Self {
            type_id: outbound.type_id,
            content: outbound.content.clone(),
        }
    }
}

// A typed event sent for every received message
pub struct MessageEvent<T> {
    pub message: T,
    pub connection: ConnectionId,
}

pub trait AppExt {
    fn add_network_message<T>(&mut self) -> &mut Self where T: 'static + Serialize + DeserializeOwned + Send + Sync;
}

impl AppExt for App {
    /// Registers a message type which can be sent over the network.
    /// 
    /// Messages can be read from an [`EventReader<MessageEvent<T>>`] and sent using a [`MessageSender`].
    fn add_network_message<T>(&mut self) -> &mut Self where T: 'static + Serialize + DeserializeOwned + Send + Sync {
        let mut types = self.world.get_resource_mut::<MessageTypes>().unwrap();
        let type_id = types.register::<T>();

        let packet_reader = move |mut raw_events: EventReader<IncomingMessage>, mut events: EventWriter<MessageEvent<T>>| {
            for event in raw_events.iter() {
                if event.type_id != type_id {
                    continue;
                }

                let message: T = match bincode::deserialize(&event.content) {
                    Ok(m) => m,
                    Err(_) => {
                        // TODO: Disconnect after X invalid packets?
                        warn!("Received malformed packet from connection={} message_id={}", event.connection, event.type_id);
                        continue;
                    },
                };
                events.send(MessageEvent { message, connection: event.connection});
            }
        };

        self.add_event::<MessageEvent<T>>()
            .add_system(packet_reader.label(NetworkSystem::ReadNetworkMessages).after(MessagingSystem::ReadRaw))
    }
}

#[derive(SystemParam)]
pub struct MessageSender<'w, 's> {
    outbound_messages: EventWriter<'w, 's, OutboundMessage>,
    types: Res<'w, MessageTypes>,
}

impl<'w, 's> MessageSender<'w, 's> {
    pub fn send<T>(&mut self, message: &T, receivers: MessageReceivers) where T: 'static + Serialize + Send + Sync {
        let type_id = self.types.types.get(&TypeId::of::<T>()).expect("Tried to send unregistered message type");
        let event = OutboundMessage { type_id: *type_id, content: bincode::serialize(message).expect("Unable to serialize message"), receivers };
        self.outbound_messages.send(event);
    }

    pub fn send_to_server<T>(&mut self, message: &T) where T: 'static + Serialize + Send + Sync {
        self.send(message, MessageReceivers::Server);
    }
}

const NETWORK_MESSAGE_SETTINGS: MessageChannelSettings = MessageChannelSettings {
    channel: 0,
    channel_mode: MessageChannelMode::Reliable {
        reliability_settings: ReliableChannelSettings {
            bandwidth: 4096,
            recv_window_size: 2048,
            send_window_size: 2048,
            burst_bandwidth: 1024,
            init_send: 512,
            wakeup_time: Duration::from_millis(100),
            initial_rtt: Duration::from_millis(200),
            max_rtt: Duration::from_secs(2),
            rtt_update_factor: 0.1,
            rtt_resend_factor: 1.5,
        },
        max_message_len: 5000,
    },
    message_buffer_size: 10,
    packet_buffer_size: 10,
};

const TRANSFORM_MESSAGE_SETTINGS: MessageChannelSettings = MessageChannelSettings {
    channel: 1,
    channel_mode: MessageChannelMode::Unreliable,
    message_buffer_size: 100,
    packet_buffer_size: 100,
};

fn setup_channels(mut net: ResMut<NetworkResource>) {
    net.set_channels_builder(|builder: &mut ConnectionChannelsBuilder| {
        builder
            .register::<NetworkMessage>(NETWORK_MESSAGE_SETTINGS)
            .unwrap();
        builder
            .register::<TransformMessage>(TRANSFORM_MESSAGE_SETTINGS)
            .unwrap();
    });
}

/// Reads from the network channels and sends message events
fn read_channel(
    mut events: EventWriter<IncomingMessage>,
    mut network: ResMut<NetworkResource>,
)
{
    for (handle, connection) in network.connections.iter_mut() {
        let channels = connection.channels().unwrap();
        while let Some(message) = channels.recv::<NetworkMessage>() {
            events.send(IncomingMessage {
                type_id: message.type_id,
                content: message.content,
                connection: ConnectionId(*handle),
            });
        }
    }
}

fn flush_channels(mut net: ResMut<NetworkResource>) {
    for (_handle, connection) in net.connections.iter_mut() {
        if let Some(channels) = connection.channels() {
            channels.flush::<NetworkMessage>();
            channels.flush::<TransformMessage>();
        }
    }
}

// NOTE: This message sending method is inefficient, as it needs to clone for every receiver.
//       It should be made more efficient if the networking crate is updated or is switched for something else.
fn send_outbound_messages_server(mut messages: EventReader<OutboundMessage>, mut network: ResMut<NetworkResource>, players: Res<Players>) {
    for outbound in messages.iter() {
        let message: NetworkMessage = outbound.into();
        match &outbound.receivers {
            MessageReceivers::AllPlayers => {
                for (&id, _) in players.players.iter() {
                    network.send_message(id.0, message.clone()).unwrap();
                }
            },
            MessageReceivers::Set(connections) => {
                for &id in connections.iter() {
                    network.send_message(id.0, message.clone()).unwrap();
                }
            },
            MessageReceivers::Server => {
                if network.connections.len() != 1 {
                    panic!("Trying to send message to server while having multiple connections");
                }
                network.broadcast_message(message);
            },
            MessageReceivers::Single(id) => {
                network.send_message(id.0, message).unwrap();
            },
        }
    }
}

fn send_outbound_messages_client(mut messages: EventReader<OutboundMessage>, mut network: ResMut<NetworkResource>) {
    for outbound in messages.iter() {
        let message: NetworkMessage = outbound.into();

        if network.connections.len() != 1 {
            panic!("Trying to send message to server while having multiple connections");
        }
        network.broadcast_message(message);
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, SystemLabel)]
enum MessagingSystem {
    ReadRaw,
}

pub(crate) struct MessagingPlugin;

impl Plugin for MessagingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MessageTypes>()
            .add_event::<IncomingMessage>()
            .add_event::<OutboundMessage>()
            .add_startup_system(setup_channels)
            .add_system(read_channel.label(MessagingSystem::ReadRaw))
            .add_system_to_stage(CoreStage::PostUpdate, flush_channels);

        if app.world.get_resource::<NetworkManager>().unwrap().is_client() {
            app.add_system(send_outbound_messages_client);
        } else {
            app.add_system(send_outbound_messages_server);
        }
    }
}
