use crate::maps::TilemapMesh;

use super::{
    components::TurfMarker, tile_neighbours, AdjacencyInformation, FurnitureData,
    FurnitureDefinition, FurnitureKind, MapData, TileData, TurfData, TurfDefinition, CHUNK_LENGTH,
    CHUNK_SIZE, DIRECTIONS,
};
use bevy::{math::{IVec2, Quat, UVec2, Vec3}, pbr::PbrBundle, prelude::{
        warn, BuildChildren, Commands, DespawnRecursiveExt, Entity, Handle, Mesh, Transform,
    }};

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
    pub spawned_furniture: Option<(FurnitureData, Entity)>,
}

fn get_turf_adjacency_information(
    definition: &TurfDefinition,
    tile_position: UVec2,
    map: &MapData,
) -> AdjacencyInformation {
    let mut info = AdjacencyInformation::default();
    for (dir, position) in tile_neighbours(tile_position) {
        if let Some(turf_data) = map.tile(position).and_then(|p| p.turf) {
            let adjacent_definition = map
                .turf_definition(turf_data.definition_id)
                .expect("Turf definition must exist");
            if adjacent_definition.category == definition.category {
                info.add(dir);
                continue;
            }
        }
        if let Some(furniture_data) = map.tile(position).and_then(|p| p.furniture) {
            let adjacent_definition = map
                .furniture_definition(furniture_data.definition_id)
                .expect("Furniture definition must exist");
            if adjacent_definition.kind == FurnitureKind::Door {
                info.add(dir);
                continue;
            }
        }
    }

    info
}

fn get_furniture_adjacency_information(
    definition: &FurnitureDefinition,
    tile_position: UVec2,
    map: &MapData,
) -> AdjacencyInformation {
    let mut info = AdjacencyInformation::default();
    for &dir in DIRECTIONS.iter() {
        let offset: IVec2 = dir.into();
        let adjacent_position = tile_position.as_i32() + offset;
        if adjacent_position.x < 0 || adjacent_position.y < 0 {
            continue;
        }
        let tile = match map.tile(adjacent_position.as_u32()) {
            Some(x) => x,
            None => continue,
        };
        let adjacent_data = match tile.furniture {
            Some(x) => x,
            None => continue,
        };
        let adjacent_definition = map
            .furniture_definition(adjacent_data.definition_id)
            .expect("Furniture definition must exist");
        if adjacent_definition.kind != definition.kind {
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
        TilemapMesh::Single(h) => return Some((h.clone(), Quat::IDENTITY)),
        TilemapMesh::Multiple(m) => m,
    };

    let adjacency = get_turf_adjacency_information(definition, tile_position, map);
    Some(meshes.mesh_from_adjacency(adjacency))
}

pub fn get_furniture_mesh(
    definition: &FurnitureDefinition,
    tile_position: UVec2,
    map: &MapData,
) -> Option<(Handle<Mesh>, Quat)> {
    let meshes = match definition.mesh.as_ref()? {
        TilemapMesh::Single(h) => return Some((h.clone(), Quat::IDENTITY)),
        TilemapMesh::Multiple(m) => m,
    };

    let adjacency = get_furniture_adjacency_information(definition, tile_position, map);
    Some(meshes.mesh_from_adjacency(adjacency))
}

pub fn apply_chunk(
    commands: &mut Commands,
    spawned_chunk: Option<SpawnedChunk>,
    chunk_index: usize,
    map_data: &MapData,
    tilemap_entity: Entity,
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

        apply_turf(
            tile_data.as_ref().and_then(|t| t.turf.as_ref()),
            map_data,
            tile_position,
            tile_position_3d,
            tilemap_entity,
            spawned_tile,
            commands,
        );
        apply_furniture(
            tile_data.as_ref().and_then(|t| t.furniture.as_ref()),
            map_data,
            tile_position,
            tile_position_3d,
            tilemap_entity,
            spawned_tile,
            commands,
        );
    }

    spawned_chunk
}

