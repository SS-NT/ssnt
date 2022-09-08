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
    directions: [bool; 4],
}

impl AdjacencyInformation {
    pub fn add(&mut self, direction: Direction) {
        self.directions[direction as usize] = true;
    }

    pub fn is_o(&self) -> bool {
        self.directions == [false, false, false, false]
    }

    pub fn is_u(&self) -> Option<Direction> {
        match self.directions {
            [true, false, false, false] => Some(Direction::North),
            [false, true, false, false] => Some(Direction::East),
            [false, false, true, false] => Some(Direction::South),
            [false, false, false, true] => Some(Direction::West),
            _ => None,
        }
    }

    pub fn is_l(&self) -> Option<Direction> {
        match self.directions {
            [true, true, false, false] => Some(Direction::North),
            [false, true, true, false] => Some(Direction::East),
            [false, false, true, true] => Some(Direction::South),
            [true, false, false, true] => Some(Direction::West),
            _ => None,
        }
    }

    pub fn is_t(&self) -> Option<Direction> {
        match self.directions {
            [true, true, false, true] => Some(Direction::North),
            [true, true, true, false] => Some(Direction::East),
            [false, true, true, true] => Some(Direction::South),
            [true, false, true, true] => Some(Direction::West),
            _ => None,
        }
    }

    pub fn is_i(&self) -> Option<Direction> {
        match self.directions {
            [true, false, true, false] => Some(Direction::North),
            [false, true, false, true] => Some(Direction::East),
            _ => None,
        }
    }

    pub fn is_x(&self) -> bool {
        self.directions == [true, true, true, true]
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
}

/// Defines how a tile object fits together with others.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub(crate) struct TilemapAdjacency {
    // TODO: Allow multiple categories to mesh together
    pub category: String,
    pub meshes: AdjacencyVariants<Handle<Mesh>>,
}
