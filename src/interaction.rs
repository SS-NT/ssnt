use std::{sync::Mutex, time::Duration};

use bevy::{
    ecs::query::QuerySingleError, prelude::*, reflect::TypeUuid, utils::HashMap,
    window::PrimaryWindow,
};
use bevy_egui::{egui, EguiContexts};
use bevy_rapier3d::prelude::RapierContext;
use networking::{
    component::AppExt as ComponentAppExt,
    identity::{NetworkIdentities, NetworkIdentity},
    is_server,
    messaging::{AppExt, MessageEvent, MessageReceivers, MessageSender},
    spawning::{ClientControlled, ClientControls},
    variable::{NetworkVar, ServerVar},
    ConnectionId, Networked, Players,
};
use serde::{Deserialize, Serialize};
use utils::task::{Task, Tasks};

use crate::{
    body::{Hand, Hands},
    camera::MainCamera,
    combat::ClientCombatModeStatus,
    items::containers::Container,
    ui::has_window,
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
            app.init_resource::<SentInteractionLists>()
                .init_resource::<InteractionListEvents>()
                .init_resource::<Tasks<ExecuteInteraction>>()
                .configure_sets(
                    Update,
                    (GenerateInteractionList
                        .run_if(|interactions: Res<InteractionListEvents>| {
                            !interactions.events.is_empty()
                        })
                        .after(begin_interaction_list)
                        .before(handle_completed_interaction_list),),
                )
                .add_systems(
                    Update,
                    (
                        (
                            handle_interaction_list_request,
                            handle_default_interaction_request,
                        ),
                        begin_interaction_list,
                        handle_completed_interaction_list,
                        handle_default_interaction_request_execution,
                        handle_interaction_execute_request,
                        run_interactions,
                        clear_completed_interactions,
                    )
                        .chain(),
                );
        } else {
            app.init_resource::<ClientInteractionUi>().add_systems(
                Update,
                (
                    client_request_interaction_list.in_set(InteractionSystem::Input),
                    (
                        client_receive_interactions,
                        client_interaction_selection_ui.run_if(has_window),
                    )
                        .chain(),
                    client_progress_ui,
                ),
            );
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, SystemSet)]
pub enum InteractionSystem {
    Input,
}

/// Event to order the creation of an interaction list
#[derive(Event)]
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

#[derive(Resource, Default)]
pub struct InteractionListEvents {
    pub events: Vec<InteractionListEvent>,
}

/// The set in which all systems that generate interactions run.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, SystemSet)]
pub struct GenerateInteractionList;

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

/// Task to execute a specific interaction.
pub struct ExecuteInteraction {
    pub entity: Entity,
    pub target: Entity,
    pub interaction: Box<dyn Reflect>,
}

impl Task for ExecuteInteraction {
    // TODO: Actual result
    type Result = ();
}

