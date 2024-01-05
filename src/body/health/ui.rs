use bevy::{prelude::*, reflect::TypeUuid, utils::HashMap};
use bevy_egui::{egui, EguiContexts};
use networking::{
    component::AppExt,
    identity::{EntityCommandsExt, NetworkIdentities, NetworkIdentity},
    is_server,
    messaging::{AppExt as MessageAppExt, MessageEvent, MessageSender},
    spawning::ClientControls,
    variable::{NetworkVar, ServerVar},
    visibility::AlwaysVisible,
    Networked, Players,
};
use serde::{Deserialize, Serialize};
use utils::task::Tasks;

use crate::{
    body::{Body, ClientHeldItem, Limb},
    interaction::{
        ActiveInteraction, ExecuteInteraction, GenerateInteractionList, InteractionListEvents,
        InteractionOption, InteractionSpecificity, InteractionStatus,
    },
    items::Item,
    ui::{has_window, CloseUiMessage, NetworkUi},
};

use super::{
    items::{ApplyMedicineInteraction, HealingItem},
    OrganicLaceration,
};

pub struct HealthUiPlugin;

impl Plugin for HealthUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_networked_component::<HealthUi, HealthUiClient>()
            .add_network_message::<ApplyMedicineMessage>();
        if is_server(app) {
            app.register_type::<InspectVitalsInteraction>().add_systems(
                Update,
                (
                    vitals_interaction,
                    prepare_vitals_interaction.in_set(GenerateInteractionList),
                    collect_vitals,
                    handle_apply_medicine,
                ),
            );
        } else {
            app.add_systems(Update, vitals_ui.run_if(has_window));
        }
    }
}

#[derive(Component, Networked)]
#[networked(client = "HealthUiClient")]
pub struct HealthUi {
    last_update: f32,
    target: NetworkVar<NetworkIdentity>,
    injuries: NetworkVar<HashMap<String, Vec<Injury>>>,
}

