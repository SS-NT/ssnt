use std::fmt;

use bevy::{
    ecs::{
        entity::{EntityMapper, MapEntities},
        reflect::ReflectMapEntities,
        system::EntityCommands,
    },
    prelude::*,
    reflect::TypeUuid,
    utils::HashSet,
};
use bevy_egui::{egui, EguiContexts};
use networking::{
    component::AppExt as ComponentAppExt,
    identity::{EntityCommandsExt, NetworkIdentities, NetworkIdentity},
    is_server,
    messaging::{AppExt, MessageEvent, MessageSender},
    scene::NetworkSceneBundle,
    spawning::{ClientControlled, ClientControls},
    variable::{NetworkVar, ServerVar},
    Networked, Players,
};
use physics::{ColliderGroup, PhysicsEntityCommands};
use serde::{Deserialize, Serialize};
use utils::order::*;

use crate::{
    interaction::{
        ActiveInteraction, GenerateInteractionList, InteractionListEvents, InteractionListRequest,
        InteractionOption, InteractionSpecificity, InteractionStatus,
    },
    items::{
        containers::{Container, MoveItemOrder, MoveItemResult},
        Item, StoredItem,
    },
};

mod health;

pub struct BodyPlugin;

impl Plugin for BodyPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Body>()
            .register_type::<LimbSide>()
            .register_type::<Limb>()
            .register_type::<Hand>()
            .register_type::<Cutting>()
            .add_network_message::<ChangeHandRequest>()
            .add_networked_component::<Hands, HandsClient>();

        if is_server(app) {
            app.register_type::<PickupInteraction>()
                .register_type::<DropInteraction>()
                .register_type::<CutInteraction>()
                .add_event::<LimbEvent>()
                .register_order::<SpawnCreatureOrder, SpawnCreatureResult>()
                .add_systems(
                    Update,
                    (
                        pickup_interaction,
                        drop_interaction,
                        cut_interaction,
                        (
                            prepare_pickup_interaction,
                            prepare_drop_interaction,
                            prepare_cut_interaction,
                        )
                            .in_set(GenerateInteractionList),
                        handle_hand_modification,
                        handle_hand_separation,
                        handle_hand_change_request,
                        (process_new_limbs, process_limb_removal, create_creature).chain(),
                    ),
                );
        } else {
            app.add_systems(
                Update,
                ((client_update_limbs, hand_ui).chain(), client_hands_keybind),
            );
        }

        app.add_plugins(health::HealthPlugin);

        app.insert_resource(BodyAssets {
            scenes: app
                .world
                .resource::<AssetServer>()
                .load_folder("creatures/")
                .unwrap(),
        });
    }
}

#[derive(Component, Default, Reflect)]
#[reflect(Component, MapEntities)]
pub struct Body {
    limbs: HashSet<Entity>,
    added_limbs: Vec<Entity>,
    limbs_to_remove: Vec<Entity>,
}

impl MapEntities for Body {
    fn map_entities(&mut self, entity_mapper: &mut EntityMapper) {
        self.limbs = self
            .limbs
            .iter()
            .map(|e| entity_mapper.get_or_reserve(*e))
            .collect();
    }
}

#[derive(Reflect)]
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

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct Limb {
    attachment_position: Vec3,
}

impl FromWorld for Limb {
    fn from_world(_: &mut World) -> Self {
        Self {
            attachment_position: Vec3::ZERO,
        }
    }
}

#[derive(Event)]
struct LimbEvent {
    limb_entity: Entity,
    kind: LimbEventKind,
}

#[derive(PartialEq, Eq)]
enum LimbEventKind {
    Added,
    Removed,
}

