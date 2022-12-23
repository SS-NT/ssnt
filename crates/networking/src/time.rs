use std::collections::VecDeque;

use bevy::{
    app::ScheduleRunnerSettings,
    prelude::{
        warn, App, IntoSystemDescriptor, Plugin, Res, ResMut, Resource, SystemLabel, SystemSet,
        Time,
    },
    utils::HashMap,
};
use bevy_renet::{
    renet::{RenetClient, RenetServer},
    run_if_client_connected,
};
use serde::{Deserialize, Serialize};

use crate::{messaging::Channel, ConnectionId, NetworkManager, Players};

/// Timing data of the server.
#[derive(Resource)]
pub struct ServerNetworkTime {
    /// How many seconds a server tick lasts
    server_tick_seconds: f64,
    /// The current server tick
    server_tick: u32,
}

impl ServerNetworkTime {
    pub fn current_tick(&self) -> u32 {
        self.server_tick
    }

    pub fn tick_in_seconds(&self) -> f64 {
        self.server_tick_seconds
    }
}

#[derive(Default)]
struct ServerClientTime {
    last_rtt: Option<u32>,
    last_ping: f32,
}

#[derive(Default, Resource)]
struct ClientTimes {
    timings: HashMap<ConnectionId, ServerClientTime>,
}

/// How many rtt entries to consider when averaging
const RTT_AVERAGE_COUNT: usize = 10;
/// The maximum time a client can run behind the known server tick
const MAX_TICK_OFFSET_SECONDS: f32 = 0.3;

/// A tick sent from the server to the client
struct ReceivedServerTick {
    /// What time was the tick received on the client
    time: f32,
    /// The server tick
    tick: u32,
}

#[derive(Resource)]
pub(crate) struct ClientNetworkTime {
    /// How many seconds a server tick lasts
    pub server_tick_seconds: Option<f32>,
    /// The last received server tick
    server_tick: Option<ReceivedServerTick>,
    /// The last round-trip-times received
    rtts: VecDeque<u32>,
    /// The server tick the client bases the local state interpolation on.
    /// This is an interpolated value that tries to approach the simulation offset.
    interpolated_tick: f32,
    /// How many ticks in the past (relative to server) the client should run.
    /// This allows lerping between received state updates accurately,
    /// at the cost of delay to where the entites are on the server.
    target_tick_offset: u32,
    /// The current simulation speed multiplier.
    /// This is modified to bring the `interpolated_tick` closer to the target tick offset.
    tick_speed: f32,
}

impl Default for ClientNetworkTime {
    fn default() -> Self {
        Self {
            server_tick_seconds: Default::default(),
            server_tick: Default::default(),
            rtts: VecDeque::with_capacity(RTT_AVERAGE_COUNT),
            interpolated_tick: 0.0,
            target_tick_offset: 0,
            tick_speed: 1.0,
        }
    }
}

impl ClientNetworkTime {
    pub fn interpolated_tick(&self) -> f32 {
        self.interpolated_tick
    }

    fn push_rtt(&mut self, rtt: u32) {
        if self.rtts.len() >= RTT_AVERAGE_COUNT {
            self.rtts.pop_front();
        }

        self.rtts.push_back(rtt);

        self.target_tick_offset = self.calculate_tick_offset();
    }

    /// The average round-trip-time for a packet in server ticks
    fn average_rtt(&self) -> Option<f32> {
        let len = self.rtts.len();
        if len == 0 {
            return None;
        }

        Some(self.rtts.iter().sum::<u32>() as f32 / len as f32)
    }

    /// The estimated server tick at the current time
    fn estimated_server_tick(&self, current_time: f32) -> Option<f32> {
        let tick_rate = self.server_tick_seconds?;
        let last_tick = self.server_tick.as_ref()?;
        let rtt = self.average_rtt()?;

        // Calculate when the last tick was most likely recorded at the server
        let tick_time_server = last_tick.time - rtt * tick_rate / 2.0;
        // Calculate how many ticks have passed since then
        // TODO: Figure out why the tick rate needs to be multiplied
        let ticks_since = (current_time - tick_time_server) / (tick_rate * 2.0);

        Some(last_tick.tick as f32 + ticks_since)
    }

