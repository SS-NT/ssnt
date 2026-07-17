//! A debug inspector for *server-side* entities.

use std::collections::{HashMap, HashSet};

use bevy::{
    ecs::{reflect::ReflectComponent, resource::IsResource},
    prelude::*,
    reflect::serde::{ReflectDeserializer, ReflectSerializer},
};
use bevy_egui::{egui, EguiContexts};
use bevy_inspector_egui::bevy_inspector::ui_for_entity;
use bincode::Options;
use networking::{
    is_server,
    messaging::{AppExt, MessageEvent, MessageReceivers, MessageSender},
    ConnectionId,
};
use serde::{de::DeserializeSeed, Deserialize, Serialize};

use crate::{ui::has_window, GameState};

/// Messages exchanged between the client inspector and the server.
///
/// Entities are addressed by their raw server [`Entity`] bits. These are meaningless on the client
/// and are only ever used as opaque handles to hand back to the server — this lets us inspect
/// entities that are not networked at all, which is the whole point of a server inspector.
#[derive(Serialize, Deserialize, Clone)]
enum InspectorMessage {
    /// Client -> server: send the children of an entity (`None` = the root entities).
    ChildrenRequest(Option<u64>),
    /// Server -> client: the children of `parent` (`None` = the root entities).
    Children {
        parent: Option<u64>,
        entities: Vec<EntityInfo>,
    },
    /// Client -> server: send the components of this entity.
    EntityRequest(u64),
    /// Server -> client: the components of a single entity.
    EntityData {
        entity: u64,
        /// Each element is a `bincode`-serialized [`ReflectSerializer`] document for one component.
        components: Vec<Vec<u8>>,
        /// Names of components that could not be reflected/serialized (shown greyed out).
        opaque: Vec<String>,
        /// `false` if the entity no longer exists on the server.
        exists: bool,
    },
}

/// One entry in the entity tree.
#[derive(Serialize, Deserialize, Clone)]
struct EntityInfo {
    bits: u64,
    name: String,
    /// Whether this entity has children, so the client can show an expandable node before fetching.
    has_children: bool,
}

fn entity_label(entity: Entity, name: Option<&Name>) -> String {
    name.map(|n| n.as_str().to_owned())
        .unwrap_or_else(|| format!("{entity:?}"))
}

// ---------------------------------------------------------------------------------------------
// Server
// ---------------------------------------------------------------------------------------------

/// Entity-detail requests awaiting the reflection pass (which needs `&mut World`).
#[derive(Resource, Default)]
struct InspectorRequests(Vec<(ConnectionId, u64)>);

/// Replies produced by the reflection pass, waiting to be sent.
#[derive(Resource, Default)]
struct InspectorReplies(Vec<(ConnectionId, InspectorMessage)>);

/// Answers tree requests directly and defers entity-detail requests to [`process_entity_requests`].
fn receive_requests(
    mut messages: MessageReader<MessageEvent<InspectorMessage>>,
    // This fork stores resources as components on entities; `Without<IsResource>` excludes those so
    // the tree shows only real game entities (the client inspector lists resources separately).
    roots: Query<
        (Entity, Option<&Name>, Option<&Children>),
        (Without<ChildOf>, Without<IsResource>),
    >,
    entities: Query<(Option<&Name>, Option<&Children>), Without<IsResource>>,
    child_lists: Query<&Children>,
    mut sender: MessageSender,
    mut pending: ResMut<InspectorRequests>,
) {
    for event in messages.read() {
        match &event.message {
            InspectorMessage::ChildrenRequest(None) => {
                let list = roots
                    .iter()
                    .map(|(entity, name, children)| EntityInfo {
                        bits: entity.to_bits(),
                        name: entity_label(entity, name),
                        has_children: children.is_some_and(|c| !c.is_empty()),
                    })
                    .collect();
                sender.send(
                    &InspectorMessage::Children {
                        parent: None,
                        entities: list,
                    },
                    MessageReceivers::Single(event.connection),
                );
            }
            InspectorMessage::ChildrenRequest(Some(bits)) => {
                let mut list = Vec::new();
                if let Some(parent) = Entity::try_from_bits(*bits) {
                    if let Ok(children) = child_lists.get(parent) {
                        for child in children.iter() {
                            if let Ok((name, sub)) = entities.get(child) {
                                list.push(EntityInfo {
                                    bits: child.to_bits(),
                                    name: entity_label(child, name),
                                    has_children: sub.is_some_and(|c| !c.is_empty()),
                                });
                            }
                        }
                    }
                }
                sender.send(
                    &InspectorMessage::Children {
                        parent: Some(*bits),
                        entities: list,
                    },
                    MessageReceivers::Single(event.connection),
                );
            }
            InspectorMessage::EntityRequest(bits) => pending.0.push((event.connection, *bits)),
            // Server-bound only; the other variants are replies to the client.
            _ => {}
        }
    }
}

