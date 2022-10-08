#![allow(clippy::type_complexity)]

pub mod component;
pub mod identity;
pub mod messaging;
pub mod resource;
pub mod spawning;
pub mod time;
pub mod transform;
pub mod variable;
pub mod visibility;

pub use networking_derive::Networked;

use bevy_renet::{
    renet::{
        ClientAuthentication, RenetClient, RenetConnectionConfig, RenetError, RenetServer,
        ServerAuthentication, ServerConfig,
    },
    RenetClientPlugin, RenetServerPlugin,
};
use component::ComponentPlugin;
use resource::ResourcePlugin;
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
    prelude::{
        error, info, warn, App, Commands, EventReader, EventWriter, Local,
        ParallelSystemDescriptorCoercion, Plugin, Res, ResMut, State, SystemLabel,
    },
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
#[non_exhaustive]
enum ClientState {
    Initial,
    Joining(SocketAddr),
    Connected,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub enum ClientEvent {
    Join(SocketAddr),
    Joined,
    JoinFailed,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub enum ServerEvent {
    PlayerConnected(ConnectionId),
    PlayerDisconnected(ConnectionId),
}

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

pub fn create_server(listen_address: SocketAddr, public_address: Option<IpAddr>) -> RenetServer {
    let socket = UdpSocket::bind(listen_address).unwrap();
    let connection_config = RenetConnectionConfig {
        // TODO: Split channels for server and client
        send_channels_config: Channel::channels_config(),
        receive_channels_config: Channel::channels_config(),
        ..Default::default()
    };
    let server_config = ServerConfig::new(
        64,
        PROTOCOL_ID,
        public_address
            .map(|p| SocketAddr::from((p, 0u16)))
            .unwrap_or(listen_address),
        ServerAuthentication::Unsecure,
    );
    let current_time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap();
    RenetServer::new(current_time, server_config, connection_config, socket).unwrap()
}

fn handle_joining_server(
    mut events: EventReader<ClientEvent>,
    mut state: ResMut<State<ClientState>>,
    mut commands: Commands,
) {
    for event in events.iter() {
        if let ClientEvent::Join(address) = event {
            match state.current() {
                ClientState::Joining(_) | ClientState::Connected => {
                    warn!("Client tried to join server while already joined or connected");
                }
                _ => {
                    state.overwrite_set(ClientState::Joining(*address)).unwrap();
                    info!("Joining server {}", address);

                    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
                    let connection_config = RenetConnectionConfig {
                        // TODO: Split channels for server and client
                        send_channels_config: Channel::channels_config(),
                        receive_channels_config: Channel::channels_config(),
                        ..Default::default()
                    };
                    let current_time = SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap();
                    let client_id = current_time.as_millis() as u64;
                    // TODO: use authentication here
                    let auth = ClientAuthentication::Unsecure {
                        protocol_id: PROTOCOL_ID,
                        client_id,
                        server_addr: *address,
                        user_data: None,
                    };
                    let client =
                        RenetClient::new(current_time, socket, client_id, connection_config, auth)
                            .unwrap();
                    commands.insert_resource(client);
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
    let client = match client {
        Some(c) => c,
        None => return,
    };

    match (client.is_connected(), *last_state) {
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
    mut state: ResMut<State<ClientState>>,
    mut network_time: ResMut<ClientNetworkTime>,
) {
    for event in server_infos.iter() {
        state.set(ClientState::Connected).unwrap();
        client_events.send(ClientEvent::Joined);
        let tick_duration = event.message.tick_duration_seconds;
        network_time.server_tick_seconds = Some(tick_duration);
        info!("Joined server tick={}", tick_duration);
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

#[derive(Default)]
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
        if let bevy_renet::renet::ServerEvent::ClientDisconnected(id) = event {
            let connection = ConnectionId(*id);
            if let Some(player) = players.remove(connection) {
                let uuid = player.id.to_string();
                info!(connection = ?connection, id = uuid.as_str(), "Player disconnected");
                server_events.send(ServerEvent::PlayerDisconnected(connection));
            }
        }
    }
}

fn report_errors(mut events: EventReader<RenetError>) {
    for error in events.iter() {
        error!(?error, "Network error");
    }
}

fn client_disconnect_on_exit(
    mut events: EventReader<AppExit>,
    client: Option<ResMut<RenetClient>>,
) {
    if events.iter().last().is_some() {
        if let Some(mut client) = client {
            if client.is_connected() {
                bevy::log::info!("Disconnected");
                client.disconnect();
            }
        }
    }
}

pub fn is_server(app: &App) -> bool {
    app.world.resource::<NetworkManager>().is_server()
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
        match self.role {
            NetworkRole::Server => app.add_plugin(RenetServerPlugin),
            NetworkRole::Client => app.add_plugin(RenetClientPlugin),
        };

        app.insert_resource(NetworkManager { role: self.role })
            .add_plugin(MessagingPlugin)
            .add_network_message::<ClientHello>()
            .add_network_message::<ServerInfo>()
            .add_plugin(TimePlugin)
            .add_plugin(IdentityPlugin)
            .add_plugin(VisibilityPlugin)
            .add_plugin(SpawningPlugin)
            .add_plugin(ComponentPlugin)
            .add_plugin(ResourcePlugin)
            .add_plugin(TransformPlugin)
            .add_system(report_errors);

        if self.role == NetworkRole::Client {
            app.add_state(ClientState::Initial)
                .add_event::<ClientEvent>()
                .add_system(handle_joining_server)
                .add_system(client_joined_server.after(NetworkSystem::ReadNetworkMessages))
                .add_system(client_send_hello)
                .add_system(client_disconnect_on_exit);
        } else {
            app.add_event::<ServerEvent>()
                .init_resource::<Players>()
                .add_system(server_handle_connect.after(NetworkSystem::ReadNetworkMessages))
                .add_system(server_handle_disconnect);
        }
    }
}
