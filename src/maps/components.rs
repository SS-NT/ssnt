use bevy::{prelude::*, utils::HashMap};

use super::{spawning::SpawnedChunk, MapData};

pub struct TileMap {
    pub data: MapData,
    pub spawned_chunks: HashMap<usize, SpawnedChunk>,
}

impl TileMap {
    pub fn new(data: MapData) -> Self {
        Self {
            data,
            spawned_chunks: Default::default(),
        }
    }
}

pub struct TileMapObserver {
    pub view_range: f32,
    chunks_in_range: HashMap<Entity, Vec<usize>>,
}

impl TileMapObserver {
    pub fn new(view_range: f32) -> Self {
        Self {
            view_range,
            chunks_in_range: Default::default(),
        }
    }

    pub fn observing_chunk(&self, tilemap: Entity, index: usize) -> bool {
        self.chunks_in_range
            .get(&tilemap)
            .and_then(|x| x.iter().any(|&i| i == index).then(|| ()))
            .is_some()
    }

    pub fn observe_chunk(&mut self, tilemap: Entity, index: usize) {
        self.chunks_in_range
            .entry(tilemap)
            .or_insert_with(Default::default)
            .push(index);
    }

    pub fn remove_chunk(&mut self, tilemap: Entity, index: usize) {
        if let Some(list) = self.chunks_in_range.get_mut(&tilemap) {
            for (i, &element) in list.iter().enumerate() {
                if element == index {
                    list.remove(i);
                    return;
                }
            }
        }
    }

    pub fn remove_tilemap(&mut self, tilemap: Entity) -> Option<Vec<usize>> {
        self.chunks_in_range.remove(&tilemap)
    }
}

#[derive(Clone, Copy)]
pub struct SpawnedTileObject {
    pub tilemap: Entity,
    pub position: UVec2,
}

#[derive(Bundle)]
pub struct SpawnedTileObjectBundle {
    pub tile_object: SpawnedTileObject,
    #[bundle]
    pub pbr: PbrBundle,
}