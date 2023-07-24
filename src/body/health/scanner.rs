use bevy::{prelude::*, reflect::TypeUuid};
use bevy_egui::{egui, EguiContexts};
use networking::{
    component::AppExt as ComponentExt,
    identity::{NetworkIdentities, NetworkIdentity},
    is_server,
    messaging::{AppExt, MessageEvent, MessageReceivers, MessageSender},
    spawning::ClientControls,
    variable::{NetworkVar, ServerVar},
    Networked, Players,
};
use serde::{Deserialize, Serialize};

use crate::{
    body::Body,
    interaction::{
        ActiveInteraction, GenerateInteractionList, InteractionListEvents, InteractionOption,
        InteractionSpecificity, InteractionStatus,
    },
};

use super::{OrganicBody, OrganicHeart, MAX_BLOOD_OXYGEN};

pub struct HealthScannerPlugin;

impl Plugin for HealthScannerPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<HealthScanner>()
            .add_networked_component::<HealthScanner, HealthScannerClient>()
            .add_network_message::<OpenHealthScannerMessage>();
        if is_server(app) {
            app.register_type::<HealthScanInteraction>().add_systems(
                Update,
                (
                    collect_vitals,
                    health_scan_interaction,
                    prepare_scan_interaction.in_set(GenerateInteractionList),
                ),
            );
        } else {
            app.add_systems(Update, health_scanner_ui);
        }
    }
}

#[derive(Component, Default, Reflect, Networked)]
#[reflect(Component)]
#[networked(client = "HealthScannerClient")]
pub struct HealthScanner {
    #[reflect(ignore)]
    target: NetworkVar<Option<NetworkIdentity>>,
    #[reflect(ignore)]
    vitals: NetworkVar<Option<Vitals>>,
    last_update: f32,
    update_frequency: f32,
}

