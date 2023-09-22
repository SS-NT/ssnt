#![allow(clippy::type_complexity)]

pub mod component;
pub mod identity;
pub mod messaging;
pub mod resource;
pub mod scene;
pub mod spawning;
pub mod time;
pub mod transform;
pub mod variable;
pub mod visibility;

pub use bevy_renet::renet::transport::{ConnectToken, ServerAuthentication};
pub use networking_derive::Networked;

use bevy_renet::{
    renet::{
        transport::{
            ClientAuthentication, NetcodeClientTransport, NetcodeError, NetcodeServerTransport,
            NetcodeTransportError, ServerConfig,
        },
        ConnectionConfig, RenetClient, RenetServer,
    },
    transport::{NetcodeClientPlugin, NetcodeServerPlugin},
    RenetClientPlugin, RenetServerPlugin,
};
use component::ComponentPlugin;
use resource::ResourcePlugin;
use scene::ScenePlugin;
use time::{ClientNetworkTime, ServerNetworkTime, TimePlugin};

use std::{
    collections::hash_map::DefaultHasher,
    fmt::Display,
    hash::{Hash, Hasher},
    net::{IpAddr, SocketAddr, UdpSocket},
    time::SystemTime,
};

use bevy::{
    app::AppExit,
    ecs::schedule::ScheduleLabel,
    prelude::*,
    utils::{HashMap, Uuid},
};
use identity::IdentityPlugin;
use messaging::{AppExt, Channel, MessageEvent, MessageReceivers, MessageSender, MessagingPlugin};
use serde::{Deserialize, Serialize};
use spawning::SpawningPlugin;
use transform::TransformPlugin;
use visibility::VisibilityPlugin;

/// A "unique" id for the protocol used by this application
const PROTOCOL_ID: u64 = 859058192;

#[derive(PartialEq, Eq, Clone, Copy)]
pub enum NetworkRole {
    Server,
    Client,
}

#[derive(Resource)]
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

#[derive(States, Debug, Clone, Copy, Eq, PartialEq, Hash, Default)]
#[non_exhaustive]
pub enum ClientState {
    #[default]
    Initial,
    Joining,
    Connected,
}

#[derive(Event, Debug, Clone, Eq, PartialEq)]
pub enum ClientEvent {
    Join(TargetServer),
    Joined,
    JoinFailed(String),
    Disconnected(String),
}

/// Specifies the target server to join.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum TargetServer {
    Raw(SocketAddr),
    Token(Box<ConnectToken>),
}

impl Display for TargetServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TargetServer::Raw(socket) => {
                write!(f, "{}", socket)
            }
            // TODO: Can we extract the server address from the token?
            TargetServer::Token(_) => {
                write!(f, "(opaque token)")
            }
        }
    }
}

#[derive(Event, Debug, Clone, Eq, PartialEq, Hash)]
pub enum ClientTask {
    Leave,
}

#[derive(Event, Debug, Clone, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub enum ServerEvent {
    PlayerConnected(ConnectionId),
    PlayerDisconnected(ConnectionId),
}

