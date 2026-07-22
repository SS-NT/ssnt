use bevy::{platform::collections::HashMap, prelude::*, reflect::TypePath};
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
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
    body::ClientFreeHand,
    interaction::{
        ActiveInteraction, GenerateInteractionList, InteractionListEvents, InteractionOption,
        InteractionSpecificity, InteractionStatus,
    },
    items::{Item, StoredItemClient},
    ui::{CloseUiMessage, NetworkUi},
};

use super::{Container, MoveItem};

pub struct ContainerUiPlugin;

/// System set containing the container drag/drop UI. Other drag sources/targets
/// must run before this.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ContainerDropSet;

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
            app.init_resource::<DraggedItem>().add_systems(
                EguiPrimaryContextPass,
                container_ui.in_set(ContainerDropSet),
            );
        }
    }
}

#[derive(Component, TypePath, Networked)]
#[networked(client = "ContainerUiClient")]
pub(super) struct ContainerUi {
    pub(super) container: NetworkVar<NetworkIdentity>,
    pub(super) viewer: Entity,
}

#[derive(Component, TypePath, Default, Networked)]
#[networked(server = "ContainerUi")]
struct ContainerUiClient {
    container: ServerVar<NetworkIdentity>,
}

#[derive(Resource, Default)]
pub(crate) struct DraggedItem {
    pub(crate) info: Option<DragInfo>,
}

pub(crate) struct DragInfo {
    pub(crate) entity: Entity,
    pub(crate) size: UVec2,
    pub(crate) over_anything: bool,
    pub(crate) just_dropped: bool,
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
    free_hand: ClientFreeHand,
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
            .show(contexts.ctx_mut().unwrap(), |ui| {
                let anything_dragged = ui.ctx().dragged_id().is_some();
                let released_this_frame = ui.input(|i| i.pointer.any_released());
                if !anything_dragged && !released_this_frame {
                    dragged.info = None;
                }

                let (grid_rect, _) = ui.allocate_at_least(
                    egui::vec2(
                        SLOT_SIZE.x * container.size.x as f32,
                        SLOT_SIZE.y * container.size.y as f32,
                    ),
                    egui::Sense::hover(),
                );

                // Find where dragged item would be dropped.
                let mut drop_item = None;
                if let Some(info) = dragged.info.as_ref() {
                    if let Some(hover_pos) = ui
                        .ctx()
                        .pointer_interact_pos()
                        .filter(|p| grid_rect.contains(*p))
                    {
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
                            egui::StrokeKind::Inside,
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
                            egui::StrokeKind::Inside,
                        );
                    }

                    if !out_of_bounds
                        && entity != container_entity
                        && ui.input(|i| i.pointer.any_released())
                    {
                        if let Ok((item_entity, &identity, _, mut item)) = items.get_mut(entity) {
                            commit_item_move(
                                &mut dragged,
                                &mut sender,
                                &mut commands,
                                item_entity,
                                identity,
                                &mut item,
                                container_entity,
                                *container_ui.container,
                                position,
                            );
                        }
                    }
                }

                // Paint all items in container
                for (position, (item_entity, name, size)) in stored.iter() {
                    let id = egui::Id::new(item_entity).with(container_entity);
                    let item_rect = egui::Rect::from_min_size(
                        egui::pos2(
                            position.x as f32 * SLOT_SIZE.x,
                            position.y as f32 * SLOT_SIZE.y,
                        ),
                        egui::vec2(SLOT_SIZE.x * size.x as f32, SLOT_SIZE.y * size.y as f32),
                    )
                    .translate(grid_rect.left_top().to_vec2());
                    let response = ui.interact(item_rect, id, egui::Sense::click_and_drag());
                    if response.dragged() {
                        item_drag_source(ui, &response, &mut dragged, *item_entity, *size, name);
                    } else {
                        draw_item(ui, item_rect, name);
                    }

                    // A plain click takes the item into a free hand.
                    if response.clicked() {
                        if let Some((hand_entity, hand_identity)) = free_hand.get_active_empty() {
                            if let Ok((item_entity, &item_identity, _, mut stored)) =
                                items.get_mut(*item_entity)
                            {
                                commit_item_move(
                                    &mut dragged,
                                    &mut sender,
                                    &mut commands,
                                    item_entity,
                                    item_identity,
                                    &mut stored,
                                    hand_entity,
                                    hand_identity,
                                    UVec2::ZERO,
                                );
                            }
                        }
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
            && contexts
                .ctx_mut()
                .unwrap()
                .input(|i| i.pointer.any_released())
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
        egui::StrokeKind::Inside,
    );
    let styled_text = egui::RichText::new(name)
        .size(11.0)
        .color(egui::Color32::WHITE);
    ui.put(item_rect, egui::Label::new(styled_text).selectable(false));
}

/// Draws a floating preview box of a dragged item, centered on the pointer.
fn draw_floating_item(ui: &egui::Ui, size: UVec2, name: &str) {
    let Some(pointer) = ui.ctx().pointer_interact_pos() else {
        return;
    };
    let rect = egui::Rect::from_center_size(
        pointer,
        egui::vec2(SLOT_SIZE.x * size.x as f32, SLOT_SIZE.y * size.y as f32),
    );
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("dragged_item_preview"),
    ));
    painter.rect(
        rect,
        0.,
        egui::Color32::from_white_alpha(16),
        egui::Stroke::new(1.0, egui::Color32::WHITE),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        name,
        egui::FontId::proportional(11.0),
        egui::Color32::WHITE,
    );
}

