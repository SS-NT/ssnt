use std::clone::Clone;

use bevy::{prelude::*, utils::HashSet};
use serde::{Deserialize, Serialize};

use crate::{
    identity::{NetworkIdentities, NetworkIdentity},
    messaging::{AppExt as MessagingAppExt, MessageEvent, MessageReceivers, MessageSender},
    time::ServerNetworkTime,
    variable::*,
    visibility::NetworkVisibilities,
    ConnectionId, NetworkManager, NetworkSet,
};

/// A message that contains data for a component.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct NetworkedComponentMessage {
    identity: NetworkIdentity,
    component_id: ComponentNetworkId,
    data: Bytes,
}

/// A message that tells the client to remove a component.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct RemoveNetworkedComponentMessage {
    identity: NetworkIdentity,
    component_id: ComponentNetworkId,
}

#[derive(Debug, Copy, Clone, Hash, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct ComponentNetworkId(u16);

impl From<ComponentNetworkId> for u16 {
    fn from(id: ComponentNetworkId) -> Self {
        id.0
    }
}

impl From<u16> for ComponentNetworkId {
    fn from(id: u16) -> Self {
        Self(id)
    }
}

type NetworkedComponentRegistry = NetworkRegistry<ComponentNetworkId>;

fn send_networked_component_changed<S: NetworkedToClient + Component, C: NetworkedFromServer>(
    mut components: Query<(&NetworkIdentity, &mut S), Changed<S>>,
    visibilities: Res<NetworkVisibilities>,
    registry: Res<NetworkedComponentRegistry>,
    server_time: Res<ServerNetworkTime>,
    mut sender: MessageSender,
    mut param: bevy::ecs::system::StaticSystemParam<S::Param>,
    mut observer_cache: Local<HashSet<ConnectionId>>,
) {
    for (identity, mut component) in components.iter_mut() {
        let maybe_visibility = visibilities.visibility.get(identity);
        let explicit_observers = component.limit_observers();
        // Skip if no observer data
        if maybe_visibility.is_none() && explicit_observers.is_none() {
            continue;
        }

        // Check if component networked state changes
        if !component.is_added() && !component.update_state(server_time.current_tick()) {
            continue;
        }

        // Get relevant observers
        observer_cache.clear();
        if let Some(explicit) = explicit_observers {
            observer_cache.extend(explicit);
        } else {
            observer_cache.extend(maybe_visibility.unwrap().observers().copied());
        }

        // Skip if not observed
        if observer_cache.is_empty() {
            continue;
        }

        let component_id = registry
            .get_id(&C::TYPE_UUID)
            .expect("Networked component incorrectly registered");
        let priority = component.priority();
        if S::receiver_matters() {
            // Serialize component for every receiver
            for connection in observer_cache.iter() {
                let data = match component.serialize(&mut param, Some(*connection), None) {
                    Some(d) => d,
                    None => continue,
                };

                sender.send_with_priority(
                    &NetworkedComponentMessage {
                        identity: *identity,
                        component_id,
                        data,
                    },
                    MessageReceivers::Single(*connection),
                    priority,
                );
            }
        } else {
            let Some(data) = component.serialize(&mut param, None, None) else {
                continue;
            };
            sender.send_with_priority(
                &NetworkedComponentMessage {
                    identity: *identity,
                    component_id,
                    data,
                },
                MessageReceivers::Set(observer_cache.clone()),
                priority,
            );
        }
    }
}

