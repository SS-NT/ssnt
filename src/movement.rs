use crate::{
    camera::{MainCamera, TopDownCamera},
    Player,
};
use bevy::{math::Vec3Swizzles, prelude::*};
use bevy_rapier3d::{
    na::Vector3,
    prelude::{
        RigidBodyForcesComponent, RigidBodyMassPropsComponent,
        RigidBodyPositionComponent, RigidBodyVelocityComponent,
    },
};
use networking::{spawning::ClientControlled, NetworkManager};

pub fn movement_system(
    time: Res<Time>,
    keyboard_input: Res<Input<KeyCode>>,
    mut query: Query<(
        &mut Player,
        &RigidBodyVelocityComponent,
        &mut RigidBodyForcesComponent,
        &RigidBodyMassPropsComponent,
    ), With<ClientControlled>>,
    camera_query: Query<&TopDownCamera, With<MainCamera>>,
) {
    for (mut player, velocity, mut forces, mass_properties) in query.iter_mut() {
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
        let current_velocity = Vec2::from(velocity.0.linvel.xz());
        let needed_acceleration: Vec2 = (player.target_velocity - current_velocity) / one_tick;
        let max_acceleration = player.max_acceleration_force;
        let allowed_acceleration = needed_acceleration.clamp_length_max(max_acceleration);
        let force: Vec2 = allowed_acceleration * mass_properties.0.mass();

        forces.0.force = Vector3::new(force.x, 0.0, force.y);
    }
}

fn character_rotation_system(
    time: Res<Time>,
    mut query: Query<(&Player, &mut RigidBodyPositionComponent), With<Player>>,
) {
    for (player, mut transform) in query.iter_mut() {
        let mut target_direction: Vec2 = player.target_direction;
        if target_direction.length() < 0.01 {
            continue;
        }
        target_direction = target_direction.normalize();

        let current_rotation: Quat = transform.0.position.rotation.into();
        let target_rotation = Quat::from_rotation_arc(
            Vec3::Z,
            Vec3::new(target_direction.x, 0.0, target_direction.y),
        );
        let final_rotation = current_rotation.lerp(target_rotation, time.delta_seconds() * 5.0);
        transform.0.position.rotation = final_rotation.into();
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

pub struct MovementPlugin;

impl Plugin for MovementPlugin {
    fn build(&self, app: &mut App) {
        if app
            .world
            .get_resource::<NetworkManager>()
            .unwrap()
            .is_client()
        {
            app.add_system(movement_system.label("movement"));
            app.add_system_to_stage(CoreStage::PostUpdate, character_rotation_system);
        }
    }
}
