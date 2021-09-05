use crate::maps::TurfMesh;

use super::{
    components::TurfMarker, Direction, MapData, TileData, TurfData, TurfDefinition, CHUNK_LENGTH,
    CHUNK_SIZE,
};
use bevy::{
    math::{Quat, UVec2, Vec3},
    pbr::PbrBundle,
    prelude::{
        warn, Assets, BuildChildren, Color, Commands, DespawnRecursiveExt, Entity, Handle, Mesh,
        ResMut, StandardMaterial, Transform,
    },
};

const EMPTY_SPAWNED_TILE: Option<SpawnedTile> = None;

#[derive(Clone, Copy)]
pub struct SpawnedChunk {
    pub spawned_tiles: [Option<SpawnedTile>; CHUNK_LENGTH],
}

impl SpawnedChunk {
    pub fn top_border(&self) -> impl Iterator<Item = (usize, &Option<SpawnedTile>)> {
        self.spawned_tiles
            .iter()
            .enumerate()
            .take(CHUNK_SIZE as usize)
    }

    pub fn bottom_border(&self) -> impl Iterator<Item = (usize, &Option<SpawnedTile>)> {
        self.spawned_tiles
            .iter()
            .enumerate()
            .rev()
            .take(CHUNK_SIZE as usize)
    }

    pub fn left_border(&self) -> impl Iterator<Item = (usize, &Option<SpawnedTile>)> {
        self.spawned_tiles
            .iter()
            .enumerate()
            .step_by(CHUNK_SIZE as usize)
    }

    pub fn right_border(&self) -> impl Iterator<Item = (usize, &Option<SpawnedTile>)> {
        self.spawned_tiles
            .iter()
            .enumerate()
            .skip((CHUNK_SIZE - 1) as usize)
            .step_by(CHUNK_SIZE as usize)
    }
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

#[derive(Default)]
struct AdjacencyInformation {
    directions: [bool; 4],
}

impl AdjacencyInformation {
    pub fn add(&mut self, direction: Direction) {
        self.directions[direction as usize] = true;
    }

    pub fn is_o(&self) -> bool {
        self.directions == [false, false, false, false]
    }

    pub fn is_u(&self) -> Option<Quat> {
        match self.directions {
            [true, false, false, false] => Some(Self::rotation_from_dir(Direction::North)),
            [false, true, false, false] => Some(Self::rotation_from_dir(Direction::East)),
            [false, false, true, false] => Some(Self::rotation_from_dir(Direction::South)),
            [false, false, false, true] => Some(Self::rotation_from_dir(Direction::West)),
            _ => None,
        }
    }

    pub fn is_l(&self) -> Option<Quat> {
        match self.directions {
            [true, true, false, false] => Some(Self::rotation_from_dir(Direction::North)),
            [false, true, true, false] => Some(Self::rotation_from_dir(Direction::East)),
            [false, false, true, true] => Some(Self::rotation_from_dir(Direction::South)),
            [true, false, false, true] => Some(Self::rotation_from_dir(Direction::West)),
            _ => None,
        }
    }

    pub fn is_t(&self) -> Option<Quat> {
        match self.directions {
            [true, true, false, true] => Some(Self::rotation_from_dir(Direction::North)),
            [true, true, true, false] => Some(Self::rotation_from_dir(Direction::East)),
            [false, true, true, true] => Some(Self::rotation_from_dir(Direction::South)),
            [true, false, true, true] => Some(Self::rotation_from_dir(Direction::West)),
            _ => None,
        }
    }

    pub fn is_i(&self) -> Option<Quat> {
        match self.directions {
            [true, false, true, false] => Some(Self::rotation_from_dir(Direction::North)),
            [false, true, false, true] => Some(Self::rotation_from_dir(Direction::East)),
            _ => None,
        }
    }

    pub fn is_x(&self) -> bool {
        self.directions == [true, true, true, true]
    }

    fn rotation_from_dir(direction: Direction) -> Quat {
        let corners = match direction {
            Direction::North => 2,
            Direction::East => 1,
            Direction::South => 0,
            Direction::West => 3,
        };
        Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2 * (corners as f32))
    }
}

