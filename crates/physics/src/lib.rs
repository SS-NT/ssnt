use bevy::ecs::reflect::ReflectComponent;
use bevy::ecs::system::{Command, EntityCommands, SystemState};
use bevy::prelude::*;
use bevy::reflect::{ReflectDeserialize, ReflectSerialize};
use bevy::{
    prelude::{App, Plugin},
    reflect::Reflect,
};
use bevy_rapier3d::prelude::Collider as RapierCollider;
use bevy_rapier3d::prelude::RigidBody as RapierRigidBody;
use bevy_rapier3d::prelude::{ColliderDisabled, Real, RigidBodyDisabled};
use serde::{Deserialize, Serialize};

pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Collider>()
            .register_type::<ColliderType>()
            .register_type::<RigidBody>()
            .register_type::<RigidBodyType>()
            .add_system(add_colliders)
            .add_system(add_rigidbodies);
    }
}

// TODO: Remove once rapier supports colliders in scenes natively
/// A collider component which can be loaded from scenes.
/// It will be replaced by an actual physics collider once loaded.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Collider {
    kind: ColliderType,
}

#[derive(Reflect, Serialize, Deserialize, Clone, Debug)]
#[reflect_value(Serialize, Deserialize)]
enum ColliderType {
    Cuboid { hx: Real, hy: Real, hz: Real },
}

impl Default for ColliderType {
    fn default() -> Self {
        Self::Cuboid {
            hx: 0.5,
            hy: 0.5,
            hz: 0.5,
        }
    }
}

fn add_colliders(query: Query<(Entity, &Collider), Added<Collider>>, mut commands: Commands) {
    for (entity, loaded_collider) in query.iter() {
        let collider = match loaded_collider.kind {
            ColliderType::Cuboid { hx, hy, hz } => RapierCollider::cuboid(hx, hy, hz),
        };
        commands
            .entity(entity)
            .remove::<Collider>()
            .insert(collider);
    }
}

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct RigidBody {
    kind: RigidBodyType,
}

#[derive(Reflect, Serialize, Deserialize, Clone, Debug, Default)]
#[reflect_value(Serialize, Deserialize)]
enum RigidBodyType {
    #[default]
    Dynamic,
}

fn add_rigidbodies(query: Query<(Entity, &RigidBody), Added<RigidBody>>, mut commands: Commands) {
    for (entity, loaded_rigidbody) in query.iter() {
        let body = match loaded_rigidbody.kind {
            RigidBodyType::Dynamic => RapierRigidBody::Dynamic,
        };
        commands.entity(entity).remove::<RigidBody>().insert(body);
    }
}

pub trait PhysicsEntityCommands {
    fn set_physics(&mut self, enabled: bool) -> &mut Self;
    fn enable_physics(&mut self) -> &mut Self;
    fn disable_physics(&mut self) -> &mut Self;
}

impl<'w, 's, 'a> PhysicsEntityCommands for EntityCommands<'w, 's, 'a> {
    fn enable_physics(&mut self) -> &mut Self {
        self.set_physics(true)
    }

    fn disable_physics(&mut self) -> &mut Self {
        self.set_physics(false)
    }

    fn set_physics(&mut self, enabled: bool) -> &mut Self {
        let entity = self.id();
        self.commands().add(SetPhysicsCommand { entity, enabled });
        self
    }
}

#[derive(Component)]
#[component(storage = "SparseSet")]
struct TemporarilySensor;

struct SetPhysicsCommand {
    entity: Entity,
    enabled: bool,
}

impl Command for SetPhysicsCommand {
    fn write(self, world: &mut World) {
        let mut root = world.entity_mut(self.entity);

        // Freeze or unfreeze rigidbodies
        if self.enabled {
            root.remove::<RigidBodyDisabled>();
        } else if !root.contains::<RigidBodyDisabled>() {
            root.insert(RigidBodyDisabled);
        }

        // Disable colliders
        let mut to_change = Vec::new();
        let mut children_system_state: SystemState<(Query<&Children>,)> = SystemState::new(world);
        let (child_query,) = children_system_state.get(world);
        for child_entity in child_query
            .iter_descendants(self.entity)
            .chain(std::iter::once(self.entity))
        {
            let child = world.entity(child_entity);
            let is_enabled = !child.contains::<ColliderDisabled>();
            if self.enabled != is_enabled {
                to_change.push(child_entity);
            }
        }

        for entity_id in to_change {
            let mut entity = world.entity_mut(entity_id);
            if self.enabled {
                entity.remove::<ColliderDisabled>();
            } else {
                entity.insert(ColliderDisabled);
            }
        }
    }
}
