use std::{net::SocketAddr, time::Duration};

use bevy::prelude::{info, warn, App, CoreStage, EventReader, EventWriter, Plugin, ResMut, State};
use bevy_networking_turbulence::{
    ConnectionChannelsBuilder, MessageChannelMode, MessageChannelSettings, MessageFlushingStrategy,
    NetworkEvent, NetworkResource, ReliableChannelSettings,
};
use serde::{Deserialize, Serialize};

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

#[derive(Serialize, Deserialize)]
struct ClientHello {
    token: Vec<u8>,
    version: String,
}

#[derive(Serialize, Deserialize)]
struct ServerInfo {
    token: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
enum NetworkControlMessage {
    ClientHello(ClientHello),
    ServerInfo(ServerInfo),
}

fn flush_channels(mut net: ResMut<NetworkResource>) {
    for (_handle, connection) in net.connections.iter_mut() {
        if let Some(channels) = connection.channels() {
            channels.flush::<NetworkControlMessage>();
        }
    }
}

const CONTROL_MESSAGE_SETTINGS: MessageChannelSettings = MessageChannelSettings {
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
            .register::<NetworkControlMessage>(CONTROL_MESSAGE_SETTINGS)
            .unwrap();
    });
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
                    network.connect(*address);
                    info!("Joining server {}", address);
                }
            }
        }
    }
}

fn joined_server(
    mut network_events: EventReader<NetworkEvent>,
    mut client_events: EventWriter<ClientEvent>,
    mut state: ResMut<State<ClientState>>,
) {
    for event in network_events.iter() {
        if let NetworkEvent::Connected(_) = event {
            state.set(ClientState::Connected).unwrap();
            client_events.send(ClientEvent::Joined);
            info!("Joined server");
        }
    }
}

pub struct NetworkingPlugin {
    pub role: NetworkRole,
}

impl Plugin for NetworkingPlugin {
    fn build(&self, app: &mut App) {
        let sub_plugin = bevy_networking_turbulence::NetworkingPlugin {
            message_flushing_strategy: MessageFlushingStrategy::Never,
            ..Default::default()
        };

        app.add_plugin(sub_plugin)
            .insert_resource(NetworkManager { role: self.role })
            .add_system_to_stage(CoreStage::PostUpdate, flush_channels)
            .add_startup_system(setup_channels);

        if self.role == NetworkRole::Client {
            app.add_state(ClientState::Initial)
                .add_event::<ClientEvent>()
                .add_system(handle_joining_server)
                .add_system(joined_server);
        }
    }
}
