use std::{sync::Mutex, time::Duration};

use bevy::{
    ecs::{query::QuerySingleError, schedule::SystemLabelId, system::SystemState},
    prelude::*,
    reflect::TypeUuid,
    utils::HashMap,
};
use bevy_egui::{egui::Window, EguiContext};
use bevy_inspector_egui::egui;
use bevy_rapier3d::prelude::RapierContext;
use networking::{
    component::AppExt as ComponentAppExt,
    identity::{NetworkIdentities, NetworkIdentity},
    is_server,
    messaging::{AppExt, MessageEvent, MessageReceivers, MessageSender},
    spawning::{ClientControlled, ClientControls},
    variable::{NetworkVar, ServerVar},
    ConnectionId, NetworkSystem, Networked, Players,
};
use serde::{Deserialize, Serialize};

use crate::{
    body::{Body, Hand, Hands},
    camera::MainCamera,
    event::*,
    items::containers::Container,
};

pub struct InteractionPlugin;

impl Plugin for InteractionPlugin {
    fn build(&self, app: &mut App) {
        app.add_network_message::<InteractionListRequest>()
            .add_network_message::<InteractionListClient>()
            .add_network_message::<InteractionExecuteRequest>()
            .add_network_message::<InteractionExecuteDefaultRequest>()
            .add_networked_component::<ActiveInteraction, ActiveInteractionClient>()
            .add_event::<InteractionListOrder>();

        if is_server(app) {
            app.register_type::<TestInteraction>();
            app.init_resource::<SentInteractionLists>()
                .add_interceptable_event::<InteractionListEvent>()
                .add_system(
                    check_interaction_example
                        .into_descriptor()
                        .intercept::<InteractionListEvent>(),
                )
                .add_system(execute_interaction_example)
                .add_system(begin_interaction_list.label(InteractionListEvent::start_label()))
                .add_system(
                    handle_completed_interaction_list.label(InteractionListEvent::end_label()),
                )
                .add_system(
                    handle_interaction_list_request
                        .before(InteractionListEvent::start_label())
                        .after(NetworkSystem::ReadNetworkMessages),
                )
                .add_system(
                    handle_default_interaction_request
                        .before(InteractionListEvent::start_label())
                        .after(NetworkSystem::ReadNetworkMessages),
                )
                .add_system(
                    handle_default_interaction_request_execution
                        .after(InteractionListEvent::end_label())
                        .before(handle_interaction_execute_request),
                )
                .add_system(handle_interaction_execute_request)
                .add_system(clear_completed_interactions);
        } else {
            app.init_resource::<ClientInteractionUi>()
                .add_system(client_request_interaction_list.label(InteractionSystem::Input))
                .add_system(client_receive_interactions)
                .add_system(client_interaction_selection_ui.after(client_receive_interactions))
                .add_system(client_progress_ui);
        }
    }
}

#[derive(SystemLabel)]
pub enum InteractionSystem {
    Input,
}

/// Event to order the creation of an interaction list
struct InteractionListOrder {
    connection: ConnectionId,
    target: NetworkIdentity,
    send_to_client: bool,
}

pub struct InteractionListEvent {
    /// If we should send the result of this list to the client
    send_to_client: bool,
    connection: ConnectionId,
    pub source: Entity,
    pub target: Entity,
    pub used_hand: Option<Entity>,
    pub item_in_hand: Option<Entity>,
    // Behind a mutex to allow concurrent execution of interaction systems
    interactions: Mutex<Vec<InteractionOption>>,
}

impl InteractionListEvent {
    pub fn add_interaction(&self, interaction: InteractionOption) {
        self.interactions.lock().unwrap().push(interaction);
    }
}

#[derive(SystemLabel)]
enum InteractionListEventLabel {
    Start,
    End,
}

impl InterceptableEvent for InteractionListEvent {
    fn start_label() -> SystemLabelId {
        InteractionListEventLabel::Start.as_label()
    }

    fn end_label() -> SystemLabelId {
        InteractionListEventLabel::End.as_label()
    }
}

