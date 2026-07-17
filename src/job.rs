use bevy::{
    asset::{Asset, LoadedFolder},
    platform::collections::HashMap,
    prelude::*,
    reflect::TypePath,
};
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
        app.add_plugins(RonAssetPlugin::<JobDefinition>::new(&["job.ron"]))
            .add_network_message::<SelectJobMessage>()
            .add_systems(Startup, load_assets);
        if is_server(app) {
            app.init_resource::<SelectedJobs>()
                .add_systems(Update, handle_job_selection);
        }
    }
}

#[derive(Asset, Deserialize, TypePath)]
pub struct JobDefinition {
    pub id: String,
    pub name: String,
    pub description: String,
    pub clothing: Vec<String>,
}

#[derive(Resource)]
pub struct JobAssets {
    // Used to keep definitions loaded
    #[allow(dead_code)]
    definitions: Handle<LoadedFolder>,
}

fn load_assets(mut commands: Commands, server: ResMut<AssetServer>) {
    let assets = JobAssets {
        definitions: server.load_folder("jobs"),
    };
    commands.insert_resource(assets);
}

#[derive(Default, Resource)]
pub struct SelectedJobs {
    selected: HashMap<ConnectionId, Handle<JobDefinition>>,
}

impl SelectedJobs {
    pub fn selected<'a>(
        &'a self,
        assets: &'a Assets<JobDefinition>,
    ) -> impl Iterator<Item = (ConnectionId, &'a JobDefinition)> {
        self.selected
            .iter()
            .filter_map(|(&c, handle)| assets.get(handle).map(|j| (c, j)))
    }

    pub fn get<'a>(
        &'a self,
        connection: ConnectionId,
        assets: &'a Assets<JobDefinition>,
    ) -> Option<&'a JobDefinition> {
        self.selected
            .get(&connection)
            .and_then(|handle| assets.get(handle))
    }
}

#[derive(Serialize, Deserialize)]
pub struct SelectJobMessage {
    /// Asset path of the selected job, or `None` to deselect.
    pub job: Option<String>,
}

fn handle_job_selection(
    mut messages: MessageReader<MessageEvent<SelectJobMessage>>,
    players: Res<Players>,
    controlled: Res<ClientControls>,
    mut resource: ResMut<SelectedJobs>,
    asset_server: Res<AssetServer>,
) {
    for event in messages.read() {
        let player = match players.get(event.connection) {
            Some(p) => p,
            None => continue,
        };
        // Only allow job selection if not already a character in the game
        if controlled.controlled_entity(player.id).is_some() {
            return;
        }
        match &event.message.job {
            Some(path) => {
                resource
                    .selected
                    .insert(event.connection, asset_server.load(path));
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
