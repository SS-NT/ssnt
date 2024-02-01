use bevy::{prelude::*, reflect::TypeUuid, utils::HashMap};
use bevy_egui::{egui, EguiContexts};
use networking::{
    component::AppExt as _,
    identity::{EntityCommandsExt as _, NetworkIdentities, NetworkIdentity},
    is_server,
    messaging::{AppExt, MessageEvent, MessageSender},
    variable::{NetworkVar, ServerVar},
    visibility::AlwaysVisible,
    Networked,
};
use serde::{Deserialize, Serialize};
use utils::task::Tasks;

use crate::{
    interaction::{
        ActiveInteraction, GenerateInteractionList, InteractionListEvents, InteractionOption,
        InteractionSpecificity, InteractionStatus,
    },
    items::{Item, StoredItemClient},
    ui::{has_window, CloseUiMessage, NetworkUi},
};

use super::{Container, MoveItem};

pub struct ContainerUiPlugin;

impl Plugin for ContainerUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_networked_component::<ContainerUi, ContainerUiClient>()
            .add_network_message::<MoveItemMessage>();
        if is_server(app) {
            app.register_type::<ViewContainerInteraction>()
                .register_type::<InsertItemInteraction>()
                .add_systems(
                    Update,
                    (
                        view_interaction,
                        insert_interaction,
                        (prepare_view_interaction, prepare_insert_interaction)
                            .in_set(GenerateInteractionList),
                        handle_move_message,
                    ),
                );
        } else {
            app.init_resource::<DraggedItem>()
                .add_systems(Update, container_ui.run_if(has_window));
        }
    }
}

#[derive(Component, Networked)]
#[networked(client = "ContainerUiClient")]
struct ContainerUi {
    container: NetworkVar<NetworkIdentity>,
}

#[derive(Component, TypeUuid, Default, Networked)]
#[uuid = "56ca80f9-e239-48f9-86c3-4bf06249ec0e"]
#[networked(server = "ContainerUi")]
struct ContainerUiClient {
    container: ServerVar<NetworkIdentity>,
}

#[derive(Resource, Default)]
struct DraggedItem {
    info: Option<DragInfo>,
}

struct DragInfo {
    entity: Entity,
    size: UVec2,
    over_anything: bool,
    just_dropped: bool,
}

const SLOT_SIZE: egui::Vec2 = egui::vec2(36.0, 36.0);