#[derive(Resource)]
pub struct UserData {
    pub username: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ClientHello {
    token: Vec<u8>,
    version: String,
    // TODO: Put these into the token
    username: String,
    id: Uuid,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ServerInfo {
    /// How many seconds a server tick takes
    tick_duration_seconds: f32,
}

pub fn create_server(
    listen_address: SocketAddr,
    public_address: Option<IpAddr>,
    authentication: ServerAuthentication,
) -> (RenetServer, NetcodeServerTransport) {
    let socket = UdpSocket::bind(listen_address).unwrap();
    let server_config = ServerConfig {
        max_clients: 64,
        protocol_id: PROTOCOL_ID,
        public_addr: public_address
            .map(|p| SocketAddr::from((p, listen_address.port())))
            .unwrap_or(listen_address),
        authentication,
    };
    let current_time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let transport = NetcodeServerTransport::new(current_time, server_config, socket).unwrap();
    let server = RenetServer::new(connection_config());
    (server, transport)
}

fn connection_config() -> ConnectionConfig {
    ConnectionConfig {
        client_channels_config: Channel::channels_config(),
        server_channels_config: Channel::channels_config(),
        ..Default::default()
    }
}

fn handle_joining_server(
    mut events: EventReader<ClientEvent>,
    state: ResMut<State<ClientState>>,
    mut next_state: ResMut<NextState<ClientState>>,
    mut commands: Commands,
) {
    for event in events.iter() {
        if let ClientEvent::Join(target) = event {
            match state.get() {
                ClientState::Joining | ClientState::Connected => {
                    warn!("Client tried to join server while already joined or connected");
                }
                _ => {
                    next_state.set(ClientState::Joining);
                    info!("Joining server {}", target);

                    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
                    let current_time = SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap();
                    let auth = match target {
                        TargetServer::Raw(address) => {
                            let client_id = current_time.as_millis() as u64;
                            ClientAuthentication::Unsecure {
                                protocol_id: PROTOCOL_ID,
                                client_id,
                                server_addr: *address,
                                user_data: None,
                            }
                        }
                        TargetServer::Token(token) => ClientAuthentication::Secure {
                            connect_token: *token.clone(),
                        },
                    };
                    let client = RenetClient::new(connection_config());
                    commands.insert_resource(client);
                    let transport =
                        NetcodeClientTransport::new(current_time, auth, socket).unwrap();
                    commands.insert_resource(transport);
                }
            }
        }
    }
}

fn client_send_hello(
    transport: Res<NetcodeClientTransport>,
    data: Option<Res<UserData>>,
    mut sender: MessageSender,
    mut last_state: Local<bool>,
) {
    match (transport.is_connected(), *last_state) {
        // Connected
        (true, false) => *last_state = true,
        // Disconnected
        (false, true) => {
            *last_state = false;
            return;
        }
        _ => return,
    }

    info!("Connected to server");
    let username = data
        .map(|d| d.username.clone())
        .unwrap_or_else(|| "Beep".to_string());

    // TODO: Replace with actual user id
    let mut hasher = DefaultHasher::default();
    username.hash(&mut hasher);
    let hash = hasher.finish();

    sender.send_to_server(&ClientHello {
        token: Vec::new(),
        version: "TODO".into(),
        username,
        // 128 bits, trust me bro
        id: Uuid::from_u64_pair(hash, hash),
    });
}

fn client_joined_server(
    mut server_infos: EventReader<MessageEvent<ServerInfo>>,
    mut client_events: EventWriter<ClientEvent>,
    mut next_state: ResMut<NextState<ClientState>>,
    mut network_time: ResMut<ClientNetworkTime>,
) {
    for event in server_infos.iter() {
        next_state.set(ClientState::Connected);
        client_events.send(ClientEvent::Joined);
        let tick_duration = event.message.tick_duration_seconds;
        network_time.server_tick_seconds = Some(tick_duration);
        info!("Joined server tick={}", tick_duration);
    }
}

fn client_handle_join_error(
    mut events: EventReader<NetcodeTransportError>,
    mut client_events: EventWriter<ClientEvent>,
    mut next_state: ResMut<NextState<ClientState>>,
    mut commands: Commands,
) {
    let err = events.iter().last().unwrap();
    // For now we return to the menu on any network error while joining
    next_state.set(ClientState::Initial);
    client_events.send(ClientEvent::JoinFailed(err.to_string()));
    commands.remove_resource::<RenetClient>();
}

fn client_handle_disconnect(
    mut events: EventReader<NetcodeTransportError>,
    mut client_events: EventWriter<ClientEvent>,
    mut next_state: ResMut<NextState<ClientState>>,
    mut commands: Commands,
) {
    let reason = match events.iter().last().unwrap() {
        NetcodeTransportError::Netcode(NetcodeError::Disconnected(reason)) => reason.to_string(),
        NetcodeTransportError::IO(err) => err.to_string(),
        _ => return,
    };

    next_state.set(ClientState::Initial);
    client_events.send(ClientEvent::Disconnected(reason));
    commands.remove_resource::<RenetClient>();
}

fn client_handle_tasks(
    mut tasks: EventReader<ClientTask>,
    mut client: Option<ResMut<RenetClient>>,
) {
    for task in tasks.iter() {
        match task {
            ClientTask::Leave => {
                if let Some(client) = client.as_mut() {
                    client.disconnect();
                }
            }
        }
    }
}

#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub struct ConnectionId(u64);

impl Display for ConnectionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub struct Player {
    pub id: Uuid,
    pub username: String,
}

#[derive(Default, Resource)]
pub struct Players {
    players: HashMap<ConnectionId, Player>,
    user_ids: HashMap<Uuid, ConnectionId>,
}

impl Players {
    fn add(&mut self, connection: ConnectionId, message: &ClientHello) {
        self.players.insert(
            connection,
            Player {
                id: message.id,
                username: message.username.clone(),
            },
        );
        self.user_ids.insert(message.id, connection);
    }

    fn remove(&mut self, connection: ConnectionId) -> Option<Player> {
        if let Some(player) = self.players.remove(&connection) {
            self.user_ids.remove(&player.id);
            Some(player)
        } else {
            None
        }
    }

    pub fn players(&self) -> &HashMap<ConnectionId, Player> {
        &self.players
    }

    pub fn get_connection(&self, k: &Uuid) -> Option<ConnectionId> {
        self.user_ids.get(k).copied()
    }

    pub fn get(&self, connection: ConnectionId) -> Option<&Player> {
        self.players.get(&connection)
    }
}

fn server_handle_connect(
    mut hello_messages: EventReader<MessageEvent<ClientHello>>,
    mut players: ResMut<Players>,
    mut server_events: EventWriter<ServerEvent>,
    mut sender: MessageSender,
    network_time: Res<ServerNetworkTime>,
) {
    for event in hello_messages.iter() {
        // TODO: Auth
        let server_info = ServerInfo {
            tick_duration_seconds: network_time.tick_in_seconds() as f32,
        };
        sender.send(&server_info, MessageReceivers::Single(event.connection));
        players.add(event.connection, &event.message);
        server_events.send(ServerEvent::PlayerConnected(event.connection));

        let uuid = event.message.id.to_string();
        info!(connection = ?event.connection, id = uuid.as_str(), "New client connected");
    }
}

fn server_handle_disconnect(
    mut renet_events: EventReader<bevy_renet::renet::ServerEvent>,
    mut players: ResMut<Players>,
    mut server_events: EventWriter<ServerEvent>,
) {
    for event in renet_events.iter() {
        if let bevy_renet::renet::ServerEvent::ClientDisconnected { client_id: id, .. } = event {
            let connection = ConnectionId(*id);
            if let Some(player) = players.remove(connection) {
                let uuid = player.id.to_string();
                info!(connection = ?connection, id = uuid.as_str(), "Player disconnected");
                server_events.send(ServerEvent::PlayerDisconnected(connection));
            }
        }
    }
}

fn report_errors(mut events: EventReader<NetcodeTransportError>) {
    for error in events.iter() {
        error!(?error, "Network error");
    }
}

fn client_disconnect_on_exit(
    mut client: ResMut<RenetClient>,
    transport: ResMut<NetcodeClientTransport>,
) {
    if transport.is_connected() {
        client.disconnect();
    }
}

pub fn is_server(app: &App) -> bool {
    app.world.resource::<NetworkManager>().is_server()
}

pub fn is_client(app: &App) -> bool {
    app.world.resource::<NetworkManager>().is_client()
}

pub fn has_client() -> impl FnMut(Option<Res<RenetClient>>) -> bool {
    resource_exists::<RenetClient>()
}

pub struct NetworkingPlugin {
    pub role: NetworkRole,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, SystemSet)]
pub enum NetworkSet {
    ReadIncoming,
    UpdateTick,
    ServerVisibility,
    ClientSpawn,
    ClientApply,
    ServerWrite,
    SendOutgoing,
    ServerSyncPhysics,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, ScheduleLabel)]