fn process_new_limbs(
    mut bodies: Query<&mut Body, Changed<Body>>,
    mut limbs: Query<(&Limb, &mut Transform)>,
    mut writer: EventWriter<LimbEvent>,
    mut commands: Commands,
) {
    for mut body in bodies.iter_mut() {
        body.added_limbs.retain(|&limb_entity| {
            let Ok((limb, mut transform)) = limbs.get_mut(limb_entity) else {
                return true;
            };
            transform.translation = limb.attachment_position;
            commands
                .entity(limb_entity)
                .freeze(Some(ColliderGroup::AttachedLimbs));
            writer.send(LimbEvent {
                limb_entity,
                kind: LimbEventKind::Added,
            });
            false
        });
    }
}

fn process_limb_removal(
    mut bodies: Query<&mut Body, Changed<Body>>,
    mut transforms: Query<(&mut Transform, &GlobalTransform)>,
    mut writer: EventWriter<LimbEvent>,
    mut commands: Commands,
) {
    for mut body in bodies.iter_mut() {
        let body = body.as_mut();
        for limb_entity in body.limbs_to_remove.drain(..) {
            if !body.limbs.remove(&limb_entity) {
                continue;
            }
            if let Ok((mut transform, global_transform)) = transforms.get_mut(limb_entity) {
                *transform = global_transform.compute_transform();
            }
            commands
                .entity(limb_entity)
                .remove_parent()
                .unfreeze(Some(ColliderGroup::Default));
            writer.send(LimbEvent {
                limb_entity,
                kind: LimbEventKind::Removed,
            });
        }
    }
}

fn client_update_limbs(
    mut added_limbs: Query<(Entity, &Parent), (Or<(Added<Limb>, Changed<Parent>)>,)>,
    parents: Query<&Parent>,
    hands: Query<(), With<Hand>>,
    mut bodies: Query<&mut Body, With<ClientControlled>>,
) {
    for (limb_entity, limb_parent) in added_limbs.iter_mut() {
        // HACK: assume limb is handled as item if nested under hands
        if hands.contains(limb_parent.get()) {
            continue;
        }

        let Some(body_entity) = parents.iter_ancestors(limb_entity).find(|&e| bodies.contains(e)) else {
            continue;
        };
        let mut body = bodies.get_mut(body_entity).unwrap();
        body.limbs.insert(limb_entity);
    }
    // TODO: removed limbs
}

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct Hand {
    pub side: LimbSide,
    order: u32,
}

impl FromWorld for Hand {
    fn from_world(_: &mut World) -> Self {
        Self {
            side: LimbSide::Left,
            order: 0,
        }
    }
}

#[derive(Component, Networked)]
#[networked(client = "HandsClient")]
pub struct Hands {
    #[networked(
        with = "Self::network_active_hand(Res<'static, NetworkIdentities>) -> NetworkIdentity"
    )]
    active_hand: NetworkVar<Entity>,
}

impl Hands {
    pub fn active_hand(&self) -> Entity {
        *self.active_hand
    }

    fn network_active_hand(entity: &Entity, param: Res<NetworkIdentities>) -> NetworkIdentity {
        param
            .get_identity(*entity)
            .expect("Hand entity must have network identity")
    }
}

#[derive(Component, Networked, TypeUuid, Default)]
#[networked(server = "Hands")]
#[uuid = "9c9b2476-15e1-4d34-9336-7368f6702406"]
struct HandsClient {
    active_hand: ServerVar<NetworkIdentity>,
}

/// Updates the selected hand when limbs of a body get changed
fn handle_hand_modification(
    mut bodies: Query<(Entity, &Body, Option<&mut Hands>), Changed<Body>>,
    hands: Query<(), With<Hand>>,
    mut commands: Commands,
) {
    for (body_entity, body, existing_hands) in bodies.iter_mut() {
        // TODO: We should only check added and removed limbs
        let current_hands: HashSet<_> = body
            .limbs
            .iter()
            .copied()
            .filter(|entity| hands.contains(*entity))
            .collect();
        if let Some(mut hands) = existing_hands {
            // We still have the hand that's currently active, nothing to change
            if current_hands.contains(&*((&hands).active_hand)) {
                continue;
            }
            // If we lost that hand, choose a random one or remove hands entirely
            match current_hands.iter().next() {
                Some(&hand) => *hands.active_hand = hand,
                None => {
                    commands.entity(body_entity).remove::<Hands>();
                }
            };
        } else if let Some(&first_hand) = current_hands.iter().next() {
            commands.entity(body_entity).insert(Hands {
                active_hand: first_hand.into(),
            });
        }
    }
}