#[derive(Component, Default, TypeUuid, Networked)]
#[uuid = "1f5958eb-9709-4e56-891d-384819f4fb0d"]
#[networked(server = "HealthUi")]
pub(crate) struct HealthUiClient {
    target: ServerVar<NetworkIdentity>,
    injuries: ServerVar<HashMap<String, Vec<Injury>>>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Injury {
    server_entity: Entity,
    name: String,
}

#[derive(Component, Reflect)]
#[reflect(Component)]
#[component(storage = "SparseSet")]
struct InspectVitalsInteraction {
    viewer: Entity,
}

impl FromWorld for InspectVitalsInteraction {
    fn from_world(_: &mut World) -> Self {
        Self {
            viewer: Entity::from_raw(0),
        }
    }
}

fn prepare_vitals_interaction(
    interaction_list: Res<InteractionListEvents>,
    bodies: Query<(), With<Body>>,
) {
    for event in interaction_list.events.iter() {
        if !bodies.contains(event.target) {
            continue;
        }

        event.add_interaction(InteractionOption {
            text: "Inspect vitals".into(),
            interaction: Box::new(InspectVitalsInteraction {
                viewer: event.source,
            }),
            specificity: InteractionSpecificity::Generic,
        });
    }
}

fn vitals_interaction(
    mut query: Query<(&mut InspectVitalsInteraction, &mut ActiveInteraction)>,
    ids: Query<(Entity, &NetworkIdentity)>,
    mut commands: Commands,
) {
    for (interaction, mut active) in query.iter_mut() {
        let Ok((_, &network_id)) = ids.get(active.target) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        // Create UI
        commands
            .spawn((
                NetworkUi,
                HealthUi {
                    last_update: 0.0,
                    target: network_id.into(),
                    injuries: Default::default(),
                },
                AlwaysVisible::single(interaction.viewer),
            ))
            .networked();
        active.status = InteractionStatus::Completed;
    }
}

const COLLECT_VITALS_INTERVAL: f32 = 0.1;

fn collect_vitals(
    mut uis: Query<(Entity, &mut HealthUi)>,
    bodies: Query<&Body>,
    limbs: Query<(&Children, &Item), With<Limb>>,
    identities: Res<NetworkIdentities>,
    injuries: Query<(Entity, AnyOf<(&OrganicLaceration, ())>)>,
    time: Res<Time>,
    mut commands: Commands,
) {
    for (ui_entity, mut ui) in uis.iter_mut() {
        // Only update in interval
        let time = time.elapsed_seconds();
        if ui.last_update + COLLECT_VITALS_INTERVAL > time {
            continue;
        }
        ui.last_update = time;

        let Some(target_entity) = identities.get_entity(*ui.target) else {
            continue;
        };

        let Ok(body) = bodies.get(target_entity) else {
            commands.entity(ui_entity).despawn();
            continue;
        };

        // TODO: Reduce allocations
        let mut all_injuries = HashMap::default();
        for &limb in body.limbs.iter() {
            let Ok((children, item)) = limbs.get(limb) else {
                continue;
            };

            let mut limb_injuries = Vec::default();
            let name = &item.name;
            for (entity, (organic_laceration, _)) in injuries.iter_many(children) {
                if let Some(injury) = organic_laceration {
                    limb_injuries.push(Injury {
                        server_entity: entity,
                        name: format!("{} Laceration", injury.size),
                    });
                }
            }
            if limb_injuries.is_empty() {
                continue;
            }
            limb_injuries.sort_unstable_by_key(|i| i.server_entity);
            all_injuries.insert(name.clone(), limb_injuries);
        }

        if *ui.injuries != all_injuries {
            *ui.injuries = all_injuries;
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy)]
struct ApplyMedicineMessage {
    medicine: NetworkIdentity,
    wound: Entity,
}

fn handle_apply_medicine(
    mut messages: EventReader<MessageEvent<ApplyMedicineMessage>>,
    mut interactions: ResMut<Tasks<ExecuteInteraction>>,
    controls: Res<ClientControls>,
    identities: Res<NetworkIdentities>,
    players: Res<Players>,
) {
    for event in messages.iter() {
        let Some(player) = players.get(event.connection) else {
            continue;
        };
        let Some(entity) = controls.controlled_entity(player.id) else {
            continue;
        };
        let Some(medicine) = identities.get_entity(event.message.medicine) else {
            continue;
        };

        let wound = event.message.wound;
        interactions.create_ignore(ExecuteInteraction {
            entity,
            target: wound,
            interaction: Box::new(ApplyMedicineInteraction { medicine, wound }),
        });
    }
}

fn vitals_ui(
    mut contexts: EguiContexts,
    uis: Query<(Entity, &NetworkIdentity, &HealthUiClient)>,
    held_item: ClientHeldItem,
    healing_items: Query<&NetworkIdentity, With<HealingItem>>,
    mut sender: MessageSender,
) {
    for (entity, identity, health_ui) in uis.iter() {
        let held_entity = held_item
            .get()
            .and_then(|item| healing_items.get(item).ok());
        let mut keep_open = true;
        egui::Window::new("Vitals")
            .id(egui::Id::new(("vitals", entity)))
            .open(&mut keep_open)
            .show(contexts.ctx_mut(), |ui| {
                if health_ui.injuries.is_empty() {
                    ui.label("You find no injuries");
                    return;
                }

                for (body_part, injuries) in health_ui.injuries.iter() {
                    ui.label(body_part);
                    ui.indent("idk", |ui| {
                        for injury in injuries.iter() {
                            ui.horizontal(|ui| {
                                ui.label(injury.name.as_str());
                                if let Some(&medicine) = held_entity {
                                    ui.spacing();
                                    if ui.button("Apply").clicked() {
                                        sender.send_to_server(&ApplyMedicineMessage {
                                            medicine,
                                            wound: injury.server_entity,
                                        });
                                    }
                                }
                            });
                        }
                    });
                }
            });

        if !keep_open {
            sender.send_to_server(&CloseUiMessage { ui: *identity });
        }
    }
}
