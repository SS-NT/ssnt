use bevy::{
    asset::AssetPathId,
    ecs::query::{Has, QuerySingleError},
    prelude::*,
    scene::DynamicScene,
    utils::{HashMap, HashSet, Uuid},
};
use serde::{Deserialize, Serialize};

use crate::{
    identity::{IdentitySystem, NetworkIdentities, NetworkIdentity},
    messaging::{AppExt, MessageEvent, MessageReceivers, MessageSender},
    scene::{NetworkScene, NetworkSceneBundle, NetworkedChild},
    visibility::NetworkVisibilities,
    ConnectionId, NetworkManager, NetworkSet, Players, ServerEvent,
};

/// A message that instructs the client to spawn a specific entity.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct SpawnEntity {
    pub network_id: NetworkIdentity,
    pub identifier: SpawnAssetIdentifier,
}

/// Tells a client what object to spawn.
#[derive(Serialize, Deserialize, Debug, Clone)]
enum SpawnAssetIdentifier {
    // TODO: Remove once obsoleted.
    // We should always use unique ids instead of strings when networking.
    Named(String),
    AssetPath(AssetPathId),
    /// Objects that are used as references and don't need an asset
    Empty {
        in_world: bool,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
enum SpawnMessage {
    Spawn(SpawnEntity),
    Despawn(NetworkIdentity),
}

// Temporary struct to label networked objects
// This should be replaced with the scene identifier in a future bevy release
#[derive(Component)]
pub struct PrefabPath(pub String);

/// Events related to networked entities on the server
#[derive(Event)]
pub enum ServerEntityEvent {
    /// A spawn message has been sent to a connection
    Spawned((Entity, ConnectionId)),
    /// A despawn message has been sent to a connection
    Despawned((Entity, ConnectionId)),
}

const DESPAWN_MESSAGE_PRIORITY: i16 = -10;

/// Events related to networked entities on the client
#[derive(Event)]
pub enum NetworkedEntityEvent {
    /// A networked entity has been spawned
    Spawned(Entity),
    /// A networked entity has been despawned
    Despawned(Entity),
}

#[allow(clippy::too_many_arguments)]
fn send_spawn_messages(
    query: Query<
        (
            Entity,
            &NetworkIdentity,
            Option<&PrefabPath>,
            Option<&NetworkScene>,
            Has<ComputedVisibility>,
        ),
        Without<NetworkedChild>,
    >,
    with_parents: Query<(), With<Parent>>,
    visibilities: Res<NetworkVisibilities>,
    controlled: Res<ClientControls>,
    players: Res<Players>,
    mut sender: MessageSender,
    mut entity_events: EventWriter<ServerEntityEvent>,
    scenes: Res<Assets<DynamicScene>>,
) {
    for (entity, identity, name, scene, has_visibiliy) in query.iter() {
        // Only send scenes once they're loaded
        if let Some(scene) = scene {
            // TODO: Can we check for scene spawned instead of asset existence?
            if !scenes.contains(&scene.0) {
                continue;
            }
        }

        if let Some(visibility) = visibilities.visibility.get(identity) {
            let new_observers: HashSet<ConnectionId> =
                visibility.new_observers().copied().collect();
            if !new_observers.is_empty() {
                // Get the asset hash or the string name that identifies the object
                let identifier = match (name, scene) {
                    (None, None) => {
                        // Only allow root objects without asset identifier
                        // TODO: There's probably a better way to prevent scene children spawning
                        if with_parents.contains(entity) {
                            continue;
                        }
                        SpawnAssetIdentifier::Empty {
                            in_world: has_visibiliy,
                        }
                    }
                    (None, Some(scene)) => SpawnAssetIdentifier::AssetPath(match scene.0.id() {
                        bevy::asset::HandleId::Id(_, _) => {
                            warn!(entity = ?entity, "Cannot spawn networked object with dynamic handle id. Handle must be created from a loaded asset.");
                            continue;
                        }
                        bevy::asset::HandleId::AssetPathId(p) => p,
                    }),
                    (Some(name), None) => SpawnAssetIdentifier::Named(name.0.clone()),
                    (Some(_), Some(_)) => {
                        warn!("Entity has both an asset path id and a prefab path. Skipping.");
                        continue;
                    }
                };

                let message = SpawnEntity {
                    identifier,
                    network_id: *identity,
                };

                // Increase priority if object is owned by a player
                let priority = if controlled.controlling_player(entity).is_some() {
                    60
                } else {
                    50
                };
                sender.send_with_priority(
                    &SpawnMessage::Spawn(message),
                    MessageReceivers::Set(new_observers.clone()),
                    priority,
                );
                entity_events.send_batch(
                    new_observers
                        .iter()
                        .map(|c| ServerEntityEvent::Spawned((entity, *c))),
                );
            }

            let connected_players = players.players();
            let removed_observers: HashSet<ConnectionId> = visibility
                .removed_observers()
                .copied()
                .filter(|c| connected_players.contains_key(c))
                .collect();
            if !removed_observers.is_empty() {
                entity_events.send_batch(
                    removed_observers
                        .iter()
                        .map(|c| ServerEntityEvent::Despawned((entity, *c))),
                );
                // Send despawn message
                sender.send_with_priority(
                    &SpawnMessage::Despawn(*identity),
                    MessageReceivers::Set(removed_observers),
                    DESPAWN_MESSAGE_PRIORITY,
                );
            }
        }
    }
}

/// Sends despawn messages for entities that were deleted on the server.
fn network_deleted_entities(
    mut removed: RemovedComponents<NetworkIdentity>,
    identities: Res<NetworkIdentities>,
    visibilities: Res<NetworkVisibilities>,
    mut sender: MessageSender,
    mut entity_events: EventWriter<ServerEntityEvent>,
) {
    for entity in removed.iter() {
        let identity = identities.get_identity(entity).unwrap();
        if let Some(visibility) = visibilities.visibility.get(&identity) {
            let observers: HashSet<ConnectionId> = visibility.all_observers().copied().collect();
            if !observers.is_empty() {
                entity_events.send_batch(
                    observers
                        .iter()
                        .map(|c| ServerEntityEvent::Despawned((entity, *c))),
                );
                sender.send_with_priority(
                    &SpawnMessage::Despawn(identity),
                    MessageReceivers::Set(observers),
                    DESPAWN_MESSAGE_PRIORITY,
                );
            }
        }
    }
}

fn receive_spawn(
    mut spawn_events: EventReader<MessageEvent<SpawnMessage>>,
    mut entity_events: EventWriter<NetworkedEntityEvent>,
    mut ids: ResMut<NetworkIdentities>,
    mut commands: Commands,
    asset_server: ResMut<AssetServer>,
) {
    for event in spawn_events.iter() {
        match &event.message {
            SpawnMessage::Spawn(s) => {
                let spawn = s.clone();

                if ids.get_entity(spawn.network_id).is_some() {
                    warn!(
                        "Received spawn message for already existing {:?}",
                        spawn.network_id
                    );
                    continue;
                }

                let mut builder = commands.spawn(spawn.network_id);

                match spawn.identifier {
                    SpawnAssetIdentifier::Named(name) => {
                        builder.insert(PrefabPath(name));
                    }
                    SpawnAssetIdentifier::AssetPath(id) => {
                        builder.insert(NetworkSceneBundle {
                            scene: asset_server.get_handle(id).into(),
                            ..Default::default()
                        });
                    }
                    SpawnAssetIdentifier::Empty { in_world } => {
                        if in_world {
                            builder.insert(SpatialBundle::default());
                        }
                    }
                }

                let entity = builder.id();
                ids.set_identity(entity, spawn.network_id);
                entity_events.send(NetworkedEntityEvent::Spawned(entity));

                debug!("Received spawn message for {:?}", spawn.network_id);
            }
            SpawnMessage::Despawn(id) => {
                if let Some(entity) = ids.get_entity(*id) {
                    commands.entity(entity).despawn_recursive();
                    ids.remove_entity(entity);
                    entity_events.send(NetworkedEntityEvent::Despawned(entity));
                    debug!("Received despawn message for {:?}", id);
                } else {
                    warn!("Received despawn message for non-existent {:?}", id);
                }
            }
        }
    }
}

/// Tracks which connected client controls which entity
#[derive(Default, Resource)]
pub struct ClientControls {
    mapping: HashMap<Uuid, Entity>,
    reverse_mapping: HashMap<Entity, Uuid>,
    changed: HashSet<Uuid>,
}

impl ClientControls {
    pub fn give_control(&mut self, id: Uuid, entity: Entity) {
        if let Some(entity) = self.mapping.remove(&id) {
            self.reverse_mapping.remove(&entity);
        }

        self.mapping.insert(id, entity);
        self.reverse_mapping.insert(entity, id);
        self.changed.insert(id);
    }

    pub fn does_control(&self, id: Uuid, entity: Entity) -> bool {
        self.mapping.get(&id) == Some(&entity)
    }

    pub fn controlled_entity(&self, id: Uuid) -> Option<Entity> {
        self.mapping.get(&id).copied()
    }

    pub fn controlling_player(&self, entity: Entity) -> Option<Uuid> {
        self.reverse_mapping.get(&entity).copied()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ControlUpdate {
    controlled_entity: Option<NetworkIdentity>,
}

fn send_control_updates(
    mut controls: ResMut<ClientControls>,
    identities: Res<NetworkIdentities>,
    players: Res<Players>,
    mut sender: MessageSender,
) {
    let controls = &mut *controls;

    controls.changed.retain(|id| {
        let new_entity = controls.mapping.get(id).copied();
        let newly_controlled = new_entity.and_then(|e| identities.get_identity(e));

        // Keep change if no network identity available yet
        if new_entity.is_some() && newly_controlled.is_none() {
            return true;
        }

        let update = ControlUpdate {
            controlled_entity: newly_controlled,
        };
        if let Some(connection) = players.get_connection(id) {
            sender.send_with_priority(&update, MessageReceivers::Single(connection), 55);
        }
        false
    });
}

/// Sends the controlled entities to joined player that already had control of an entity (rejoin).
fn send_control_updates_to_rejoined(
    mut events: EventReader<ServerEvent>,
    mut controls: ResMut<ClientControls>,
    identities: Res<NetworkIdentities>,
    players: Res<Players>,
    mut sender: MessageSender,
) {
    let controls = &mut *controls;

    for connection in events.iter().filter_map(|e| match e {
        ServerEvent::PlayerConnected(c) => Some(c),
        _ => None,
    }) {
        let player = players.get(*connection).unwrap();
        if let Some(controlled) = controls.controlled_entity(player.id) {
            let identity = identities.get_identity(controlled).unwrap();
            let update = ControlUpdate {
                controlled_entity: Some(identity),
            };
            sender.send_with_priority(&update, MessageReceivers::Single(*connection), 55);
        }
    }
}

fn receive_control_updates(
    mut events: EventReader<MessageEvent<ControlUpdate>>,
    query: Query<Entity, With<ClientControlled>>,
    ids: Res<NetworkIdentities>,
    mut buffered_controlled: Local<Option<NetworkIdentity>>,
    mut commands: Commands,
) {
    if let Some(network_id) = buffered_controlled.as_ref() {
        if let Some(new_entity) = ids.get_entity(*network_id) {
            commands.entity(new_entity).insert(ClientControlled);
            *buffered_controlled = None;
        }
    }

    for event in events.iter() {
        info!(
            "Client control updating to {:?}",
            event.message.controlled_entity
        );

        let existing_entity = query.get_single().map_or_else(
            |e| match e {
                QuerySingleError::NoEntities(_) => None,
                QuerySingleError::MultipleEntities(_) => {
                    panic!("Multiple entities with ClientControlled")
                }
            },
            Some,
        );
        if let Some(e) = existing_entity {
            commands.entity(e).remove::<ClientControlled>();
        }

        let new_id = event.message.controlled_entity;
        if let Some(id) = new_id {
            if let Some(new_entity) = ids.get_entity(id) {
                commands.entity(new_entity).insert(ClientControlled);
            } else {
                *buffered_controlled = Some(id);
            }
        }
    }
}

/// A marker component to signify that an enity is controlled by this client
#[derive(Component)]
pub struct ClientControlled;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, SystemSet)]
pub enum SpawningSet {
    SpawnEmpty,
    BeforeDespawn,
    SpawnScenes,
}

pub(crate) struct SpawningPlugin;

impl Plugin for SpawningPlugin {
    fn build(&self, app: &mut App) {
        app.add_network_message::<SpawnMessage>()
            .add_network_message::<ControlUpdate>();

        if app
            .world
            .get_resource::<NetworkManager>()
            .unwrap()
            .is_server()
        {
            app.add_event::<ServerEntityEvent>()
                .init_resource::<ClientControls>()
                .add_systems(
                    PostUpdate,
                    (
                        send_spawn_messages,
                        send_control_updates,
                        send_control_updates_to_rejoined,
                        network_deleted_entities.before(IdentitySystem::ClearRemoved),
                    )
                        .in_set(NetworkSet::ServerWrite),
                );
        } else {
            app.add_event::<NetworkedEntityEvent>()
                .configure_sets(
                    PreUpdate,
                    (
                        SpawningSet::SpawnEmpty,
                        SpawningSet::BeforeDespawn,
                        SpawningSet::SpawnScenes,
                    )
                        .chain()
                        .in_set(NetworkSet::ClientSpawn),
                )
                .add_systems(
                    PreUpdate,
                    (
                        receive_spawn.in_set(SpawningSet::SpawnEmpty),
                        receive_control_updates.in_set(NetworkSet::ClientApply),
                    ),
                );
        }
    }
}