fn handle_hand_separation(
    mut events: EventReader<LimbEvent>,
    hands: Query<&Container, With<Hand>>,
    mut move_orderer: Orderer<MoveItemOrder>,
) {
    for event in events.iter() {
        if event.kind != LimbEventKind::Removed {
            continue;
        }
        let Ok(container) = hands.get(event.limb_entity) else {
            continue;
        };
        if container.is_empty() {
            continue;
        }
        // Drop all items this hand is holding
        // TODO: This should be handled in health system (ex. no nerve signal)
        for item in container.iter().map(|(_, i)| *i) {
            move_orderer.create(MoveItemOrder {
                item,
                container: None,
                position: None,
            });
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ChangeHandRequest {
    identity: NetworkIdentity,
}

fn hand_ui(
    mut contexts: EguiContexts,
    mut bodies: Query<(&Body, &mut HandsClient), With<ClientControlled>>,
    hands: Query<(Entity, &NetworkIdentity, &Hand, Option<&Children>)>,
    items: Query<(&Item, &NetworkIdentity)>,
    mut ordered_hands: Local<Vec<(Entity, u32)>>,
    mut sender: MessageSender,
) {
    let Ok((body, hand_data)) = bodies.get_single_mut() else {
        return;
    };

    egui::Window::new("hands")
        .title_bar(false)
        .anchor(egui::Align2::CENTER_BOTTOM, egui::Vec2::ZERO)
        .resizable(false)
        .show(contexts.ctx_mut(), |ui| {
            ui.horizontal_wrapped(|ui| {
                // Order hands for display
                ordered_hands.clear();
                ordered_hands.extend(
                    hands
                        .iter_many(&body.limbs)
                        .map(|(entity, .., hand, _)| (entity, hand.order)),
                );
                ordered_hands.sort_unstable_by_key(|(_, k)| *k);

                for (_, &identity, hand, children) in
                    hands.iter_many(ordered_hands.iter().map(|(e, _)| e))
                {
                    let mut held_item_name = None;
                    let mut held_item_id = None;
                    if let Some(children) = children {
                        if let Some((item, identity)) = items.iter_many(children).next() {
                            held_item_name = Some(item.name.as_str());
                            held_item_id = Some(*identity);
                        }
                    }
                    let label = ui.selectable_label(
                        identity == *hand_data.active_hand,
                        format!("{}: {}", hand.side, held_item_name.unwrap_or("empty")),
                    );
                    if label.clicked() {
                        sender.send_to_server(&ChangeHandRequest { identity });
                    } else if label.clicked_by(egui::PointerButton::Secondary) {
                        // Request interaction list on right-click
                        if let Some(target) = held_item_id {
                            sender.send_to_server(&InteractionListRequest { target });
                        }
                    }
                }
            });
        });
}

fn client_hands_keybind(
    keyboard_input: Res<Input<KeyCode>>,
    mut bodies: Query<(&Body, &mut HandsClient), With<ClientControlled>>,
    hands: Query<&NetworkIdentity, With<Hand>>,
    mut sender: MessageSender,
) {
    if !keyboard_input.just_pressed(KeyCode::X) {
        return;
    }

    let Ok((body, hand_data)) = bodies.get_single_mut() else {
        return;
    };

    let mut previous_was_active_hand = false;
    for &identity in hands.iter_many(&body.limbs) {
        if previous_was_active_hand {
            sender.send_to_server(&ChangeHandRequest { identity });
            return;
        }

        if *hand_data.active_hand == identity {
            previous_was_active_hand = true;
        }
    }
    // If we get here we haven't changed hands
    // Try to just change to first hand
    if let Some(&identity) = hands.iter_many(&body.limbs).next() {
        if *hand_data.active_hand == identity {
            return;
        }
        sender.send_to_server(&ChangeHandRequest { identity });
    }
}

fn handle_hand_change_request(
    mut events: EventReader<MessageEvent<ChangeHandRequest>>,
    players: Res<Players>,
    controls: Res<ClientControls>,
    identities: Res<NetworkIdentities>,
    mut hands: Query<&mut Hands>,
) {
    for event in events.iter() {
        let Some(controlled) = players.get(event.connection).and_then(|player| controls.controlled_entity(player.id)) else {
            continue;
        };
        let Ok(mut hands) = hands.get_mut(controlled) else {
            continue;
        };
        let Some(hand_entity) = identities.get_entity(event.message.identity) else {
            continue;
        };
        // TODO: Validate object is actually hand
        *hands.active_hand = hand_entity;
    }
}

#[derive(Resource)]
struct BodyAssets {
    // Used to keep strong handles to prevent asset unloading
    #[allow(dead_code)]
    scenes: Vec<HandleUntyped>,
}

/// An order to create the body of a given creature archetype
#[derive(Event)]
pub struct SpawnCreatureOrder {
    pub archetype: String,
}

#[derive(Event)]
pub struct SpawnCreatureResult {
    pub root: Entity,
}

fn spawn_limb<'w, 's, 'a: 'b, 'b: 'c, 'c>(
    builder: &'b mut ChildBuilder<'w, 's, 'a>,
    server: &AssetServer,
    name: &str,
) -> EntityCommands<'w, 's, 'c> {
    let mut entity = builder.spawn(NetworkSceneBundle {
        scene: server.load(format!("creatures/{}.scn.ron", name)).into(),
        ..Default::default()
    });
    entity.networked();
    entity
}

fn create_creature(
    mut orders: EventReader<Order<SpawnCreatureOrder>>,
    mut results: EventWriter<OrderResult<SpawnCreatureOrder, SpawnCreatureResult>>,
    server: Res<AssetServer>,
    mut commands: Commands,
) {
    for order in orders.iter() {
        let data = order.data();
        let mut creature = commands.spawn(NetworkSceneBundle {
            scene: server.load("creatures/player.scn.ron").into(),
            ..Default::default()
        });
        // TODO: Replace with species configuration in assets
        match data.archetype.as_str() {
            "human" => {
                let mut limbs = HashSet::default();
                creature.with_children(|builder| {
                    let torso = spawn_limb(builder, server.as_ref(), "human_torso")
                        .with_children(|builder| {
                            // Head
                            let head = spawn_limb(builder, server.as_ref(), "human_head")
                                .with_children(|builder| {
                                    let brain =
                                        spawn_limb(builder, server.as_ref(), "organic_brain").id();
                                    limbs.insert(brain);
                                })
                                .id();
                            limbs.insert(head);

                            // Arms
                            let arm_left = spawn_limb(builder, server.as_ref(), "human_arm_left")
                                .with_children(|builder| {
                                    let hand_left =
                                        spawn_limb(builder, server.as_ref(), "human_hand_left")
                                            .id();
                                    limbs.insert(hand_left);
                                })
                                .id();
                            limbs.insert(arm_left);
                            let arm_right = spawn_limb(builder, server.as_ref(), "human_arm_right")
                                .with_children(|builder| {
                                    let hand_right =
                                        spawn_limb(builder, server.as_ref(), "human_hand_right")
                                            .id();
                                    limbs.insert(hand_right);
                                })
                                .id();
                            limbs.insert(arm_right);

                            // Legs
                            let leg_left = spawn_limb(builder, server.as_ref(), "human_leg_left")
                                .with_children(|builder| {
                                    let foot_left =
                                        spawn_limb(builder, server.as_ref(), "human_foot_left")
                                            .id();
                                    limbs.insert(foot_left);
                                })
                                .id();
                            limbs.insert(leg_left);
                            let leg_right = spawn_limb(builder, server.as_ref(), "human_leg_right")
                                .with_children(|builder| {
                                    let foot_right =
                                        spawn_limb(builder, server.as_ref(), "human_foot_right")
                                            .id();
                                    limbs.insert(foot_right);
                                })
                                .id();
                            limbs.insert(leg_right);

                            let heart = spawn_limb(builder, server.as_ref(), "organic_heart").id();
                            limbs.insert(heart);
                            let lung = spawn_limb(builder, server.as_ref(), "organic_lung").id();
                            limbs.insert(lung);
                        })
                        .id();
                    limbs.insert(torso);
                });
                let added_limbs = limbs.iter().copied().collect();
                creature.insert(Body {
                    limbs,
                    added_limbs,
                    ..Default::default()
                });
            }
            _ => todo!(),
        }

        bevy::log::info!("Created creature");
        results.send(order.complete(SpawnCreatureResult {
            root: creature.networked().id(),
        }));
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
    interaction_lists: Res<InteractionListEvents>,
    items: Query<&Item>,
    bodies: Query<(&Body, &Hands)>,
    hand_query: Query<(&Hand, &Container)>,
) {
    for event in interaction_lists.events.iter() {
        let Ok(_) = items.get(event.target) else {
            continue;
        };

        let Ok((body, hands)) = bodies.get(event.source) else {
            continue;
        };

        let hand_entity = *hands.active_hand;
        if !body.limbs.contains(&hand_entity) {
            continue;
        }

        let Ok((_, hand_container)) = hand_query.get(hand_entity) else {
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
    hands: Query<&Hands>,
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

        let Ok(hands) = hands.get(source) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        let Ok((hand_entity, _, hand_container)) = hand_query.get(*hands.active_hand) else {
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
    interaction_list: Res<InteractionListEvents>,
    items: Query<&StoredItem>,
    bodies: Query<&Body>,
    hand_query: Query<Entity, With<Hand>>,
) {
    for event in interaction_list.events.iter() {
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

// NOTE: This is just for funny content

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Cutting {}

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
#[component(storage = "SparseSet")]
struct CutInteraction {}

fn prepare_cut_interaction(
    interaction_list: Res<InteractionListEvents>,
    cutting_items: Query<(), (With<Item>, With<Cutting>)>,
) {
    for event in interaction_list.events.iter() {
        let Some(item) = event.item_in_hand else {
            continue;
        };

        if !cutting_items.contains(item) {
            continue;
        }

        event.add_interaction(InteractionOption {
            text: "Cut".into(),
            interaction: Box::<CutInteraction>::default(),
            specificity: InteractionSpecificity::Specific,
        });
    }
}

fn cut_interaction(
    mut query: Query<(&mut CutInteraction, &mut ActiveInteraction)>,
    mut bodies: Query<&mut Body>,
    mut transforms: Query<(&mut Transform, &GlobalTransform)>,
    mut commands: Commands,
) {
    for (_, mut active) in query.iter_mut() {
        let Ok(mut body) = bodies.get_mut(active.target) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };
        commands.entity(active.target).disable_physics();

        #[allow(clippy::needless_collect)]
        let limbs: Vec<_> = body.limbs.iter().copied().collect();
        body.limbs_to_remove.extend(limbs.into_iter());
        for limb_entity in body.limbs.iter().copied() {
            if let Ok((mut transform, global_transform)) = transforms.get_mut(limb_entity) {
                *transform = global_transform.compute_transform();
            }
            commands
                .entity(limb_entity)
                .remove_parent()
                .enable_physics();
        }
        active.status = InteractionStatus::Completed;
    }
}
