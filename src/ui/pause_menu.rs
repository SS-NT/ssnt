use bevy::prelude::*;
use bevy_egui::EguiContexts;
use bevy_inspector_egui::egui;
use networking::{ClientState, ClientTask};

use crate::GameState;

use super::has_window;

pub struct PauseMenuPlugin;

impl Plugin for PauseMenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            ui.run_if(in_state(GameState::Game)).run_if(has_window),
        );
    }
}

fn ui(
    mut contexts: EguiContexts,
    keys: Res<Input<KeyCode>>,
    mut visible: Local<bool>,
    state: Res<State<ClientState>>,
    mut tasks: EventWriter<ClientTask>,
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
        .show(contexts.ctx_mut(), |ui| {
            ui.vertical_centered(|ui| {
                if ui.button("Resume").clicked() {
                    *visible = !*visible;
                }
                ui.add_space(5.0);
                if ui.button("Leave").clicked() {
                    tasks.send(ClientTask::Leave);
                }
            });
        });
}
