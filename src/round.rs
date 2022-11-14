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
    ConnectionId, Networked, Player, Players,
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
            .add_network_message::<RequestJoin>()
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
                .add_system_set(
                    SystemSet::on_update(RoundState::Running).with_system(spawn_player_latejoin),
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

#[derive(Networked, Resource)]
#[networked(client = "RoundDataClient")]
struct RoundData {
    state: NetworkVar<RoundState>,
    /// The server tick the round was started.
    start: NetworkVar<Option<u32>>,
}

#[derive(Default, TypeUuid, Networked, Resource)]
#[uuid = "0db42b69-f2bd-4b28-96a2-e8123e51f45a"]
#[networked(server = "RoundData")]
pub struct RoundDataClient {
    state: ServerVar<RoundState>,
    start: ServerVar<Option<u32>>,
}

impl RoundDataClient {
    pub fn state(&self) -> &RoundState {
        &self.state
    }

    pub fn start(&self) -> Option<u32> {
        *self.start
    }
}

#[derive(Serialize, Deserialize)]
pub struct StartRoundRequest;

fn load_map(mut commands: Commands, server: Res<AssetServer>) {
    // TODO: Make map selection configurable
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

fn spawn_player(
    connection: ConnectionId,
    player: &Player,
    main_map: &TileMap,
    job: &JobDefinition,
    commands: &mut Commands,
    controls: &mut ClientControls,
    sender: &mut MessageSender,
) {
    let player_entity = crate::create_player(&mut commands.spawn_empty());
    // Get spawn position for job
    let spawn_tile = main_map
        .job_spawn_positions
        .get(&job.id)
        .map(|p| *p.first().unwrap()) // TODO: Use random selection
        .unwrap_or_default();
    let spawn_position = Vec3::new(spawn_tile.x as f32, 1.0, spawn_tile.y as f32);
    // Insert server-only components
    commands
        .entity(player_entity)
        .insert((
            NetworkObserver {
                range: 1,
                player_id: player.id,
            },
            PrefabPath("player".into()),
            NetworkTransform::default(),
            Transform::from_translation(spawn_position),
        ))
        .networked();

    controls.give_control(player.id, player_entity);
    // Force client to accept new position (unless they cheat lol)
    sender.send_with_priority(
        &ForcePositionMessage {
            position: spawn_position,
            rotation: Quat::IDENTITY,
        },
        MessageReceivers::Single(connection),
        10,
    );
}

fn spawn_players_roundstart(
    selected_jobs: Res<SelectedJobs>,
    job_data: Res<Assets<JobDefinition>>,
    players: Res<Players>,
    maps: Query<&TileMap>,
    mut controls: ResMut<ClientControls>,
    mut commands: Commands,
    mut sender: MessageSender,
) {
    // TODO: Support multiple maps
    let main_map = maps.single();
    for (connection, job) in selected_jobs.selected(&job_data) {
        let player = match players.get(connection) {
            Some(p) => p,
            None => continue,
        };

        spawn_player(
            connection,
            player,
            main_map,
            job,
            &mut commands,
            controls.as_mut(),
            &mut sender,
        );
    }
}

#[derive(Serialize, Deserialize)]
pub struct RequestJoin;

#[allow(clippy::too_many_arguments)]
fn spawn_player_latejoin(
    mut messages: EventReader<MessageEvent<RequestJoin>>,
    selected_jobs: Res<SelectedJobs>,
    job_data: Res<Assets<JobDefinition>>,
    players: Res<Players>,
    maps: Query<&TileMap>,
    mut controls: ResMut<ClientControls>,
    mut commands: Commands,
    mut sender: MessageSender,
) {
    // TODO: Support multiple maps
    let main_map = match maps.get_single() {
        Ok(m) => m,
        Err(_) => return,
    };

    for event in messages.iter() {
        let player = match players.get(event.connection) {
            Some(p) => p,
            None => continue,
        };

        let job = match selected_jobs.get(event.connection, &job_data) {
            Some(j) => j,
            None => continue,
        };

        spawn_player(
            event.connection,
            player,
            main_map,
            job,
            &mut commands,
            controls.as_mut(),
            &mut sender,
        );
    }
}