/// Turns `response` into a drag source for `item`. While dragged, it populates the shared
/// [`DraggedItem`] state and draws a floating preview box following the cursor.
pub(crate) fn item_drag_source(
    ui: &egui::Ui,
    response: &egui::Response,
    dragged: &mut DraggedItem,
    item: Entity,
    size: UVec2,
    name: &str,
) {
    if !response.dragged() {
        return;
    }
    if dragged.info.is_none() {
        dragged.info = Some(DragInfo {
            entity: item,
            size,
            over_anything: false,
            just_dropped: false,
        });
    }
    // Only draw the preview for the item that actually started this drag.
    if dragged.info.as_ref().map(|info| info.entity) == Some(item) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        draw_floating_item(ui, size, name);
    }
}

/// A single-slot drop zone covering `rect`. While an item is dragged over it, the drag is
/// claimed (so it isn't dropped into the world) and the zone is highlighted. Returns `true`
/// on the frame the item is released here while `can_accept` is set (caller should commit).
pub(crate) fn item_drop_zone(
    ui: &egui::Ui,
    rect: egui::Rect,
    dragged: &mut DraggedItem,
    can_accept: bool,
) -> bool {
    let Some(info) = dragged.info.as_mut() else {
        return false;
    };
    let hovered = ui
        .ctx()
        .pointer_interact_pos()
        .is_some_and(|p| rect.contains(p));
    if !hovered {
        return false;
    }
    info.over_anything = true;
    ui.painter().rect(
        rect,
        0.,
        if can_accept {
            egui::Color32::GREEN
        } else {
            egui::Color32::RED
        }
        .gamma_multiply(0.25),
        egui::Stroke::NONE,
        egui::StrokeKind::Inside,
    );
    can_accept && ui.input(|i| i.pointer.any_released())
}

/// Requests an item move from the server and predicts it locally, marking the drag dropped.
#[allow(clippy::too_many_arguments)]
pub(crate) fn commit_item_move(
    dragged: &mut DraggedItem,
    sender: &mut MessageSender,
    commands: &mut Commands,
    item_entity: Entity,
    item_identity: NetworkIdentity,
    stored: &mut StoredItemClient,
    container_entity: Entity,
    container_identity: NetworkIdentity,
    slot: UVec2,
) {
    sender.send_to_server(&MoveItemMessage {
        item: item_identity,
        to_container: Some(container_identity),
        to_slot: slot,
    });
    // Predict the move; the server confirms or rolls back.
    // TODO: actually do rollback on fail
    stored.predict_move(container_identity, slot);
    commands
        .entity(item_entity)
        .insert(ChildOf(container_entity));
    if let Some(info) = dragged.info.as_mut() {
        info.just_dropped = true;
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct MoveItemMessage {
    pub(crate) item: NetworkIdentity,
    pub(crate) to_container: Option<NetworkIdentity>,
    pub(crate) to_slot: UVec2,
}

fn handle_move_message(
    mut messages: MessageReader<MessageEvent<MoveItemMessage>>,
    identities: Res<NetworkIdentities>,
    mut item_moves: ResMut<Tasks<MoveItem>>,
) {
    for event in messages.read() {
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
                    viewer: source,
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
