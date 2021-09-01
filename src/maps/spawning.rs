use super::{CHUNK_LENGTH, CHUNK_SIZE, Chunk, MapData, TileData, TurfData, components::TileMap};
use bevy::{math::{UVec2, Vec3}, pbr::PbrBundle, prelude::{Assets, BuildChildren, Color, Commands, DespawnRecursiveExt, Entity, Mesh, ResMut, StandardMaterial, Transform, shape}};

const EMPTY_SPAWNED_TILE: Option<SpawnedTile> = None;

#[derive(Clone, Copy)]
pub struct SpawnedChunk {
    pub spawned_tiles: [Option<SpawnedTile>; CHUNK_LENGTH],
}

impl Default for SpawnedChunk {
    fn default() -> Self {
        Self {
            spawned_tiles: [EMPTY_SPAWNED_TILE; CHUNK_LENGTH],
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct SpawnedTile {
    pub spawned_turf: Option<(TurfData, Entity)>,
}

pub fn apply_chunk(
    commands: &mut Commands,
    spawned_chunk: Option<SpawnedChunk>,
    data: &Chunk,
    chunk_position: UVec2,
    map_data: &MapData,
    tilemap_entity: Entity,
    materials: &mut ResMut<Assets<StandardMaterial>>,
    meshes: &mut ResMut<Assets<Mesh>>,
) -> SpawnedChunk {
    let changed_indicies: Vec<usize> = match spawned_chunk {
        Some(_) => data
            .changed_tiles
            .iter()
            .enumerate()
            .filter(|(_, &c)| c)
            .map(|(i, _)| i)
            .collect(),
        None => (0..data.tiles.len()).collect(),
    };
    let mut spawned_chunk = spawned_chunk.unwrap_or_else(Default::default);

    for &index in changed_indicies.iter() {
        let tile_data = data.tiles.get(index).unwrap();
        let spawned_tile = spawned_chunk.spawned_tiles.get_mut(index).unwrap();

        let tile_exists = tile_data.is_some();
        let tile_spawned = spawned_tile.is_some();

        if !tile_exists && !tile_spawned {
            continue;
        }

        let tile_position = chunk_position * UVec2::new(CHUNK_SIZE, CHUNK_SIZE) + TileData::position_in_chunk(index);
        let tile_position = Vec3::new(tile_position.x as f32, 0.0, tile_position.y as f32);

        if let Some(turf_data) = tile_data.and_then(|t| t.turf) {
            let spawned_turf = &mut spawned_tile
                .get_or_insert_with(Default::default)
                .spawned_turf;
            if let Some((current_data, entity)) = spawned_turf {
                if turf_data != *current_data {
                    let wall_material_handle = materials.add(StandardMaterial {
                        base_color: Color::rgb(0.8, 0.8, 0.8),
                        ..Default::default()
                    });
                    let mesh_handle = meshes.add(Mesh::from(shape::Cube { size: 1.0 }));
                    commands.entity(*entity).insert_bundle(PbrBundle {
                        mesh: mesh_handle,
                        material: wall_material_handle,
                        transform: Transform::from_translation(tile_position),
                        ..Default::default()
                    });
                }
            } else {
                let wall_material_handle = materials.add(StandardMaterial {
                    base_color: Color::rgb(0.8, 0.8, 0.8),
                    ..Default::default()
                });
                let mesh_handle = meshes.add(Mesh::from(shape::Cube { size: 1.0 }));
                let turf = commands.spawn_bundle(PbrBundle {
                    mesh: mesh_handle,
                    material: wall_material_handle,
                    transform: Transform::from_translation(tile_position),
                    ..Default::default()
                }).id();
                commands.entity(tilemap_entity).push_children(&[turf]);
                *spawned_turf = Some((turf_data, turf));
            }
        } else if tile_spawned {
            let x = spawned_tile.as_mut().unwrap();
            if x.spawned_turf.is_some() {
                commands.entity(x.spawned_turf.unwrap().1).despawn_recursive();
                x.spawned_turf = None;
            }
        }
    }

    spawned_chunk
}

pub fn despawn_chunk(commands: &mut Commands, spawned_chunk: SpawnedChunk) {
    for tile in spawned_chunk.spawned_tiles.iter() {
        if let Some(tile) = tile {
            if let Some((_, entity)) = tile.spawned_turf {
                commands.entity(entity).despawn_recursive();
            }
        }
    }
}