fn begin_interaction_list(
    mut orders: EventReader<InteractionListOrder>,
    mut interaction_lists: ResMut<InteractionListEvents>,
    identities: Res<NetworkIdentities>,
    players: Res<Players>,
    controls: Res<ClientControls>,
    bodies: Query<&Hands>,
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
        let hand = bodies
            .get(player_entity)
            .ok()
            .and_then(|hands| hand_query.get(hands.active_hand()).ok());
        let item_in_hand =
            hand.and_then(|(_, container)| container.iter().next().map(|(_, item)| *item));
        let used_hand = hand.unzip().0;

        interaction_lists.events.push(InteractionListEvent {
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
    mut interaction_lists: ResMut<InteractionListEvents>,
    mut sent: ResMut<SentInteractionLists>,
    identities: Res<NetworkIdentities>,
    mut sender: MessageSender,
) {
    for event in interaction_lists.events.drain(..) {
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
    mut messages: EventReader<MessageEvent<InteractionExecuteRequest>>,
    mut sent_interactions: ResMut<SentInteractionLists>,
    controls: Res<ClientControls>,
    players: Res<Players>,
    mut execute: ResMut<Tasks<ExecuteInteraction>>,
) {
    for event in messages.iter() {
        let Some((_, (target, mut options))) =
            sent_interactions.map.remove_entry(&event.connection)
        else {
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

        let connection = event.connection;
        let Some(player) = players.get(connection).map(|p| p.id) else {
            warn!(connection=?connection, "Received interaction execute request from connection without player data");
            continue;
        };
        let Some(player_entity) = controls.controlled_entity(player) else {
            warn!(connection=?connection, player=?player, "Received interaction execute request from player without controlled entity");
            continue;
        };

        execute.create_ignore(ExecuteInteraction {
            entity: player_entity,
            target,
            interaction: option.interaction,
        });
    }
}

fn run_interactions(world: &mut World) {
    let started = world.resource::<Time>().elapsed_seconds();

    world.resource_scope(|world, mut tasks: Mut<Tasks<ExecuteInteraction>>| {
        tasks.process(|task| {
            if world.entity(task.entity).contains::<ActiveInteraction>() {
                // TODO: Cancel running interaction, then start new one
                warn!("Starting new interaction while performing one not yet supported");
                return;
            }

            // Add the interaction component to the players entity
            let cloned_registry = world.resource::<AppTypeRegistry>().clone();
            let registry = cloned_registry.read();
            let registration = registry
                .get_with_name(task.interaction.type_name())
                .expect("Interaction must be registered with app.register_type::<T>()");
            let reflect_component = registration
                .data::<ReflectComponent>()
                .expect("Interaction must #[reflect(Component)]");
            reflect_component.insert(
                &mut world.entity_mut(task.entity),
                task.interaction.as_ref(),
            );

            // Record active interaction
            world.entity_mut(task.entity).insert(ActiveInteraction {
                started,
                estimate_duration: None.into(),
                target: task.target,
                status: InteractionStatus::Running,
                reflect_component: reflect_component.clone(),
            });
        });
    });
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
            .take::<ActiveInteraction>()
            .unwrap();
        active
            .reflect_component
            .remove(&mut world.entity_mut(entity));
    }
}

#[allow(clippy::too_many_arguments)]
fn client_request_interaction_list(
    buttons: Res<Input<MouseButton>>,
    mut contexts: EguiContexts,
    rapier_context: Res<RapierContext>,
    windows: Query<(Entity, &Window), With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    parents: Query<&Parent>,
    identities: Res<NetworkIdentities>,
    combat_status: ClientCombatModeStatus,
    mut sender: MessageSender,
) {
    let execute_default = buttons.just_pressed(MouseButton::Left);
    let request_list = buttons.just_pressed(MouseButton::Right);
    if !execute_default && !request_list {
        return;
    }

    // We prevent interaction with the world while fighting
    // so we can reuse the same mouse buttons for attacking
    if combat_status.is_enabled() {
        return;
    }

    let Ok((window_entity, window)) = windows.get_single() else {
        return;
    };

    if contexts
        .try_ctx_for_window_mut(window_entity)
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
        rapier_context.cast_ray(ray.origin, ray.direction, 100.0, true, Default::default())
    else {
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
    mut contexts: EguiContexts,
    mut state: ResMut<ClientInteractionUi>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut sender: MessageSender,
) {
    let Some(list) = &state.current else {
        return;
    };

    let mut clear = false;

    let mut ui_window = egui::Window::new("Interact")
        .resizable(false)
        .collapsible(false);
    if state.is_changed() {
        // Position window at cursor
        if let Ok(window) = windows.get_single() {
            if let Some(pos) = window.cursor_position() {
                ui_window = ui_window.current_pos(egui::pos2(pos.x, pos.y));
            }
        }
    }

    ui_window.show(contexts.ctx_mut(), |ui| {
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
    mut contexts: EguiContexts,
    mut interactions: Query<
        (&mut ActiveInteractionClient, &GlobalTransform),
        With<ClientControlled>,
    >,
    windows: Query<&Window, With<PrimaryWindow>>,
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

    let Ok(window) = windows.get_single() else {
        return;
    };

    let Some((camera, camera_transform)) = cameras.iter().next() else {
        return;
    };

    let Some(screen_position) = camera.world_to_viewport(camera_transform, transform.translation())
    else {
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
        .show(contexts.ctx_mut(), |ui| {
            if let Some(estimate) = *interaction.estimate_duration {
                let remaining = estimate + interaction.started.unwrap() - time.elapsed_seconds();
                let t = (estimate - remaining) / estimate;
                ui.add(egui::ProgressBar::new(t).desired_width(80.0));
            } else {
                ui.spinner();
            }
        });
}
