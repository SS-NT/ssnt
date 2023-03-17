use bevy::{asset::AssetPathId, prelude::*, reflect::TypeUuid, utils::HashMap};
use bevy_common_assets::ron::RonAssetPlugin;
use maps::TileMap;
use networking::{
    is_server,
    messaging::{AppExt, MessageEvent},
    spawning::ClientControls,
    ConnectionId, Players,
};
use serde::{Deserialize, Serialize};

pub struct JobPlugin;

impl Plugin for JobPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugin(RonAssetPlugin::<JobDefinition>::new(&["job.ron"]))
            .add_network_message::<SelectJobMessage>()
            .add_startup_system(load_assets);
        if is_server(app) {
            app.init_resource::<SelectedJobs>()
                .add_system(handle_job_selection);
        }
    }
}

#[derive(Deserialize, TypeUuid)]
#[uuid = "17e73665-dcec-4791-ad92-a2fb83c82767"]
pub struct JobDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Resource)]
pub struct JobAssets {
    // Used to keep definitions loaded
    #[allow(dead_code)]
    definitions: Vec<Handle<JobDefinition>>,
}

fn load_assets(mut commands: Commands, server: ResMut<AssetServer>) {
    let assets = JobAssets {
        definitions: server
            .load_folder("jobs")
            .expect("assets/jobs is missing")
            .into_iter()
            .map(HandleUntyped::typed)
            .collect(),
    };
    commands.insert_resource(assets);
}

#[derive(Default, Resource)]
pub struct SelectedJobs {
    selected: HashMap<ConnectionId, AssetPathId>,
}

impl SelectedJobs {
    pub fn selected<'a>(
        &'a self,
        assets: &'a Assets<JobDefinition>,
    ) -> impl Iterator<Item = (ConnectionId, &JobDefinition)> {
        self.selected
            .iter()
            .map(|(&c, &asset_id)| (c, assets.get(&assets.get_handle(asset_id))))
            .filter_map(|(c, def)| def.map(|j| (c, j)))
    }

    pub fn get<'a>(
        &'a self,
        connection: ConnectionId,
        assets: &'a Assets<JobDefinition>,
    ) -> Option<&JobDefinition> {
        self.selected
            .get(&connection)
            .and_then(|id| assets.get(&assets.get_handle(*id)))
    }
}

#[derive(Serialize, Deserialize)]
pub struct SelectJobMessage {
    pub job: Option<AssetPathId>,
}

fn handle_job_selection(
    mut messages: EventReader<MessageEvent<SelectJobMessage>>,
    players: Res<Players>,
    controlled: Res<ClientControls>,
    mut resource: ResMut<SelectedJobs>,
) {
    for event in messages.iter() {
        let player = match players.get(event.connection) {
            Some(p) => p,
            None => continue,
        };
        // Only allow job selection if not already a character in the game
        if controlled.controlled_entity(player.id).is_some() {
            return;
        }
        match event.message.job {
            Some(job) => {
                resource.selected.insert(event.connection, job);
            }
            None => {
                resource.selected.remove(&event.connection);
            }
        }
    }
}

pub fn get_spawn_position(map: &TileMap, job: &JobDefinition) -> Vec3 {
    let spawn_tile = map
        .job_spawn_positions
        .get(&job.id)
        .map(|p| *p.first().unwrap()) // TODO: Use random selection
        .unwrap_or_default();
    Vec3::new(spawn_tile.x as f32, 1.0, spawn_tile.y as f32)
}
