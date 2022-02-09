use std::{net::SocketAddr, time::Duration, fmt::Display, any::TypeId};

use bevy::{
    prelude::{
        info, warn, App, Component, CoreStage, Entity, EventReader, EventWriter,
        ParallelSystemDescriptorCoercion, Plugin, ResMut, State, SystemLabel, error, Res, Query,
    },
    utils::{HashMap, HashSet}, ecs::system::{EntityCommands, Command, SystemParam},
};
use bevy_networking_turbulence::{
    ConnectionChannelsBuilder, MessageChannelMode, MessageChannelSettings, MessageFlushingStrategy,
    NetworkEvent, NetworkResource, ReliableChannelSettings,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

#[derive(PartialEq, Clone, Copy)]
pub enum NetworkRole {
    Server,
    Client,
}

pub struct NetworkManager {
    pub role: NetworkRole,
}

impl NetworkManager {
    pub fn is_server(&self) -> bool {
        self.role == NetworkRole::Server
    }

    pub fn is_client(&self) -> bool {
        self.role == NetworkRole::Client
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
enum ClientState {
    Initial,
    Joining(SocketAddr),
    JoinFailed,
    Connected,
    Disconnected,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum ClientEvent {
    Join(SocketAddr),
    Joined,
    JoinFailed,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum ServerEvent {
    PlayerConnected(ConnectionId),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ClientHello {
    token: Vec<u8>,
    version: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ServerInfo {}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct EntitySpawnData {
    pub network_id: u32,
    // TODO: Replace with asset path hash?
    pub name: String,
}

fn flush_channels(mut net: ResMut<NetworkResource>) {
    for (_handle, connection) in net.connections.iter_mut() {
        if let Some(channels) = connection.channels() {
            channels.flush::<NetworkMessage>();
        }
    }
}

const NETWORK_MESSAGE_SETTINGS: MessageChannelSettings = MessageChannelSettings {
    channel: 0,
    channel_mode: MessageChannelMode::Reliable {
        reliability_settings: ReliableChannelSettings {
            bandwidth: 4096,
            recv_window_size: 1024,
            send_window_size: 1024,
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

fn setup_channels(mut net: ResMut<NetworkResource>) {
    net.set_channels_builder(|builder: &mut ConnectionChannelsBuilder| {
        builder
            .register::<NetworkMessage>(NETWORK_MESSAGE_SETTINGS)
            .unwrap();
    });
}

/// Assigns packet numbers to types uniquely and allows to lookup the id for a specific type.
/// Used in packet registration, serialization and deserialization.
#[derive(Default)]
struct MessageTypes {
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
enum MessageReceivers {
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
struct OutboundMessage {
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
struct MessageEvent<T> {
    pub message: T,
    pub connection: ConnectionId,
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

fn handle_joining_server(
    mut events: EventReader<ClientEvent>,
    mut network: ResMut<NetworkResource>,
    mut state: ResMut<State<ClientState>>,
) {
    for event in events.iter() {
        if let ClientEvent::Join(address) = event {
            match state.current() {
                ClientState::Joining(_) | ClientState::Connected => {
                    warn!("Client tried to join server while already joined or connected");
                }
                _ => {
                    state.set(ClientState::Joining(*address)).unwrap();
                    info!("Joining server {}", address);
                    network.connect(*address);
                }
            }
        }
    }
}

fn client_send_hello(
    mut network_events: EventReader<NetworkEvent>,
    mut sender: MessageSender,
) {
    for event in network_events.iter() {
        if let NetworkEvent::Connected(_) = event {
            sender
                .send(
                    &ClientHello {
                        token: Vec::new(),
                        version: "TODO".into(),
                    },
                    MessageReceivers::Server,
                );
        }
    }
}

fn client_joined_server(
    mut server_infos: EventReader<MessageEvent<ServerInfo>>,
    mut client_events: EventWriter<ClientEvent>,
    mut state: ResMut<State<ClientState>>,
) {
    for _ in server_infos.iter() {
        state.set(ClientState::Connected).unwrap();
        client_events.send(ClientEvent::Joined);
        info!("Joined server");
    }
}

#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub struct ConnectionId(u32);

impl Display for ConnectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

struct Player {}

#[derive(Default)]
struct Players {
    players: HashMap<ConnectionId, Player>,
}

impl Players {
    fn add(&mut self, connection: ConnectionId) {
        self.players.insert(connection, Player {});
    }
}

fn server_handle_connect(
    mut hello_messages: EventReader<MessageEvent<ClientHello>>,
    mut players: ResMut<Players>,
    mut server_events: EventWriter<ServerEvent>,
    mut sender: MessageSender,
) {
    for event in hello_messages.iter() {
        // TODO: Auth
        info!("New client connected!");
        sender.send(&ServerInfo {}, MessageReceivers::Single(event.connection));
        players.add(event.connection);
        server_events.send(ServerEvent::PlayerConnected(event.connection));
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

/// A numeric id which matches on the server and clients
#[derive(Component, Debug, Copy, Clone, Hash, PartialEq, Eq)]
struct NetworkIdentity(u32);

/// A lookup to match network identities with ECS entity ids.
/// 
/// Entity ids cannot be used over the network as they are an implementation detail and may conflict.
/// To solve this, we create our own counter and map it to the actual entity id.
#[derive(Default)]
struct NetworkIdentities {
    last_id: u32,
    pub identities: HashMap<NetworkIdentity, Entity>,
}

/// Allows connections to observer networked objects in range
#[derive(Component)]
pub struct NetworkObserver {
    pub range: u32,
    pub connection: ConnectionId,
}

/// Stores which connections are observing something
#[derive(Default)]
struct NetworkVisibility {
    observers: HashSet<ConnectionId>,
    new_observers: HashSet<ConnectionId>,
}

impl NetworkVisibility {
    fn add_observer(&mut self, connection: ConnectionId) {
        if self.observers.insert(connection) {
            self.new_observers.insert(connection);
        }
    }

    fn update(&mut self) {
        self.new_observers.clear();
    }
}

/// Stores a mapping between network identities and their observers
#[derive(Default)]
struct NetworkVisibilities {
    visibility: HashMap<NetworkIdentity, NetworkVisibility>,
}

// TODO: Replace with actual visibility system
fn dummy_visibility(mut visibilities: ResMut<NetworkVisibilities>, players: Res<Players>, identities: Query<&NetworkIdentity>) {
    for (_, visibility) in visibilities.visibility.iter_mut() {
        visibility.update();
    }

    for &identity in identities.iter() {
        let visibility = visibilities.visibility.entry(identity).or_default();
        for (&id, _) in players.players.iter() {
            visibility.add_observer(id);
        }
    }
}

struct NetworkCommand {
    entity: Entity,
}

impl Command for NetworkCommand {
    fn write(self, world: &mut bevy::prelude::World) {
        let manager = world.get_resource::<NetworkManager>().expect("Network manager must exist for networked entities");
        if !manager.is_server() {
            error!("Tried to create networked entity {:?} without being the server", self.entity);
        }
        let mut identities = world.get_resource_mut::<NetworkIdentities>().unwrap();
        let id = identities.last_id + 1;

        identities.last_id = id;
        identities.identities.insert(NetworkIdentity(id), self.entity);

        world.entity_mut(self.entity).insert(NetworkIdentity(id));
    }
}

trait EntityCommandsExt {
    fn networked(&mut self);
}

impl EntityCommandsExt for EntityCommands<'_, '_, '_> {
    /// Adds a network identity to this entity
    fn networked(&mut self) {
        let entity = self.id();
        self.commands().add(NetworkCommand { entity });
    }
}

// Temporary struct to label networked objects
// This should be replaced with the scene identifier in a future bevy release
#[derive(Component)]
struct PrefabPath(String);

trait AppExt {
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
            .add_system(packet_reader)
    }
}

#[derive(SystemParam)]
struct MessageSender<'w, 's> {
    outbound_messages: EventWriter<'w, 's, OutboundMessage>,
    types: Res<'w, MessageTypes>,
}

impl<'w, 's> MessageSender<'w, 's> {
    fn send<T>(&mut self, message: &T, receivers: MessageReceivers) where T: 'static + Serialize + Send + Sync {
        let type_id = self.types.types.get(&TypeId::of::<T>()).expect("Tried to send unregistered message type");
        let event = OutboundMessage { type_id: *type_id, content: bincode::serialize(message).expect("Unable to serialize message"), receivers };
        self.outbound_messages.send(event);
    }
}

pub struct NetworkingPlugin {
    pub role: NetworkRole,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, SystemLabel)]
enum NetworkSystem {
    ReadNetworkMessages,
    Visibility,
}

impl Plugin for NetworkingPlugin {
    fn build(&self, app: &mut App) {
        let sub_plugin = bevy_networking_turbulence::NetworkingPlugin {
            message_flushing_strategy: MessageFlushingStrategy::Never,
            ..Default::default()
        };

        app.add_plugin(sub_plugin)
            .insert_resource(NetworkManager { role: self.role })
            .init_resource::<MessageTypes>()
            .init_resource::<NetworkIdentities>()
            .add_event::<IncomingMessage>()
            .add_event::<OutboundMessage>()
            .add_system_to_stage(CoreStage::PostUpdate, flush_channels)
            .add_startup_system(setup_channels)
            .add_system(read_channel.label(NetworkSystem::ReadNetworkMessages))
            .add_network_message::<ClientHello>()
            .add_network_message::<ServerInfo>();

        if self.role == NetworkRole::Client {
            app.add_state(ClientState::Initial)
                .add_event::<ClientEvent>()
                .add_system(handle_joining_server)
                .add_system(client_joined_server.after(NetworkSystem::ReadNetworkMessages))
                .add_system(client_send_hello)
                .add_system(send_outbound_messages_client);
        } else {
            app.add_event::<ServerEvent>()
                .init_resource::<Players>()
                .init_resource::<NetworkVisibilities>()
                .add_system(server_handle_connect.after(NetworkSystem::ReadNetworkMessages))
                .add_system(dummy_visibility.label(NetworkSystem::Visibility))
                .add_system(send_outbound_messages_server);
        }
    }
}
