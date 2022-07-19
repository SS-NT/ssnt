use bevy::{
    ecs::system::EntityCommands,
    input::Input,
    pbr::PbrBundle,
    prelude::{
        info, shape, warn, App, Assets, Camera, Commands, Component, EventReader, GlobalTransform,
        Handle, Mesh, MouseButton, ParallelSystemDescriptorCoercion, Plugin, Query, Res, ResMut,
        Transform, With,
    },
    transform::TransformBundle,
    utils::HashMap,
    window::Windows,
};
use bevy_egui::{egui::Window, EguiContext};
use bevy_rapier3d::{
    plugin::RapierContext,
    prelude::{Collider, RigidBody, Velocity},
    rapier::prelude::ColliderShape,
};
use glam::{Mat4, Vec2, Vec3};
use networking::{
    identity::{EntityCommandsExt, NetworkIdentities, NetworkIdentity},
    messaging::{AppExt, MessageEvent, MessageReceivers, MessageSender},
    spawning::{PrefabPath, ServerEntityEvent, SpawningSystems},
    transform::{NetworkTransform, NetworkedTransform},
    NetworkManager,
};
use serde::{Deserialize, Serialize};

use crate::camera::MainCamera;

#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum Spawnable {
    Cube,
    Sphere,
}

struct SpawnableDefinition {
    mesh: Handle<Mesh>,
    shape: ColliderShape,
}

struct SpawnerAssets {
    spawnables: HashMap<Spawnable, SpawnableDefinition>,
}

fn load_spawner_assets(mut commands: Commands, mut meshes: Option<ResMut<Assets<Mesh>>>) {
    let cube_mesh = meshes
        .as_mut()
        .map(|m| m.add(Mesh::from(shape::Cube::default())));
    let sphere_mesh = meshes.as_mut().map(|m| {
        m.add(Mesh::from(shape::UVSphere {
            sectors: 128,
            stacks: 64,
            ..Default::default()
        }))
    });

    let mut spawnables: HashMap<Spawnable, SpawnableDefinition> = Default::default();
    spawnables.insert(
        Spawnable::Cube,
        SpawnableDefinition {
            mesh: cube_mesh.unwrap_or_default(),
            shape: ColliderShape::cuboid(0.5, 0.5, 0.5),
        },
    );
    spawnables.insert(
        Spawnable::Sphere,
        SpawnableDefinition {
            mesh: sphere_mesh.unwrap_or_default(),
            shape: ColliderShape::ball(1.0),
        },
    );

    commands.insert_resource(SpawnerAssets { spawnables });
}

#[derive(Default)]
struct SpawnerUiState {
    to_spawn: Option<Spawnable>,
}

fn spawning_ui(mut egui_context: ResMut<EguiContext>, mut state: ResMut<SpawnerUiState>) {
    Window::new("Spawning").show(egui_context.ctx_mut(), |ui| {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut state.to_spawn, None, "None");
            ui.selectable_value(&mut state.to_spawn, Some(Spawnable::Cube), "Cube");
            ui.selectable_value(&mut state.to_spawn, Some(Spawnable::Sphere), "Sphere");
        });
    });
}

#[derive(Serialize, Deserialize, Clone)]
enum SpawnerMessage {
    Request((Vec3, Spawnable)),
    Spawned((NetworkIdentity, Spawnable)),
}

