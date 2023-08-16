use std::{net::SocketAddr, str::FromStr};

use bevy::prelude::*;
use bevy_egui::EguiContexts;
use bevy_inspector_egui::egui::{self, TextEdit};
use networking::{ClientEvent, TargetServer, UserData};

use crate::GameState;

pub struct MainMenuPlugin;

impl Plugin for MainMenuPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            (
                ui.run_if(in_state(GameState::MainMenu)),
                react_to_client_change,
            ),
        );
    }
}

#[derive(Resource)]
struct DisconnectReason {
    reason: String,
}

fn ui(
    mut contexts: EguiContexts,
    mut ip: Local<String>,
    mut name: Local<String>,
    mut client_events: EventWriter<ClientEvent>,
    disconnect: Option<Res<DisconnectReason>>,
    mut commands: Commands,
) {
    egui::Area::new("main buttons")
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(contexts.ctx_mut(), |ui| {
            ui.horizontal(|ui| {
                // TODO: Actually use name
                let name_field = TextEdit::singleline(&mut *name).hint_text("Name");
                if name_field.show(ui).response.changed() {
                    commands.insert_resource(UserData {
                        username: name.clone(),
                    });
                }

                let ip_field = TextEdit::singleline(&mut *ip).hint_text("Server IP");
                ip_field.show(ui);

                if ui.button("Join").clicked() {
                    if let Ok(address) = SocketAddr::from_str(ip.as_ref()) {
                        client_events.send(ClientEvent::Join(TargetServer::Raw(address)));
                    }
                }
            });

            if !ip.is_empty() && SocketAddr::from_str(ip.as_ref()).is_err() {
                ui.colored_label(egui::Color32::DARK_RED, "Invalid address");
            }

            if let Some(disconnect) = disconnect {
                ui.label("Connection failed");
                ui.colored_label(egui::Color32::RED, &disconnect.reason);
            }
        });
}

fn react_to_client_change(
    mut events: EventReader<ClientEvent>,
    mut game_state: ResMut<NextState<GameState>>,
    mut commands: Commands,
) {
    for event in events.iter() {
        match event {
            ClientEvent::Join(_) => {
                commands.remove_resource::<DisconnectReason>();
                game_state.set(GameState::Joining)
            }
            ClientEvent::Joined => game_state.set(GameState::Game),
            ClientEvent::JoinFailed(reason) | ClientEvent::Disconnected(reason) => {
                commands.insert_resource(DisconnectReason {
                    reason: reason.clone(),
                });
                game_state.set(GameState::MainMenu)
            }
        }
    }
}