#[derive(Component, Default, TypeUuid, Networked)]
#[uuid = "f34a6894-fecf-49d5-9f32-89b3cbf8e689"]
#[networked(server = "HealthScanner")]
pub(crate) struct HealthScannerClient {
    target: ServerVar<Option<NetworkIdentity>>,
    vitals: ServerVar<Option<Vitals>>,
    open: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct Vitals {
    bpm: u32,
    blood: f32,
    blood_capacity: f32,
    oxygen_in_blood: f32,
    oxygen_capacity: f32,
    max_oxygen_capacity: f32,
}

fn collect_vitals(
    mut scanners: Query<&mut HealthScanner>,
    identities: Res<NetworkIdentities>,
    bodies: Query<(&Body, &OrganicBody)>,
    hearts: Query<&OrganicHeart>,
    time: Res<Time>,
) {
    for mut scanner in scanners.iter_mut() {
        let Some(target_id) = *scanner.target else {
            continue;
        };
        if scanner.last_update + scanner.update_frequency > time.elapsed_seconds() {
            continue;
        }
        scanner.last_update = time.elapsed_seconds();

        let Some(target_entity) = identities.get_entity(target_id) else {
            *scanner.vitals = None;
            continue;
        };
        let Ok((body, organic_body)) = bodies.get(target_entity) else {
            *scanner.vitals = None;
            continue;
        };

        // Grab heart beat of first heart, or zero
        let bpm = hearts
            .iter_many(&body.limbs)
            .next()
            .map(|heart| heart.heart_rate)
            .unwrap_or_default();
        let vitals = Vitals {
            blood: organic_body.blood,
            blood_capacity: organic_body.blood_capacity,
            oxygen_in_blood: organic_body.oxygen_in_blood,
            bpm,
            oxygen_capacity: organic_body.oxygen_capacity(),
            max_oxygen_capacity: organic_body.blood_capacity * MAX_BLOOD_OXYGEN,
        };
        *scanner.vitals = Some(vitals);
    }
}

fn health_scanner_ui(
    mut contexts: EguiContexts,
    mut scanners: Query<(Entity, &mut HealthScannerClient)>,
    identities: Res<NetworkIdentities>,
    mut open_messages: EventReader<MessageEvent<OpenHealthScannerMessage>>,
) {
    // Open any UIs if requested
    for event in open_messages.iter() {
        let Some(scanner_entity) = identities.get_entity(event.message.scanner) else {
            bevy::log::info!("No entity found");
            continue;
        };
        let Ok((_, mut scanner)) = scanners.get_mut(scanner_entity) else {
            bevy::log::info!("No health scanner");
            continue;
        };
        scanner.open = true;
    }

    for (entity, mut scanner) in scanners.iter_mut() {
        if !scanner.open {
            continue;
        }

        let mut keep_open = true;
        egui::Window::new("Health Scanner")
            .id(egui::Id::new(("health scanner", entity)))
            .open(&mut keep_open)
            .show(contexts.ctx_mut(), |ui| {
                if let Some(target) = *scanner.target {
                    if let Some(vitals) = &*scanner.vitals {
                        ui.label(format!("BPM: {}", vitals.bpm));
                        ui.label(format!(
                            "Blood level: {:.0}% ({:.2}/{:.2}l)",
                            vitals.blood / vitals.blood_capacity * 100.0,
                            vitals.blood,
                            vitals.blood_capacity
                        ));
                        ui.label(format!(
                            "Oxygen blood saturation: {:.0}% ({:.2}/{:.2}l)",
                            vitals.oxygen_in_blood / vitals.oxygen_capacity * 100.0,
                            vitals.oxygen_in_blood,
                            vitals.oxygen_capacity
                        ));
                        ui.label(format!(
                            "Oxygen level: {:.0}% ({:.2}/{:.2}l)",
                            vitals.oxygen_in_blood / vitals.max_oxygen_capacity * 100.0,
                            vitals.oxygen_in_blood,
                            vitals.max_oxygen_capacity
                        ));
                    } else {
                        ui.label("No vitals available");
                    }
                } else {
                    ui.label("No target selected");
                }
            });

        if !keep_open {
            scanner.open = false;
        }
    }
}

#[derive(Component, Reflect)]
#[reflect(Component)]
#[component(storage = "SparseSet")]
struct HealthScanInteraction {
    viewer: Entity,
    scanner: Entity,
}

impl FromWorld for HealthScanInteraction {
    fn from_world(_: &mut World) -> Self {
        Self {
            viewer: Entity::from_raw(0),
            scanner: Entity::from_raw(0),
        }
    }
}

fn prepare_scan_interaction(
    interaction_list: Res<InteractionListEvents>,
    scanners: Query<(), With<HealthScanner>>,
    bodies: Query<(), With<Body>>,
) {
    for event in interaction_list.events.iter() {
        let Some(item) = event.item_in_hand else {
            continue;
        };

        if !scanners.contains(item) {
            continue;
        }

        if !bodies.contains(event.target) {
            continue;
        }

        event.add_interaction(InteractionOption {
            text: "Scan".into(),
            interaction: Box::new(HealthScanInteraction {
                viewer: event.source,
                scanner: item,
            }),
            specificity: InteractionSpecificity::Specific,
        });
    }
}

fn health_scan_interaction(
    mut query: Query<(&mut HealthScanInteraction, &mut ActiveInteraction)>,
    mut scanners: Query<&mut HealthScanner>,
    mut bodies: Query<&mut Body>,
    ids: Query<(Entity, &NetworkIdentity)>,
    controls: Res<ClientControls>,
    players: Res<Players>,
    mut sender: MessageSender,
) {
    for (interaction, mut active) in query.iter_mut() {
        let Ok(mut scanner) = scanners.get_mut(interaction.scanner) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        let Ok((_, &network_id)) = ids.get(active.target) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        // TODO: Wait for scanning
        *scanner.target = Some(network_id);

        // Send open message to player
        // TODO: This needs an abstraction so bad
        if let Some(player) = controls.controlling_player(interaction.viewer) {
            if let Some(connection) = players.get_connection(&player) {
                let Ok((_, &scanner_network_id)) = ids.get(interaction.scanner) else {
                    active.status = InteractionStatus::Canceled;
                    continue;
                };
                sender.send(
                    &OpenHealthScannerMessage {
                        scanner: scanner_network_id,
                    },
                    MessageReceivers::Single(connection),
                );
            }
        }

        active.status = InteractionStatus::Completed;
    }
}

#[derive(Serialize, Deserialize)]
struct OpenHealthScannerMessage {
    scanner: NetworkIdentity,
}
