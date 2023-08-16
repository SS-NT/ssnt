use bevy::prelude::*;

#[allow(dead_code)]
pub enum KineticShape {
    Blunt,
    Sharp,
    Point,
}

#[derive(Component)]
pub struct KineticDamage {
    /// Relative velocity on impact in m/s
    pub velocity: f32,
    /// Object mass in kg
    pub mass: f32,
    pub shape: KineticShape,
}

/// Marker component for entities representing an attack / impact
#[derive(Component)]
pub struct Attack;

#[derive(Component)]
pub struct AffectedEntity(pub Entity);