    fn calculate_tick_offset(&self) -> u32 {
        // We target running behind by how many ticks a packet takes in one direction
        // plus a fixed tick amount
        // TODO: Why does it need such a big fixed offset ???
        let mut offset = (self.average_rtt().unwrap().ceil() / 2.0).ceil() as u32 + 4;

        // Limit the offset by the maximum time we are allowed to lag behind
        if let Some(seconds) = self.server_tick_seconds {
            if offset as f32 * seconds > MAX_TICK_OFFSET_SECONDS {
                offset = (MAX_TICK_OFFSET_SECONDS / seconds).floor() as u32;
            }
        }

        offset
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct ServerTick {
    /// The current server tick (at the time of sending from the server)
    tick: u32,
    /// The round-trip-time of the last server to client to server packet
    rtt: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) enum TimeMessage {
    ServerTick(ServerTick),
    ClientResponse { server_tick: u32 },
}

/// How many seconds between each ping interval
const PING_INTERVAL: f32 = 0.2;

fn update_server_tick(mut network_time: ResMut<ServerNetworkTime>) {
    network_time.server_tick += 1;
}

fn send_server_tick(
    mut server: ResMut<RenetServer>,
    mut client_times: ResMut<ClientTimes>,
    time: Res<Time>,
    network_time: Res<ServerNetworkTime>,
    players: Res<Players>,
) {
    let seconds = time.raw_elapsed_seconds();
    let tick = network_time.server_tick;

    for (connection, _) in players.players.iter() {
        let timing = client_times.timings.get(connection);
        if let Some(ServerClientTime { last_ping, .. }) = timing {
            if *last_ping + PING_INTERVAL > seconds {
                continue;
            }
        }

        let rtt = timing.and_then(|t| t.last_rtt);
        let message = TimeMessage::ServerTick(ServerTick { tick, rtt });
        server.send_message(
            connection.0,
            Channel::Timing.id(),
            bincode::serialize(&message).unwrap(),
        );
        // TODO: Can we send this message immediately?

        client_times
            .timings
            .entry(*connection)
            .or_default()
            .last_ping = seconds;
    }
}

fn receive_server_tick(
    mut client: ResMut<RenetClient>,
    mut network_time: ResMut<ClientNetworkTime>,
    time: Res<Time>,
) {
    while let Some(message) = client.receive_message(Channel::Timing.id()) {
        let message = match bincode::deserialize(&message) {
            Ok(m) => m,
            Err(_) => {
                warn!("Invalid time message from server");
                continue;
            }
        };

        if let TimeMessage::ServerTick(tick) = message {
            // Ignore if last received tick is higher
            if let Some(previous_tick) = &network_time.server_tick {
                if previous_tick.tick > tick.tick {
                    continue;
                }
            }

            // Send response as fast as possible
            client.send_message(
                Channel::Timing.id(),
                bincode::serialize(&TimeMessage::ClientResponse {
                    server_tick: tick.tick,
                })
                .unwrap(),
            );
            // TODO: Can we send this message immediately?

            let received_tick = ReceivedServerTick {
                tick: tick.tick,
                time: time.raw_elapsed_seconds(),
            };
            network_time.server_tick = Some(received_tick);

            if let Some(rtt) = tick.rtt {
                network_time.push_rtt(rtt);
            }
        }
    }
}

fn server_handle_response(
    mut server: ResMut<RenetServer>,
    network_time: Res<ServerNetworkTime>,
    mut client_times: ResMut<ClientTimes>,
) {
    'clients: for client_id in server.clients_id().into_iter() {
        while let Some(message) = server.receive_message(client_id, Channel::Timing.id()) {
            let message: TimeMessage = match bincode::deserialize(&message) {
                Ok(m) => m,
                Err(_) => {
                    warn!(client_id, "Invalid time message from client");
                    continue 'clients;
                }
            };

            if let TimeMessage::ClientResponse { server_tick } = message {
                let rtt = network_time.server_tick - server_tick;
                if let Some(timing) = client_times.timings.get_mut(&ConnectionId(client_id)) {
                    timing.last_rtt = Some(rtt);
                }
            }
        }
    }
}

fn update_interpolated_tick(mut network_time: ResMut<ClientNetworkTime>, time: Res<Time>) {
    let server_tick = match network_time.estimated_server_tick(time.raw_elapsed_seconds()) {
        Some(t) => t,
        None => return,
    };

    let target = server_tick - network_time.target_tick_offset as f32;
    let current = network_time.interpolated_tick;
    // If we haven't interpolated yet, snap to the target
    if current <= f32::EPSILON {
        network_time.interpolated_tick = target;
    }

    // Modify the tick speed to get closer to the target tick
    let mut speed = network_time.tick_speed;
    let target_speed = 1.0 + (target - current) / 5.0;
    if target_speed < 1.008 && target_speed > 0.992 {
        speed = 1.0;
    } else {
        speed = (speed * (1.0 - 0.1)) + (target_speed * 0.1);
    }

    network_time.interpolated_tick += speed;
    network_time.tick_speed = speed;
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, SystemLabel)]
pub(crate) enum TimeSystem {
    Tick,
    Interpolate,
}

pub(crate) struct TimePlugin;

impl Plugin for TimePlugin {
    fn build(&self, app: &mut App) {
        let is_server = app
            .world
            .get_resource::<NetworkManager>()
            .unwrap()
            .is_server();

        if is_server {
            let runner_settings = app.world.get_resource::<ScheduleRunnerSettings>().unwrap();
            let tick = if let bevy::app::RunMode::Loop { wait } = runner_settings.run_mode {
                wait
            } else {
                panic!()
            };
            app.insert_resource(ServerNetworkTime {
                server_tick: 0,
                server_tick_seconds: tick.unwrap().as_secs_f64(),
            })
            .init_resource::<ClientTimes>()
            .add_system(update_server_tick.label(TimeSystem::Tick))
            .add_system_set(
                SystemSet::new()
                    .after(TimeSystem::Tick)
                    .with_system(send_server_tick)
                    .with_system(server_handle_response),
            );
        } else {
            app.init_resource::<ClientNetworkTime>()
                .add_system(
                    receive_server_tick
                        .label(TimeSystem::Tick)
                        .with_run_criteria(run_if_client_connected),
                )
                .add_system(
                    update_interpolated_tick
                        .label(TimeSystem::Interpolate)
                        .after(TimeSystem::Tick),
                );
        }
    }
}
