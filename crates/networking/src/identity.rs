use bevy::{prelude::{Component, Entity, error, Plugin, App}, utils::HashMap, ecs::system::{Command, EntityCommands}};
use serde::{Deserialize, Serialize};

use crate::NetworkManager;


/// A numeric id which matches on the server and clients
#[derive(Component, Debug, Copy, Clone, Hash, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkIdentity(u32);

/// A lookup to match network identities with ECS entity ids.
/// 
/// Entity ids cannot be used over the network as they are an implementation detail and may conflict.
/// To solve this, we create our own counter and map it to the actual entity id.
#[derive(Default)]
pub struct NetworkIdentities {
    last_id: u32,
    pub identities: HashMap<NetworkIdentity, Entity>,
}

impl NetworkIdentities {
    pub fn get_entity(&self, identity: NetworkIdentity) -> Option<Entity> {
        self.identities.get(&identity).copied()
    }
}

struct NetworkCommand {
    entity: Entity,
}

impl Command for NetworkCommand {
    fn write(self, world: &mut bevy::prelude::World) {
        let manager = world.get_resource::<NetworkManager>().expect("Network manager must exist for networked entities");
        if !manager.is_server() {
            error!("Tried to create networked entity {:?} without being the server", self.entity);
        }
        let mut identities = world.get_resource_mut::<NetworkIdentities>().unwrap();
        let id = identities.last_id + 1;

        identities.last_id = id;
        identities.identities.insert(NetworkIdentity(id), self.entity);

        world.entity_mut(self.entity).insert(NetworkIdentity(id));
    }
}

pub trait EntityCommandsExt {
    fn networked(&mut self);
}

impl EntityCommandsExt for EntityCommands<'_, '_, '_> {
    /// Adds a network identity to this entity
    fn networked(&mut self) {
        let entity = self.id();
        self.commands().add(NetworkCommand { entity });
    }
}

pub(crate) struct IdentityPlugin;

impl Plugin for IdentityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkIdentities>();
    }
}
