use std::path::PathBuf;

use bevy::prelude::*;
use bevy_egui::*;

use maps::components::TileMap;

#[derive(Default)]
struct MapLoaderState {
    pub maps: Vec<(String, PathBuf)>,
}

#[derive(Default)]
struct UiState {
    pub loader: MapLoaderState,
}

fn map_loader_system(
    egui_context: Res<EguiContext>,
    mut commands: Commands,
    server: Res<AssetServer>,
    tilemaps: Query<Entity, With<TileMap>>,
) {
    egui::Window::new("Load map").show(egui_context.ctx(), |ui| {
        for &map_name in ["DeltaStation2", "BoxStation", "MetaStation"].iter() {
            if ui.button(map_name).clicked() {
                // Delete existing maps
                for entity in tilemaps.iter() {
                    commands.entity(entity).despawn_recursive();
                }
                // Add new map to load
                let handle = server.load(PathBuf::from(format!("maps/{}.dmm", map_name)));
                commands.insert_resource(crate::Map {
                    handle,
                    spawned: false,
                });
            }
        }
    });
}

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugin(EguiPlugin)
            .insert_resource(UiState::default())
            .add_system(map_loader_system);
    }
}