/// Reflects each requested entity's components into serialized bytes.
fn process_entity_requests(world: &mut World) {
    let requests = std::mem::take(&mut world.resource_mut::<InspectorRequests>().0);
    if requests.is_empty() {
        return;
    }

    let mut replies = Vec::with_capacity(requests.len());
    let type_registry = world.resource::<AppTypeRegistry>().clone();
    {
        let registry = type_registry.read();
        for (connection, bits) in requests {
            let reply = match Entity::try_from_bits(bits).and_then(|e| world.get_entity(e).ok()) {
                Some(entity_ref) => {
                    let mut components = Vec::new();
                    let mut opaque = Vec::new();
                    for &component_id in entity_ref.archetype().components() {
                        let Some(info) = world.components().get_info(component_id) else {
                            continue;
                        };
                        let reflected = info
                            .type_id()
                            .and_then(|type_id| registry.get(type_id))
                            .and_then(|registration| registration.data::<ReflectComponent>())
                            .and_then(|reflect_component| reflect_component.reflect(entity_ref));

                        match reflected {
                            Some(value) => {
                                let serializer =
                                    ReflectSerializer::new(value.as_partial_reflect(), &registry);
                                match bincode::options().serialize(&serializer) {
                                    Ok(bytes) => components.push(bytes),
                                    Err(err) => {
                                        debug!(component = %info.name(), "Inspector: failed to serialize component: {err}");
                                        opaque.push(info.name().to_string());
                                    }
                                }
                            }
                            None => opaque.push(info.name().to_string()),
                        }
                    }
                    opaque.sort();
                    InspectorMessage::EntityData {
                        entity: bits,
                        components,
                        opaque,
                        exists: true,
                    }
                }
                None => InspectorMessage::EntityData {
                    entity: bits,
                    components: Vec::new(),
                    opaque: Vec::new(),
                    exists: false,
                },
            };
            replies.push((connection, reply));
        }
    }

    world.resource_mut::<InspectorReplies>().0.extend(replies);
}

/// Sends the replies produced by [`process_entity_requests`].
fn send_replies(mut replies: ResMut<InspectorReplies>, mut sender: MessageSender) {
    for (connection, message) in replies.0.drain(..) {
        sender.send(&message, MessageReceivers::Single(connection));
    }
}

// ---------------------------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------------------------

/// Whether the server inspector window is shown. Toggled from the shared Debug Menu (see
/// `crate::debug`) so the inspector lives alongside the existing debug toggles rather than in its
/// own window.
#[derive(Resource, Default)]
pub(crate) struct ServerInspectorEnabled(pub bool);

/// One node in the client's copy of the server entity tree.
struct TreeNode {
    name: String,
    has_children: bool,
    /// The node's children; `None` until they have been fetched from the server.
    children: Option<Vec<u64>>,
}

/// Client-side inspector state.
///
/// Held as a non-send resource because it owns a [`World`] and is only ever touched by the
/// main-thread egui systems anyway.
struct ServerInspector {
    /// Previous value of [`ServerInspectorEnabled`], to detect when the window is opened.
    was_enabled: bool,
    /// Root entities; `None` until fetched.
    roots: Option<Vec<u64>>,
    /// Every entity we've heard about, keyed by server [`Entity`] bits.
    nodes: HashMap<u64, TreeNode>,
    /// Children requests already sent, to avoid re-requesting every frame. `None` = the roots.
    requested: HashSet<Option<u64>>,
    /// Queue of children requests to send (`None` = the roots).
    pending_children: Vec<Option<u64>>,
    selected: Option<u64>,
    pending_entity: Option<u64>,
    /// Holds only the currently-inspected entity, rebuilt on every snapshot.
    mirror: World,
    mirror_entity: Option<Entity>,
    /// Names of the selected entity's non-reflectable components.
    opaque: Vec<String>,
}