struct NetworkUpdate;

impl Plugin for NetworkingPlugin {
    fn build(&self, app: &mut App) {
        match self.role {
            NetworkRole::Server => app.add_plugins((RenetServerPlugin, NetcodeServerPlugin)),
            NetworkRole::Client => app.add_plugins((RenetClientPlugin, NetcodeClientPlugin)),
        };

        app.insert_resource(NetworkManager { role: self.role })
            .configure_sets(
                PreUpdate,
                (
                    NetworkSet::ReadIncoming,
                    NetworkSet::UpdateTick,
                    NetworkSet::ServerVisibility,
                    NetworkSet::ClientSpawn,
                    NetworkSet::ClientApply,
                )
                    .chain(),
            )
            .configure_sets(
                PostUpdate,
                (
                    NetworkSet::ServerWrite,
                    NetworkSet::SendOutgoing,
                    NetworkSet::ServerSyncPhysics,
                )
                    .chain(),
            )
            .add_plugins(MessagingPlugin)
            .add_network_message::<ClientHello>()
            .add_network_message::<ServerInfo>()
            .add_plugins((
                TimePlugin,
                IdentityPlugin,
                VisibilityPlugin,
                SpawningPlugin,
                ComponentPlugin,
                ResourcePlugin,
                TransformPlugin,
                ScenePlugin,
            ))
            .add_systems(
                Update,
                report_errors.run_if(on_event::<NetcodeTransportError>()),
            );

        if self.role == NetworkRole::Client {
            app.add_state::<ClientState>()
                .add_event::<ClientEvent>()
                .add_event::<ClientTask>()
                .configure_sets(
                    PreUpdate,
                    (
                        NetworkSet::ReadIncoming.run_if(has_client()),
                        NetworkSet::UpdateTick.run_if(has_client()),
                        NetworkSet::ClientSpawn.run_if(has_client()),
                        NetworkSet::ClientApply.run_if(has_client()),
                    ),
                )
                .configure_sets(PostUpdate, (NetworkSet::SendOutgoing.run_if(has_client()),))
                .add_systems(
                    Update,
                    (
                        handle_joining_server,
                        client_joined_server,
                        client_send_hello.run_if(resource_exists::<NetcodeClientTransport>()),
                        (
                            client_handle_join_error.run_if(in_state(ClientState::Joining)),
                            client_handle_disconnect.run_if(in_state(ClientState::Connected)),
                        )
                            .run_if(on_event::<NetcodeTransportError>()),
                        client_handle_tasks.run_if(on_event::<ClientTask>()),
                        client_disconnect_on_exit
                            .run_if(on_event::<AppExit>())
                            .run_if(resource_exists::<NetcodeClientTransport>()),
                    ),
                );
        } else {
            app.add_event::<ServerEvent>()
                .init_resource::<Players>()
                .add_systems(Update, (server_handle_connect, server_handle_disconnect));
        }
    }
}
