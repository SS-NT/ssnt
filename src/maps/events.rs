use bevy::prelude::Entity;

pub struct ChunkObserverAddedEvent {
    pub tilemap_entity: Entity,
    pub observer: Entity,
    pub chunk_index: usize,
}

pub struct ChunkObserverRemovedEvent {
    pub tilemap_entity: Entity,
    pub observer: Entity,
    pub chunk_index: usize,
}
