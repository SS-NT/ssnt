use crate::{
    job::{JobDefinition, SelectJobMessage},
    round::{RequestJoin, RoundDataClient, RoundState, StartRoundRequest},
    GameState,
};
use bevy::{asset::HandleId, prelude::*};
use bevy_egui::EguiContext;
use bevy_inspector_egui::egui;
use networking::{messaging::MessageSender, spawning::ClientControlled};

pub struct LobbyPlugin;

impl Plugin for LobbyPlugin {
    fn build(&self, app: &mut App) {
        app.add_system_set(
            SystemSet::on_update(GameState::Game)
                .with_system(ui)
                .with_system(job_ui),
        );
    }
}

fn ui(
    mut egui_context: ResMut<EguiContext>,
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
        .show(egui_context.ctx_mut(), |ui| {
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
    mut egui_context: ResMut<EguiContext>,
    client_controlled: Query<(), With<ClientControlled>>,
    jobs: Res<Assets<JobDefinition>>,
    mut sender: MessageSender,
    mut selected_job: Local<Option<HandleId>>,
    mut sorted_jobs: Local<Vec<Handle<JobDefinition>>>,
) {
    // Only show lobby UI if not controlling any entity
    if !client_controlled.is_empty() {
        return;
    }

    if jobs.len() != sorted_jobs.len() {
        let mut new_sorted: Vec<_> = jobs.iter().collect();
        new_sorted.sort_unstable_by_key(|x| &x.1.name);
        sorted_jobs.clear();
        sorted_jobs.extend(
            new_sorted
                .into_iter()
                .map(|x| Handle::<JobDefinition>::weak(x.0)),
        );
    }

    let previous_job = *selected_job;
    egui::Window::new("Jobs")
        .anchor(egui::Align2::RIGHT_CENTER, egui::vec2(-30.0, 0.0))
        .show(egui_context.ctx_mut(), |ui| {
            for handle in sorted_jobs.iter() {
                let job_definition = jobs.get(handle).unwrap();
                ui.radio_value(&mut *selected_job, Some(handle.id()), &job_definition.name);
                ui.label(&job_definition.description);
            }
        });

    if previous_job != *selected_job {
        let asset_id = selected_job.map(|handle| match handle {
            HandleId::Id(_, _) => panic!("Job must be asset"),
            HandleId::AssetPathId(id) => id,
        });
        sender.send_to_server(&SelectJobMessage { job: asset_id });
    }
}
