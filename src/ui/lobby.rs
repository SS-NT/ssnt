use crate::{
    job::{JobDefinition, SelectJobMessage},
    round::{RequestJoin, RoundDataClient, RoundState, StartRoundRequest},
    GameState,
};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass};
use bevy_inspector_egui::egui;
use networking::{messaging::MessageSender, spawning::ClientControlled};

pub struct LobbyPlugin;

impl Plugin for LobbyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            EguiPrimaryContextPass,
            (ui, job_ui).run_if(in_state(GameState::Game)),
        );
    }
}

fn ui(
    mut contexts: EguiContexts,
    round_data: Option<Res<RoundDataClient>>,
    client_controlled: Query<(), With<ClientControlled>>,
    mut sender: MessageSender,
) {
    // Only show lobby UI if not controlling any entity
    if !client_controlled.is_empty() {
        return;
    }

    egui::Window::new("Lobby")
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(contexts.ctx_mut().unwrap(), |ui| {
            if let Some(data) = round_data {
                ui.label(format!("Round state: {:?}", data.state()));

                match data.state() {
                    RoundState::Ready => {
                        if ui.button("Start round").clicked() {
                            sender.send_to_server(&StartRoundRequest);
                        }
                    }
                    RoundState::Running => {
                        ui.label(format!("Round started tick: {}", data.start().unwrap()));
                        if ui.button("Join").clicked() {
                            sender.send_to_server(&RequestJoin);
                        }
                    }
                    _ => {}
                }
            } else {
                ui.label("Loading...");
            }
        });
}

fn job_ui(
    mut contexts: EguiContexts,
    client_controlled: Query<(), With<ClientControlled>>,
    jobs: Res<Assets<JobDefinition>>,
    asset_server: Res<AssetServer>,
    mut sender: MessageSender,
    mut selected_job: Local<Option<AssetId<JobDefinition>>>,
    mut sorted_jobs: Local<Vec<AssetId<JobDefinition>>>,
) {
    // Only show lobby UI if not controlling any entity
    if !client_controlled.is_empty() {
        return;
    }

    if jobs.len() != sorted_jobs.len() {
        let mut new_sorted: Vec<_> = jobs.iter().collect();
        new_sorted.sort_unstable_by_key(|x| &x.1.name);
        *sorted_jobs = new_sorted.into_iter().map(|x| x.0).collect();
    }

    let previous_job = *selected_job;
    egui::Window::new("Jobs")
        .anchor(egui::Align2::RIGHT_CENTER, egui::vec2(-30.0, 0.0))
        .show(contexts.ctx_mut().unwrap(), |ui| {
            for &id in sorted_jobs.iter() {
                let job_definition = jobs.get(id).unwrap();
                ui.radio_value(&mut *selected_job, Some(id), &job_definition.name);
                ui.label(&job_definition.description);
            }
        });

    if previous_job != *selected_job {
        let path = selected_job
            .and_then(|id| asset_server.get_path(id))
            .map(|p| p.to_string());
        sender.send_to_server(&SelectJobMessage { job: path });
    }
}
