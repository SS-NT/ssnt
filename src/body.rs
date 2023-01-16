use std::fmt;

use bevy::prelude::*;
use bevy_egui::{egui, EguiContext};
use networking::{
    identity::NetworkIdentity,
    is_server,
    messaging::{AppExt, MessageEvent, MessageSender},
    spawning::{ClientControlled, ClientControls},
    Players,
};
use serde::{Deserialize, Serialize};
use utils::order::{OrderId, Orderer, Results};

use crate::{
    event::*,
    interaction::{
        ActiveInteraction, InteractionListEvent, InteractionListRequest, InteractionOption,
        InteractionSpecificity, InteractionStatus,
    },
    items::{
        containers::{Container, MoveItemOrder, MoveItemResult},
        Item, StoredItem,
    },
};

pub struct BodyPlugin;

impl Plugin for BodyPlugin {
    fn build(&self, app: &mut App) {
        app.add_network_message::<ChangeHandRequest>();

        if is_server(app) {
            app.register_type::<PickupInteraction>()
                .add_system(pickup_interaction)
                .add_system(
                    prepare_pickup_interaction
                        .into_descriptor()
                        .intercept::<InteractionListEvent>(),
                )
                .register_type::<DropInteraction>()
                .add_system(drop_interaction)
                .add_system(
                    prepare_drop_interaction
                        .into_descriptor()
                        .intercept::<InteractionListEvent>(),
                )
                .add_system(handle_hand_change_request);
        } else {
            app.add_system(hand_ui);
        }
    }
}

#[derive(Component)]
pub struct Body {
    pub limbs: Vec<Entity>,
}

pub enum LimbSide {
    Left,
    Right,
}

impl fmt::Display for LimbSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LimbSide::Left => f.write_str("Left"),
            LimbSide::Right => f.write_str("Right"),
        }
    }
}

#[derive(Component)]
pub struct Hand {
    pub side: LimbSide,
}

#[derive(Component, Default)]
pub struct Hands {
    active_hand: usize,
}

#[derive(Serialize, Deserialize)]
struct ChangeHandRequest {
    index: usize,
}

fn hand_ui(
    mut egui_context: ResMut<EguiContext>,
    mut bodies: Query<(&Body, &mut Hands), With<ClientControlled>>,
    hands: Query<(&Hand, Option<&Children>)>,
    items: Query<(&Item, &NetworkIdentity)>,
    mut sender: MessageSender,
) {
    let Ok((body, mut hand_data)) = bodies.get_single_mut() else {
        return;
    };

    egui::Window::new("hands")
        .title_bar(false)
        .anchor(egui::Align2::CENTER_BOTTOM, egui::Vec2::ZERO)
        .resizable(false)
        .show(egui_context.ctx_mut(), |ui| {
            let mut new_active = None;

            for (index, (hand, children)) in hands.iter_many(&body.limbs).enumerate() {
                let mut held_item_name = None;
                let mut held_item_id = None;
                if let Some(children) = children {
                    if let Some(item_entity) = children.first() {
                        let (item, identity) = items.get(*item_entity).unwrap();
                        held_item_name = Some(item.name.as_str());
                        held_item_id = Some(*identity);
                    }
                }
                let label = ui.selectable_label(
                    index == hand_data.active_hand,
                    format!("{}: {}", hand.side, held_item_name.unwrap_or("empty")),
                );
                if label.clicked() {
                    sender.send_to_server(&ChangeHandRequest { index });
                    // Assume the server will let us change hands
                    // TODO: Do we need to verify this?
                    new_active = Some(index);
                } else if label.clicked_by(egui::PointerButton::Secondary) {
                    // Request interaction list on right-click
                    if let Some(target) = held_item_id {
                        sender.send_to_server(&InteractionListRequest { target });
                    }
                }
            }

            if let Some(index) = new_active {
                hand_data.active_hand = index;
            }
        });
}

fn handle_hand_change_request(
    mut events: EventReader<MessageEvent<ChangeHandRequest>>,
    players: Res<Players>,
    controls: Res<ClientControls>,
    mut hands: Query<&mut Hands>,
) {
    for event in events.iter() {
        let Some(controlled) = players.get(event.connection).and_then(|player| controls.controlled_entity(player.id)) else {
            continue;
        };
        let Ok(mut hands) = hands.get_mut(controlled)  else {
            continue;
        };
        // TODO: Validate index
        hands.active_hand = event.message.index;
    }
}

#[derive(Component, Reflect)]
#[reflect(Component)]
#[component(storage = "SparseSet")]
struct PickupInteraction {
    #[reflect(ignore)]
    move_order: Option<OrderId<MoveItemOrder>>,
}

impl PickupInteraction {
    fn new() -> Self {
        Self { move_order: None }
    }
}

// Dummy implementation for reflection
impl FromWorld for PickupInteraction {
    fn from_world(_: &mut World) -> Self {
        Self::new()
    }
}

