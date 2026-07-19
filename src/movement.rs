use crate::{
    body::{
        health::{BrainState, BrainStateEvent},
        Body,
    },
    camera::{MainCamera, TopDownCamera},
    combat::{ClientCombatModeStatus, CombatModeClient},
    Player,
};
use bevy::{ecs::query::Has, math::Vec3Swizzles, prelude::*};
use bevy_rapier3d::prelude::{
    ExternalForce, ReadMassProperties, RigidBody as RapierRigidBody, Velocity,
};
use networking::{
    messaging::{AppExt, MessageEvent, MessageReceivers, MessageSender},
    spawning::{ClientControlled, ClientControls},
    time::ClientNetworkTime,
    transform::{ClientMovement, ClientMovementClient},
    NetworkManager, NetworkSet, Players, ServerEvent,
};
use serde::{Deserialize, Serialize};

pub fn movement_system(
    time: Res<Time>,
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut query: Query<
        (
            Entity,
            &mut Player,
            &Velocity,
            Option<&mut ExternalForce>,
            &ReadMassProperties,
            Has<ClientMovementClient>,
        ),
        With<ClientControlled>,
    >,
    camera_query: Query<&TopDownCamera, With<MainCamera>>,
    mut commands: Commands,
) {
    for (entity, mut player, velocity, forces, mass_properties, can_move) in query.iter_mut() {
        // Reset force if we can't move
        if !can_move {
            if let Some(mut forces) = forces {
                forces.force = Vec3::ZERO;
            }
            continue;
        }

        let axis_x = movement_axis(&keyboard_input, KeyCode::KeyW, KeyCode::KeyS);
        let axis_z = movement_axis(&keyboard_input, KeyCode::KeyD, KeyCode::KeyA);

        let current_angle = match camera_query.single() {
            Ok(c) => c.current_angle(),
            Err(_) => return,
        };

        // Figure out where we want to go by key input and camera angle
        let target_direction = Quat::from_euler(bevy::math::EulerRot::XYZ, 0.0, current_angle, 0.0)
            .mul_vec3(Vec3::new(axis_x, 0.0, axis_z))
            .xz();
        player.target_direction = target_direction;

        // What is our ideal speed
        let mut ideal_speed: Vec2 = target_direction * player.max_velocity;

        // Prevent diagonal movement being twice as fast
        if target_direction.length_squared() > f32::EPSILON {
            ideal_speed /= target_direction.length();
        }

        // Move target velocity towards ideal speed, by acceleration
        let difference: Vec2 = ideal_speed - player.target_velocity;
        let step: f32 = player.acceleration * time.delta_secs();
        let difference_magnitude = difference.length();
        if difference_magnitude < step || difference_magnitude < f32::EPSILON {
            player.target_velocity = ideal_speed;
        } else {
            player.target_velocity += difference / difference_magnitude * step;
        }

        // Calculate needed force to reach target velocity in one frame
        let mut one_tick = time.delta_secs();
        if one_tick < f32::EPSILON {
            one_tick = 1.0;
        }
        let current_velocity = velocity.linear.xz();
        let needed_acceleration: Vec2 = (player.target_velocity - current_velocity) / one_tick;
        let max_acceleration = player.max_acceleration_force;
        let allowed_acceleration = needed_acceleration.clamp_length_max(max_acceleration);
        let force: Vec2 = allowed_acceleration * mass_properties.get().mass;

        if let Some(mut forces) = forces {
            forces.force = Vec3::new(force.x, 0.0, force.y);
        } else {
            commands.entity(entity).insert(ExternalForce {
                force: Vec3::new(force.x, 0.0, force.y),
                ..Default::default()
            });
        }
    }
}

const NORMAL_ROTATION_RADIANS_PER_SECOND: f32 = 5.0;
const COMBAT_ROTATION_RADIANS_PER_SECOND: f32 = 10.0;

