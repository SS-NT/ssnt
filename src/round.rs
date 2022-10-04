use bevy::{prelude::*, reflect::TypeUuid};
use maps::TileMap;
use networking::{
    identity::EntityCommandsExt,
    is_server,
    messaging::{AppExt, MessageEvent, MessageReceivers, MessageSender},
    resource::AppExt as ResAppExt,
    spawning::{ClientControls, PrefabPath},
    time::ServerNetworkTime,
    transform::NetworkTransform,
    variable::{NetworkVar, ServerVar},
    visibility::NetworkObserver,
    Networked,
};
use serde::{Deserialize, Serialize};

use crate::{
    job::{JobDefinition, SelectedJobs},
    movement::ForcePositionMessage,
};

pub struct RoundPlugin;

impl Plugin for RoundPlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
        app.add_network_message::<StartRoundRequest>()
            .add_networked_resource::<RoundData, RoundDataClient>();
        if is_server(app) {
            app.add_state(RoundState::Loading)
                .insert_resource(RoundData {
                    state: RoundState::Loading.into(),
                    start: None.into(),
                })
                .add_system_set(SystemSet::on_enter(RoundState::Loading).with_system(load_map))
                .add_system_set(SystemSet::on_update(RoundState::Loading).with_system(set_ready))
                .add_system_set(
                    SystemSet::on_update(RoundState::Ready).with_system(handle_start_round_request),
                )
                .add_system_set(
                    SystemSet::on_enter(RoundState::Running)
                        .with_system(spawn_players_roundstart)
                        .with_system(start_round_timer),
                )
                .add_system(update_round_data);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RoundState {
    Loading,
    Ready,
    Running,
    Ended,
}

#[derive(Networked)]
#[client(RoundDataClient)]
struct RoundData {
    #[synced]
    state: NetworkVar<RoundState>,
    /// The server tick the round was started.
    #[synced]
    start: NetworkVar<Option<u32>>,
}

#[derive(Default, TypeUuid)]
#[uuid = "0db42b69-f2bd-4b28-96a2-e8123e51f45a"]
pub struct RoundDataClient {
    state: ServerVar<RoundState>,
    start: ServerVar<Option<u32>>,
}

impl RoundDataClient {
    pub fn state(&self) -> &RoundState {
        &*self.state
    }

    pub fn start(&self) -> Option<u32> {
        *self.start
    }
}

#[derive(Serialize, Deserialize)]
pub struct StartRoundRequest;

fn load_map(mut commands: Commands, server: Res<AssetServer>) {
    let handle = server.load("maps/BoxStation.dmm");
    commands.insert_resource(crate::Map {
        handle,
        spawned: false,
    });
}

// TODO: Make it wait for all potential maps
fn set_ready(query: Query<(), Added<TileMap>>, mut state: ResMut<State<RoundState>>) {
    if !query.is_empty() {
        state.push(RoundState::Ready).unwrap();
    }
}

fn handle_start_round_request(
    mut query: EventReader<MessageEvent<StartRoundRequest>>,
    mut state: ResMut<State<RoundState>>,
) {
    if query.iter().next().is_some() {
        state.push(RoundState::Running).unwrap();
    }
}

fn update_round_data(state: Res<State<RoundState>>, mut round_data: ResMut<RoundData>) {
    if state.is_changed() && &*round_data.state != state.current() {
        *round_data.state = *state.current();
    }
}

fn start_round_timer(mut round_data: ResMut<RoundData>, server_time: Res<ServerNetworkTime>) {
    *round_data.start = Some(server_time.current_tick());
}

fn spawn_players_roundstart(
    selected_jobs: Res<SelectedJobs>,
    job_data: Res<Assets<JobDefinition>>,
    maps: Query<&TileMap>,
    mut controls: ResMut<ClientControls>,
    mut commands: Commands,
    mut sender: MessageSender,
) {
    // TODO: Support multiple maps
    let main_map = maps.single();
    for (connection, job) in selected_jobs.selected(&job_data) {
        let player = crate::create_player(&mut commands.spawn());
        // Get spawn position for job
        let spawn_tile = main_map
            .job_spawn_positions
            .get(&job.id)
            .map(|p| *p.first().unwrap()) // TODO: Use random selection
            .unwrap_or_default();
        let spawn_position = Vec3::new(spawn_tile.x as f32, 1.0, spawn_tile.y as f32);
        // Insert server-only components
        commands
            .entity(player)
            .insert(NetworkObserver {
                range: 1,
                connection,
            })
            .insert(PrefabPath("player".into()))
            .insert(NetworkTransform::default())
            .insert(Transform::from_translation(spawn_position))
            .networked();

        controls.give_control(connection, player);
        // Force client to accept new position (unless they cheat lol)
        sender.send(
            &ForcePositionMessage {
                position: spawn_position,
                rotation: Quat::IDENTITY,
            },
            MessageReceivers::Single(connection),
        );
    }
}
