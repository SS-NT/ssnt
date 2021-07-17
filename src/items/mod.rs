use bevy::{math::UVec2, prelude::Entity};

pub mod containers;

pub struct Item {
    pub name: String,
    pub size: UVec2,
    pub container: Option<Entity>,
}

impl Item {
    pub fn new(name: String, size: UVec2) -> Self {
        Self {
            name,
            size,
            container: None,
        }
    }
}
