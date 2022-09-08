#![allow(clippy::type_complexity)]

pub mod component;
pub mod identity;
pub mod messaging;
pub mod spawning;
pub mod time;
pub mod transform;
pub mod visibility;

use bevy_renet::{
    renet::{
        ClientAuthentication, RenetClient, RenetConnectionConfig, RenetError, RenetServer,
        ServerAuthentication, ServerConfig,
    },
    RenetClientPlugin, RenetServerPlugin,
};
use component::ComponentPlugin;
use time::{ClientNetworkTime, ServerNetworkTime, TimePlugin};

use std::{
    fmt::Display,
    net::{SocketAddr, SocketAddrV4, UdpSocket},
    time::SystemTime,
};

use bevy::{
    prelude::{
        error, info, warn, App, Commands, EventReader, EventWriter, Local,
        ParallelSystemDescriptorCoercion, Plugin, Res, ResMut, State, SystemLabel,
    },
    utils::HashMap,
};
use identity::IdentityPlugin;
use messaging::{AppExt, Channel, MessageEvent, MessageReceivers, MessageSender, MessagingPlugin};
use serde::{Deserialize, Serialize};
use spawning::SpawningPlugin;
use transform::TransformPlugin;
use visibility::VisibilityPlugin;

/// A "unique" id for the protocol used by this application
const PROTOCOL_ID: u64 = 859058192;

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
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ClientHello {
    token: Vec<u8>,
    version: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ServerInfo {
    /// How many seconds a server tick takes
    tick_duration_seconds: f32,
}

pub fn create_server(port: u16) -> RenetServer {
    // TODO: Allow listen ip to be specified
    let server_addr = SocketAddrV4::new("127.0.0.1".parse().unwrap(), port);
    let socket = UdpSocket::bind(server_addr).unwrap();
    let connection_config = RenetConnectionConfig {
        // TODO: Split channels for server and client
        send_channels_config: Channel::channels_config(),
        receive_channels_config: Channel::channels_config(),
        ..Default::default()
    };
    let server_config = ServerConfig::new(
        64,
        PROTOCOL_ID,
        server_addr.into(),
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
    sender.send(
        &ClientHello {
            token: Vec::new(),
            version: "TODO".into(),
        },
        MessageReceivers::Server,
    );
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
    network_time: Res<ServerNetworkTime>,
) {
    for event in hello_messages.iter() {
        // TODO: Auth
        info!("New client connected!");
        let server_info = ServerInfo {
            tick_duration_seconds: network_time.tick_in_seconds() as f32,
        };
        sender.send(&server_info, MessageReceivers::Single(event.connection));
        players.add(event.connection);
        server_events.send(ServerEvent::PlayerConnected(event.connection));
    }
}

fn report_errors(mut events: EventReader<RenetError>) {
    for error in events.iter() {
        error!(?error, "Network error");
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
            .add_plugin(TransformPlugin)
            .add_system(report_errors);

        if self.role == NetworkRole::Client {
            app.add_state(ClientState::Initial)
                .add_event::<ClientEvent>()
                .add_system(handle_joining_server)
                .add_system(client_joined_server.after(NetworkSystem::ReadNetworkMessages))
                .add_system(client_send_hello);
        } else {
            app.add_event::<ServerEvent>()
                .init_resource::<Players>()
                .add_system(server_handle_connect.after(NetworkSystem::ReadNetworkMessages));
        }
    }
}
