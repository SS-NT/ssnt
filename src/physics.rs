use bevy::ecs::reflect::ReflectComponent;
use bevy::prelude::{Added, Commands, Component, Entity, Query};
use bevy::reflect::{ReflectDeserialize, ReflectSerialize};
use bevy::{
    prelude::{App, Plugin},
    reflect::Reflect,
};
use bevy_rapier3d::prelude::Collider as RapierCollider;
use bevy_rapier3d::prelude::Real;
use serde::{Deserialize, Serialize};

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

pub struct PhysicsPlugin;

impl Plugin for PhysicsPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Collider>()
            .register_type::<ColliderType>()
            .add_system(add_colliders);
    }
}
