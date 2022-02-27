use bevy::{
    ecs::system::QuerySingleError,
    prelude::{
        info, warn, App, Commands, Component, Entity, EventReader, EventWriter,
        ParallelSystemDescriptorCoercion, Plugin, Query, Res, ResMut, SystemLabel, SystemSet, With, error,
    },
    utils::{HashMap, HashSet},
};
use serde::{Deserialize, Serialize};

use crate::{
    identity::{NetworkIdentities, NetworkIdentity},
    messaging::{AppExt, MessageEvent, MessageReceivers, MessageSender},
    visibility::NetworkVisibilities,
    ConnectionId, NetworkManager, NetworkSystem,
};

#[derive(Serialize, Deserialize, Debug, Clone)]
struct SpawnEntity {
    pub network_id: NetworkIdentity,
    // TODO: Replace with asset path hash?
    pub name: String,
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
    query: Query<(Entity, &NetworkIdentity, &PrefabPath)>,
    visibilities: Res<NetworkVisibilities>,
    mut sender: MessageSender,
    mut entity_events: EventWriter<ServerEntityEvent>,
) {
    for (entity, identity, prefab) in query.iter() {
        if let Some(visibility) = visibilities.visibility.get(identity) {
            let new_observers: HashSet<ConnectionId> = visibility.new_observers().copied().collect();
            if !new_observers.is_empty() {
                let message = SpawnEntity {
                    name: prefab.0.clone(),
                    network_id: *identity,
                };
                sender.send(&SpawnMessage::Spawn(message), MessageReceivers::Set(new_observers.clone()));
                entity_events.send_batch(
                    new_observers
                        .iter()
                        .map(|c| ServerEntityEvent::Spawned((entity, *c))),
                );
            }

            let removed_observers: HashSet<ConnectionId> = visibility.removed_observers().copied().collect();
            if !removed_observers.is_empty() {
                sender.send(&SpawnMessage::Despawn(*identity), MessageReceivers::Set(removed_observers.clone()));
                entity_events.send_batch(
                    removed_observers
                        .iter()
                        .map(|c| ServerEntityEvent::Despawned((entity, *c))),
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
) {
    for event in spawn_events.iter() {
        match &event.message {
            SpawnMessage::Spawn(s) => {
                let spawn = s.clone();

                if ids.get_entity(spawn.network_id).is_some() {
                    warn!("Received spawn message for already existing {:?}", spawn.network_id);
                    continue;
                }

                // TODO: Actually spawn entity from asset path
                let entity = commands
                    .spawn()
                    .insert(spawn.network_id)
                    .insert(PrefabPath(spawn.name))
                    .id();

                ids.set_identity(entity, spawn.network_id);
                entity_events.send(NetworkedEntityEvent::Spawned(entity));

                info!("Received spawn message for {:?}", spawn.network_id);
            },
            SpawnMessage::Despawn(id) => {
                if let Some(entity) = ids.get_entity(*id) {
                    // TODO: Uncomment once rapier doesn't f***ing panic
                    // commands.entity(entity).despawn();
                    ids.remove_entity(entity);
                    info!("Received despawn message for {:?}", id);
                } else {
                    warn!("Received despawn message for non-existent {:?}", id);
                }
            },
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
                .init_resource::<ClientControls>().add_system_set(
                SystemSet::new()
                    .after(NetworkSystem::ReadNetworkMessages)
                    .with_system(send_spawn_messages.label(SpawningSystems::Spawn).after(NetworkSystem::Visibility))
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