fn send_networked_component_to_new<S: NetworkedToClient + Component, C: NetworkedFromServer>(
    mut components: Query<(&NetworkIdentity, &S)>,
    visibilities: Res<NetworkVisibilities>,
    registry: Res<NetworkedComponentRegistry>,
    mut sender: MessageSender,
    mut param: bevy::ecs::system::StaticSystemParam<S::Param>,
) {
    for (identity, component) in components.iter_mut() {
        let visibility = match visibilities.visibility.get(identity) {
            Some(v) => v,
            None => continue,
        };

        let component_id = registry
            .get_id(&C::TYPE_UUID)
            .expect("Networked component incorrectly registered");
        if S::receiver_matters() {
            // Serialize component for every receiver
            for connection in visibility.new_observers() {
                let data = match component.serialize(&mut param, Some(*connection), None) {
                    Some(d) => d,
                    None => continue,
                };
                let priority = component.priority();

                sender.send_with_priority(
                    &NetworkedComponentMessage {
                        identity: *identity,
                        component_id,
                        data,
                    },
                    MessageReceivers::Single(*connection),
                    priority,
                );
            }
        } else {
            let new_observers: HashSet<_> = visibility.new_observers().copied().collect();
            if !new_observers.is_empty() {
                let data = component
                    .serialize(&mut param, None, None)
                    .expect("Serializing without a specific receiver should always return data");
                sender.send_with_priority(
                    &NetworkedComponentMessage {
                        identity: *identity,
                        component_id,
                        data,
                    },
                    MessageReceivers::Set(new_observers),
                    component.priority(),
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn receive_networked_component<C: NetworkedFromServer + Component>(
    mut events: EventReader<MessageEvent<NetworkedComponentMessage>>,
    mut buffer: Local<Vec<NetworkedComponentMessage>>,
    mut components: Query<&mut C>,
    registry: Res<NetworkedComponentRegistry>,
    identities: Res<NetworkIdentities>,
    mut param: bevy::ecs::system::StaticSystemParam<C::Param>,
    mut commands: Commands,
) {
    for event in events.iter() {
        // TODO: Move the id->uuid conversion into one system for performance?
        // Check if the message is for this component
        let uuid = registry
            .get_uuid(event.message.component_id)
            .expect("Received component message for unknown component");
        if uuid != &C::TYPE_UUID {
            continue;
        }
        // TODO: We should just consume network messages instead of cloning them
        buffer.push(event.message.clone());
    }

    // TODO: add logging for long-retained messages (indicates BUG)
    buffer.retain(|message| {
        let Some(entity) = identities.get_entity(message.identity) else {
            return true;
        };

        apply_component_update(entity, message, &mut components, &mut param, &mut commands);
        false
    });
}

fn apply_component_update<C: NetworkedFromServer + Component>(
    entity: Entity,
    message: &NetworkedComponentMessage,
    components: &mut Query<&mut C>,
    param: &mut bevy::ecs::system::StaticSystemParam<C::Param>,
    commands: &mut Commands,
) {
    match components.get_mut(entity) {
        Ok(mut c) => c.deserialize(param, &message.data),
        Err(_) => {
            // Apply data to default component value if possible
            if let Some(mut default) = C::default_if_missing() {
                default.deserialize(param, &message.data);
                commands.entity(entity).insert(default);
            } else {
                warn!(
                    ?entity,
                    component = std::any::type_name::<C>(),
                    "Received component message for entity without that component"
                );
            }
        }
    };
    bevy::log::trace!(component=std::any::type_name::<C>(), entity = ?entity, "Applied networked component data");
}

fn send_networked_component_removed<S: NetworkedToClient + Component, C: NetworkedFromServer>(
    mut removed_from: RemovedComponents<S>,
    entities: Query<()>,
    identities: Res<NetworkIdentities>,
    visibilities: Res<NetworkVisibilities>,
    registry: Res<NetworkedComponentRegistry>,
    mut sender: MessageSender,
) {
    for entity in removed_from.iter() {
        // Skip if entire entity was deleted -> networked separately
        if !entities.contains(entity) {
            return;
        }

        let Some(identity) = identities.get_identity(entity) else {
            continue;
        };
        let visibility = match visibilities.visibility.get(&identity) {
            Some(v) => v,
            None => continue,
        };

        let component_id = registry
            .get_id(&C::TYPE_UUID)
            .expect("Networked component incorrectly registered");

        let observers: HashSet<_> = visibility.observers().copied().collect();
        if !observers.is_empty() {
            sender.send_with_priority(
                &RemoveNetworkedComponentMessage {
                    identity,
                    component_id,
                },
                MessageReceivers::Set(observers),
                -10,
            );
        }
    }
}

fn client_handle_component_removal<C: NetworkedFromServer + Component>(
    mut events: EventReader<MessageEvent<RemoveNetworkedComponentMessage>>,
    registry: Res<NetworkedComponentRegistry>,
    identities: Res<NetworkIdentities>,
    mut commands: Commands,
) {
    for event in events.iter() {
        // TODO: Move the id->uuid conversion into one system for performance?
        // Check if the message is for this component
        let uuid = registry
            .get_uuid(event.message.component_id)
            .expect("Received component message for unknown component");
        if uuid != &C::TYPE_UUID {
            continue;
        }

        let target = event.message.identity;
        let Some(entity) = identities.get_entity(target) else {
            continue;
        };

        commands.entity(entity).remove::<C>();
    }
}
pub trait AppExt {
    fn add_networked_component<S, C>(&mut self) -> &mut App
    where
        S: NetworkedToClient + Component,
        C: NetworkedFromServer + Component;
}

impl AppExt for App {
    /// Registers a networked component.
    /// Changes are synced from the server component (`S`) to the client component (`C`).
    fn add_networked_component<S, C>(&mut self) -> &mut App
    where
        S: NetworkedToClient + Component,
        C: NetworkedFromServer + Component,
    {
        assert_compatible::<S, C>();
        self.init_resource::<NetworkedComponentRegistry>();
        let mut registry = self.world.resource_mut::<NetworkedComponentRegistry>();
        if !registry.register::<C>() {
            panic!("Client component was already registered");
        }
        if self.world.resource::<NetworkManager>().is_server() {
            self.add_systems(
                PostUpdate,
                (
                    send_networked_component_to_new::<S, C>,
                    send_networked_component_changed::<S, C>,
                    send_networked_component_removed::<S, C>,
                )
                    .in_set(NetworkSet::ServerWrite),
            );
        } else {
            self.add_systems(
                PreUpdate,
                (
                    receive_networked_component::<C>,
                    client_handle_component_removal::<C>,
                )
                    .in_set(NetworkSet::ClientApply),
            );
        }
        self
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, SystemSet)]
pub enum ComponentSystem {
    Apply,
}

pub(crate) struct ComponentPlugin;

impl Plugin for ComponentPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkedComponentRegistry>()
            .add_network_message::<NetworkedComponentMessage>()
            .add_network_message::<RemoveNetworkedComponentMessage>();
    }
}
