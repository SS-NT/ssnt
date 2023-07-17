use bevy::ecs::reflect::ReflectComponent;
use bevy::ecs::system::{Command, EntityCommands, SystemState};
use bevy::prelude::*;
use bevy::reflect::{ReflectDeserialize, ReflectSerialize};
use bevy::{
    prelude::{App, Plugin},
    reflect::Reflect,
};
use bevy_rapier3d::prelude::RigidBody as RapierRigidBody;
use bevy_rapier3d::prelude::{Collider as RapierCollider, CollisionGroups, Group};
use bevy_rapier3d::prelude::{ColliderDisabled, Real, RigidBodyDisabled};
use serde::{Deserialize, Serialize};

pub struct PhysicsPlugin;

pub enum PhsyicsSystem {
    AddFromScene,
}

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Collider>()
            .register_type::<ColliderType>()
            .register_type::<ColliderGroup>()
            .register_type::<RigidBody>()
            .register_type::<RigidBodyType>()
            .register_type::<bevy_rapier3d::dynamics::ReadMassProperties>()
            .add_system(add_colliders.at_start())
            .add_system(add_rigidbodies.at_start());
    }
}

// TODO: Remove once rapier supports colliders in scenes natively
/// A collider component which can be loaded from scenes.
/// It will be replaced by an actual physics collider once loaded.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Collider {
    kind: ColliderType,
    group: ColliderGroup,
}

#[derive(Reflect, Serialize, Deserialize, Clone, Debug)]
#[reflect_value(Serialize, Deserialize)]
enum ColliderType {
    Cuboid { hx: Real, hy: Real, hz: Real },
    Capsule { hy: Real, r: Real },
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

#[derive(Reflect, Serialize, Deserialize, Clone, Copy, Debug, Default)]
#[reflect_value(Serialize, Deserialize)]
pub enum ColliderGroup {
    #[default]
    Default,
    CharacterColliders,
    AttachedLimbs,
}

pub const DEFAULT_GROUP: Group = Group::GROUP_1;
pub const LIMB_GROUP: Group = Group::GROUP_3;
pub const RAYCASTING_GROUP: Group = Group::GROUP_32;

impl From<ColliderGroup> for CollisionGroups {
    fn from(value: ColliderGroup) -> Self {
        match value {
            ColliderGroup::Default => CollisionGroups::new(DEFAULT_GROUP, Group::ALL),
            // Colliders on characters (pushing and blocking)
            ColliderGroup::CharacterColliders => CollisionGroups::new(Group::GROUP_2, Group::ALL),
            // Limbs attached to bodies collide with raycasts
            ColliderGroup::AttachedLimbs => CollisionGroups::new(LIMB_GROUP, RAYCASTING_GROUP),
        }
    }
}

fn add_colliders(query: Query<(Entity, &Collider), Added<Collider>>, mut commands: Commands) {
    for (entity, loaded_collider) in query.iter() {
        let collider = match loaded_collider.kind {
            ColliderType::Cuboid { hx, hy, hz } => RapierCollider::cuboid(hx, hy, hz),
            ColliderType::Capsule { hy, r } => RapierCollider::capsule_y(hy, r),
        };
        commands
            .entity(entity)
            .remove::<Collider>()
            .insert((collider, CollisionGroups::from(loaded_collider.group)));
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
    fn freeze(&mut self, new_group: Option<ColliderGroup>) -> &mut Self;
    fn unfreeze(&mut self, new_group: Option<ColliderGroup>) -> &mut Self;
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
        self.commands().add(SetPhysicsCommand {
            entity,
            enabled,
            disable_colliders: true,
            new_group: None,
        });
        self
    }

    fn freeze(&mut self, new_group: Option<ColliderGroup>) -> &mut Self {
        let entity = self.id();
        self.commands().add(SetPhysicsCommand {
            entity,
            enabled: false,
            disable_colliders: false,
            new_group,
        });
        self
    }

    fn unfreeze(&mut self, new_group: Option<ColliderGroup>) -> &mut Self {
        let entity = self.id();
        self.commands().add(SetPhysicsCommand {
            entity,
            enabled: true,
            disable_colliders: false,
            new_group,
        });
        self
    }
}

#[derive(Component)]
#[component(storage = "SparseSet")]
struct TemporarilySensor;

struct SetPhysicsCommand {
    entity: Entity,
    enabled: bool,
    disable_colliders: bool,
    new_group: Option<ColliderGroup>,
}

impl Command for SetPhysicsCommand {
    fn write(self, world: &mut World) {
        let mut root = world.entity_mut(self.entity);

        if self.enabled {
            root.remove::<RigidBodyDisabled>();
        } else if root.contains::<RapierRigidBody>() && !root.contains::<RigidBodyDisabled>() {
            root.insert(RigidBodyDisabled);
        }

        if !self.disable_colliders && self.new_group.is_none() {
            return;
        }

        // Find colliders
        let mut colliders = Vec::new();
        let mut children_system_state: SystemState<(Query<&Children>,)> = SystemState::new(world);
        let (child_query,) = children_system_state.get(world);
        for child_entity in child_query
            .iter_descendants(self.entity)
            .chain(std::iter::once(self.entity))
        {
            let child = world.entity(child_entity);
            if !child.contains::<RapierCollider>() {
                continue;
            }
            colliders.push(child_entity);
        }

        for entity_id in colliders {
            let mut entity = world.entity_mut(entity_id);
            if self.disable_colliders {
                if self.enabled {
                    entity.remove::<ColliderDisabled>();
                } else {
                    entity.insert(ColliderDisabled);
                }
            }
            if let Some(group) = self.new_group {
                entity.insert(CollisionGroups::from(group));
            }
        }
    }
}
