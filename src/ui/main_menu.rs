use std::{net::SocketAddr, str::FromStr};

use bevy::prelude::*;
use bevy_egui::EguiContext;
use bevy_inspector_egui::egui;
use networking::ClientEvent;

use crate::GameState;

pub struct MainMenuPlugin;

impl Plugin for MainMenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_system_set(SystemSet::on_update(GameState::MainMenu).with_system(ui))
            .add_system(react_to_client_change);
    }
}

fn ui(
    mut egui_context: ResMut<EguiContext>,
    mut ip: Local<String>,
    mut client_events: EventWriter<ClientEvent>,
) {
    egui::Area::new("main buttons")
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(egui_context.ctx_mut(), |ui| {
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut *ip);

                if ui.button("Join").clicked() {
                    if let Ok(address) = SocketAddr::from_str(ip.as_ref()) {
                        client_events.send(ClientEvent::Join(address));
                    }
                }
            });
            if !ip.is_empty() && SocketAddr::from_str(ip.as_ref()).is_err() {
                ui.colored_label(egui::Color32::DARK_RED, "Invalid address");
            }
        });
}

fn react_to_client_change(
    mut events: EventReader<ClientEvent>,
    mut game_state: ResMut<State<GameState>>,
) {
    for event in events.iter() {
        match event {
            ClientEvent::Join(_) => game_state.set(GameState::Joining),
            ClientEvent::Joined => game_state.set(GameState::Game),
            ClientEvent::JoinFailed => game_state.set(GameState::MainMenu), // TODO: show error
        }
        .unwrap();
    }
}
