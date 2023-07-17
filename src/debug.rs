use bevy::prelude::*;
use bevy_egui::{egui, EguiContext};
use bevy_inspector_egui::{WorldInspectorParams, WorldInspectorPlugin};
use bevy_rapier3d::render::DebugRenderContext;

pub(crate) struct DebugPlugin;

impl Plugin for DebugPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugin(bevy_rapier3d::render::RapierDebugRenderPlugin::default().disabled())
            .insert_resource(WorldInspectorParams {
                enabled: false,
                ..Default::default()
            })
            .add_plugin(WorldInspectorPlugin::new())
            .add_system(debug_menu)
            .add_system(debug_watermark);
    }
}

fn debug_menu(
    mut egui_context: ResMut<EguiContext>,
    mut rapier_debug: ResMut<DebugRenderContext>,
    mut inspector: ResMut<WorldInspectorParams>,
) {
    egui::Window::new("Debug Menu").show(egui_context.ctx_mut(), |ui| {
        ui.checkbox(&mut inspector.enabled, "World inspector");
        ui.checkbox(&mut rapier_debug.enabled, "Show physics objects");
    });
}

fn debug_watermark(mut egui_context: ResMut<EguiContext>) {
    egui::Area::new("watermark")
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(0.0, 0.0))
        .order(egui::Order::Foreground)
        .show(egui_context.ctx_mut(), |ui| {
            ui.label(
                egui::RichText::new("SSNT Dev Build")
                    .color(egui::color::Rgba::WHITE)
                    .size(21.0),
            );
        });
}
