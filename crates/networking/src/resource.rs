use bevy::{ecs::system::StaticSystemParam, prelude::*, utils::HashSet};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::{
    is_server,
    messaging::{
        AppExt as MessageAppExt, MessageEvent, MessageReceivers, MessageSender, MessagingSystem,
    },
    time::ServerNetworkTime,
    variable::{self, NetworkRegistry, NetworkedFromServer, NetworkedToClient},
    NetworkSystem, Players, ServerEvent,
};

#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct ResourceNetworkId(u16);

impl From<ResourceNetworkId> for u16 {
    fn from(id: ResourceNetworkId) -> Self {
        id.0
    }
}

impl From<u16> for ResourceNetworkId {
    fn from(id: u16) -> Self {
        Self(id)
    }
}

type NetworkedResourceRegistry = NetworkRegistry<ResourceNetworkId>;

/// A message that contains data for a resource.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct NetworkedResourceMessage {
    resource_id: ResourceNetworkId,
    data: Bytes,
}

fn send_networked_resource_to_new<
    S: NetworkedToClient + Send + Sync + 'static,
    C: NetworkedFromServer,
>(
    resource: Res<S>,
    registry: Res<NetworkedResourceRegistry>,
    mut sender: MessageSender,
    mut events: EventReader<ServerEvent>,
    mut param: StaticSystemParam<S::Param>,
) {
    let resource_id = registry
        .get_id(&C::TYPE_UUID)
        .expect("Networked resource incorrectly registered");
    let new_players = events.iter().filter_map(|e| match e {
        ServerEvent::PlayerConnected(c) => Some(c),
        _ => None,
    });
    if S::receiver_matters() {
        // Serialize resource for every receiver
        for connection in new_players {
            let data = match resource.serialize(&mut param, Some(*connection), None) {
                Some(d) => d,
                None => continue,
            };

            sender.send(
                &NetworkedResourceMessage { resource_id, data },
                MessageReceivers::Single(*connection),
            );
        }
    } else {
        let new_observers: HashSet<_> = new_players.copied().collect();
        if !new_observers.is_empty() {
            let data = resource
                .serialize(&mut param, None, None)
                .expect("Serializing without a specific receiver should always return data");
            sender.send(
                &NetworkedResourceMessage { resource_id, data },
                MessageReceivers::Set(new_observers),
            );
        }
    }
}

fn send_changed_networked_resource<
    S: NetworkedToClient + Send + Sync + 'static,
    C: NetworkedFromServer,
>(
    mut resource: ResMut<S>,
    registry: Res<NetworkedResourceRegistry>,
    players: Res<Players>,
    server_time: Res<ServerNetworkTime>,
    mut sender: MessageSender,
    mut param: StaticSystemParam<S::Param>,
) {
    if !resource.is_changed() {
        return;
    }

    let resource_id = registry
        .get_id(&C::TYPE_UUID)
        .expect("Networked resource incorrectly registered");

    if !resource.update_state(server_time.current_tick()) {
        // Resource didn't change this tick
        return;
    }

    let players = players.players().keys();
    if S::receiver_matters() {
        // Serialize resource for every receiver
        for connection in players {
            let data = match resource.serialize(&mut param, Some(*connection), None) {
                Some(d) => d,
                None => continue,
            };

            sender.send(
                &NetworkedResourceMessage { resource_id, data },
                MessageReceivers::Single(*connection),
            );
        }
    } else {
        let all_players: HashSet<_> = players.copied().collect();
        let data = resource
            .serialize(&mut param, None, None)
            .expect("Serializing without a specific receiver should always return data");
        sender.send(
            &NetworkedResourceMessage { resource_id, data },
            MessageReceivers::Set(all_players),
        );
    }
}

fn receive_networked_resource<C: NetworkedFromServer + Send + Sync + 'static>(
    mut events: EventReader<MessageEvent<NetworkedResourceMessage>>,
    mut resource: Option<ResMut<C>>,
    registry: Res<NetworkedResourceRegistry>,
    mut param: bevy::ecs::system::StaticSystemParam<C::Param>,
    mut commands: Commands,
) {
    for event in events.iter() {
        let message = &event.message;
        // Check if the message is for this resource
        let uuid = registry
            .get_uuid(message.resource_id)
            .expect("Received network message for unknown resource");
        if uuid != &C::TYPE_UUID {
            continue;
        }

        match resource.as_deref_mut() {
            Some(res) => res.deserialize(&mut param, &message.data),
            None => {
                // Apply data to default resource value if possible
                if let Some(mut default) = C::default_if_missing() {
                    default.deserialize(&mut param, &message.data);
                    commands.insert_resource(default);
                } else {
                    warn!(
                        resource = std::any::type_name::<C>(),
                        "Received message for non-existent resource"
                    );
                }
            }
        };
    }
}

pub trait AppExt {
    fn add_networked_resource<S, C>(&mut self) -> &mut App
    where
        S: NetworkedToClient + Send + Sync + 'static,
        C: NetworkedFromServer + Send + Sync + 'static;
}

impl AppExt for App {
    /// Registers a networked resource.
    /// Changes are synced from the server resource (`S`) to the client resource (`C`).
    fn add_networked_resource<S, C>(&mut self) -> &mut App
    where
        S: NetworkedToClient + Send + Sync + 'static,
        C: NetworkedFromServer + Send + Sync + 'static,
    {
        variable::assert_compatible::<S, C>();
        self.init_resource::<NetworkedResourceRegistry>();
        let mut registry = self.world.resource_mut::<NetworkedResourceRegistry>();
        if !registry.register::<C>() {
            panic!("Client resource was already registered");
        }
        if is_server(self) {
            self.add_system(
                send_networked_resource_to_new::<S, C>.before(MessagingSystem::SendOutbound),
            )
            .add_system(
                send_changed_networked_resource::<S, C>.before(MessagingSystem::SendOutbound),
            );
        } else {
            self.add_system(
                receive_networked_resource::<C>.after(NetworkSystem::ReadNetworkMessages),
            );
        }
        self
    }
}

pub(crate) struct ResourcePlugin;

impl Plugin for ResourcePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkedResourceRegistry>()
            .add_network_message::<NetworkedResourceMessage>();
    }
}
