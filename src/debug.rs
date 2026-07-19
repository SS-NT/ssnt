use bevy::gizmos::config::GizmoConfigStore;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use bevy_inspector_egui::quick::WorldInspectorPlugin;
use bevy_rapier3d::prelude::RigidBody;
use bevy_rapier3d::render::DebugRenderContext;
use networking::identity::NetworkIdentity;
use networking::transform::NetworkedTransform;
use physics::VisualError;

use crate::GameState;

pub(crate) struct DebugPlugin;

#[derive(Resource, Default)]
struct DebugState {
    inspector_enabled: bool,
    visual_error_gizmos: bool,
    physics_mode_gizmos: bool,
    snapshot_age_gizmos: bool,
}

/// How old (seconds) the last snapshot must be for the age gizmo to reach full darkness.
const SNAPSHOT_AGE_FADE: f32 = 1.0;

impl Plugin for DebugPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DebugState>()
            .register_type::<GizmoConfigStore>()
            .add_plugins((
                bevy_rapier3d::render::RapierDebugRenderPlugin::default().disabled(),
                WorldInspectorPlugin::new()
                    .run_if(|state: Res<DebugState>| state.inspector_enabled),
            ))
            .add_systems(
                EguiPrimaryContextPass,
                (debug_menu, debug_watermark).run_if(in_state(GameState::Game)),
            )
            .add_systems(
                Update,
                (
                    draw_visual_error_gizmos
                        .run_if(|state: Res<DebugState>| state.visual_error_gizmos),
                    draw_physics_mode_gizmos
                        .run_if(|state: Res<DebugState>| state.physics_mode_gizmos),
                    draw_snapshot_age_gizmos
                        .run_if(|state: Res<DebugState>| state.snapshot_age_gizmos),
                ),
            );
    }
}

fn debug_menu(
    mut contexts: EguiContexts,
    mut rapier_debug: ResMut<DebugRenderContext>,
    mut state: ResMut<DebugState>,
    mut server_inspector: ResMut<crate::admin::inspector::ServerInspectorEnabled>,
    mut items: Query<(Entity, Option<&mut VisualError>), (With<NetworkIdentity>, With<RigidBody>)>,
    mut commands: Commands,
) {
    egui::Window::new("Debug Menu").show(contexts.ctx_mut().unwrap(), |ui| {
        ui.checkbox(&mut state.inspector_enabled, "World inspector");
        ui.checkbox(&mut server_inspector.0, "Server inspector");
        ui.checkbox(&mut rapier_debug.enabled, "Show physics objects");
        ui.checkbox(&mut state.visual_error_gizmos, "Visual error gizmos");
        ui.checkbox(&mut state.physics_mode_gizmos, "Physics mode gizmos");
        ui.checkbox(&mut state.snapshot_age_gizmos, "Snapshot age gizmos");
    });
}

fn draw_snapshot_age_gizmos(
    query: Query<(&GlobalTransform, &NetworkedTransform)>,
    time: Res<Time>,
    mut gizmos: Gizmos,
) {
    let now = time.elapsed_secs();
    for (global, networked) in &query {
        let age = now - networked.last_received();
        // Fresh snapshots are bright orange, fading to near-black with age.
        let brightness = (1.0 - age / SNAPSHOT_AGE_FADE).clamp(0.1, 1.0);
        let color = Color::srgb(brightness, brightness * 0.5, brightness * 0.1);
        gizmos.sphere(global.to_isometry(), 0.12, color);
    }
}

fn draw_physics_mode_gizmos(
    query: Query<(&GlobalTransform, &NetworkedTransform)>,
    mut gizmos: Gizmos,
) {
    for (global, networked) in &query {
        let color = match networked.is_simulating() {
            Some(true) => Color::srgb(1.0, 0.0, 0.0),
            Some(false) => Color::srgb(0.0, 1.0, 0.0),
            None => continue,
        };
        gizmos.sphere(global.to_isometry(), 0.08, color);
    }
}

fn draw_visual_error_gizmos(
    query: Query<(&Transform, &VisualError, Option<&ChildOf>)>,
    globals: Query<&GlobalTransform>,
    mut gizmos: Gizmos,
) {
    for (transform, error, child_of) in &query {
        let parent = child_of
            .and_then(|c| globals.get(c.parent()).ok())
            .copied()
            .unwrap_or(GlobalTransform::IDENTITY);

        let mut visual = *transform;
        visual.translation += error.translation;
        visual.rotation = error.rotation * visual.rotation;

        let accurate = parent.mul_transform(*transform);
        let visual = parent.mul_transform(visual);

        gizmos.cross(accurate.to_isometry(), 0.3, Color::srgb(0.0, 1.0, 0.0));
        gizmos.cross(visual.to_isometry(), 0.3, Color::srgb(1.0, 0.0, 0.0));
        gizmos.line(
            accurate.translation(),
            visual.translation(),
            Color::srgb(1.0, 1.0, 0.0),
        );
    }
}

fn debug_watermark(mut contexts: EguiContexts) {
    egui::Area::new(egui::Id::new("watermark"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-50.0, 0.0))
        .order(egui::Order::Foreground)
        .show(contexts.ctx_mut().unwrap(), |ui| {
            ui.label(
                egui::RichText::new("SSNT Dev Build")
                    .color(egui::Rgba::WHITE)
                    .size(21.0),
            );
        });
}