fn get_turf_adjacency_information(
    definition: &TurfDefinition,
    tile_position: UVec2,
    map: &MapData,
) -> AdjacencyInformation {
    let mut info = AdjacencyInformation::default();
    for &dir in [
        Direction::North,
        Direction::South,
        Direction::East,
        Direction::West,
    ]
    .iter()
    {
        let adjacent_position = tile_position.as_i32() + dir.into();
        if adjacent_position.x < 0 || adjacent_position.y < 0 {
            continue;
        }
        let tile = match map.tile(adjacent_position.as_u32()) {
            Some(x) => x,
            None => continue,
        };
        let adjacent_data = match tile.turf {
            Some(x) => x,
            None => continue,
        };
        let adjacent_definition = map
            .turf_definition(adjacent_data.definition_id)
            .expect("Turf definition must exist");
        if adjacent_definition.category != definition.category {
            continue;
        }
        info.add(dir);
    }

    info
}

pub fn get_turf_mesh(
    definition: &TurfDefinition,
    tile_position: UVec2,
    map: &MapData,
) -> Option<(Handle<Mesh>, Quat)> {
    let meshes = match definition.mesh.as_ref()? {
        TurfMesh::Single(h) => return Some((h.clone(), Quat::IDENTITY)),
        TurfMesh::Multiple(m) => m,
    };

    let adjacency = get_turf_adjacency_information(definition, tile_position, map);
    let info = if adjacency.is_o() {
        (meshes.o.clone(), Quat::IDENTITY)
    } else if let Some(quat) = adjacency.is_u() {
        (meshes.u.clone(), quat)
    } else if let Some(quat) = adjacency.is_i() {
        (meshes.i.clone(), quat)
    } else if let Some(quat) = adjacency.is_l() {
        (meshes.l.clone(), quat)
    } else if let Some(quat) = adjacency.is_t() {
        (meshes.t.clone(), quat)
    } else if adjacency.is_x() {
        (meshes.x.clone(), Quat::IDENTITY)
    } else {
        (meshes.default.clone(), Quat::IDENTITY)
    };
    Some(info)
}

pub fn apply_chunk(
    commands: &mut Commands,
    spawned_chunk: Option<SpawnedChunk>,
    chunk_index: usize,
    map_data: &MapData,
    tilemap_entity: Entity,
    materials: &mut ResMut<Assets<StandardMaterial>>,
) -> SpawnedChunk {
    let chunk_position = MapData::position_from_chunk_index(map_data.size, chunk_index);
    let data = map_data.chunk(chunk_index).unwrap();
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

        let tile_position = chunk_position * UVec2::new(CHUNK_SIZE, CHUNK_SIZE)
            + TileData::position_in_chunk(index);
        let tile_position_3d = Vec3::new(tile_position.x as f32, 0.0, tile_position.y as f32);

        if let Some(turf_data) = tile_data.and_then(|t| t.turf) {
            let turf_definition = map_data
                .turf_definition(turf_data.definition_id)
                .expect("Turf definition must be present if referenced by a tile");
            let (mesh_handle, rotation) =
                match get_turf_mesh(turf_definition, tile_position, map_data) {
                    Some(m) => m,
                    None => {
                        warn!(
                            "Mesh handle for turf {} is not available",
                            turf_definition.name
                        );
                        continue;
                    }
                };
            let spawned_turf = &mut spawned_tile
                .get_or_insert_with(Default::default)
                .spawned_turf;

            let turf_transform = Transform {
                rotation,
                translation: tile_position_3d,
                ..Default::default()
            };

            let material = turf_definition.material.clone().unwrap();
            let mesh = mesh_handle;

            if let Some((current_data, entity)) = spawned_turf {
                if turf_data != *current_data {
                    commands.entity(*entity).insert_bundle(PbrBundle {
                        mesh,
                        material,
                        transform: turf_transform,
                        ..Default::default()
                    });
                }
            } else {
                let turf = commands
                    .spawn_bundle(PbrBundle {
                        mesh,
                        material,
                        transform: turf_transform,
                        ..Default::default()
                    })
                    .insert(TurfMarker)
                    .id();
                commands.entity(tilemap_entity).push_children(&[turf]);
                *spawned_turf = Some((turf_data, turf));
            }
        } else if tile_spawned {
            let x = spawned_tile.as_mut().unwrap();
            if x.spawned_turf.is_some() {
                commands
                    .entity(x.spawned_turf.unwrap().1)
                    .despawn_recursive();
                x.spawned_turf = None;
            }
        }
    }

    spawned_chunk
}

pub fn despawn_chunk(commands: &mut Commands, spawned_chunk: SpawnedChunk) {
    for tile in spawned_chunk.spawned_tiles.iter().flatten() {
        if let Some((_, entity)) = tile.spawned_turf {
            commands.entity(entity).despawn_recursive();
        }
    }
}
