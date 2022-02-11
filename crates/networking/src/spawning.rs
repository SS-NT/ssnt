use bevy::prelude::{
    App, Commands, Component, Entity, EventReader, EventWriter, Plugin, Query, Res,
};
use serde::{Deserialize, Serialize};

use crate::{
    identity::NetworkIdentity,
    messaging::{AppExt, MessageEvent, MessageReceivers, MessageSender},
    visibility::NetworkVisibilities,
    NetworkManager,
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

        entity_events.send(NetworkedEntityEvent::Spawned(entity));
    }
}

pub(crate) struct SpawningPlugin;

impl Plugin for SpawningPlugin {
    fn build(&self, app: &mut App) {
        app.add_network_message::<SpawnEntity>();

        if app
            .world
            .get_resource::<NetworkManager>()
            .unwrap()
            .is_server()
        {
            app.add_system(send_spawn);
        } else {
            app.add_event::<NetworkedEntityEvent>()
                .add_system(receive_spawn);
        }
    }
}
