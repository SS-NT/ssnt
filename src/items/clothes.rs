use bevy::{prelude::*, utils::HashMap};
use bevy_egui::{egui, EguiContexts};
use networking::{
    identity::{NetworkIdentities, NetworkIdentity},
    is_server,
    messaging::{AppExt, MessageEvent, MessageSender},
    spawning::ClientControlled,
};
use serde::{Deserialize, Serialize};
use utils::task::{Task, TaskId, TaskStatus, Tasks};

use crate::{body::HandsClient, ui::has_window, GameState};

use super::{
    containers::{Container, MoveItem},
    Item, StoredItemClient,
};

pub struct ClothingPlugin;

impl Plugin for ClothingPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Clothing>()
            .register_type::<ClothingHolder>()
            .add_network_message::<EquipClothingMessage>();

        if is_server(app) {
            app.init_resource::<Tasks<EquipClothing>>().add_systems(
                Update,
                (
                    handle_equip_clothing_message
                        .run_if(on_event::<MessageEvent<EquipClothingMessage>>()),
                    process_equip_clothing.in_set(EquipClothingSystem),
                ),
            );
        } else {
            app.add_systems(
                Update,
                client_clothing_ui
                    .run_if(in_state(GameState::Game))
                    .run_if(has_window),
            );
        }
    }
}

/// An item that can be worn.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct Clothing {
    clothing_type: String,
    attachment_offset: Vec3,
}

impl FromWorld for Clothing {
    fn from_world(_: &mut World) -> Self {
        Self {
            clothing_type: "".into(),
            attachment_offset: Vec3::ZERO,
        }
    }
}

/// A body part on which clothing can be worn.
#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct ClothingHolder {
    clothing_type: String,
}

impl FromWorld for ClothingHolder {
    fn from_world(_: &mut World) -> Self {
        Self {
            clothing_type: "".into(),
        }
    }
}

pub struct EquipClothing {
    pub creature: Entity,
    pub clothing: Entity,
    pub slot: Option<Entity>,
}

impl Task for EquipClothing {
    type Result = Result<(), ()>;
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, SystemSet)]
pub struct EquipClothingSystem;

#[doc(hidden)]
#[derive(Default)]
pub enum EquipClothingState {
    #[default]
    Initial,
    Moving(TaskId<MoveItem>),
}

fn process_equip_clothing(
    mut tasks: ResMut<Tasks<EquipClothing>>,
    mut task_state: Local<HashMap<TaskId<EquipClothing>, EquipClothingState>>,
    mut item_move: ResMut<Tasks<MoveItem>>,
    clothing: Query<&Clothing>,
    child_query: Query<&Children>,
    clothing_holders: Query<&ClothingHolder>,
) {
    // Wow I reinvented a bad version of async, great
    tasks.try_process(&mut task_state, |data, state| match state {
        EquipClothingState::Initial => {
            let Ok(clothing) = clothing.get(data.clothing) else {
                return TaskStatus::Done(Err(()));
            };
            let slot_entity = match data.slot {
                Some(s) => s,
                None => {
                    let Some(e) = child_query.iter_descendants(data.creature).find(|entity| {
                        clothing_holders
                            .get(*entity)
                            .map(|holder| holder.clothing_type == clothing.clothing_type)
                            .ok()
                            .unwrap_or_default()
                    }) else {
                        return TaskStatus::Done(Err(()));
                    };
                    e
                }
            };
            let Ok(holder) = clothing_holders.get(slot_entity) else {
                return TaskStatus::Done(Err(()));
            };

            if holder.clothing_type != clothing.clothing_type {
                return TaskStatus::Done(Err(()));
            }

            let task = item_move.create(MoveItem {
                item: data.clothing,
                container: Some(slot_entity),
                position: Some(UVec2::ZERO),
            });
            *state = EquipClothingState::Moving(task);

            TaskStatus::Pending
        }
        EquipClothingState::Moving(task) => {
            let Some(move_result) = item_move.result(*task) else {
                return TaskStatus::Pending;
            };

            if move_result.was_success() {
                TaskStatus::Done(Ok(()))
            } else {
                TaskStatus::Done(Err(()))
            }
        }
    });
}

#[derive(Serialize, Deserialize, Clone, Copy)]
struct EquipClothingMessage {
    body_part: NetworkIdentity,
    clothing: NetworkIdentity,
}

fn client_clothing_ui(
    mut contexts: EguiContexts,
    bodies: Query<(Entity, Option<&HandsClient>), With<ClientControlled>>,
    child_query: Query<&Children>,
    clothing_holders: Query<(&NetworkIdentity, &ClothingHolder, Option<&Children>)>,
    clothing: Query<(&Clothing, &Item, &NetworkIdentity), With<StoredItemClient>>,
    identities: Res<NetworkIdentities>,
    mut sender: MessageSender,
) {
    let Ok((body_entity, hands)) = bodies.get_single() else {
        return;
    };
    let holders = child_query
        .iter_descendants(body_entity)
        .filter_map(|e| clothing_holders.get(e).ok());
    let held_clothing = hands
        .and_then(|h| identities.get_entity(h.active_hand()))
        .and_then(|e| child_query.get(e).ok())
        .and_then(|c| clothing.iter_many(c.iter()).next());

    egui::Window::new("Clothing")
        .anchor(egui::Align2::LEFT_BOTTOM, egui::Vec2::ZERO)
        .resizable(false)
        .show(contexts.ctx_mut(), |ui| {
            for (holder_id, holder, holder_children) in holders {
                ui.horizontal(|ui| {
                    // Check if clothing is equipped on the slot
                    let clothing_in_slot =
                        holder_children.and_then(|children| clothing.iter_many(children).next());
                    // Label slot
                    ui.label(format!(
                        "{} - {}",
                        holder.clothing_type,
                        if let Some((_, item, _)) = clothing_in_slot {
                            &item.name
                        } else {
                            "empty"
                        }
                    ));
                    if let Some((clothing, _, &clothing_id)) = held_clothing {
                        if clothing.clothing_type == holder.clothing_type
                            && ui.button("Equip").clicked()
                        {
                            sender.send_to_server(&EquipClothingMessage {
                                body_part: *holder_id,
                                clothing: clothing_id,
                            });
                        }
                    }
                });
            }
        });
}

fn handle_equip_clothing_message(
    mut messages: EventReader<MessageEvent<EquipClothingMessage>>,
    holders: Query<(&ClothingHolder, &Container)>,
    clothes: Query<&Clothing>,
    identities: Res<NetworkIdentities>,
    mut item_moves: ResMut<Tasks<MoveItem>>,
) {
    for event in messages.iter() {
        let message = &event.message;
        let Some(holder_entity) = identities.get_entity(message.body_part) else {
            continue;
        };

        let Ok((holder, container)) = holders.get(holder_entity) else {
            continue;
        };

        if !container.is_empty() {
            continue;
        }

        let Some(clothing_entity) = identities.get_entity(message.clothing) else {
            continue;
        };

        let Ok(clothing) = clothes.get(clothing_entity) else {
            continue;
        };

        if clothing.clothing_type != holder.clothing_type {
            continue;
        }

        // TODO: Verify sending client has access to body part and clothing

        item_moves.create_ignore(MoveItem {
            item: clothing_entity,
            container: Some(holder_entity),
            position: Some(UVec2::ZERO),
        });
    }
}
