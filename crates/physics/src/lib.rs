use bevy::ecs::reflect::ReflectComponent;
use bevy::ecs::system::{Command, EntityCommands, SystemState};
use bevy::prelude::*;
use bevy::reflect::{ReflectDeserialize, ReflectSerialize};
use bevy::transform::TransformSystems;
use bevy::{
    prelude::{App, Plugin},
    reflect::Reflect,
};
use bevy_rapier3d::plugin::PhysicsSet;
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
            .register_type::<Frozen>()
            .register_type::<bevy_rapier3d::dynamics::ReadMassProperties>()
            .add_systems(Update, (add_colliders, add_rigidbodies))
            .add_systems(
                PostUpdate,
                guard_disabled_body_colliders.before(PhysicsSet::SyncBackend),
            )
            .add_systems(
                PostUpdate,
                (
                    apply_visual_error
                        .after(PhysicsSet::Writeback)
                        .before(TransformSystems::Propagate),
                    revert_visual_error.after(TransformSystems::Propagate),
                )
                    .run_if(any_with_component::<VisualError>),
            );
    }
}

/// Render-only offset that smooths discontinuous corrections without disturbing physics
#[derive(Component, Debug, Clone, Copy)]
pub struct VisualError {
    pub translation: Vec3,
    pub rotation: Quat,
}

impl Default for VisualError {
    fn default() -> Self {
        Self {
            translation: Vec3::ZERO,
            rotation: Quat::IDENTITY,
        }
    }
}

const VISUAL_ERROR_DECAY_PER_SECOND: f32 = 0.02;
const VISUAL_ERROR_POSITION_EPSILON: f32 = 0.001;
const VISUAL_ERROR_ROTATION_EPSILON: f32 = 0.005;

impl VisualError {
    pub fn add(&mut self, translation: Vec3, rotation: Quat) {
        self.translation += translation;
        self.rotation = (rotation * self.rotation).normalize();
    }

    fn decay(&mut self, dt: f32) {
        let keep = VISUAL_ERROR_DECAY_PER_SECOND.powf(dt);
        self.translation *= keep;
        self.rotation = Quat::IDENTITY.slerp(self.rotation, keep);
    }

    fn is_negligible(&self) -> bool {
        self.translation.length_squared()
            < VISUAL_ERROR_POSITION_EPSILON * VISUAL_ERROR_POSITION_EPSILON
            && self.rotation.angle_between(Quat::IDENTITY) < VISUAL_ERROR_ROTATION_EPSILON
    }
}

fn apply_visual_error(
    mut query: Query<(Entity, &mut Transform, &mut VisualError)>,
    time: Res<Time>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (entity, mut transform, mut error) in &mut query {
        error.decay(dt);
        if error.is_negligible() {
            commands.entity(entity).remove::<VisualError>();
        }
        transform.translation += error.translation;
        transform.rotation = error.rotation * transform.rotation;
    }
}

fn revert_visual_error(mut query: Query<(&mut Transform, &VisualError)>) {
    for (mut transform, error) in &mut query {
        transform.rotation = error.rotation.inverse() * transform.rotation;
        transform.translation -= error.translation;
    }
}

