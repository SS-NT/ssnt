use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use networking::{
    identity::{NetworkIdentities, NetworkIdentity},
    is_server,
    messaging::{AppExt, MessageEvent, MessageSender},
    spawning::ClientControlled,
};
use serde::{Deserialize, Serialize};
use utils::order::Orderer;

use crate::{body::HandsClient, GameState};

use super::{
    containers::{Container, MoveItemOrder},
    Item, StoredItemClient,
};

pub struct ClothingPlugin;

impl Plugin for ClothingPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Clothing>()
            .register_type::<ClothingHolder>()
            .add_network_message::<EquipClothingMessage>();

        if is_server(app) {
            app.add_systems(
                Update,
                handle_equip_clothing_message
                    .run_if(on_event::<MessageEvent<EquipClothingMessage>>()),
            );
        } else {
            app.add_systems(Update, client_clothing_ui.run_if(in_state(GameState::Game)));
        }
    }
}

/// An item that can be worn.
#[derive(Component, Reflect)]
#[reflect(Component)]
struct Clothing {
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
struct ClothingHolder {
    clothing_type: String,
}

impl FromWorld for ClothingHolder {
    fn from_world(_: &mut World) -> Self {
        Self {
            clothing_type: "".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy)]
struct EquipClothingMessage {
    body_part: NetworkIdentity,
    clothing: NetworkIdentity,
}

fn client_clothing_ui(
    mut contexts: EguiContexts,
    mut bodies: Query<(Entity, Option<&HandsClient>), With<ClientControlled>>,
    child_query: Query<&Children>,
    clothing_holders: Query<(&NetworkIdentity, &ClothingHolder, Option<&Children>)>,
    clothing: Query<(&Clothing, &Item, &NetworkIdentity), With<StoredItemClient>>,
    identities: Res<NetworkIdentities>,
    mut sender: MessageSender,
) {
    let Ok((body_entity, hands )) = bodies.get_single_mut() else {
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
    mut move_orderer: Orderer<MoveItemOrder>,
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

        move_orderer.create(MoveItemOrder {
            item: clothing_entity,
            container: Some(holder_entity),
            position: Some(UVec2::ZERO),
        });
    }
}