pub struct InteractionOption {
    /// Displayed to a player when selecting an interaction.
    pub text: String,
    /// A component that will be attached to the entity if this interaction is started.
    pub interaction: Box<dyn Reflect>,
    /// How specific this interaction is to the objects involved.
    pub specificity: InteractionSpecificity,
}

/// Keeps track of the interaction list a client was last sent.
/// This is necessary so the client can send us an index of what interaction they want to execute.
#[derive(Resource, Default)]
struct SentInteractionLists {
    map: HashMap<ConnectionId, (Entity, Vec<InteractionOption>)>,
}

#[derive(Serialize, Deserialize)]
pub struct InteractionListRequest {
    pub target: NetworkIdentity,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
struct InteractionExecuteRequest {
    index: usize,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct InteractionExecuteDefaultRequest {
    pub target: NetworkIdentity,
}

#[derive(Serialize, Deserialize, Clone)]
struct InteractionOptionClient {
    text: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct InteractionListClient {
    target: NetworkIdentity,
    interactions: Vec<InteractionOptionClient>,
}

pub enum InteractionStatus {
    Running,
    Canceled,
    Completed,
}

/// How specific an interaction is to the objects involved.
/// Interactions with higher specificty will be prioritised when listed.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub enum InteractionSpecificity {
    /// The interaction is available on a limited set of specific objects.
    Specific,
    // The interaction is available on many objects.
    Generic,
}

/// Contains information about the interaction an entity is currently executing.
#[derive(Component, Networked)]
#[component(storage = "SparseSet")]
#[networked(client = "ActiveInteractionClient")]
pub struct ActiveInteraction {
    started: f32,
    estimate_duration: NetworkVar<Option<f32>>,
    pub target: Entity,
    pub status: InteractionStatus,
    reflect_component: ReflectComponent,
}

impl ActiveInteraction {
    /// The time the interaction was started at in seconds.
    pub fn start_time(&self) -> f32 {
        self.started
    }

