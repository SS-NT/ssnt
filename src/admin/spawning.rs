use bevy::{
    asset::{AssetPathId, HandleId},
    input::Input,
    math::Vec3,
    prelude::*,
    reflect::Reflect,
    scene::DynamicScene,
    window::PrimaryWindow,
};
use bevy_egui::{egui, EguiContexts};
use bevy_rapier3d::plugin::RapierContext;
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
    id: AssetPathId,
}

#[derive(Resource, Default)]
struct SpawnerUiState {
    all_items: Vec<ItemData>,
    to_spawn: Option<AssetPathId>,
}

fn spawning_ui(mut contexts: EguiContexts, mut state: ResMut<SpawnerUiState>) {
    let state = state.as_mut();
    egui::Window::new("Spawning").show(contexts.ctx_mut(), |ui| {
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
    mut contexts: EguiContexts,
    rapier_context: Res<RapierContext>,
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

    let Ok((window_entity, window)) = windows.get_single() else {
        return;
    };

    if contexts
        .try_ctx_for_window_mut(window_entity)
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

    let Ray { origin, direction } =
        match camera.viewport_to_world(camera_transform, cursor_position) {
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
        commands.spawn(NetworkSceneBundle {
            scene: Handle::weak(id.into()).into(),
            transform: Transform::from_translation(position + Vec3::Y * 5.0),
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
                handle_spawn_request.run_if(on_event::<MessageEvent<SpawnerMessage>>()),
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
