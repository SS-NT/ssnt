use bevy::{
    prelude::*,
    reflect::TypeUuid,
    utils::{HashMap, Uuid},
};
use maps::TileMap;
use networking::{
    is_client, is_server,
    messaging::{AppExt, MessageEvent, MessageReceivers, MessageSender},
    resource::AppExt as ResAppExt,
    spawning::ClientControls,
    time::ServerNetworkTime,
    variable::{NetworkVar, ServerVar},
    visibility::{NetworkObserver, NetworkObserverBundle},
    Networked, Players,
};
use serde::{Deserialize, Serialize};
use utils::order::*;

use crate::{
    body::{SpawnCreatureOrder, SpawnCreatureResult},
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
            app.add_state::<RoundState>()
                .insert_resource(RoundData {
                    state: RoundState::Loading.into(),
                    start: None.into(),
                })
                .init_resource::<SpawnsInProgress>()
                .add_systems(OnEnter(RoundState::Loading), load_map)
                .add_systems(
                    OnEnter(RoundState::Running),
                    (spawn_players_roundstart, start_round_timer),
                )
                .add_systems(
                    Update,
                    (
                        set_ready.run_if(in_state(RoundState::Loading)),
                        handle_start_round_request.run_if(in_state(RoundState::Ready)),
                        spawn_player_latejoin.run_if(in_state(RoundState::Running)),
                        update_round_data.run_if(state_changed::<RoundState>()),
                        finalise_player_spawn,
                    ),
                );
        }

        let player_scene = app
            .world
            .resource::<AssetServer>()
            .load("creatures/player.scn.ron");
        app.insert_resource(PlayerAssets {
            player_scene,
            player_model: is_client(app)
                .then(|| app.world.resource::<AssetServer>().load("models/human.glb")),
        });
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, States)]
pub enum RoundState {
    #[default]
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
fn set_ready(query: Query<(), Added<TileMap>>, mut state: ResMut<NextState<RoundState>>) {
    if !query.is_empty() {
        state.set(RoundState::Ready);
    }
}

fn handle_start_round_request(
    mut query: EventReader<MessageEvent<StartRoundRequest>>,
    mut state: ResMut<NextState<RoundState>>,
) {
    if query.iter().next().is_some() {
        state.set(RoundState::Running);
    }
}

fn update_round_data(state: Res<State<RoundState>>, mut round_data: ResMut<RoundData>) {
    if state.is_changed() && &*round_data.state != state.get() {
        *round_data.state = *state.get();
    }
}

fn start_round_timer(mut round_data: ResMut<RoundData>, server_time: Res<ServerNetworkTime>) {
    *round_data.start = Some(server_time.current_tick());
}

#[derive(Resource)]
struct PlayerAssets {
    #[allow(dead_code)]
    player_scene: Handle<DynamicScene>,
    #[allow(dead_code)]
    player_model: Option<Handle<Scene>>,
}

#[derive(Resource, Default)]
struct SpawnsInProgress {
    orders: HashMap<OrderId<SpawnCreatureOrder>, Uuid>,
}

fn spawn_players_roundstart(
    selected_jobs: Res<SelectedJobs>,
    job_data: Res<Assets<JobDefinition>>,
    players: Res<Players>,
    mut spawns: ResMut<SpawnsInProgress>,
    mut spawning: Orderer<SpawnCreatureOrder>,
) {
    for (connection, _) in selected_jobs.selected(&job_data) {
        let player = match players.get(connection) {
            Some(p) => p,
            None => continue,
        };

        let spawn_id = spawning.create(SpawnCreatureOrder {
            archetype: "human".into(),
        });

        spawns.orders.insert(spawn_id, player.id);
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
    mut spawns: ResMut<SpawnsInProgress>,
    mut spawning: Orderer<SpawnCreatureOrder>,
) {
    for event in messages.iter() {
        let Some(player) =  players.get(event.connection) else {
            continue;
        };

        if selected_jobs.get(event.connection, &job_data).is_none() {
            continue;
        }

        let spawn_id = spawning.create(SpawnCreatureOrder {
            archetype: "human".into(),
        });

        spawns.orders.insert(spawn_id, player.id);
    }
}

#[allow(clippy::too_many_arguments)]
fn finalise_player_spawn(
    players: Res<Players>,
    maps: Query<&TileMap>,
    selected_jobs: Res<SelectedJobs>,
    job_data: Res<Assets<JobDefinition>>,
    mut spawns: ResMut<SpawnsInProgress>,
    mut results: Results<SpawnCreatureOrder, SpawnCreatureResult>,
    mut controls: ResMut<ClientControls>,
    mut commands: Commands,
    mut sender: MessageSender,
) {
    for result in results.iter() {
        let Some(player_id) = spawns.orders.remove(&result.id) else {
            continue;
        };

        let Some(connection) = players.get_connection(&player_id) else {
            continue;
        };

        let Some(job) = selected_jobs.get(connection, &job_data) else {
            continue;
        };

        // TODO: Support multiple maps
        let main_map = match maps.get_single() {
            Ok(m) => m,
            Err(_) => return,
        };

        let spawn_position = crate::job::get_spawn_position(main_map, job);

        // Add some player specific components
        let player_entity = result.data.root;
        commands.entity(player_entity).insert((
            NetworkObserverBundle {
                observer: NetworkObserver {
                    range: 1,
                    player_id,
                },
                cells: Default::default(),
            },
            Transform::from_translation(spawn_position),
        ));

        controls.give_control(player_id, player_entity);

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
}
