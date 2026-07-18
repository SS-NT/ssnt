use bevy::gizmos::config::GizmoConfigStore;
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use bevy_inspector_egui::quick::WorldInspectorPlugin;
use bevy_rapier3d::prelude::RigidBody;
use bevy_rapier3d::render::DebugRenderContext;
use networking::identity::NetworkIdentity;
use physics::VisualError;

use crate::GameState;

pub(crate) struct DebugPlugin;

#[derive(Resource, Default)]
struct DebugState {
    inspector_enabled: bool,
    visual_error_gizmos: bool,
}

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
                draw_visual_error_gizmos.run_if(|state: Res<DebugState>| state.visual_error_gizmos),
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
    });
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