/// A collider that participates in solver contacts must never sit on a disabled rigid
/// body: rapier pulls disabled bodies out of the island set, so a starting contact trips
/// `rb.is_fixed() || active_island_id != MAX` in `step_simulation`.
fn guard_disabled_body_colliders(
    colliders: Query<
        (Entity, Option<&CollisionGroups>),
        (With<RapierCollider>, Without<ColliderDisabled>),
    >,
    bodies: Query<(), With<RapierRigidBody>>,
    disabled: Query<(), With<RigidBodyDisabled>>,
    parents: Query<&ChildOf>,
    names: Query<&Name>,
    mut commands: Commands,
) {
    let attached: CollisionGroups = ColliderGroup::AttachedLimbs.into();
    for (collider, groups) in &colliders {
        if groups
            .is_some_and(|g| g.memberships == attached.memberships && g.filters == attached.filters)
        {
            continue;
        }
        // The owning body is this entity or its nearest ancestor with a rigid body.
        let mut owner = collider;
        let body = loop {
            if bodies.contains(owner) {
                break Some(owner);
            }
            match parents.get(owner) {
                Ok(child_of) => owner = child_of.parent(),
                Err(_) => break None,
            }
        };
        let Some(body) = body.filter(|&b| disabled.contains(b)) else {
            continue;
        };
        warn!(
            ?collider,
            ?body,
            name = ?names.get(body).ok(),
            ?groups,
            "enabled solver collider on a disabled rigid body; disabling it to avoid the rapier island assertion"
        );
        commands.entity(collider).insert(ColliderDisabled);
    }
}

// TODO: Remove once rapier supports colliders in scenes natively
/// A collider component which can be loaded from scenes.
/// It will be replaced by an actual physics collider once loaded.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Collider {
    kind: ColliderType,
    group: ColliderGroup,
}

#[derive(Reflect, Serialize, Deserialize, Clone, Debug)]
#[reflect(Serialize, Deserialize, Default)]
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

#[derive(Reflect, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Serialize, Deserialize, Default)]
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

impl TryFrom<CollisionGroups> for ColliderGroup {
    type Error = ();

