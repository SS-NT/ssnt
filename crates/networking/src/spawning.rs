use bevy::{
    ecs::system::QuerySingleError,
    prelude::{
        info, warn, App, Commands, Component, Entity, EventReader, EventWriter,
        ParallelSystemDescriptorCoercion, Plugin, Query, Res, ResMut, SystemLabel, With, SystemSet,
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

// Temporary struct to label networked objects
// This should be replaced with the scene identifier in a future bevy release
#[derive(Component)]
pub struct PrefabPath(pub String);

/// Events related to networked entities
pub enum NetworkedEntityEvent {
    /// A networked entity has been spawned
    Spawned(Entity),
}

fn send_spawn(
    query: Query<(&NetworkIdentity, &PrefabPath)>,
    visibilities: Res<NetworkVisibilities>,
    mut sender: MessageSender,
) {
    for (identity, prefab) in query.iter() {
        if let Some(visibility) = visibilities.visibility.get(identity) {
            let new_observers = visibility.new_observers();
            if new_observers.is_empty() {
                continue;
            }

            let message = SpawnEntity {
                name: prefab.0.clone(),
                network_id: *identity,
            };
            sender.send(&message, MessageReceivers::Set(new_observers.clone()));
        }
    }
}

fn receive_spawn(
    mut spawn_events: EventReader<MessageEvent<SpawnEntity>>,
    mut entity_events: EventWriter<NetworkedEntityEvent>,
    mut ids: ResMut<NetworkIdentities>,
    mut commands: Commands,
) {
    for event in spawn_events.iter() {
        let spawn = event.message.clone();

        // TODO: Actually spawn entity from asset path
        let entity = commands
            .spawn()
            .insert(spawn.network_id)
            .insert(PrefabPath(spawn.name))
            .id();

        ids.set_identity(entity, spawn.network_id);
        entity_events.send(NetworkedEntityEvent::Spawned(entity));
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

    fn does_control(&self, connection: ConnectionId, entity: Entity) -> bool {
        self.entities.get(&connection) == Some(&entity)
    }

    fn controlled_entity(&self, connection: ConnectionId) -> Option<Entity> {
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
        info!("Client control updating to {:?}", event.message.controlled_entity);

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
                warn!(
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
enum SpawningSystems {
    Spawn,
    ClientControl,
}

pub(crate) struct SpawningPlugin;

impl Plugin for SpawningPlugin {
    fn build(&self, app: &mut App) {
        app.add_network_message::<SpawnEntity>()
            .add_network_message::<ControlUpdate>();

        if app
            .world
            .get_resource::<NetworkManager>()
            .unwrap()
            .is_server()
        {
            app.init_resource::<ClientControls>().add_system_set(
                SystemSet::new()
                    .after(NetworkSystem::ReadNetworkMessages)
                    .with_system(send_spawn.label(SpawningSystems::Spawn))
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