fn character_rotation_system(
    time: Res<Time>,
    mut query: Query<
        (&Player, &mut Transform, Option<&CombatModeClient>),
        (With<ClientControlled>, With<ClientMovementClient>),
    >,
    combat_mode: ClientCombatModeStatus,
) {
    let is_combat = combat_mode.is_enabled();
    for (player, mut transform, combat) in query.iter_mut() {
        let target_direction = match (is_combat, combat) {
            (true, Some(combat)) => {
                let position = combat.aim.target_position;
                let origin = combat.aim.origin;
                (position.xz() - origin.xz()).normalize_or_zero()
            }
            _ => {
                let direction = player.target_direction;
                if direction.length() < 0.01 {
                    continue;
                }
                direction.normalize()
            }
        };

        let current_rotation = transform.rotation;
        let target_rotation = Quat::from_rotation_arc(
            Vec3::Z,
            Vec3::new(target_direction.x, 0.0, target_direction.y),
        );

        let angle = target_rotation.angle_between(current_rotation);
        if angle == 0.0 {
            continue;
        }

        // Max rotation depends on if combat is enabled
        let max_angle = if is_combat {
            COMBAT_ROTATION_RADIANS_PER_SECOND
        } else {
            NORMAL_ROTATION_RADIANS_PER_SECOND
        };

        // Linearly move towards target rotation
        transform.rotation = current_rotation.slerp(
            target_rotation,
            1f32.min(time.delta_secs() * max_angle / angle),
        );
    }
}

fn movement_axis(input: &Res<ButtonInput<KeyCode>>, plus: KeyCode, minus: KeyCode) -> f32 {
    let mut axis = 0.0;
    if input.pressed(plus) {
        axis += 1.0;
    }
    if input.pressed(minus) {
        axis -= 1.0;
    }
    axis
}

fn send_movement_update(
    // Require client control and already having a position from the server
    query: Query<
        &Transform,
        (
            With<ClientControlled>,
            With<ClientMovementClient>,
            With<ForcePositionReceived>,
        ),
    >,
    time: Res<Time>,
    network_time: Res<ClientNetworkTime>,
    mut accumulated: Local<f32>,
    mut sender: MessageSender,
) {
    let Some(tick_duration) = network_time.tick_duration() else {
        return;
    };
    *accumulated += time.delta_secs();
    if *accumulated < tick_duration {
        return;
    }
    // Keep the remainder so timing doesn't drift, but cap it to avoid a burst after a stall.
    *accumulated = (*accumulated - tick_duration).min(tick_duration);

    for transform in query.iter() {
        sender.send(
            &MovementMessage {
                position: transform.translation,
                rotation: transform.rotation,
            },
            MessageReceivers::Server,
        );
    }
}

fn handle_movement_message(
    mut query: Query<&mut Transform, With<ClientMovement>>,
    controls: Res<ClientControls>,
    players: Res<Players>,
    mut messages: MessageReader<MessageEvent<MovementMessage>>,
    mut commands: Commands,
) {
    for event in messages.read() {
        let player = match players.get(event.connection) {
            Some(p) => p,
            None => continue,
        };

        if let Some(controlled) = controls.controlled_entity(player.id) {
            if let Ok(mut transform) = query.get_mut(controlled) {
                transform.translation = event.message.position;
                transform.rotation = event.message.rotation;
                // Reset velocity to prevent server physics from going crazy
                // Once movement is server authoritative this won't be necessary
                commands.entity(controlled).insert((
                    Velocity {
                        linear: Vec3::ZERO,
                        angular: Vec3::ZERO,
                    },
                    // TODO: Remove once client no longer has authority
                    ClientAuthoritativeTransform {
                        position: event.message.position,
                        rotation: event.message.rotation,
                    },
                ));
            }
        }
    }
}

// HACK: forces the client to be at a position
// The code needs to die.
fn handle_force_position_client(
    mut query: Query<Entity, (With<ClientControlled>, With<Transform>)>,
    mut messages: MessageReader<MessageEvent<ForcePositionMessage>>,
    mut current: Local<Option<(f32, ForcePositionMessage)>>,
    time: Res<Time>,
    mut commands: Commands,
) {
    if let Some(event) = messages.read().last() {
        *current = Some((time.elapsed_secs(), event.message.clone()));
    }

    if let Ok(entity) = query.single_mut() {
        if let Some((start_time, message)) = current.as_mut() {
            // Mfw I can't be bothered to fix this properly
            if *start_time + 0.5 <= time.elapsed_secs() {
                *current = None;
                return;
            }

            commands.entity(entity).insert((
                ForcePositionReceived,
                Transform {
                    translation: message.position,
                    rotation: message.rotation,
                    ..Default::default()
                },
                Velocity::default(),
            ));
        }
    }
}