    fn try_from(value: CollisionGroups) -> Result<Self, Self::Error> {
        match (value.memberships, value.filters) {
            (DEFAULT_GROUP, Group::ALL) => Ok(ColliderGroup::Default),
            (Group::GROUP_2, Group::ALL) => Ok(ColliderGroup::CharacterColliders),
            (LIMB_GROUP, RAYCASTING_GROUP) => Ok(ColliderGroup::AttachedLimbs),
            _ => {
                bevy::log::info!("Error converting collision groups {:?}", value);
                Err(())
            }
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
            .insert(collider);
        let group = loaded_collider.group;
        commands.queue(move |world: &mut World| {
            // Create the collider as disabled if the owning body is disabled to avoid physics engine crashes
            let mut owner = entity;
            let disabled = loop {
                let e = world.entity(owner);
                if e.contains::<RigidBodyDisabled>() {
                    break true;
                }
                if e.contains::<RapierRigidBody>() {
                    break false;
                }
                match e.get::<ChildOf>() {
                    Some(child_of) => owner = child_of.parent(),
                    None => break false,
                }
            };
            let mut entity = world.entity_mut(entity);
            if !entity.contains::<CollisionGroups>() {
                entity.insert(CollisionGroups::from(group));
            }
            if disabled {
                entity.insert(ColliderDisabled);
            }
        });
    }
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct RigidBody {
    kind: RigidBodyType,
}

#[derive(Reflect, Serialize, Deserialize, Clone, Debug, Default)]
#[reflect(Serialize, Deserialize, Default)]
enum RigidBodyType {
    #[default]
    Dynamic,
}

fn add_rigidbodies(query: Query<(Entity, &RigidBody), Added<RigidBody>>, mut commands: Commands) {
    for (entity, loaded_rigidbody) in query.iter() {
        let kind = loaded_rigidbody.kind.clone();
        commands.queue(move |world: &mut World| {
            let mut entity = world.entity_mut(entity);
            entity.remove::<RigidBody>();
            let body = if entity.contains::<Frozen>() {
                RapierRigidBody::KinematicPositionBased
            } else {
                match kind {
                    RigidBodyType::Dynamic => RapierRigidBody::Dynamic,
                }
            };
            entity.insert(body);
        });
    }
}

pub trait PhysicsEntityCommands {
    fn set_physics(&mut self, enabled: bool) -> &mut Self;
    fn enable_physics(&mut self) -> &mut Self;
    fn disable_physics(&mut self) -> &mut Self;
    fn freeze(&mut self, new_group: Option<ColliderGroup>) -> &mut Self;
    fn unfreeze(&mut self, new_group: Option<ColliderGroup>) -> &mut Self;
}

impl<'a> PhysicsEntityCommands for EntityCommands<'a> {
    fn enable_physics(&mut self) -> &mut Self {
        self.set_physics(true)
    }

    fn disable_physics(&mut self) -> &mut Self {
        self.set_physics(false)
    }

    fn set_physics(&mut self, enabled: bool) -> &mut Self {
        let entity = self.id();
        self.commands().queue(SetPhysicsCommand {
            entity,
            enabled,
            disable_colliders: true,
            new_group: None,
        });
        self
    }

    fn freeze(&mut self, new_group: Option<ColliderGroup>) -> &mut Self {
        let entity = self.id();
        self.commands().queue(FreezeCommand {
            entity,
            frozen: true,
            new_group,
        });
        self
    }

    fn unfreeze(&mut self, new_group: Option<ColliderGroup>) -> &mut Self {
        let entity = self.id();
        self.commands().queue(FreezeCommand {
            entity,
            frozen: false,
            new_group,
        });
        self
    }
}

/// An attached, immobile body (e.g. a limb) whose collider stays live so raycasts hit it.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct Frozen;

/// Freezes/unfreezes a body
pub struct FreezeCommand {
    pub entity: Entity,
    pub frozen: bool,
    pub new_group: Option<ColliderGroup>,
}

impl Command for FreezeCommand {
    type Out = ();

    fn apply(self, world: &mut World) {
        let mut root = world.entity_mut(self.entity);
        root.remove::<RigidBodyDisabled>();
        if self.frozen {
            root.insert((RapierRigidBody::KinematicPositionBased, Frozen));
        } else {
            root.remove::<Frozen>();
            root.insert(RapierRigidBody::Dynamic);
        }

        let Some(group) = self.new_group else {
            return;
        };
        root.insert(CollisionGroups::from(group));

        for entity_id in body_colliders(world, self.entity) {
            world
                .entity_mut(entity_id)
                .insert(CollisionGroups::from(group));
        }
    }
}

/// Colliders belonging to `root`'s own rigid body: `root` plus its descendants, without
/// crossing into nested rigid bodies.
fn body_colliders(world: &mut World, root: Entity) -> Vec<Entity> {
    let mut state: SystemState<(
        Query<&Children>,
        Query<(), With<RapierCollider>>,
        Query<(), Or<(With<RapierRigidBody>, With<RigidBody>)>>,
    )> = SystemState::new(world);
    let (children, has_collider, is_body) = state.get(world).unwrap();

    let mut result = Vec::new();
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if has_collider.contains(entity) {
            result.push(entity);
        }
        let Ok(kids) = children.get(entity) else {
            continue;
        };
        for child in kids.iter() {
            // A nested body owns the colliders below it; don't descend into it.
            if is_body.contains(child) {
                continue;
            }
            stack.push(child);
        }
    }
    result
}

#[derive(Component)]
#[component(storage = "SparseSet")]
struct TemporarilySensor;

pub struct SetPhysicsCommand {
    pub entity: Entity,
    pub enabled: bool,
    pub disable_colliders: bool,
    pub new_group: Option<ColliderGroup>,
}

impl Command for SetPhysicsCommand {
    type Out = ();

    fn apply(self, world: &mut World) {
        let mut root = world.entity_mut(self.entity);

        if self.enabled {
            root.remove::<RigidBodyDisabled>();
        } else if !root.contains::<RigidBodyDisabled>() {
            root.insert(RigidBodyDisabled);
        }

        if !self.disable_colliders && self.new_group.is_none() {
            return;
        }

        if let Some(group) = self.new_group {
            root.insert(CollisionGroups::from(group));
        }

        for entity_id in body_colliders(world, self.entity) {
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
