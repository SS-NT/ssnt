use bevy::{
    asset::AssetPathId,
    ecs::query::QuerySingleError,
    prelude::{
        debug, error, info, warn, App, AssetServer, Commands, Component, DespawnRecursiveExt,
        Entity, EventReader, EventWriter, Handle, ParallelSystemDescriptorCoercion, Plugin, Query,
        RemovedComponents, Res, ResMut, SystemLabel, SystemSet, With,
    },
    scene::{DynamicScene, DynamicSceneBundle},
    utils::{HashMap, HashSet},
};
use serde::{Deserialize, Serialize};

use crate::{
    identity::{NetworkIdentities, NetworkIdentity},
    messaging::{AppExt, MessageEvent, MessageReceivers, MessageSender},
    visibility::{NetworkVisibilities, VisibilitySystem},
    ConnectionId, NetworkManager, NetworkSystem,
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
    Empty,
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
pub enum ServerEntityEvent {
    /// A spawn message has been sent to a connection
    Spawned((Entity, ConnectionId)),
    /// A despawn message has been sent to a connection
    Despawned((Entity, ConnectionId)),
}

/// Events related to networked entities on the client
pub enum NetworkedEntityEvent {
    /// A networked entity has been spawned
    Spawned(Entity),
    /// A networked entity has been despawned
    Despawned(Entity),
}

fn send_spawn_messages(
    query: Query<(
        Entity,
        &NetworkIdentity,
        Option<&PrefabPath>,
        Option<&Handle<DynamicScene>>,
    )>,
    visibilities: Res<NetworkVisibilities>,
    mut sender: MessageSender,
    mut entity_events: EventWriter<ServerEntityEvent>,
) {
    for (entity, identity, name, scene) in query.iter() {
        if let Some(visibility) = visibilities.visibility.get(identity) {
            let new_observers: HashSet<ConnectionId> =
                visibility.new_observers().copied().collect();
            if !new_observers.is_empty() {
                // Get the asset hash or the string name that identifies the object
                let identifier = match (name, scene) {
                    (None, None) => SpawnAssetIdentifier::Empty,
                    (None, Some(scene)) => SpawnAssetIdentifier::AssetPath(match scene.id {
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
                sender.send(
                    &SpawnMessage::Spawn(message),
                    MessageReceivers::Set(new_observers.clone()),
                );
                entity_events.send_batch(
                    new_observers
                        .iter()
                        .map(|c| ServerEntityEvent::Spawned((entity, *c))),
                );
            }

            let removed_observers: HashSet<ConnectionId> =
                visibility.removed_observers().copied().collect();
            if !removed_observers.is_empty() {
                sender.send(
                    &SpawnMessage::Despawn(*identity),
                    MessageReceivers::Set(removed_observers.clone()),
                );
                entity_events.send_batch(
                    removed_observers
                        .iter()
                        .map(|c| ServerEntityEvent::Despawned((entity, *c))),
                );
            }
        }
    }
}

/// Sends despawn messages for entities that were deleted on the server.
fn network_deleted_entities(
    removed: RemovedComponents<NetworkIdentity>,
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
                sender.send(
                    &SpawnMessage::Despawn(identity),
                    MessageReceivers::Set(observers),
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

                let mut builder = commands.spawn();
                builder.insert(spawn.network_id);

                match spawn.identifier {
                    SpawnAssetIdentifier::Named(name) => {
                        builder.insert(PrefabPath(name));
                    }
                    SpawnAssetIdentifier::AssetPath(id) => {
                        builder.insert_bundle(DynamicSceneBundle {
                            scene: asset_server.get_handle(id),
                            ..Default::default()
                        });
                    }
                    SpawnAssetIdentifier::Empty => {}
                }
                let entity = builder.insert(spawn.network_id).id();
                ids.set_identity(entity, spawn.network_id);
                entity_events.send(NetworkedEntityEvent::Spawned(entity));

                debug!("Received spawn message for {:?}", spawn.network_id);
            }
            SpawnMessage::Despawn(id) => {
                if let Some(entity) = ids.get_entity(*id) {
                    commands.entity(entity).despawn_recursive();
                    ids.remove_entity(entity);
                    debug!("Received despawn message for {:?}", id);
                } else {
                    warn!("Received despawn message for non-existent {:?}", id);
                }
            }
        }
    }
}

/// Tracks which connected client controls which entity
#[derive(Default)]
pub struct ClientControls {
    entities: HashMap<ConnectionId, Entity>,
    changed: HashSet<ConnectionId>,
}

impl ClientControls {
    pub fn give_control(&mut self, connection: ConnectionId, entity: Entity) {
        self.entities.insert(connection, entity);
        self.changed.insert(connection);
    }

    pub fn does_control(&self, connection: ConnectionId, entity: Entity) -> bool {
        self.entities.get(&connection) == Some(&entity)
    }

    pub fn controlled_entity(&self, connection: ConnectionId) -> Option<Entity> {
        self.entities.get(&connection).copied()
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct ControlUpdate {
    controlled_entity: Option<NetworkIdentity>,
}

fn send_control_updates(
    mut controls: ResMut<ClientControls>,
    identities: Res<NetworkIdentities>,
    mut sender: MessageSender,
) {
    let controls = &mut *controls;

    controls.changed.retain(|connection| {
        let new_entity = controls.entities.get(connection).copied();
        let newly_controlled = new_entity.and_then(|e| identities.get_identity(e));

        // Keep change if no network identity available yet
        if new_entity.is_some() && newly_controlled.is_none() {
            return true;
        }

        let update = ControlUpdate {
            controlled_entity: newly_controlled,
        };
        sender.send(&update, MessageReceivers::Single(*connection));
        false
    });
}

fn receive_control_updates(
    mut events: EventReader<MessageEvent<ControlUpdate>>,
    query: Query<Entity, With<ClientControlled>>,
    ids: Res<NetworkIdentities>,
    mut commands: Commands,
) {
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
            // TODO: Somehow buffer until network identity is known?
            if let Some(new_entity) = ids.get_entity(id) {
                commands.entity(new_entity).insert(ClientControlled);
            } else {
                error!(
                    "Received client control update for non-existing identity {:?}",
                    id
                );
            }
        }
    }
}

/// A marker component to signify that an enity is controlled by this client
#[derive(Component)]
pub struct ClientControlled;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, SystemLabel)]
pub enum SpawningSystems {
    Spawn,
    ClientControl,
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
                .add_system_set(
                    SystemSet::new()
                        .after(NetworkSystem::ReadNetworkMessages)
                        .with_system(
                            send_spawn_messages
                                .label(SpawningSystems::Spawn)
                                .after(NetworkSystem::Visibility),
                        )
                        .with_system(
                            network_deleted_entities.before(VisibilitySystem::UpdateVisibility),
                        )
                        .with_system(
                            send_control_updates
                                .label(SpawningSystems::ClientControl)
                                .after(SpawningSystems::Spawn),
                        ),
                );
        } else {
            app.add_event::<NetworkedEntityEvent>().add_system_set(
                SystemSet::new()
                    .after(NetworkSystem::ReadNetworkMessages)
                    .with_system(receive_spawn.label(SpawningSystems::Spawn))
                    .with_system(
                        receive_control_updates
                            .label(SpawningSystems::ClientControl)
                            .after(SpawningSystems::Spawn),
                    ),
            );
        }
    }
}
