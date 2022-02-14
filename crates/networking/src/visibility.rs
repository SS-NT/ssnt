use bevy::{utils::{HashMap, HashSet}, prelude::{Component, ResMut, Res, Query, Plugin, App, ParallelSystemDescriptorCoercion}};

use crate::{identity::NetworkIdentity, ConnectionId, Players, NetworkManager, NetworkSystem};

/// Allows connections to observer networked objects in range
#[derive(Component)]
pub struct NetworkObserver {
    pub range: u32,
    pub connection: ConnectionId,
}

/// Stores which connections are observing something
#[derive(Default)]
pub struct NetworkVisibility {
    observers: HashSet<ConnectionId>,
    new_observers: HashSet<ConnectionId>,
}

impl NetworkVisibility {
    fn add_observer(&mut self, connection: ConnectionId) {
        if self.observers.insert(connection) {
            self.new_observers.insert(connection);
        }
    }

    fn update(&mut self) {
        self.new_observers.clear();
    }

    pub fn observers(&self) -> &HashSet<ConnectionId> {
        &self.observers
    }

    pub fn new_observers(&self) -> &HashSet<ConnectionId> {
        &self.new_observers
    }

    pub fn has_observer(&self, connection: &ConnectionId) -> bool {
        self.observers.get(connection).is_some()
    }
}

/// Stores a mapping between network identities and their observers
#[derive(Default)]
pub(crate) struct NetworkVisibilities {
    pub(crate) visibility: HashMap<NetworkIdentity, NetworkVisibility>,
}

// TODO: Replace with actual visibility system
fn dummy_visibility(mut visibilities: ResMut<NetworkVisibilities>, players: Res<Players>, identities: Query<&NetworkIdentity>) {
    for (_, visibility) in visibilities.visibility.iter_mut() {
        visibility.update();
    }

    for &identity in identities.iter() {
        let visibility = visibilities.visibility.entry(identity).or_default();
        for (&id, _) in players.players.iter() {
            visibility.add_observer(id);
        }
    }
}

pub(crate) struct VisibilityPlugin;

impl Plugin for VisibilityPlugin {
    fn build(&self, app: &mut App) {
        if app.world.get_resource::<NetworkManager>().unwrap().is_server() {
            app.init_resource::<NetworkVisibilities>()
                .add_system(dummy_visibility.label(NetworkSystem::Visibility));
        }
    }
}
