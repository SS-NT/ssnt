pub mod messaging;
pub mod identity;
pub mod visibility;
pub mod spawning;
pub mod transform;
pub mod time;

pub use bevy_networking_turbulence::NetworkResource;
use time::{TimePlugin, ServerNetworkTime, ClientNetworkTime};

use std::{net::SocketAddr, fmt::Display};

use bevy::{
    prelude::{
        info, warn, App, Component, EventReader, EventWriter,
        ParallelSystemDescriptorCoercion, Plugin, ResMut, State, SystemLabel, Res,
    },
    utils::HashMap,
};
use bevy_networking_turbulence::{
    MessageFlushingStrategy,
    NetworkEvent,
};
use identity::IdentityPlugin;
use messaging::{MessageSender, MessageReceivers, MessageEvent, MessagingPlugin, AppExt};
use serde::{Deserialize, Serialize};
use spawning::SpawningPlugin;
use transform::TransformPlugin;
use visibility::VisibilityPlugin;

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
struct ServerInfo {
    /// How many seconds a server tick takes
    tick_duration_seconds: f32,
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
            .add_plugin(MessagingPlugin)
            .add_network_message::<ClientHello>()
            .add_network_message::<ServerInfo>()
            .add_plugin(TimePlugin)
            .add_plugin(IdentityPlugin)
            .add_plugin(VisibilityPlugin)
            .add_plugin(SpawningPlugin)
            .add_plugin(TransformPlugin);

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