fn apply_turf(
    turf_data: Option<&TurfData>,
    map_data: &MapData,
    tile_position: UVec2,
    tile_position_3d: Vec3,
    tilemap_entity: Entity,
    spawned_tile: &mut Option<SpawnedTile>,
    commands: &mut Commands,
) {
    if turf_data.is_none() {
        if let Some(spawned_turf) = spawned_tile.as_mut().map(|t| &mut t.spawned_turf) {
            if let Some((_, entity)) = spawned_turf {
                commands.entity(*entity).despawn_recursive();
            }
            *spawned_turf = None;
        }
        return;
    }

    let turf_data = turf_data.unwrap();

    let turf_definition = map_data
        .turf_definition(turf_data.definition_id)
        .expect("Turf definition must be present if referenced by a tile");
    let (mesh_handle, rotation) = match get_turf_mesh(turf_definition, tile_position, map_data) {
        Some(m) => m,
        None => {
            warn!(
                "Mesh handle for turf {} is not available",
                turf_definition.name
            );
            return;
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
        if turf_data != current_data {
            commands
                .entity(*entity)
                .insert(mesh)
                .insert(material)
                .insert(turf_transform);
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
        *spawned_turf = Some((*turf_data, turf));
    }
}

fn apply_furniture(
    furniture_data: Option<&FurnitureData>,
    map_data: &MapData,
    tile_position: UVec2,
    tile_position_3d: Vec3,
    tilemap_entity: Entity,
    spawned_tile: &mut Option<SpawnedTile>,
    commands: &mut Commands,
) {
    if furniture_data.is_none() {
        if let Some(spawned_furniture) = spawned_tile.as_mut().map(|t| &mut t.spawned_furniture) {
            if let Some((_, entity)) = spawned_furniture {
                commands.entity(*entity).despawn_recursive();
            }
            *spawned_furniture = None;
        }
        return;
    }

    let furniture_data = furniture_data.unwrap();

    let furniture_definition = map_data
        .furniture_definition(furniture_data.definition_id)
        .expect("Furniture definition must be present if referenced by a tile");
    let (mesh_handle, mut rotation) =
        match get_furniture_mesh(furniture_definition, tile_position, map_data) {
            Some(m) => m,
            None => {
                warn!(
                    "Mesh handle for furniture {} is not available",
                    furniture_definition.name
                );
                return;
            }
        };
    let spawned_furniture = &mut spawned_tile
        .get_or_insert_with(Default::default)
        .spawned_furniture;

    if let Some(dir) = furniture_data.direction {
        rotation = Quat::from_axis_angle(
            Vec3::Y,
            std::f32::consts::FRAC_PI_2 * ((dir as u32 + 1) as f32),
        );
    }

    let turf_transform = Transform {
        rotation,
        translation: tile_position_3d,
        ..Default::default()
    };

    let material = furniture_definition.material.as_ref().unwrap();
    let mesh = mesh_handle;

    if let Some((current_data, entity)) = spawned_furniture {
        if furniture_data != current_data {
            commands
                .entity(*entity)
                .insert(mesh)
                .insert(material.clone())
                .insert(turf_transform);
        }
    } else {
        let turf = commands
            .spawn_bundle(PbrBundle {
                mesh,
                material: material.clone(),
                transform: turf_transform,
                ..Default::default()
            })
            .insert(TurfMarker)
            .id();
        if furniture_definition.kind == FurnitureKind::Door {
            let connector_left = commands
                .spawn_bundle(PbrBundle {
                    mesh: furniture_definition.connector_mesh.clone().unwrap(),
                    material: material.clone(),
                    transform: Transform {
                        translation: -Vec3::X,
                        rotation: Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .id();
            let connector_right = commands
                .spawn_bundle(PbrBundle {
                    mesh: furniture_definition.connector_mesh.clone().unwrap(),
                    material: material.clone(),
                    transform: Transform {
                        translation: Vec3::X,
                        rotation: Quat::from_axis_angle(Vec3::Y, -std::f32::consts::FRAC_PI_2),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .id();
            commands
                .entity(turf)
                .push_children(&[connector_left, connector_right]);
        }
        commands.entity(tilemap_entity).push_children(&[turf]);
        *spawned_furniture = Some((*furniture_data, turf));
    }
}

pub fn despawn_chunk(commands: &mut Commands, spawned_chunk: SpawnedChunk) {
    for tile in spawned_chunk.spawned_tiles.iter().flatten() {
        if let Some((_, entity)) = tile.spawned_turf {
            commands.entity(entity).despawn_recursive();
        }
        if let Some((_, entity)) = tile.spawned_furniture {
            commands.entity(entity).despawn_recursive();
        }
    }
}
