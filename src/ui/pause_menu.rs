use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use bevy_inspector_egui::egui;
use networking::{ClientState, ClientTask};

use crate::GameState;

pub struct PauseMenuPlugin;

impl Plugin for PauseMenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(EguiPrimaryContextPass, ui.run_if(in_state(GameState::Game)));
    }
}

fn ui(
    mut contexts: EguiContexts,
    keys: Res<ButtonInput<KeyCode>>,
    mut visible: Local<bool>,
    state: Res<State<ClientState>>,
    mut tasks: MessageWriter<ClientTask>,
) {
    if !matches!(state.get(), ClientState::Connected) {
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
        .show(contexts.ctx_mut().unwrap(), |ui| {
            ui.vertical_centered(|ui| {
                if ui.button("Resume").clicked() {
                    *visible = !*visible;
                }
                ui.add_space(5.0);
                if ui.button("Leave").clicked() {
                    tasks.write(ClientTask::Leave);
                }
            });
        });
}