fn setup_inspector(world: &mut World) {
    // Clone shares the underlying `Arc<RwLock<TypeRegistry>>`, so the mirror world's registry stays
    // in sync with the app's registrations — exactly what `ui_for_entity` needs to render widgets.
    let registry = world.resource::<AppTypeRegistry>().clone();
    let mut mirror = World::new();
    mirror.insert_resource(registry);
    world.insert_non_send(ServerInspector {
        was_enabled: false,
        roots: None,
        nodes: HashMap::new(),
        requested: HashSet::new(),
        pending_children: Vec::new(),
        selected: None,
        pending_entity: None,
        mirror,
        mirror_entity: None,
        opaque: Vec::new(),
    });
}

/// Draws one entity and (if expanded) recurses into its children, requesting them lazily.
fn render_node(
    ui: &mut egui::Ui,
    bits: u64,
    nodes: &HashMap<u64, TreeNode>,
    selected: Option<u64>,
    clicked: &mut Option<u64>,
    expand: &mut Vec<u64>,
    depth: usize,
) {
    // Guard against a malformed (cyclic) hierarchy blowing the stack.
    if depth > 64 {
        return;
    }
    let Some(node) = nodes.get(&bits) else {
        return;
    };
    let label = format!("{} ({bits})", node.name);

    if !node.has_children {
        if ui.selectable_label(selected == Some(bits), label).clicked() {
            *clicked = Some(bits);
        }
        return;
    }

    let id = egui::Id::new(("ssnt-server-inspector", bits));
    egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false)
        .show_header(ui, |ui| {
            if ui.selectable_label(selected == Some(bits), label).clicked() {
                *clicked = Some(bits);
            }
        })
        .body(|ui| match &node.children {
            // Children not fetched yet — the body only runs while expanded, so this is exactly the
            // moment to ask the server for them.
            None => {
                expand.push(bits);
                ui.weak("Loading…");
            }
            Some(children) => {
                for &child in children {
                    render_node(ui, child, nodes, selected, clicked, expand, depth + 1);
                }
            }
        });
}

fn inspector_ui(
    mut contexts: EguiContexts,
    enabled: Res<ServerInspectorEnabled>,
    state: NonSendMut<ServerInspector>,
) {
    if !enabled.0 {
        return;
    }
    let state = state.into_inner();
    let ctx = contexts.ctx_mut().unwrap();

    let selected = state.selected;
    let mut clicked: Option<u64> = None;
    let mut expand: Vec<u64> = Vec::new();
    let mut refresh = false;

    egui::Window::new("Server Inspector").show(ctx, |ui| {
        if ui.button("Refresh").clicked() {
            refresh = true;
        }
        ui.separator();

        // Don't shrink to content width, so the tree fills the whole window width and rows aren't
        // squeezed/truncated.
        egui::ScrollArea::vertical()
            .id_salt("server-inspector-tree")
            .max_height(250.0)
            .auto_shrink([false, true])
            .show(ui, |ui| match &state.roots {
                None => {
                    ui.weak("Loading…");
                }
                Some(roots) => {
                    for &bits in roots {
                        render_node(
                            ui,
                            bits,
                            &state.nodes,
                            selected,
                            &mut clicked,
                            &mut expand,
                            0,
                        );
                    }
                }
            });

        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("server-inspector-detail")
            .max_height(300.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if let Some(entity) = state.mirror_entity {
                    ui_for_entity(&mut state.mirror, entity, ui);
                    if !state.opaque.is_empty() {
                        ui.separator();
                        ui.label("Non-reflectable components:");
                        for name in &state.opaque {
                            ui.label(egui::RichText::new(name).weak());
                        }
                    }
                } else if state.selected.is_some() {
                    ui.weak("Loading…");
                }
            });
    });

    // Apply UI-driven state changes once the borrows above have ended.
    if refresh {
        state.roots = None;
        state.nodes.clear();
        state.requested.clear();
        state.mirror_entity = None;
        state.selected = None;
        state.requested.insert(None);
        state.pending_children.push(None);
    }
    if let Some(bits) = clicked {
        state.selected = Some(bits);
        state.pending_entity = Some(bits);
    }
    for bits in expand {
        if state.requested.insert(Some(bits)) {
            state.pending_children.push(Some(bits));
        }
    }
}

