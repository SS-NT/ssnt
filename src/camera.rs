use bevy::prelude::*;

pub struct MainCamera;

pub struct TopDownCamera {
    pub target: Entity,
    pub target_angle: f32,
    current_angle: f32,
}

impl TopDownCamera {
    pub fn new(target: Entity) -> Self {
        Self {
            target,
            target_angle: 0.0,
            current_angle: 0.0,
        }
    }

    pub fn current_angle(&self) -> f32 {
        self.current_angle
    }
}

pub fn top_down_camera_input_system(
    mut camera_query: Query<(&mut TopDownCamera, &Transform)>,
    keyboard_input: Res<Input<KeyCode>>,
) {
    for (mut camera, transform) in camera_query.iter_mut() {
        let mut rotation = None;
        if keyboard_input.just_pressed(KeyCode::Q) {
            rotation = Some(-1.0);
        } else if keyboard_input.just_pressed(KeyCode::E) {
            rotation = Some(1.0);
        }

        if let Some(rotation) = rotation {
            let offset = std::f32::consts::FRAC_PI_2 * rotation;
            camera.target_angle = camera.target_angle + offset;
        }
    }
}

pub fn top_down_camera_update_system(
    time: Res<Time>,
    mut camera_query: Query<(&mut TopDownCamera, &mut Transform)>,
    target_query: Query<&Transform, Without<TopDownCamera>>,
) {
    for (mut camera, mut transform) in camera_query.iter_mut() {
        // TODO: Interpolate
        let interpolate = time.delta_seconds() * 10.0;
        camera.current_angle = camera.current_angle * (1.0 - interpolate) + camera.target_angle * interpolate;

        let target_transform = match target_query.get(camera.target) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let offset_rotation = Quat::from_euler(bevy::math::EulerRot::XYZ, 0.0, camera.current_angle, 45.0 * 0.017453);
        let offset = offset_rotation.mul_vec3(Vec3::new(0.0, 15.0, 0.0));
        transform.translation = target_transform.translation + offset;
        transform.look_at(target_transform.translation, Vec3::Y);
    }
}