fn prepare_pickup_interaction(
    events: Res<InterceptableEvents<InteractionListEvent>>,
    items: Query<&Item>,
    bodies: Query<(&Body, &Hands)>,
    hand_query: Query<(&Hand, &Container)>,
) {
    for event in events.iter() {
        let Ok(_) = items.get(event.target) else {
            continue;
        };

        let Ok((body, hands)) = bodies.get(event.source) else {
            continue;
        };

        let Some((_, hand_container)) = hand_query.iter_many(&body.limbs).nth(hands.active_hand) else {
            continue;
        };

        if !hand_container.is_empty() {
            continue;
        }

        event.add_interaction(InteractionOption {
            text: "Pick Up".into(),
            interaction: Box::new(PickupInteraction::new()),
            specificity: InteractionSpecificity::Generic,
        });
    }
}

fn pickup_interaction(
    mut query: Query<(Entity, &mut PickupInteraction, &mut ActiveInteraction)>,
    items: Query<&Item>,
    bodies: Query<(&Body, &Hands)>,
    hand_query: Query<(Entity, &Hand, &Container)>,
    mut move_orderer: Orderer<MoveItemOrder>,
    mut move_results: Results<MoveItemOrder, MoveItemResult>,
) {
    for (source, mut interaction, mut active) in query.iter_mut() {
        if interaction.move_order.is_some() {
            continue;
        }

        let Ok(_) = items.get(active.target) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        let Ok((body, hands)) = bodies.get(source) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        let Some((hand_entity, _, hand_container)) = hand_query.iter_many(&body.limbs).nth(hands.active_hand) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        if !hand_container.is_empty() {
            active.status = InteractionStatus::Canceled;
            continue;
        }

        // Sending an order to move the target item
        let id = move_orderer.create(MoveItemOrder {
            item: active.target,
            container: Some(hand_entity),
            position: Some(UVec2::ZERO),
        });
        interaction.move_order = Some(id);
    }

    // Check for completed container moves
    for result in move_results.iter() {
        for (_, interaction, mut active) in query.iter_mut() {
            if interaction.move_order == Some(result.id) {
                active.status = if result.data.was_success() {
                    InteractionStatus::Completed
                } else {
                    InteractionStatus::Canceled
                };

                break;
            }
        }
    }
}

#[derive(Component, Reflect)]
#[reflect(Component)]
#[component(storage = "SparseSet")]
struct DropInteraction {
    #[reflect(ignore)]
    move_order: Option<OrderId<MoveItemOrder>>,
}

impl DropInteraction {
    fn new() -> Self {
        Self { move_order: None }
    }
}

// Dummy implementation for reflection
impl FromWorld for DropInteraction {
    fn from_world(_: &mut World) -> Self {
        Self::new()
    }
}

fn prepare_drop_interaction(
    events: Res<InterceptableEvents<InteractionListEvent>>,
    items: Query<&StoredItem>,
    bodies: Query<&Body>,
    hand_query: Query<Entity, With<Hand>>,
) {
    for event in events.iter() {
        let Ok(stored) = items.get(event.target) else {
            continue;
        };
        let container_entity = stored.container();

        let Ok(body) = bodies.get(event.source) else {
            continue;
        };

        let Some(_) = hand_query.iter_many(&body.limbs).find(|entity| container_entity == *entity) else {
            continue;
        };

        event.add_interaction(InteractionOption {
            text: "Drop".into(),
            interaction: Box::new(DropInteraction::new()),
            specificity: InteractionSpecificity::Generic,
        });
    }
}

fn drop_interaction(
    mut query: Query<(Entity, &mut DropInteraction, &mut ActiveInteraction)>,
    items: Query<&StoredItem>,
    bodies: Query<&Body>,
    hand_query: Query<Entity, With<Hand>>,
    mut move_orderer: Orderer<MoveItemOrder>,
    mut move_results: Results<MoveItemOrder, MoveItemResult>,
) {
    for (source, mut interaction, mut active) in query.iter_mut() {
        if interaction.move_order.is_some() {
            continue;
        }

        let Ok(stored) = items.get(active.target) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };
        let container_entity = stored.container();

        let Ok(body) = bodies.get(source) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        let Some(_) = hand_query.iter_many(&body.limbs).find(|entity| container_entity == *entity) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        // Sending an order to move the item held
        let id = move_orderer.create(MoveItemOrder {
            item: active.target,
            container: None,
            position: None,
        });
        interaction.move_order = Some(id);
    }

    // Check for completed container moves
    for result in move_results.iter() {
        for (_, interaction, mut active) in query.iter_mut() {
            if interaction.move_order == Some(result.id) {
                active.status = if result.data.was_success() {
                    InteractionStatus::Completed
                } else {
                    InteractionStatus::Canceled
                };

                break;
            }
        }
    }
}
