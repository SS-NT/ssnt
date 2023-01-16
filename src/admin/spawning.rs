use bevy::{
    asset::{AssetPathId, HandleId},
    input::Input,
    math::{Mat4, Vec2, Vec3},
    prelude::*,
    reflect::Reflect,
    scene::DynamicScene,
    window::Windows,
};
use bevy_egui::{egui::Window, EguiContext};
use bevy_rapier3d::plugin::RapierContext;
use networking::{
    identity::EntityCommandsExt,
    is_server,
    messaging::{AppExt, MessageEvent, MessageSender},
    scene::NetworkSceneBundle,
};
use serde::{Deserialize, Serialize};

use crate::{
    camera::MainCamera,
    interaction::InteractionSystem,
    items::{Item, ItemAssets},
    GameState,
};

struct ItemData {
    name: String,
    id: AssetPathId,
}

#[derive(Resource, Default)]
struct SpawnerUiState {
    all_items: Vec<ItemData>,
    to_spawn: Option<AssetPathId>,
}

fn spawning_ui(mut egui_context: ResMut<EguiContext>, mut state: ResMut<SpawnerUiState>) {
    let state = state.as_mut();
    Window::new("Spawning").show(egui_context.ctx_mut(), |ui| {
        ui.selectable_value(&mut state.to_spawn, None, "None");
        for data in state.all_items.iter() {
            ui.selectable_value(&mut state.to_spawn, Some(data.id), &data.name);
        }
    });
}

fn prepare_item_ui_data(
    assets: Res<ItemAssets>,
    mut events: EventReader<AssetEvent<DynamicScene>>,
    mut ui_data: ResMut<SpawnerUiState>,
    scenes: Res<Assets<DynamicScene>>,
) {
    let loaded_item = events.iter().any(|e| match e {
        AssetEvent::Created { handle } => assets.definitions.contains(handle),
        _ => false,
    });
    if !loaded_item {
        return;
    }

    ui_data.all_items.clear();
    for handle in &assets.definitions {
        let scene = match scenes.get(handle) {
            Some(s) => s,
            None => continue,
        };
        let entity = scene.entities.first().unwrap();
        let dynamic = match entity
            .components
            .iter()
            .find(|c| c.type_name() == "ssnt::items::Item")
        {
            Some(i) => i,
            None => {
                warn!("No item component in item file");
                continue;
            }
        };
        let mut item = Item::default();
        item.apply(dynamic.as_ref());
        ui_data.all_items.push(ItemData {
            name: item.name.clone(),
            id: match handle.id() {
                HandleId::AssetPathId(p) => p,
                _ => panic!("Item must be loaded from disk"),
            },
        });
    }
}

#[derive(Serialize, Deserialize, Clone)]
enum SpawnerMessage {
    Request((Vec3, AssetPathId)),
}

#[allow(clippy::too_many_arguments)]
fn spawn_requesting(
    ui_state: Res<SpawnerUiState>,
    mut buttons: ResMut<Input<MouseButton>>,
    mut context: ResMut<EguiContext>,
    rapier_context: Res<RapierContext>,
    windows: Res<Windows>,
    cameras: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    mut sender: MessageSender,
) {
    if ui_state.to_spawn.is_none() {
        return;
    }

    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }

    let window = match windows.get_primary() {
        Some(w) => w,
        None => return,
    };

    if context
        .try_ctx_for_window_mut(window.id())
        .map(|c| c.wants_pointer_input())
        == Some(true)
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

    let (origin, direction) = match ray_from_cursor(cursor_position, camera, camera_transform) {
        Some(r) => r,
        None => return,
    };

    if let Some((_, toi)) =
        rapier_context.cast_ray(origin, direction, 100.0, true, Default::default())
    {
        let hit_point = origin + direction * toi;
        info!(position=?hit_point, "Requesting object spawn");
        sender.send_to_server(&SpawnerMessage::Request((
            hit_point,
            ui_state.to_spawn.unwrap(),
        )));
    }
}

fn handle_spawn_request(
    mut messages: EventReader<MessageEvent<SpawnerMessage>>,
    mut commands: Commands,
    assets: Res<ItemAssets>,
) {
    for event in messages.iter() {
        let SpawnerMessage::Request((position, id)) = event.message;
        let exists = assets
            .definitions
            .iter()
            // TODO: Fix O(n) lookup
            .any(|h| h.id() == HandleId::AssetPathId(id));
        if !exists {
            warn!("Invalid item id received from {:?}", event.connection);
            continue;
        }
        commands
            .spawn(NetworkSceneBundle {
                scene: Handle::weak(id.into()).into(),
                transform: Transform::from_translation(position + Vec3::Y * 5.0),
                ..Default::default()
            })
            .networked();
        info!(connection=?event.connection, "Spawned item");
    }
}

pub(crate) struct SpawningPlugin;

impl Plugin for SpawningPlugin {
    fn build(&self, app: &mut App) {
        app.add_network_message::<SpawnerMessage>();

        if is_server(app) {
            app.add_system(handle_spawn_request);
        } else {
            app.init_resource::<SpawnerUiState>()
                .add_system_set(
                    SystemSet::on_update(GameState::Game)
                        .with_system(spawning_ui.label("admin spawn ui")),
                )
                .add_system(prepare_item_ui_data)
                .add_system(
                    spawn_requesting
                        .after("admin spawn ui")
                        .before(InteractionSystem::Input),
                );
        }
    }
}

// Taken from https://github.com/aevyrie/bevy_mod_raycast/blob/51d9e2c99066ea769db27c0ae79d11b258fcef4f/src/primitives.rs#L192
pub fn ray_from_cursor(
    cursor_pos_screen: Vec2,
    camera: &Camera,
    camera_transform: &GlobalTransform,
) -> Option<(Vec3, Vec3)> {
    let view = camera_transform.compute_matrix();
    let screen_size = camera.logical_target_size()?;
    let projection = camera.projection_matrix();
    let far_ndc = projection.project_point3(Vec3::NEG_Z).z;
    let near_ndc = projection.project_point3(Vec3::Z).z;
    let cursor_ndc = (cursor_pos_screen / screen_size) * 2.0 - Vec2::ONE;
    let ndc_to_world: Mat4 = view * projection.inverse();
    let near = ndc_to_world.project_point3(cursor_ndc.extend(near_ndc));
    let far = ndc_to_world.project_point3(cursor_ndc.extend(far_ndc));
    let ray_direction = far - near;
    Some((near, ray_direction))
}
