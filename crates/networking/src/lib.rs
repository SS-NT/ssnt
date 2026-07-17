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

pub use bevy_renet::netcode::{ConnectToken, ServerAuthentication};
pub use networking_derive::Networked;

use bevy_renet::{
    netcode::{
        ClientAuthentication, NetcodeClientPlugin, NetcodeClientTransport, NetcodeError,
        NetcodeErrorEvent, NetcodeServerPlugin, NetcodeServerTransport, NetcodeTransportError,
        ServerConfig,
    },
    renet::ConnectionConfig,
    RenetClient, RenetClientPlugin, RenetServer, RenetServerEvent, RenetServerPlugin,
};
use component::ComponentPlugin;
use resource::ResourcePlugin;
use scene::ScenePlugin;
use time::{ClientNetworkTime, ServerNetworkTime, TimePlugin};
use uuid::Uuid;

use std::{
    collections::hash_map::DefaultHasher,
    fmt::Display,
    hash::{Hash, Hasher},
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    time::SystemTime,
};

use bevy::{
    app::AppExit, ecs::schedule::ScheduleLabel, platform::collections::HashMap, prelude::*,
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

#[derive(Message, Debug, Clone, Eq, PartialEq)]
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

#[derive(Message, Debug, Clone, Eq, PartialEq, Hash)]
pub enum ClientTask {
    Leave,
}

#[derive(Message, Debug, Clone, Eq, PartialEq, Hash)]
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
    let current_time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    let server_config = ServerConfig {
        current_time,
        max_clients: 64,
        protocol_id: PROTOCOL_ID,
        public_addresses: vec![match public_address {
            Some(public) => SocketAddr::from((public, listen_address.port())),
            // If listening on 0.0.0.0 allow connections that target localhost
            None if listen_address.ip().is_unspecified() => {
                SocketAddr::from((Ipv4Addr::LOCALHOST, listen_address.port()))
            }
            None => listen_address,
        }],
        authentication,
    };
    let transport = NetcodeServerTransport::new(server_config, socket).unwrap();
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
    mut events: MessageReader<ClientEvent>,
    state: ResMut<State<ClientState>>,
    mut next_state: ResMut<NextState<ClientState>>,
    mut commands: Commands,
) {
    for event in events.read() {
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
    client: Option<Res<RenetClient>>,
    data: Option<Res<UserData>>,
    mut sender: MessageSender,
    mut last_state: Local<bool>,
) {
    let connected = client.as_ref().is_some_and(|c| c.is_connected());
    match (connected, *last_state) {
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
    mut server_infos: MessageReader<MessageEvent<ServerInfo>>,
    mut client_events: MessageWriter<ClientEvent>,
    mut next_state: ResMut<NextState<ClientState>>,
    mut network_time: ResMut<ClientNetworkTime>,
) {
    for event in server_infos.read() {
        next_state.set(ClientState::Connected);
        client_events.write(ClientEvent::Joined);
        let tick_duration = event.message.tick_duration_seconds;
        network_time.server_tick_seconds = Some(tick_duration);
        info!("Joined server tick={}", tick_duration);
    }
}

fn client_handle_join_error(
    error: On<NetcodeErrorEvent>,
    state: Res<State<ClientState>>,
    mut client_events: MessageWriter<ClientEvent>,
    mut next_state: ResMut<NextState<ClientState>>,
    mut commands: Commands,
) {
    if *state.get() != ClientState::Joining {
        return;
    }
    // For now we return to the menu on any network error while joining
    next_state.set(ClientState::Initial);
    client_events.write(ClientEvent::JoinFailed(error.0.to_string()));
    commands.remove_resource::<RenetClient>();
}

fn client_handle_disconnect(
    error: On<NetcodeErrorEvent>,
    state: Res<State<ClientState>>,
    mut client_events: MessageWriter<ClientEvent>,
    mut next_state: ResMut<NextState<ClientState>>,
    mut commands: Commands,
) {
    if *state.get() != ClientState::Connected {
        return;
    }
    let reason = match &error.0 {
        NetcodeTransportError::Netcode(NetcodeError::Disconnected(reason)) => reason.to_string(),
        NetcodeTransportError::IO(err) => err.to_string(),
        _ => return,
    };

    next_state.set(ClientState::Initial);
    client_events.write(ClientEvent::Disconnected(reason));
    commands.remove_resource::<RenetClient>();
}

fn client_handle_tasks(
    mut tasks: MessageReader<ClientTask>,
    mut client: Option<ResMut<RenetClient>>,
) {
    for task in tasks.read() {
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
    mut hello_messages: MessageReader<MessageEvent<ClientHello>>,
    mut players: ResMut<Players>,
    mut server_events: MessageWriter<ServerEvent>,
    mut sender: MessageSender,
    network_time: Res<ServerNetworkTime>,
) {
    for event in hello_messages.read() {
        // TODO: Auth
        let server_info = ServerInfo {
            tick_duration_seconds: network_time.tick_in_seconds() as f32,
        };
        sender.send(&server_info, MessageReceivers::Single(event.connection));
        players.add(event.connection, &event.message);
        server_events.write(ServerEvent::PlayerConnected(event.connection));

        let uuid = event.message.id.to_string();
        info!(connection = ?event.connection, id = uuid.as_str(), "New client connected");
    }
}

fn server_handle_disconnect(
    renet_event: On<RenetServerEvent>,
    mut players: ResMut<Players>,
    mut server_events: MessageWriter<ServerEvent>,
) {
    if let bevy_renet::renet::ServerEvent::ClientDisconnected { client_id: id, .. } = &renet_event.0
    {
        let connection = ConnectionId(*id);
        if let Some(player) = players.remove(connection) {
            let uuid = player.id.to_string();
            info!(connection = ?connection, id = uuid.as_str(), "Player disconnected");
            server_events.write(ServerEvent::PlayerDisconnected(connection));
        }
    }
}

fn report_errors(error: On<NetcodeErrorEvent>) {
    error!(error = ?error.0, "Network error");
}

fn client_disconnect_on_exit(mut client: ResMut<RenetClient>) {
    if client.is_connected() {
        client.disconnect();
    }
}

pub fn is_server(app: &App) -> bool {
    app.world().resource::<NetworkManager>().is_server()
}

pub fn is_client(app: &App) -> bool {
    app.world().resource::<NetworkManager>().is_client()
}

pub fn has_client() -> impl FnMut(Option<Res<RenetClient>>) -> bool {
    resource_exists::<RenetClient>
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
            .add_observer(report_errors);

        if self.role == NetworkRole::Client {
            app.init_state::<ClientState>()
                .add_message::<ClientEvent>()
                .add_message::<ClientTask>()
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
                        client_send_hello.run_if(resource_exists::<NetcodeClientTransport>),
                        client_handle_tasks.run_if(on_message::<ClientTask>),
                        client_disconnect_on_exit
                            .run_if(on_message::<AppExit>)
                            .run_if(resource_exists::<NetcodeClientTransport>),
                    ),
                )
                .add_observer(client_handle_join_error)
                .add_observer(client_handle_disconnect);
        } else {
            app.add_message::<ServerEvent>()
                .init_resource::<Players>()
                .add_systems(Update, server_handle_connect)
                .add_observer(server_handle_disconnect);
        }
    }
}