    pub fn set_initial_duration(&mut self, duration: Duration) {
        if self.estimate_duration.is_none() {
            *self.estimate_duration = Some(duration.as_secs_f32());
        }
    }
}

// TODO: Restrict networking to owning player
#[derive(Component, Networked, TypeUuid, Default)]
#[uuid = "6af71909-2f7e-4020-846e-2496ed1faec5"]
#[component(storage = "SparseSet")]
#[networked(server = "ActiveInteraction")]
struct ActiveInteractionClient {
    started: Option<f32>,
    estimate_duration: ServerVar<Option<f32>>,
}

fn begin_interaction_list(
    mut orders: EventReader<InteractionListOrder>,
    mut events: ResMut<InterceptableEvents<InteractionListEvent>>,
    identities: Res<NetworkIdentities>,
    players: Res<Players>,
    controls: Res<ClientControls>,
    bodies: Query<(&Body, &Hands)>,
    hand_query: Query<(Entity, &Container), With<Hand>>,
) {
    for event in orders.iter() {
        let connection = event.connection;
        let Some(target) = identities.get_entity(event.target) else {
            warn!(connection=?connection, "Interaction list attempted for non-existent identity {:?}", event.target);
            continue;
        };
        let Some(player) = players.get(connection).map(|p| p.id) else {
            warn!(connection=?connection, "Interaction list attempted for connection without player data");
            continue;
        };
        let Some(player_entity) = controls.controlled_entity(player) else {
            warn!(connection=?connection, player=?player, "Interaction list attempted for player without controlled entity");
            continue;
        };

        // Fetch the used hand and item once here, as it's used in many interactions
        let hand = bodies.get(player_entity).ok().and_then(|(body, hands)| {
            hand_query
                .iter_many(&body.limbs)
                .nth(hands.active_hand_index())
        });
        let item_in_hand =
            hand.and_then(|(_, container)| container.iter().next().map(|(_, item)| *item));
        let used_hand = hand.unzip().0;

        events.push(InteractionListEvent {
            send_to_client: event.send_to_client,
            connection,
            source: player_entity,
            target,
            used_hand,
            item_in_hand,
            interactions: Default::default(),
        });

        debug!(connection=?connection, target=?target, "Interaction list build started");
    }
}

fn handle_completed_interaction_list(
    mut events: ResMut<InterceptableEvents<InteractionListEvent>>,
    mut sent: ResMut<SentInteractionLists>,
    identities: Res<NetworkIdentities>,
    mut sender: MessageSender,
) {
    for event in events.drain(..) {
        let mut interactions = event.interactions.into_inner().unwrap();
        // Sort interactions by specificity and name
        // TODO: Add another criteria to sort by (name is probably not a good criteria)
        interactions
            .sort_unstable_by(|a, b| a.specificity.cmp(&b.specificity).then(a.text.cmp(&b.text)));

        // Send interaction list to client
        if event.send_to_client {
            sender.send(
                &InteractionListClient {
                    target: identities.get_identity(event.target).unwrap(),
                    interactions: interactions
                        .iter()
                        .map(|i| InteractionOptionClient {
                            text: i.text.clone(),
                        })
                        .collect(),
                },
                MessageReceivers::Single(event.connection),
            );
        }

        // Remember the options to actually use later
        // TODO: Remove from map for disconnected clients
        sent.map
            .insert(event.connection, (event.target, interactions));
    }
}

fn handle_interaction_list_request(
    mut messages: EventReader<MessageEvent<InteractionListRequest>>,
    mut orders: EventWriter<InteractionListOrder>,
) {
    for event in messages.iter() {
        orders.send(InteractionListOrder {
            connection: event.connection,
            target: event.message.target,
            send_to_client: true,
        });
        debug!(connection=?event.connection, target=?event.message.target, "Interaction list requested");
    }
}

fn handle_default_interaction_request(
    mut messages: EventReader<MessageEvent<InteractionExecuteDefaultRequest>>,
    mut orders: EventWriter<InteractionListOrder>,
) {
    for event in messages.iter() {
        orders.send(InteractionListOrder {
            connection: event.connection,
            target: event.message.target,
            send_to_client: false,
        });
        debug!(connection=?event.connection, target=?event.message.target, "Default interaction requested");
    }
}

fn handle_default_interaction_request_execution(
    mut messages: EventReader<MessageEvent<InteractionExecuteDefaultRequest>>,
    lists: Res<SentInteractionLists>,
    mut events: EventWriter<MessageEvent<InteractionExecuteRequest>>,
) {
    for event in messages.iter() {
        let connection = event.connection;
        let Some((_, interactions)) = lists.map.get(&connection) else {
            continue;
        };

        // Can't execute default interaction if there are no available interactions.
        if interactions.is_empty() {
            continue;
        }

        events.send(MessageEvent {
            message: InteractionExecuteRequest { index: 0 },
            connection,
        })
    }
}

fn handle_interaction_execute_request(
    world: &mut World,
    state: &mut SystemState<(
        EventReader<MessageEvent<InteractionExecuteRequest>>,
        ResMut<SentInteractionLists>,
    )>,
    lookup_state: &mut SystemState<(Res<ClientControls>, Res<Players>, Res<Time>)>,
) {
    let (mut messages, mut sent_interactions) = state.get_mut(world);
    let mut to_execute = Vec::default();
    for event in messages.iter() {
        let Some((_, (target, mut options))) = sent_interactions.map.remove_entry(&event.connection) else {
            warn!(connection=?event.connection, "Received interaction execute request with no interaction list");
            continue;
        };

        let index = event.message.index;
        if index >= options.len() {
            warn!(connection=?event.connection, index=event.message.index, "Received interaction execute request with out of bounds index");
            continue;
        }

        // We just want one item from the options
        // Using swap remove is fine, as the Vec will be dropped after this
        let option = options.swap_remove(index);

        debug!(
            "Client wants to execute interaction \"{}\" on {:?}",
            &option.text, target
        );

        to_execute.push((event.connection, target, option));
    }

    for (connection, target, option) in to_execute.into_iter() {
        let (controls, players, time) = lookup_state.get(world);
        let Some(player) = players.get(connection).map(|p| p.id) else {
            warn!(connection=?connection, "Received interaction execute request from connection without player data");
            continue;
        };
        let Some(player_entity) = controls.controlled_entity(player) else {
            warn!(connection=?connection, player=?player, "Received interaction execute request from player without controlled entity");
            continue;
        };

        let started = time.elapsed_seconds();

        if world
            .entity(player_entity)
            .get::<ActiveInteraction>()
            .is_some()
        {
            // TODO: Cancel running interaction, then start new one
            warn!(connection=?connection, player=?player, "Starting new interaction while performing one not yet supported");
            continue;
        }

        // Add the interaction component to the players entity
        let cloned_registry = world.resource::<AppTypeRegistry>().clone();
        let registry = cloned_registry.read();
        let registration = registry
            .get_with_name(option.interaction.type_name())
            .expect("Interaction must be registered with app.register_type::<T>()");
        let reflect_component = registration
            .data::<ReflectComponent>()
            .expect("Interaction must #[reflect(Component)]");
        reflect_component.insert(world, player_entity, option.interaction.as_ref());

        // Record active interaction
        world.entity_mut(player_entity).insert(ActiveInteraction {
            started,
            estimate_duration: None.into(),
            target,
            status: InteractionStatus::Running,
            reflect_component: reflect_component.clone(),
        });
    }
}

fn clear_completed_interactions(
    world: &mut World,
    query: &mut QueryState<(Entity, &ActiveInteraction), Changed<ActiveInteraction>>,
) {
    let mut to_clear = Vec::default();
    for (entity, interaction) in query.iter(world) {
        // TODO: Handle canceled interaction information
        if matches!(
            interaction.status,
            InteractionStatus::Completed | InteractionStatus::Canceled
        ) {
            to_clear.push(entity);
        }
    }

    // Remove active interaction and component
    for entity in to_clear.into_iter() {
        let active = world
            .entity_mut(entity)
            .remove::<ActiveInteraction>()
            .unwrap();
        active.reflect_component.remove(world, entity);
    }
}

#[allow(clippy::too_many_arguments)]
fn client_request_interaction_list(
    buttons: Res<Input<MouseButton>>,
    mut context: ResMut<EguiContext>,
    rapier_context: Res<RapierContext>,
    windows: Res<Windows>,
    cameras: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    parents: Query<&Parent>,
    identities: Res<NetworkIdentities>,
    mut sender: MessageSender,
) {
    let execute_default = buttons.just_pressed(MouseButton::Left);
    let request_list = buttons.just_pressed(MouseButton::Right);
    if !execute_default && !request_list {
        return;
    }

    let window = match windows.get_primary() {
        Some(w) => w,
        None => return,
    };

    if context
        .try_ctx_for_window_mut(window.id())
        .map(|c| c.is_pointer_over_area())
        == Some(true)
    {
        return;
    }

    let Some((camera, camera_transform)) = cameras.iter().next() else {
        return;
    };
    let Some(cursor_position) = window.cursor_position() else {
        return;
    };

    let Some(ray) = camera.viewport_to_world(camera_transform, cursor_position) else {
        return;
    };

    let Some((entity, _)) =
        rapier_context.cast_ray(ray.origin, ray.direction, 100.0, true, Default::default()) else
    {
        return;
    };

    // Get network identity on hit or parents
    let target = identities.get_identity(entity).or_else(|| {
        parents
            .iter_ancestors(entity)
            .find_map(|e| identities.get_identity(e))
    });

    let Some(target) = target else {
        return;
    };

    if execute_default {
        sender.send_to_server(&InteractionExecuteDefaultRequest { target });
    } else {
        sender.send_to_server(&InteractionListRequest { target });
    }
}

#[derive(Resource, Default)]
struct ClientInteractionUi {
    current: Option<InteractionListClient>,
}

fn client_receive_interactions(
    mut messages: EventReader<MessageEvent<InteractionListClient>>,
    mut state: ResMut<ClientInteractionUi>,
) {
    let Some(event) = messages.iter().last() else {
        return;
    };

    if event.message.interactions.is_empty() {
        // Ensures possible existing dialog disappears
        state.current = None;
    } else {
        state.current = Some(event.message.clone());
    }
}

fn client_interaction_selection_ui(
    mut egui_context: ResMut<EguiContext>,
    mut state: ResMut<ClientInteractionUi>,
    windows: Res<Windows>,
    mut sender: MessageSender,
) {
    let Some(list) = &state.current else {
        return;
    };

    let mut clear = false;

    let mut ui_window = Window::new("Interact").resizable(false).collapsible(false);
    if state.is_changed() {
        // Position window at cursor
        if let Some(window) = windows.get_primary() {
            if let Some(pos) = window.cursor_position() {
                ui_window = ui_window.current_pos(egui::pos2(pos.x, window.height() - pos.y));
            }
        }
    }

    ui_window.show(egui_context.ctx_mut(), |ui| {
        for (index, interaction) in list.interactions.iter().enumerate() {
            if ui.button(&interaction.text).clicked() {
                sender.send_to_server(&InteractionExecuteRequest { index });
                clear = true;
            }
        }
    });

    if clear {
        state.current = None;
    }
}

fn client_progress_ui(
    mut egui_context: ResMut<EguiContext>,
    mut interactions: Query<
        (&mut ActiveInteractionClient, &GlobalTransform),
        With<ClientControlled>,
    >,
    windows: Res<Windows>,
    cameras: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    time: Res<Time>,
) {
    let (mut interaction, transform) = match interactions.get_single_mut() {
        Ok(i) => i,
        Err(QuerySingleError::MultipleEntities(_)) => {
            warn!("Multiple entities with active interaction. There should only be one (on the entity controlled by the player).");
            return;
        }
        _ => return,
    };

    let window = match windows.get_primary() {
        Some(w) => w,
        None => return,
    };

    let Some((camera, camera_transform)) = cameras.iter().next() else {
        return;
    };

    let Some(screen_position) = camera.world_to_viewport(camera_transform, transform.translation()) else {
        return;
    };

    // TODO: This may be inaccurate with high RTT, use interpolated tick instead
    if interaction.started.is_none() {
        interaction.started = Some(time.elapsed_seconds());
    }

    egui::Area::new("interaction progress")
        .fixed_pos(egui::pos2(
            screen_position.x,
            window.height() - screen_position.y,
        ))
        .show(egui_context.ctx_mut(), |ui| {
            if let Some(estimate) = *interaction.estimate_duration {
                let remaining = estimate + interaction.started.unwrap() - time.elapsed_seconds();
                let t = (estimate - remaining) / estimate;
                ui.add(egui::ProgressBar::new(t).desired_width(80.0));
            } else {
                ui.spinner();
            }
        });
}

#[derive(Component, Reflect)]
#[component(storage = "SparseSet")]
#[reflect(Component)]
struct TestInteraction {
    first_run: bool,
}

impl TestInteraction {
    fn new() -> Self {
        Self { first_run: true }
    }
}

// Dummy implementation for reflection
impl FromWorld for TestInteraction {
    fn from_world(_: &mut World) -> Self {
        Self::new()
    }
}

fn check_interaction_example(events: Res<InterceptableEvents<InteractionListEvent>>) {
    for event in events.iter() {
        event.add_interaction(InteractionOption {
            text: "Test Interact".into(),
            interaction: Box::new(TestInteraction::new()),
            specificity: InteractionSpecificity::Generic,
        });
    }
}

fn execute_interaction_example(
    mut query: Query<(&mut TestInteraction, &mut ActiveInteraction)>,
    time: Res<Time>,
) {
    for (mut interaction, mut active) in query.iter_mut() {
        if interaction.first_run {
            interaction.first_run = false;
        }

        if active.started + 2.0 < time.elapsed_seconds() {
            active.status = InteractionStatus::Completed;
        }
    }
}