#[allow(clippy::too_many_arguments)]
fn container_ui(
    mut contexts: EguiContexts,
    uis: Query<(Entity, &NetworkIdentity, &ContainerUiClient)>,
    mut items: Query<(Entity, &NetworkIdentity, &Item, &mut StoredItemClient)>,
    containers: Query<(&Container, &Children)>,
    identities: Res<NetworkIdentities>,
    mut dragged: ResMut<DraggedItem>,
    mut sender: MessageSender,
    mut commands: Commands,
) {
    for (ui_entity, identity, container_ui) in uis.iter() {
        let Some(container_entity) = identities.get_entity(*container_ui.container) else {
            continue;
        };
        let Ok((container, children)) = containers.get(container_entity) else {
            continue;
        };

        let stored: HashMap<_, _> = items
            .iter_many(children)
            .map(|(entity, _, item, stored)| (*stored.slot, (entity, item.name.clone(), item.size)))
            .collect();

        let mut keep_open = true;
        egui::Window::new("Container")
            .id(egui::Id::new(("container", ui_entity)))
            .open(&mut keep_open)
            .show(contexts.ctx_mut(), |ui| {
                let anything_dragged = ui.memory(|mem| mem.is_anything_being_dragged());
                if !anything_dragged {
                    dragged.info = None;
                }

                let (grid_rect, grid_response) = ui.allocate_at_least(
                    egui::vec2(
                        SLOT_SIZE.x * container.size.x as f32,
                        SLOT_SIZE.y * container.size.y as f32,
                    ),
                    egui::Sense::hover(),
                );

                // Find where dragged item would be dropped
                let mut drop_item = None;
                if let Some(info) = dragged.info.as_ref() {
                    if let Some(hover_pos) = grid_response.hover_pos() {
                        let offset = hover_pos
                            - egui::vec2(
                                (info.size.x) as f32 * SLOT_SIZE.x / 2.0,
                                (info.size.y) as f32 * SLOT_SIZE.y / 2.0,
                            )
                            - grid_rect.left_top();
                        let position = UVec2::new(
                            (offset.x / SLOT_SIZE.x).round() as u32,
                            (offset.y / SLOT_SIZE.y).round() as u32,
                        );
                        drop_item = Some((info.entity, position, info.size));
                    }
                }

                // Paint slots
                for y in 0..container.size.y {
                    for x in 0..container.size.x {
                        let slot_rect = egui::Rect::from_min_size(
                            egui::pos2(x as f32 * SLOT_SIZE.x, y as f32 * SLOT_SIZE.y),
                            egui::vec2(SLOT_SIZE.x, SLOT_SIZE.y),
                        )
                        .translate(grid_rect.left_top().to_vec2());
                        ui.painter().rect(
                            slot_rect,
                            0.,
                            egui::Color32::from_gray(32),
                            egui::Stroke::new(0.8, egui::Color32::from_gray(80)),
                        );
                    }
                }

                // Draw dragged item preview in container
                if let Some((entity, position, size)) = drop_item {
                    let x_slots_to_draw = size.x.min(container.size.x - position.x);
                    let y_slots_to_draw = size.y.min(container.size.y - position.y);
                    let out_of_bounds = x_slots_to_draw != size.x || y_slots_to_draw != size.y;
                    if x_slots_to_draw != 0 && y_slots_to_draw != 0 {
                        dragged.info.as_mut().unwrap().over_anything = true;

                        let item_rect = egui::Rect::from_min_size(
                            egui::pos2(
                                position.x as f32 * SLOT_SIZE.x,
                                position.y as f32 * SLOT_SIZE.y,
                            ),
                            egui::vec2(
                                SLOT_SIZE.x * x_slots_to_draw as f32,
                                SLOT_SIZE.y * y_slots_to_draw as f32,
                            ),
                        )
                        .translate(grid_rect.left_top().to_vec2());
                        ui.painter().rect(
                            item_rect,
                            0.,
                            if out_of_bounds {
                                egui::Color32::RED
                            } else {
                                egui::Color32::GREEN
                            }
                            .gamma_multiply(0.25),
                            egui::Stroke::NONE,
                        );
                    }

                    if !out_of_bounds {
                        // Drop if pointer released
                        if ui.input(|i| i.pointer.any_released()) {
                            if let Ok((item_entity, &identity, _, mut item)) = items.get_mut(entity)
                            {
                                // Tell server to move it
                                sender.send_to_server(&MoveItemMessage {
                                    item: identity,
                                    to_container: Some(*container_ui.container),
                                    to_slot: position,
                                });
                                // Predict item move
                                // TODO: actually do rollback on fail
                                item.slot.set(position);
                                item.container.set(*container_ui.container);
                                commands.entity(item_entity).set_parent(container_entity);

                                dragged.info.as_mut().unwrap().just_dropped = true;
                            }
                        }
                    }
                }

                // Paint all items in container
                for (position, (item_entity, name, size)) in stored.iter() {
                    let id = egui::Id::new(item_entity).with(container_entity);
                    let is_being_dragged = ui.memory(|mem| mem.is_being_dragged(id));
                    let item_rect = egui::Rect::from_min_size(
                        egui::pos2(
                            position.x as f32 * SLOT_SIZE.x,
                            position.y as f32 * SLOT_SIZE.y,
                        ),
                        egui::vec2(SLOT_SIZE.x * size.x as f32, SLOT_SIZE.y * size.y as f32),
                    )
                    .translate(grid_rect.left_top().to_vec2());
                    if is_being_dragged {
                        if dragged.info.is_none() {
                            dragged.info = Some(DragInfo {
                                entity: *item_entity,
                                size: *size,
                                over_anything: false,
                                just_dropped: false,
                            });
                        }
                        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);

                        let layer_id = egui::LayerId::new(egui::Order::Tooltip, id);
                        ui.with_layer_id(layer_id, |ui| {
                            draw_item(ui, item_rect, name);
                        });

                        if let Some(pointer_pos) = ui.ctx().pointer_interact_pos() {
                            let delta = pointer_pos - item_rect.center();
                            ui.ctx().translate_layer(layer_id, delta);
                        }
                    } else {
                        ui.interact(item_rect, id, egui::Sense::drag());
                        draw_item(ui, item_rect, name);
                    }
                }
            });

        if !keep_open {
            sender.send_to_server(&CloseUiMessage { ui: *identity });
        }
    }

    // Dropping over empty space
    if let Some(info) = dragged.info.as_ref() {
        if !info.over_anything
            && !info.just_dropped
            && contexts.ctx_mut().input(|i| i.pointer.any_released())
        {
            let Ok((_, &item, ..)) = items.get(info.entity) else {
                return;
            };
            sender.send_to_server(&MoveItemMessage {
                item,
                to_container: None,
                to_slot: UVec2::ZERO,
            });
        }
    }

    // Remove old drag info
    if let Some(info) = dragged.info.as_mut() {
        if info.just_dropped {
            dragged.info = None;
        } else {
            info.over_anything = false;
        }
    }
}

