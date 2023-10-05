use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use bevy_inspector_egui::quick::WorldInspectorPlugin;
use bevy_rapier3d::render::DebugRenderContext;

use crate::{ui::has_window, GameState};

pub(crate) struct DebugPlugin;

#[derive(Resource, Default)]
struct DebugState {
    inspector_enabled: bool,
}

impl Plugin for DebugPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<DebugState>()
            .add_plugins((
                bevy_rapier3d::render::RapierDebugRenderPlugin::default().disabled(),
                WorldInspectorPlugin::new()
                    .run_if(|state: Res<DebugState>| state.inspector_enabled),
            ))
            .add_systems(
                Update,
                (debug_menu, debug_watermark)
                    .run_if(has_window)
                    .run_if(in_state(GameState::Game)),
            );
    }
}

fn debug_menu(
    mut contexts: EguiContexts,
    mut rapier_debug: ResMut<DebugRenderContext>,
    mut state: ResMut<DebugState>,
) {
    egui::Window::new("Debug Menu").show(contexts.ctx_mut(), |ui| {
        ui.checkbox(&mut state.inspector_enabled, "World inspector");
        ui.checkbox(&mut rapier_debug.enabled, "Show physics objects");
    });
}

fn debug_watermark(mut contexts: EguiContexts) {
    egui::Area::new("watermark")
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-50.0, 0.0))
        .order(egui::Order::Foreground)
        .show(contexts.ctx_mut(), |ui| {
            ui.label(
                egui::RichText::new("SSNT Dev Build")
                    .color(egui::Rgba::WHITE)
                    .size(21.0),
            );
        });
}