fn send_requests(
    enabled: Res<ServerInspectorEnabled>,
    state: NonSendMut<ServerInspector>,
    mut sender: MessageSender,
) {
    let state = state.into_inner();
    // Fetch the root entities automatically when the inspector is first opened.
    if enabled.0 && !state.was_enabled && state.requested.insert(None) {
        state.pending_children.push(None);
    }
    state.was_enabled = enabled.0;

    for request in state.pending_children.drain(..) {
        sender.send_to_server(&InspectorMessage::ChildrenRequest(request));
    }
    if let Some(bits) = state.pending_entity.take() {
        sender.send_to_server(&InspectorMessage::EntityRequest(bits));
    }
}

fn receive_data(
    mut messages: MessageReader<MessageEvent<InspectorMessage>>,
    state: NonSendMut<ServerInspector>,
) {
    let state = state.into_inner();
    for event in messages.read() {
        match &event.message {
            InspectorMessage::Children { parent, entities } => {
                let child_bits: Vec<u64> = entities.iter().map(|info| info.bits).collect();
                for info in entities {
                    state.nodes.entry(info.bits).or_insert_with(|| TreeNode {
                        name: info.name.clone(),
                        has_children: info.has_children,
                        children: None,
                    });
                }
                match parent {
                    None => state.roots = Some(child_bits),
                    Some(parent) => {
                        if let Some(node) = state.nodes.get_mut(parent) {
                            node.children = Some(child_bits);
                        }
                    }
                }
            }
            InspectorMessage::EntityData {
                entity,
                components,
                opaque,
                exists,
            } => {
                // Ignore stale snapshots for an entity we're no longer looking at.
                if state.selected == Some(*entity) {
                    apply_entity_data(state, components, opaque, *exists);
                }
            }
            // Client receives replies only.
            _ => {}
        }
    }
}

/// Rebuilds the mirror world's single entity from a server snapshot.
fn apply_entity_data(
    state: &mut ServerInspector,
    components: &[Vec<u8>],
    opaque: &[String],
    exists: bool,
) {
    if let Some(old) = state.mirror_entity.take() {
        state.mirror.despawn(old);
    }
    state.opaque = opaque.to_vec();
    if !exists {
        return;
    }

    let registry = state.mirror.resource::<AppTypeRegistry>().clone();
    let entity = state.mirror.spawn_empty().id();
    {
        let registry = registry.read();
        for bytes in components {
            let mut deserializer = bincode::Deserializer::from_slice(bytes, bincode::options());
            let value = match ReflectDeserializer::new(&registry).deserialize(&mut deserializer) {
                Ok(value) => value,
                Err(err) => {
                    debug!("Inspector: failed to deserialize component: {err}");
                    continue;
                }
            };
            let Some(registration) = value
                .get_represented_type_info()
                .and_then(|info| registry.get(info.type_id()))
            else {
                continue;
            };
            let Some(reflect_component) = registration.data::<ReflectComponent>() else {
                continue;
            };
            let mut entity_mut = state.mirror.entity_mut(entity);
            reflect_component.insert(&mut entity_mut, value.as_ref(), &registry);
        }
    }
    state.mirror_entity = Some(entity);
}

// ---------------------------------------------------------------------------------------------

pub(crate) struct InspectorPlugin;

impl Plugin for InspectorPlugin {
    fn build(&self, app: &mut App) {
        // Registered on both roles (in the shared plugin section) so message ids stay aligned.
        app.add_network_message::<InspectorMessage>();

        if is_server(app) {
            app.init_resource::<InspectorRequests>()
                .init_resource::<InspectorReplies>()
                .add_systems(
                    Update,
                    (
                        receive_requests.run_if(on_message::<MessageEvent<InspectorMessage>>),
                        process_entity_requests,
                        send_replies,
                    )
                        .chain(),
                );
        } else {
            app.init_resource::<ServerInspectorEnabled>()
                .add_systems(Startup, setup_inspector)
                .add_systems(
                    Update,
                    (receive_data, inspector_ui.run_if(has_window), send_requests)
                        .chain()
                        .run_if(in_state(GameState::Game)),
                );
        }
    }
}
