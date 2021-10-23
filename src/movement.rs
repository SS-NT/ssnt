use crate::{
    camera::{MainCamera, TopDownCamera},
    Player,
};
use bevy::{math::Vec3Swizzles, prelude::*};

pub fn movement_system(
    time: Res<Time>,
    keyboard_input: Res<Input<KeyCode>>,
    mut query: Query<(&mut Player, &mut Transform)>,
    camera_query: Query<&TopDownCamera, With<MainCamera>>,
) {
    for (mut player, mut transform) in query.iter_mut() {
        let axis_x = movement_axis(&keyboard_input, KeyCode::W, KeyCode::S);
        let axis_z = movement_axis(&keyboard_input, KeyCode::D, KeyCode::A);

        let current_angle = match camera_query.get_single() {
            Ok(c) => c.current_angle(),
            Err(_) => return,
        };

        // Calculate velocity to add
        let additional_velocity =
            Quat::from_euler(bevy::math::EulerRot::XYZ, 0.0, current_angle, 0.0)
                .mul_vec3(Vec3::new(axis_x, 0.0, axis_z)).xz() * player.acceleration;
        // Calculate friction from velocity
        let friction = if player.velocity.length_squared() != 0.0 {
            player.velocity.normalize() * player.friction * -1.0
        } else {
            Vec2::ZERO
        };

        // Add velocity
        player.velocity += additional_velocity * time.delta_seconds();

        // Clamp velocity
        if player.velocity.length() > player.max_velocity {
            player.velocity = player.velocity.normalize() * player.max_velocity;
        }

        let velocity_with_friction = player.velocity + friction * time.delta_seconds();
        player.velocity = if player.velocity.signum() != velocity_with_friction.signum() {
            Vec2::ZERO
        } else {
            velocity_with_friction
        };

        transform.translation += Vec3::new(player.velocity.x, 0.0, player.velocity.y);
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
