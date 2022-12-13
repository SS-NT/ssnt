use std::marker::{Send, Sync};

use bevy::ecs::reflect::ReflectComponent;
use bevy::{
    prelude::{Component, Handle, Mesh, Quat, Vec3},
    reflect::Reflect,
};

use crate::Direction;

/// Defines data that depends on what sides a tile is surrounded.
#[derive(Clone, Default, Reflect)]
pub struct AdjacencyVariants<T: Reflect + Sync + Send + 'static> {
    pub default: T,
    // No neighbours
    pub o: T,
    // Connected north
    pub u: T,
    // Connected north & south
    pub i: T,
    // Connected north & east
    pub l: T,
    // Connected north & east & west
    pub t: T,
    // Connected in all 4 directions
    pub x: T,
}

impl<T: std::clone::Clone + Reflect + Sync + Send + 'static> AdjacencyVariants<T> {
    pub fn get(&self, adjacency: AdjacencyInformation) -> (T, Quat) {
        if adjacency.is_o() {
            (self.o.clone(), Quat::IDENTITY)
        } else if let Some(dir) = adjacency.is_u() {
            (self.u.clone(), AdjacencyInformation::rotation_from_dir(dir))
        } else if let Some(dir) = adjacency.is_i() {
            (self.i.clone(), AdjacencyInformation::rotation_from_dir(dir))
        } else if let Some(dir) = adjacency.is_l() {
            (self.l.clone(), AdjacencyInformation::rotation_from_dir(dir))
        } else if let Some(dir) = adjacency.is_t() {
            (self.t.clone(), AdjacencyInformation::rotation_from_dir(dir))
        } else if adjacency.is_x() {
            (self.x.clone(), Quat::IDENTITY)
        } else {
            (self.default.clone(), Quat::IDENTITY)
        }
    }
}

/// Stores in what directions an object is surrounded.
#[derive(Default)]
pub struct AdjacencyInformation {
    /// Represents a bitfield of the neighbours presence.
    /// Starts at the tile right above and goes clockwise.
    directions: u8,
}

macro_rules! match_pattern {
    ($val:expr, $pat:literal) => {{
        const PAT: u8 = $pat;
        const PATR: u8 = AdjacencyInformation::rotate_right(PAT);
        const PATRR: u8 = AdjacencyInformation::rotate_right(PATR);
        const PATRRR: u8 = AdjacencyInformation::rotate_right(PATRR);
        match ($val) & 0b10101010 {
            PAT => Some(Direction::North),
            PATR => Some(Direction::East),
            #[allow(unreachable_patterns)]
            PATRR => Some(Direction::South),
            #[allow(unreachable_patterns)]
            PATRRR => Some(Direction::West),
            _ => None,
        }
    }};
}

impl AdjacencyInformation {
    pub fn add(&mut self, direction: Direction) {
        let add = match direction {
            Direction::North => 0b10000000,
            Direction::East => 0b00100000,
            Direction::South => 0b00001000,
            Direction::West => 0b00000010,
        };
        self.directions |= add;
    }

    pub fn is_o(&self) -> bool {
        // For now we ignore diagonals
        self.directions & 0b10101010 == 0
    }

    pub fn is_u(&self) -> Option<Direction> {
        match_pattern!(self.directions, 0b10000000)
    }

    pub fn is_l(&self) -> Option<Direction> {
        match_pattern!(self.directions, 0b10100000)
    }

    pub fn is_t(&self) -> Option<Direction> {
        match_pattern!(self.directions, 0b10100010)
    }

    pub fn is_i(&self) -> Option<Direction> {
        match_pattern!(self.directions, 0b10001000)
    }

    pub fn is_x(&self) -> bool {
        // For now we ignore diagonals
        self.directions & 0b10101010 == 0b10101010
    }

    pub fn rotation_from_dir(direction: Direction) -> Quat {
        let corners = match direction {
            Direction::North => 2,
            Direction::East => 1,
            Direction::South => 0,
            Direction::West => 3,
        };
        Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2 * (corners as f32))
    }

    /// Rotates the directional map by 90 degrees clockwise
    const fn rotate_right(directions: u8) -> u8 {
        // Move the lowest two bits all the way to the left
        ((directions & 0b11) << 6)
            // Combine it with the rest shifted to the right
            | (directions >> 2)
    }
}

/// Defines how a tile object fits together with others.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub(crate) struct TilemapAdjacency {
    // TODO: Allow multiple categories to mesh together
    pub category: String,
    pub meshes: AdjacencyVariants<Handle<Mesh>>,
}
