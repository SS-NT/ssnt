use crate::{
    camera::{MainCamera, TopDownCamera},
    combat::{ClientCombatModeStatus, CombatModeClient},
    Player,
};
use bevy::{math::Vec3Swizzles, prelude::*, time::FixedTimestep};
use bevy_rapier3d::prelude::{ExternalForce, ReadMassProperties, Velocity};
use networking::{
    messaging::{AppExt, MessageEvent, MessageReceivers, MessageSender},
    spawning::{ClientControlled, ClientControls},
    NetworkManager, Players, ServerEvent,
};
use serde::{Deserialize, Serialize};

pub fn movement_system(
    time: Res<Time>,
    keyboard_input: Res<Input<KeyCode>>,
    mut query: Query<
        (
            Entity,
            &mut Player,
            &Velocity,
            Option<&mut ExternalForce>,
            &ReadMassProperties,
        ),
        With<ClientControlled>,
    >,
    camera_query: Query<&TopDownCamera, With<MainCamera>>,
    mut commands: Commands,
) {
    for (entity, mut player, velocity, forces, mass_properties) in query.iter_mut() {
        let axis_x = movement_axis(&keyboard_input, KeyCode::W, KeyCode::S);
        let axis_z = movement_axis(&keyboard_input, KeyCode::D, KeyCode::A);

        let current_angle = match camera_query.get_single() {
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
        let step: f32 = player.acceleration * time.delta_seconds();
        let difference_magnitude = difference.length();
        if difference_magnitude < step || difference_magnitude < f32::EPSILON {
            player.target_velocity = ideal_speed;
        } else {
            player.target_velocity += difference / difference_magnitude * step;
        }

        // Calculate needed force to reach target velocity in one frame
        let mut one_tick = time.delta_seconds();
        if one_tick < f32::EPSILON {
            one_tick = 1.0;
        }
        let current_velocity = velocity.linvel.xz();
        let needed_acceleration: Vec2 = (player.target_velocity - current_velocity) / one_tick;
        let max_acceleration = player.max_acceleration_force;
        let allowed_acceleration = needed_acceleration.clamp_length_max(max_acceleration);
        let force: Vec2 = allowed_acceleration * mass_properties.0.mass;

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
    mut query: Query<(&Player, &mut Transform, Option<&CombatModeClient>), With<ClientControlled>>,
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
            1f32.min(time.delta_seconds() * max_angle / angle),
        );
    }
}

fn movement_axis(input: &Res<Input<KeyCode>>, plus: KeyCode, minus: KeyCode) -> f32 {
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
    query: Query<&Transform, (With<ClientControlled>, With<ForcePositionReceived>)>,
    mut sender: MessageSender,
) {
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
    mut query: Query<&mut Transform>,
    controls: Res<ClientControls>,
    players: Res<Players>,
    mut messages: EventReader<MessageEvent<MovementMessage>>,
) {
    for event in messages.iter() {
        let player = match players.get(event.connection) {
            Some(p) => p,
            None => continue,
        };

        if let Some(controlled) = controls.controlled_entity(player.id) {
            if let Ok(mut transform) = query.get_mut(controlled) {
                transform.translation = event.message.position;
                transform.rotation = event.message.rotation;
            }
        }
    }
}

// HACK: forces the client to be at a position
// The code needs to die.
fn handle_force_position_client(
    mut query: Query<Entity, (With<ClientControlled>, With<Transform>)>,
    mut messages: EventReader<MessageEvent<ForcePositionMessage>>,
    mut current: Local<Option<(u8, ForcePositionMessage)>>,
    mut commands: Commands,
) {
    if let Some(event) = messages.iter().last() {
        *current = Some((0, event.message.clone()));
    }

    if let Ok(entity) = query.get_single_mut() {
        if let Some((count, message)) = current.as_mut() {
            // Mfw I can't be bothered to fix this properly
            if *count >= 5 {
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
            ));
            *count += 1;
        }
    }
}

fn force_position_on_rejoin(
    mut server_events: EventReader<ServerEvent>,
    controlled: Res<ClientControls>,
    players: Res<Players>,
    transforms: Query<&Transform>,
    mut sender: MessageSender,
) {
    for event in server_events.iter() {
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

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, SystemLabel)]
pub enum MovementSystem {
    Update,
}

pub struct MovementPlugin;

impl Plugin for MovementPlugin {
    fn build(&self, app: &mut App) {
        app.add_network_message::<MovementMessage>()
            .add_network_message::<ForcePositionMessage>();

        if app
            .world
            .get_resource::<NetworkManager>()
            .unwrap()
            .is_client()
        {
            app.add_system(movement_system.label(MovementSystem::Update))
                .add_system(
                    send_movement_update
                        .after(MovementSystem::Update)
                        .with_run_criteria(FixedTimestep::step(0.1)),
                )
                .add_system_to_stage(CoreStage::PostUpdate, character_rotation_system)
                .add_system(handle_force_position_client);
        } else {
            app.add_system(handle_movement_message)
                .add_system(force_position_on_rejoin);
        }
    }
}
