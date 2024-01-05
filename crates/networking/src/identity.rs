use bevy::{
    ecs::system::{Command, EntityCommands},
    prelude::*,
    reflect::Reflect,
    utils::HashMap,
};
use serde::{Deserialize, Serialize};

use crate::{visibility::InGrid, NetworkManager};

/// A numeric id which matches on the server and clients
#[derive(Component, Debug, Copy, Clone, Hash, PartialEq, Eq, Serialize, Deserialize, Reflect)]
#[reflect(Component)]
pub struct NetworkIdentity(u32);

impl NetworkIdentity {
    pub(crate) fn next(&self) -> Self {
        Self(self.0 + 1)
    }
}

// Mock implementation for component reflection
impl FromWorld for NetworkIdentity {
    fn from_world(_: &mut bevy::prelude::World) -> Self {
        Self(u32::MAX)
    }
}

/// A lookup to match network identities with ECS entity ids.
///
/// Entity ids cannot be used over the network as they are an implementation detail and may conflict.
/// To solve this, we create our own counter and map it to the actual entity id.
#[derive(Default, Resource)]
pub struct NetworkIdentities {
    last_id: u32,
    identities: HashMap<NetworkIdentity, Entity>,
    entities: HashMap<Entity, NetworkIdentity>,
}

impl NetworkIdentities {
    pub fn set_identity(&mut self, entity: Entity, identity: NetworkIdentity) {
        self.identities.insert(identity, entity);
        self.entities.insert(entity, identity);
    }

    pub(crate) fn remove_entity(&mut self, entity: Entity) {
        if let Some(identity) = self.entities.remove(&entity) {
            self.identities.remove(&identity);
        }
    }

    pub fn get_entity(&self, identity: NetworkIdentity) -> Option<Entity> {
        self.identities.get(&identity).copied()
    }

    pub fn get_identity(&self, entity: Entity) -> Option<NetworkIdentity> {
        self.entities.get(&entity).copied()
    }
}

pub struct NetworkCommand {
    pub entity: Entity,
}

impl Command for NetworkCommand {
    fn apply(self, world: &mut World) {
        let manager = world
            .get_resource::<NetworkManager>()
            .expect("Network manager must exist for networked entities");
        if !manager.is_server() {
            error!(
                "Tried to create networked entity {:?} without being the server",
                self.entity
            );
            return;
        }
        let mut identities = world.get_resource_mut::<NetworkIdentities>().unwrap();
        let id = identities.last_id + 1;

        identities.last_id = id;
        identities.set_identity(self.entity, NetworkIdentity(id));

        let mut entity = world.entity_mut(self.entity);
        entity.insert(NetworkIdentity(id));

        if !entity.contains::<InGrid>() && entity.contains::<Transform>() {
            entity.insert(InGrid::default());
        }
    }
}

pub trait EntityCommandsExt {
    fn networked(&mut self) -> &mut Self;
}

impl EntityCommandsExt for EntityCommands<'_, '_, '_> {
    /// Adds a network identity to this entity
    fn networked(&mut self) -> &mut Self {
        let entity = self.id();
        self.commands().add(NetworkCommand { entity });
        self
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, SystemSet)]
pub(crate) enum IdentitySystem {
    ClearRemoved,
}

pub(crate) struct IdentityPlugin;

impl Plugin for IdentityPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<NetworkIdentity>()
            .init_resource::<NetworkIdentities>()
            .add_systems(
                PostUpdate,
                unregister_deleted_entities.in_set(IdentitySystem::ClearRemoved),
            );
    }
}

fn unregister_deleted_entities(
    mut removed: RemovedComponents<NetworkIdentity>,
    mut identities: ResMut<NetworkIdentities>,
) {
    for entity in removed.iter() {
        identities.remove_entity(entity);
    }
}