fn force_position_on_rejoin(
    mut server_events: MessageReader<ServerEvent>,
    controlled: Res<ClientControls>,
    players: Res<Players>,
    transforms: Query<&Transform>,
    mut sender: MessageSender,
) {
    for event in server_events.read() {
        if let ServerEvent::PlayerConnected(connection) = event {
            let player = players.get(*connection).unwrap();
            if let Some(entity) = controlled.controlled_entity(player.id) {
                if let Ok(transform) = transforms.get(entity) {
                    sender.send(
                        &ForcePositionMessage {
                            position: transform.translation,
                            rotation: transform.rotation,
                        },
                        MessageReceivers::Single(*connection),
                    );
                }
            }
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct MovementMessage {
    position: Vec3,
    rotation: Quat,
}

// TODO: Remove once movement is server authoritative
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ForcePositionMessage {
    pub position: Vec3,
    pub rotation: Quat,
}

#[derive(Component)]
#[component(storage = "SparseSet")]
struct ForcePositionReceived;

#[derive(Component)]
struct ClientAuthoritativeTransform {
    position: Vec3,
    rotation: Quat,
}

// Maintaining this movement code is getting exponentially more painful.
fn restore_client_position(
    mut query: Query<(&mut Transform, &ClientAuthoritativeTransform), With<ClientMovement>>,
) {
    for (mut transform, target_transform) in query.iter_mut() {
        transform.translation = target_transform.position;
        transform.rotation = target_transform.rotation;
    }
}

#[allow(clippy::too_many_arguments)]
fn prevent_movement_when_unconcious(
    mut reader: MessageReader<BrainStateEvent>,
    bodies: Query<&Body>,
    mut transforms: Query<&mut Transform>,
    parents: Query<&ChildOf>,
    controls: Res<ClientControls>,
    players: Res<Players>,
    mut sender: MessageSender,
    mut commands: Commands,
) {
    for event in reader.read() {
        let Some(body_entity) = parents
            .iter_ancestors(event.brain)
            .find(|e| bodies.contains(*e))
        else {
            continue;
        };
        let Ok(mut transform) = transforms.get_mut(body_entity) else {
            continue;
        };
        let mut entity = commands.entity(body_entity);
        match event.new_state {
            BrainState::Conscious => {
                entity.insert((
                    ClientMovement,
                    bevy_rapier3d::prelude::LockedAxes::ROTATION_LOCKED_X
                        | bevy_rapier3d::prelude::LockedAxes::ROTATION_LOCKED_Z,
                ));
                transform.rotation = Quat::IDENTITY;

                // Force position on client
                if let Some(uuid) = controls.controlling_player(body_entity) {
                    if let Some(connection) = players.get_connection(&uuid) {
                        sender.send(
                            &ForcePositionMessage {
                                position: transform.translation,
                                rotation: transform.rotation,
                            },
                            MessageReceivers::Single(connection),
                        );
                    }
                }
            }
            BrainState::Unconscious | BrainState::Dead => {
                entity
                    .remove::<ClientMovement>()
                    .insert(bevy_rapier3d::prelude::LockedAxes::default());
            }
        };
    }
}

/// Client controlled bodies should be kinematic so rapier applies forces when we move it.
fn update_server_body_control(
    controlled: Query<(Entity, Option<&RapierRigidBody>), With<ClientMovement>>,
    mut removed: RemovedComponents<ClientMovement>,
    still_controlled: Query<(), With<ClientMovement>>,
    mut commands: Commands,
) {
    for (entity, body) in controlled.iter() {
        if body.copied() != Some(RapierRigidBody::KinematicPositionBased) {
            commands
                .entity(entity)
                .insert(RapierRigidBody::KinematicPositionBased);
        }
    }
    for entity in removed.read() {
        if still_controlled.contains(entity) {
            continue;
        }
        if let Ok(mut entity) = commands.get_entity(entity) {
            entity.insert(RapierRigidBody::Dynamic);
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, SystemSet)]
pub enum MovementSystem {
    Update,
}

pub struct MovementPlugin;

impl Plugin for MovementPlugin {
    fn build(&self, app: &mut App) {
        app.add_network_message::<MovementMessage>()
            .add_network_message::<ForcePositionMessage>();

        if app
            .world()
            .get_resource::<NetworkManager>()
            .unwrap()
            .is_client()
        {
            app.add_systems(
                Update,
                (
                    (
                        movement_system,
                        character_rotation_system,
                        send_movement_update,
                    )
                        .chain()
                        .in_set(MovementSystem::Update),
                    handle_force_position_client,
                ),
            );
        } else {
            app.add_systems(
                Update,
                (
                    handle_movement_message,
                    force_position_on_rejoin,
                    prevent_movement_when_unconcious.run_if(on_message::<BrainStateEvent>),
                    update_server_body_control,
                )
                    .chain(),
            )
            .add_systems(
                PostUpdate,
                // To prevent server physics simulation messing up the position before sending
                restore_client_position
                    .after(bevy_rapier3d::plugin::PhysicsSet::Writeback)
                    .before(NetworkSet::ServerSyncPhysics),
            );
        }
    }
}