fn draw_item(ui: &mut egui::Ui, item_rect: egui::Rect, name: &str) {
    ui.painter().rect(
        item_rect,
        0.,
        egui::Color32::from_white_alpha(16),
        egui::Stroke::new(1.0, egui::Color32::WHITE),
    );
    let styled_text = egui::RichText::new(name)
        .size(11.0)
        .color(egui::Color32::WHITE);
    ui.put(item_rect, egui::Label::new(styled_text));
}

#[derive(Serialize, Deserialize)]
struct MoveItemMessage {
    item: NetworkIdentity,
    to_container: Option<NetworkIdentity>,
    to_slot: UVec2,
}

fn handle_move_message(
    mut messages: EventReader<MessageEvent<MoveItemMessage>>,
    identities: Res<NetworkIdentities>,
    mut item_moves: ResMut<Tasks<MoveItem>>,
) {
    for event in messages.iter() {
        let message = &event.message;
        let Some(item_entity) = identities.get_entity(message.item) else {
            continue;
        };
        let container_entity = message.to_container.and_then(|i| identities.get_entity(i));
        item_moves.create_ignore(MoveItem {
            item: item_entity,
            container: container_entity,
            position: Some(message.to_slot),
        })

        // TODO: Support rollback
    }
}
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
#[component(storage = "SparseSet")]
struct ViewContainerInteraction {}

fn prepare_view_interaction(
    interaction_lists: Res<InteractionListEvents>,
    containers: Query<&Container>,
) {
    for event in interaction_lists.events.iter() {
        let Ok(_) = containers.get(event.target) else {
            continue;
        };

        event.add_interaction(InteractionOption {
            text: "View container".into(),
            interaction: Box::<ViewContainerInteraction>::default(),
            specificity: InteractionSpecificity::Common,
        });
    }
}

fn view_interaction(
    mut query: Query<(
        Entity,
        &mut ViewContainerInteraction,
        &mut ActiveInteraction,
    )>,
    containers: Query<&NetworkIdentity, With<Container>>,
    mut commands: Commands,
) {
    for (source, _, mut active) in query.iter_mut() {
        let Ok(identity) = containers.get(active.target) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        commands
            .spawn((
                NetworkUi,
                ContainerUi {
                    container: (*identity).into(),
                },
                AlwaysVisible::single(source),
            ))
            .networked();
        active.status = InteractionStatus::Completed;
    }
}

#[derive(Component, Reflect)]
#[reflect(Component)]
#[component(storage = "SparseSet")]
struct InsertItemInteraction {
    item: Entity,
}

impl FromWorld for InsertItemInteraction {
    fn from_world(_: &mut World) -> Self {
        Self {
            item: Entity::PLACEHOLDER,
        }
    }
}

fn prepare_insert_interaction(
    interaction_lists: Res<InteractionListEvents>,
    containers: Query<&Container>,
) {
    for event in interaction_lists.events.iter() {
        let Ok(_) = containers.get(event.target) else {
            continue;
        };

        let Some(item) = event.item_in_hand else {
            continue;
        };

        // Don't let a container be inserted into itself
        if event.target == item {
            continue;
        }

        event.add_interaction(InteractionOption {
            text: "Insert".into(),
            interaction: Box::new(InsertItemInteraction { item }),
            specificity: InteractionSpecificity::Common,
        });
    }
}

fn insert_interaction(
    mut query: Query<(Entity, &mut InsertItemInteraction, &mut ActiveInteraction)>,
    containers: Query<Entity, With<Container>>,
    mut move_tasks: ResMut<Tasks<MoveItem>>,
) {
    for (_, interaction, mut active) in query.iter_mut() {
        let Ok(container) = containers.get(active.target) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        move_tasks.create_ignore(MoveItem {
            item: interaction.item,
            container: Some(container),
            position: None,
        });
        active.status = InteractionStatus::Completed;
    }
}
