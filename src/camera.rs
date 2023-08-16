use bevy::{input::mouse::MouseWheel, prelude::*};

use crate::movement::MovementSystem;

#[derive(Component)]
pub struct MainCamera;

#[derive(Component)]
pub struct TopDownCamera {
    pub target: Entity,
    pub target_angle: f32,
    current_angle: f32,
    current_zoom: f32,
    target_zoom: f32,
    closest_offset: Vec3,
    farthest_offset: Vec3,
}

impl TopDownCamera {
    pub fn new(target: Entity) -> Self {
        Self {
            target,
            target_angle: 0.0,
            current_angle: 0.0,
            current_zoom: 0.5,
            target_zoom: 0.5,
            closest_offset: Vec3::new(0.0, 5.0, 0.0),
            farthest_offset: Vec3::new(0.0, 15.0, 0.0),
        }
    }

    pub fn current_angle(&self) -> f32 {
        self.current_angle
    }
}

pub fn top_down_camera_input_system(
    mut camera_query: Query<&mut TopDownCamera>,
    keyboard_input: Res<Input<KeyCode>>,
    mut mouse_wheel: EventReader<MouseWheel>,
) {
    let scroll_amount: f32 = mouse_wheel.iter().map(|e| e.y).sum();
    for mut camera in camera_query.iter_mut() {
        let mut rotation = None;
        if keyboard_input.just_pressed(KeyCode::Q) {
            rotation = Some(-1.0);
        } else if keyboard_input.just_pressed(KeyCode::E) {
            rotation = Some(1.0);
        }

        if let Some(rotation) = rotation {
            let offset = std::f32::consts::FRAC_PI_2 * rotation;
            camera.target_angle += offset;
        }

        if !(-0.01..=0.01).contains(&scroll_amount) {
            camera.target_zoom = (camera.target_zoom - scroll_amount * 0.2).clamp(0.0, 1.0);
        }
    }
}

pub fn top_down_camera_update_system(
    time: Res<Time>,
    mut camera_query: Query<(&mut TopDownCamera, &mut Transform)>,
    target_query: Query<&Transform, Without<TopDownCamera>>,
) {
    for (mut camera, mut transform) in camera_query.iter_mut() {
        let interpolate = time.delta_seconds() * 10.0;
        camera.current_angle =
            camera.current_angle * (1.0 - interpolate) + camera.target_angle * interpolate;
        camera.current_zoom =
            camera.current_zoom * (1.0 - interpolate) + camera.target_zoom * interpolate;

        let target_transform = match target_query.get(camera.target) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let offset_rotation = Quat::from_euler(
            bevy::math::EulerRot::XYZ,
            0.0,
            camera.current_angle,
            35.0 * 0.017453,
        );
        let offset = offset_rotation.mul_vec3(
            camera
                .closest_offset
                .lerp(camera.farthest_offset, camera.current_zoom),
        );
        transform.translation = target_transform.translation + offset;
        transform.look_at(target_transform.translation, Vec3::Y);
    }
}

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                top_down_camera_input_system,
                top_down_camera_update_system.after(MovementSystem::Update),
            )
                .chain(),
        );
    }
}
