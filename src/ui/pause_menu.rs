use bevy::prelude::*;
use bevy_egui::EguiContext;
use bevy_inspector_egui::egui;
use networking::{ClientOrder, ClientState};

pub struct PauseMenuPlugin;

impl Plugin for PauseMenuPlugin {
    fn build(&self, app: &mut App) {
        // TODO: Only run while in game
        app.add_system(ui);
    }
}

fn ui(
    mut egui_context: ResMut<EguiContext>,
    keys: Res<Input<KeyCode>>,
    mut visible: Local<bool>,
    state: Res<State<ClientState>>,
    mut orders: EventWriter<ClientOrder>,
) {
    if !matches!(state.current(), ClientState::Connected) {
        *visible = false;
        return;
    }

    if keys.just_pressed(KeyCode::Escape) {
        *visible = !*visible;
    }

    if !*visible {
        return;
    }

    egui::Window::new("pause menu")
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .title_bar(false)
        .default_width(50.0)
        .show(egui_context.ctx_mut(), |ui| {
            ui.vertical_centered(|ui| {
                if ui.button("Resume").clicked() {
                    *visible = !*visible;
                }
                ui.add_space(5.0);
                if ui.button("Leave").clicked() {
                    orders.send(ClientOrder::Leave);
                }
            });
        });
}
