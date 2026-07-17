use bevy::{asset::LoadedFolder, math::Vec3, prelude::*, scene::ScenePatch, window::PrimaryWindow};
use bevy_egui::{egui, EguiContexts};
use bevy_rapier3d::plugin::ReadRapierContext;
use networking::{
    is_server,
    messaging::{AppExt, MessageEvent, MessageSender},
    scene::NetworkSceneBundle,
};
use serde::{Deserialize, Serialize};

use crate::{
    camera::MainCamera,
    interaction::InteractionSystem,
    items::{Item, ItemAssets},
    ui::has_window,
    GameState,
};

struct ItemData {
    name: String,
    /// Asset path of the item scene.
    id: String,
}

#[derive(Resource, Default)]
struct SpawnerUiState {
    all_items: Vec<ItemData>,
    to_spawn: Option<String>,
    loaded: bool,
}

fn spawning_ui(mut contexts: EguiContexts, mut state: ResMut<SpawnerUiState>) {
    let state = state.as_mut();
    egui::Window::new("Spawning").show(contexts.ctx_mut().unwrap(), |ui| {
        ui.selectable_value(&mut state.to_spawn, None, "None");
        for data in state.all_items.iter() {
            ui.selectable_value(&mut state.to_spawn, Some(data.id.clone()), &data.name);
        }
    });
}

fn prepare_item_ui_data(
    assets: Res<ItemAssets>,
    folders: Res<Assets<LoadedFolder>>,
    patches: Res<Assets<ScenePatch>>,
    type_registry: Res<AppTypeRegistry>,
    mut ui_data: ResMut<SpawnerUiState>,
) {
    if ui_data.loaded {
        return;
    }
    let Some(folder) = folders.get(&assets.definitions) else {
        return;
    };

    let mut items = Vec::with_capacity(folder.handles.len());
    for handle in &folder.handles {
        let Some(path) = handle
            .path()
            .map(|p| p.path().to_string_lossy().into_owned())
        else {
            continue;
        };
        let Ok(id) = handle.id().try_typed::<ScenePatch>() else {
            continue;
        };
        // Bail out until every item scene has loaded and resolved, then retry next frame.
        let Some(patch) = patches.get(id) else {
            return;
        };
        let Some(resolved) = &patch.resolved else {
            return;
        };
        // Spawn the scene into a throwaway world to read the item's display name.
        let mut scratch = World::new();
        scratch.insert_resource(type_registry.clone());
        let name = resolved
            .spawn(&mut scratch)
            .ok()
            .and_then(|entity| entity.get::<Item>().map(|item| item.name.clone()))
            .unwrap_or_else(|| {
                std::path::Path::new(&path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone())
            });
        items.push(ItemData { name, id: path });
    }

    ui_data.all_items = items;
    ui_data.loaded = true;
}

#[derive(Serialize, Deserialize, Clone)]
enum SpawnerMessage {
    Request((Vec3, String)),
}

#[allow(clippy::too_many_arguments)]
fn spawn_requesting(
    ui_state: Res<SpawnerUiState>,
    mut buttons: ResMut<ButtonInput<MouseButton>>,
    mut contexts: EguiContexts,
    rapier_context: ReadRapierContext,
    windows: Query<(Entity, &Window), With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    mut sender: MessageSender,
) {
    if ui_state.to_spawn.is_none() {
        return;
    }

    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }

    let Ok((window_entity, window)) = windows.single() else {
        return;
    };

    if contexts
        .ctx_for_entity_mut(window_entity)
        .is_ok_and(|c| c.wants_pointer_input())
    {
        return;
    }

    // Consume the click
    buttons.clear_just_pressed(MouseButton::Left);

    let (camera, camera_transform) = match cameras.iter().next() {
        Some(o) => o,
        None => return,
    };
    let cursor_position = match window.cursor_position() {
        Some(p) => p,
        None => return,
    };

    let (origin, direction) = match camera.viewport_to_world(camera_transform, cursor_position) {
        Ok(ray) => (ray.origin, ray.direction.as_vec3()),
        Err(_) => return,
    };

    if let Some((_, toi)) = rapier_context.single().unwrap().cast_ray(
        origin,
        direction,
        100.0,
        true,
        Default::default(),
    ) {
        let hit_point = origin + direction * toi;
        info!(position=?hit_point, "Requesting object spawn");
        sender.send_to_server(&SpawnerMessage::Request((
            hit_point,
            ui_state.to_spawn.clone().unwrap(),
        )));
    }
}

fn handle_spawn_request(
    mut messages: MessageReader<MessageEvent<SpawnerMessage>>,
    mut commands: Commands,
    assets: Res<ItemAssets>,
    folders: Res<Assets<LoadedFolder>>,
    asset_server: Res<AssetServer>,
) {
    for event in messages.read() {
        let SpawnerMessage::Request((position, path)) = &event.message;
        // Only allow spawning items that belong to the loaded item folder.
        let exists = folders.get(&assets.definitions).is_some_and(|folder| {
            folder.handles.iter().any(|h| {
                h.path()
                    .is_some_and(|p| p.path().to_string_lossy() == path.as_str())
            })
        });
        if !exists {
            warn!("Invalid item id received from {:?}", event.connection);
            continue;
        }
        commands.spawn(NetworkSceneBundle {
            scene: asset_server.load::<ScenePatch>(path.clone()).into(),
            transform: Transform::from_translation(*position + Vec3::Y * 5.0),
            ..Default::default()
        });
        info!(connection=?event.connection, "Spawned item");
    }
}

pub(crate) struct SpawningPlugin;

impl Plugin for SpawningPlugin {
    fn build(&self, app: &mut App) {
        app.add_network_message::<SpawnerMessage>();

        if is_server(app) {
            app.add_systems(
                Update,
                handle_spawn_request.run_if(on_message::<MessageEvent<SpawnerMessage>>),
            );
        } else {
            app.init_resource::<SpawnerUiState>().add_systems(
                Update,
                (
                    prepare_item_ui_data,
                    (
                        spawning_ui.run_if(has_window),
                        spawn_requesting.before(InteractionSystem::Input),
                    )
                        .chain()
                        .run_if(in_state(GameState::Game)),
                ),
            );
        }
    }
}
