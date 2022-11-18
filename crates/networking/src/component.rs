use std::clone::Clone;

use bevy::{prelude::*, utils::HashSet};
use serde::{Deserialize, Serialize};

use crate::{
    identity::{NetworkIdentities, NetworkIdentity},
    messaging::{
        AppExt as MessagingAppExt, MessageEvent, MessageReceivers, MessageSender, MessagingSystem,
    },
    spawning::SpawningSystems,
    variable::*,
    visibility::NetworkVisibilities,
    NetworkManager, NetworkSystem,
};

/// A message that contains data for a component.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct NetworkedComponentMessage {
    identity: NetworkIdentity,
    component_id: ComponentNetworkId,
    data: Bytes,
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

        let target = event.message.identity;
        let entity = match identities.get_entity(target) {
            Some(e) => e,
            None => {
                warn!(
                    identity = ?target,
                    "Received component message for non-existent identity"
                );
                continue;
            }
        };

        apply_component_update(
            entity,
            &event.message,
            &mut components,
            &mut param,
            &mut commands,
        );
    }
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
            self.add_system(
                send_networked_component_to_new::<S, C>
                    .before(MessagingSystem::SendOutbound)
                    .after(NetworkSystem::Visibility)
                    .after(SpawningSystems::Spawn),
            );
        } else {
            self.add_system(
                receive_networked_component::<C>
                    .before(NetworkSystem::ReadNetworkMessages)
                    .label(ComponentSystem::Apply),
            );
        }
        self
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, SystemLabel)]
pub enum ComponentSystem {
    Apply,
}

pub(crate) struct ComponentPlugin;

impl Plugin for ComponentPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkedComponentRegistry>()
            .add_network_message::<NetworkedComponentMessage>();
    }
}
