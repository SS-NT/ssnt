use std::path::PathBuf;

use bevy::prelude::*;
use bevy_egui::*;

use maps::components::TileMap;

fn map_loader_system(
    mut egui_context: ResMut<EguiContext>,
    mut commands: Commands,
    server: Res<AssetServer>,
    tilemaps: Query<Entity, With<TileMap>>,
) {
    egui::Window::new("Load map").show(egui_context.ctx_mut(), |ui| {
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
        app.add_plugin(EguiPlugin).add_system(map_loader_system);
    }
}