#[allow(clippy::too_many_arguments)]
fn spawn_requesting(
    ui_state: Res<SpawnerUiState>,
    buttons: Res<Input<MouseButton>>,
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

    let (camera, camera_transform) = match cameras.iter().next() {
        Some(o) => o,
        None => return,
    };
    let cursor_position = match window.cursor_position() {
        Some(p) => p,
        None => return,
    };

    let (origin, direction) =
        match ray_from_cursor(cursor_position, &windows, camera, camera_transform) {
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

fn create_spawnable(
    commands: &mut EntityCommands,
    kind: Spawnable,
    assets: &SpawnerAssets,
    position: Vec3,
) {
    let definition = assets.spawnables.get(&kind).unwrap();

    commands
        .insert(RigidBody::Dynamic)
        .insert(Velocity::default())
        .insert(Collider::from(definition.shape.clone()))
        .insert_bundle(TransformBundle::from(Transform::from_translation(position)))
        .insert(kind);
}

fn handle_spawn_request(
    mut messages: EventReader<MessageEvent<SpawnerMessage>>,
    mut commands: Commands,
    assets: Res<SpawnerAssets>,
) {
    for event in messages.iter() {
        if let SpawnerMessage::Request((position, kind)) = event.message {
            let mut builder = commands.spawn();
            create_spawnable(&mut builder, kind, &assets, position);
            builder
                .insert(PrefabPath("spawnable".to_owned()))
                .insert(NetworkTransform::default())
                .networked();
        }
    }
}

fn send_spawned_type(
    mut events: EventReader<ServerEntityEvent>,
    spawnables: Query<(&Spawnable, &NetworkIdentity)>,
    mut sender: MessageSender,
) {
    for event in events.iter() {
        if let ServerEntityEvent::Spawned((entity, connection)) = event {
            let (spawnable, identity) = match spawnables.get(*entity) {
                Ok(s) => s,
                Err(_) => continue,
            };

            sender.send(
                &SpawnerMessage::Spawned((*identity, *spawnable)),
                MessageReceivers::Single(*connection),
            );
        }
    }
}

fn receive_spawned_type(
    mut events: EventReader<MessageEvent<SpawnerMessage>>,
    identities: Res<NetworkIdentities>,
    mut commands: Commands,
    assets: Res<SpawnerAssets>,
) {
    for event in events.iter() {
        if let SpawnerMessage::Spawned((identity, spawnable)) = event.message {
            let entity = match identities.get_entity(identity) {
                Some(e) => e,
                None => {
                    warn!("Received spawned type for non-existent {:?}", identity);
                    continue;
                }
            };

            let mut builder = commands.entity(entity);
            create_spawnable(&mut builder, spawnable, &assets, Vec3::ZERO);
            builder
                .insert(NetworkedTransform::default())
                .insert_bundle(PbrBundle {
                    mesh: assets.spawnables.get(&spawnable).unwrap().mesh.clone(),
                    ..Default::default()
                });
        }
    }
}

pub(crate) struct SpawningPlugin;

impl Plugin for SpawningPlugin {
    fn build(&self, app: &mut App) {
        app.add_network_message::<SpawnerMessage>()
            .add_startup_system(load_spawner_assets);

        if app
            .world
            .get_resource::<NetworkManager>()
            .unwrap()
            .is_server()
        {
            app.add_system(handle_spawn_request)
                .add_system(send_spawned_type.after(SpawningSystems::Spawn));
        } else {
            app.init_resource::<SpawnerUiState>()
                .add_system(spawning_ui.label("admin spawn ui"))
                .add_system(spawn_requesting.after("admin spawn ui"))
                .add_system(receive_spawned_type.after(SpawningSystems::Spawn));
        }
    }
}

// Taken from https://github.com/aevyrie/bevy_mod_raycast/blob/d9fe7f99b928d4ba6bf670235c5cccf2d04723c7/src/primitives.rs#L109
fn ray_from_cursor(
    cursor_pos_screen: Vec2,
    windows: &Res<Windows>,
    camera: &Camera,
    camera_transform: &GlobalTransform,
) -> Option<(Vec3, Vec3)> {
    let view = camera_transform.compute_matrix();
    let window_id = match camera.target {
        bevy::render::camera::RenderTarget::Window(w) => w,
        _ => return None,
    };

    let window = match windows.get(window_id) {
        Some(window) => window,
        None => {
            return None;
        }
    };
    let screen_size = Vec2::from([window.width() as f32, window.height() as f32]);
    let projection = camera.projection_matrix;

    // 2D Normalized device coordinate cursor position from (-1, -1) to (1, 1)
    let cursor_ndc = (cursor_pos_screen / screen_size) * 2.0 - Vec2::from([1.0, 1.0]);
    let ndc_to_world: Mat4 = view * projection.inverse();
    let world_to_ndc = projection * view;
    let is_orthographic = projection.w_axis[3] == 1.0;

    // Compute the cursor position at the near plane. The bevy camera looks at -Z.
    let ndc_near = world_to_ndc.transform_point3(-Vec3::Z * camera.near).z;
    let cursor_pos_near = ndc_to_world.transform_point3(cursor_ndc.extend(ndc_near));

    // Compute the ray's direction depending on the projection used.
    let ray_direction = match is_orthographic {
        true => view.transform_vector3(-Vec3::Z), // All screenspace rays are parallel in ortho
        false => cursor_pos_near - camera_transform.translation, // Direction from camera to cursor
    };

    Some((cursor_pos_near, ray_direction))
}
