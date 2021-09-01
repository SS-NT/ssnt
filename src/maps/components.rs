use bevy::{math::UVec2, tasks::Task, utils::HashMap};

use super::{MapData, spawning::SpawnedChunk};

pub struct TileMap {
    pub data: MapData,
    pub spawned_chunks: HashMap<UVec2, SpawnedChunk>,
    pub spawning_chunks: HashMap<UVec2, Task<SpawnedChunk>>,
}

impl TileMap {
    pub fn new(data: MapData) -> Self {
        Self {
            data,
            spawned_chunks: Default::default(),
            spawning_chunks: Default::default(),
        }
    }
}

pub struct TileMapObserver {
    pub view_range: f32,
}
