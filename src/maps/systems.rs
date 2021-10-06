use super::{
    components::*,
    events::*,
    spawning::{apply_chunk, despawn_chunk, SpawnedTile},
    AdjacencyMeshes, Direction, MapData, TileData, TilemapMesh, CHUNK_SIZE,
};
use bevy::{math::{IVec2, UVec2, Vec2, Vec3, Vec3Swizzles}, prelude::{Added, AssetServer, Commands, Entity, EventReader, EventWriter, GlobalTransform, Handle, Mesh, Query, Res, Transform, With}};

pub fn tilemap_mesh_loading_system(
    mut tilemaps: Query<&mut TileMap, Added<TileMap>>,
    asset_server: Res<AssetServer>,
) {
    let mut material = None;
    for mut tilemap in tilemaps.iter_mut() {
        for definition in tilemap
            .data
            .turf_definitions
            .iter_mut()
            .filter(|d| d.mesh.is_none())
        {
            let material = material.get_or_insert_with(|| {
                asset_server.load("models/tilemap/walls windows.glb#Material0")
            });
            definition.material = Some(material.clone());

            definition.mesh = Some(match definition.name.as_str() {
                "wall" => TilemapMesh::Multiple(AdjacencyMeshes {
                    default: asset_server
                        .load("models/tilemap/walls windows.glb#Mesh29/Primitive0"),
                    o: asset_server.load("models/tilemap/walls windows.glb#Mesh29/Primitive0"),
                    u: asset_server.load("models/tilemap/walls windows.glb#Mesh30/Primitive0"),
                    i: asset_server.load("models/tilemap/walls windows.glb#Mesh25/Primitive0"),
                    l: asset_server.load("models/tilemap/walls windows.glb#Mesh26/Primitive0"),
                    t: asset_server.load("models/tilemap/walls windows.glb#Mesh27/Primitive0"),
                    x: asset_server.load("models/tilemap/walls windows.glb#Mesh28/Primitive0"),
                }),
                "reinforced wall" => TilemapMesh::Multiple(AdjacencyMeshes {
                    default: asset_server
                        .load("models/tilemap/walls windows.glb#Mesh24/Primitive0"),
                    o: asset_server.load("models/tilemap/walls windows.glb#Mesh24/Primitive0"),
                    u: asset_server.load("models/tilemap/walls windows.glb#Mesh33/Primitive0"),
                    i: asset_server.load("models/tilemap/walls windows.glb#Mesh23/Primitive0"),
                    l: asset_server.load("models/tilemap/walls windows.glb#Mesh31/Primitive0"),
                    t: asset_server.load("models/tilemap/walls windows.glb#Mesh42/Primitive0"),
                    x: asset_server.load("models/tilemap/walls windows.glb#Mesh32/Primitive0"),
                }),
                "grille" => asset_server
                    .load("models/tilemap/girders.glb#Mesh1/Primitive0")
                    .into(),
                "window" => TilemapMesh::Multiple(AdjacencyMeshes {
                    default: asset_server
                        .load("models/tilemap/walls windows.glb#Mesh22/Primitive0"),
                    o: asset_server.load("models/tilemap/walls windows.glb#Mesh22/Primitive0"),
                    u: asset_server.load("models/tilemap/walls windows.glb#Mesh21/Primitive0"),
                    i: asset_server.load("models/tilemap/walls windows.glb#Mesh18/Primitive0"),
                    l: asset_server.load("models/tilemap/walls windows.glb#Mesh0/Primitive0"),
                    t: asset_server.load("models/tilemap/walls windows.glb#Mesh1/Primitive0"),
                    x: asset_server.load("models/tilemap/walls windows.glb#Mesh2/Primitive0"),
                }),
                "reinforced window" => TilemapMesh::Multiple(AdjacencyMeshes {
                    default: asset_server
                        .load("models/tilemap/walls windows.glb#Mesh39/Primitive0"),
                    o: asset_server.load("models/tilemap/walls windows.glb#Mesh39/Primitive0"),
                    u: asset_server.load("models/tilemap/walls windows.glb#Mesh40/Primitive0"),
                    i: asset_server.load("models/tilemap/walls windows.glb#Mesh41/Primitive0"),
                    l: asset_server.load("models/tilemap/walls windows.glb#Mesh36/Primitive0"),
                    t: asset_server.load("models/tilemap/walls windows.glb#Mesh37/Primitive0"),
                    x: asset_server.load("models/tilemap/walls windows.glb#Mesh38/Primitive0"),
                }),
                "floor" => asset_server.load("models/tilemap/floors.glb#Mesh2/Primitive0").into(),
                "white floor" => asset_server.load("models/tilemap/floors.glb#Mesh6/Primitive0").into(),
                "dark floor" => asset_server.load("models/tilemap/floors.glb#Mesh3/Primitive0").into(),
                "wood floor" => asset_server.load("models/tilemap/floors.glb#Mesh3/Primitive0").into(),
                "plating" => asset_server.load("models/tilemap/floors.glb#Mesh0/Primitive0").into(),
                _ => continue,
            });
        }

        for definition in tilemap
            .data
            .furniture_definitions
            .iter_mut()
            .filter(|d| d.mesh.is_none())
        {
            let material = material.get_or_insert_with(|| {
                asset_server.load("models/tilemap/walls windows.glb#Material0")
            });
            definition.material = Some(material.clone());

            definition.mesh = Some(match definition.name.as_str() {
                "airlock" => asset_server
                    .load("models/tilemap/doors.glb#Mesh0/Primitive0")
                    .into(),
                "airlock maintenance" => asset_server
                    .load("models/tilemap/doors.glb#Mesh51/Primitive0")
                    .into(),
                "airlock command" => asset_server
                    .load("models/tilemap/doors.glb#Mesh19/Primitive0")
                    .into(),
                "airlock supply" => asset_server
                    .load("models/tilemap/doors.glb#Mesh75/Primitive0")
                    .into(),
                "airlock security" => asset_server
                    .load("models/tilemap/doors.glb#Mesh69/Primitive0")
                    .into(),
                "airlock engineering" => asset_server
                    .load("models/tilemap/doors.glb#Mesh34/Primitive0")
                    .into(),
                "airlock atmospherics" => asset_server
                    .load("models/tilemap/doors.glb#Mesh1/Primitive0")
                    .into(),
                "airlock research" => asset_server
                    .load("models/tilemap/doors.glb#Mesh63/Primitive0")
                    .into(),
                "airlock medical" => asset_server
                    .load("models/tilemap/doors.glb#Mesh57/Primitive0")
                    .into(),
                "table" => TilemapMesh::Multiple(AdjacencyMeshes {
                    default: asset_server.load("models/tilemap/tables.glb#Mesh71/Primitive0"),
                    o: asset_server.load("models/tilemap/tables.glb#Mesh71/Primitive0"),
                    u: asset_server.load("models/tilemap/tables.glb#Mesh63/Primitive0"),
                    i: asset_server.load("models/tilemap/tables.glb#Mesh74/Primitive0"),
                    l: asset_server.load("models/tilemap/tables.glb#Mesh72/Primitive0"),
                    t: asset_server.load("models/tilemap/tables.glb#Mesh75/Primitive0"),
                    x: asset_server.load("models/tilemap/tables.glb#Mesh77/Primitive0"),
                }),
                "chair" => asset_server
                    .load("models/tilemap/chairs.glb#Mesh0/Primitive0")
                    .into(),
                _ => continue,
            });
            definition.connector_mesh =
                Some(asset_server.load("models/tilemap/walls windows.glb#Mesh43/Primitive0"));
        }
    }
}

pub fn tilemap_spawning_system(
    mut commands: Commands,
    mut tilemaps: Query<(&mut TileMap, Entity)>,
    mut added_event: EventReader<ChunkObserverAddedEvent>,
    mut spawned_event: EventWriter<ChunkSpawnedEvent>,
) {
    for event in added_event.iter() {
        let (mut tilemap, tilemap_entity) = match tilemaps.get_mut(event.tilemap_entity) {
            Ok(map) => map,
            Err(_) => continue,
        };
        let chunk_index = event.chunk_index;
        if tilemap.spawned_chunks.contains_key(&chunk_index) {
            continue;
        }

        let spawned = apply_chunk(
            &mut commands,
            None,
            chunk_index,
            &tilemap.data,
            tilemap_entity,
        );
        tilemap.spawned_chunks.insert(chunk_index, spawned);
        spawned_event.send(ChunkSpawnedEvent {
            tilemap_entity,
            chunk_index,
        });
    }
}

pub fn tilemap_spawn_adjacency_update_system(
    tilemaps: Query<&TileMap>,
    mut turf_entities: Query<(&mut Transform, &mut Handle<Mesh>), With<TurfMarker>>,
    mut spawned_event: EventReader<ChunkSpawnedEvent>,
) {
    for event in spawned_event.iter() {
        let tilemap = match tilemaps.get(event.tilemap_entity) {
            Ok(x) => x,
            Err(_) => continue,
        };
        let chunk_position =
            MapData::position_from_chunk_index(tilemap.data.size, event.chunk_index).as_i32();
        for &dir in [
            Direction::North,
            Direction::South,
            Direction::East,
            Direction::West,
        ]
        .iter()
        {
            let offset: IVec2 = dir.into();
            let adjacent_position = chunk_position + offset;
            if adjacent_position.x < 0 || adjacent_position.y < 0 {
                continue;
            }
            let adjacent_position = adjacent_position.as_u32();
            let adjacent_index = tilemap.data.index_from_chunk_position(adjacent_position);
            let adjacent_chunk = match tilemap.spawned_chunks.get(&adjacent_index) {
                Some(x) => x,
                _ => continue,
            };
            let tiles: Box<dyn Iterator<Item = (usize, &Option<SpawnedTile>)>> = match dir {
                Direction::North => Box::new(adjacent_chunk.bottom_border()),
                Direction::South => Box::new(adjacent_chunk.top_border()),
                Direction::East => Box::new(adjacent_chunk.left_border()),
                Direction::West => Box::new(adjacent_chunk.right_border()),
            };
            for (tile_index, tile) in tiles.filter_map(|(i, t)| t.as_ref().map(|t| (i, t))) {
                if let Some((turf, turf_entity)) = tile.spawned_turf {
                    let tile_position = adjacent_position * UVec2::new(CHUNK_SIZE, CHUNK_SIZE)
                        + TileData::position_in_chunk(tile_index);
                    let turf_definition = tilemap.data.turf_definition(turf.definition_id).unwrap();
                    if let Some((mesh_handle, rotation)) = super::spawning::get_turf_mesh(
                        turf_definition,
                        tile_position,
                        &tilemap.data,
                    ) {
                        let (mut transform, mut mesh) = turf_entities.get_mut(turf_entity).unwrap();
                        transform.rotation = rotation;
                        *mesh = mesh_handle;
                    };
                }
            }
        }
    }
}

pub fn tilemap_despawning_system(
    mut commands: Commands,
    mut tilemaps: Query<&mut TileMap>,
    mut removed_event: EventReader<ChunkObserverRemovedEvent>,
) {
    for event in removed_event.iter() {
        let mut tilemap = match tilemaps.get_mut(event.tilemap_entity) {
            Ok(map) => map,
            Err(_) => continue,
        };
        let chunk_index = event.chunk_index;
        // TODO: Check if any other observer is still observing the chunk
        let spawned_chunk = match tilemap.spawned_chunks.remove(&chunk_index) {
            Some(c) => c,
            None => continue,
        };

        despawn_chunk(&mut commands, spawned_chunk);
    }
}

fn absolute_tilemap_position(position: UVec2, tilemap_transform: &GlobalTransform) -> Vec3 {
    tilemap_transform.mul_vec3(Vec3::new(position.x as f32, 0.0, position.y as f32))
}

fn chunk_corners(map_size: UVec2, tilemap_transform: &GlobalTransform) -> (Vec2, Vec2, Vec2, Vec2) {
    let chunk_size = UVec2::new(CHUNK_SIZE, CHUNK_SIZE);
    (
        absolute_tilemap_position(UVec2::new(0, 0), tilemap_transform).xz(),
        absolute_tilemap_position(
            UVec2::new(map_size.x - 1, 0) * chunk_size + UVec2::new(CHUNK_SIZE, 0),
            tilemap_transform,
        )
        .xz(),
        absolute_tilemap_position(
            UVec2::new(map_size.x - 1, map_size.y - 1) * chunk_size
                + UVec2::new(CHUNK_SIZE, CHUNK_SIZE),
            tilemap_transform,
        )
        .xz(),
        absolute_tilemap_position(
            UVec2::new(0, map_size.y - 1) * chunk_size + UVec2::new(0, CHUNK_SIZE),
            tilemap_transform,
        )
        .xz(),
    )
}

fn line_intersects_circle(start: Vec2, end: Vec2, circle: Vec2, radius: f32) -> bool {
    let start_relative = start - circle;
    let end_relative = end - circle;
    let a =
        (end_relative.x - start_relative.x).powi(2) + (end_relative.y - start_relative.y).powi(2);
    let b = 2.0
        * (start_relative.x * (end_relative.x - start_relative.x)
            + start_relative.y * (end_relative.y - start_relative.y));
    let c = start_relative.x.powi(2) + start_relative.y.powi(2) - radius.powi(2);
    let discriminator = b.powi(2) - 4.0 * a * c;
    if discriminator <= 0.0 {
        return false;
    }
    let sqrt_discriminator = discriminator.sqrt();
    let t1 = (-b + sqrt_discriminator) / (2.0 * a);
    if t1 > 0.0 && t1 < 1.0 {
        return true;
    }
    let t2 = (-b - sqrt_discriminator) / (2.0 * a);
    if t2 > 0.0 && t2 < 1.0 {
        return true;
    }

    false
}

fn point_right_of_line(start: Vec2, end: Vec2, point: Vec2) -> bool {
    (end.x - start.x) * (point.y - start.y) - (point.x - start.x) * (end.y - start.y) >= 0.0
}

fn is_point_in_rect(a: Vec2, b: Vec2, c: Vec2, d: Vec2, point: Vec2) -> bool {
    point_right_of_line(a, b, point)
        && point_right_of_line(b, c, point)
        && point_right_of_line(c, d, point)
        && point_right_of_line(d, a, point)
}

fn is_point_in_circle(point: Vec2, circle: Vec2, radius: f32) -> bool {
    point.distance_squared(circle) < radius.powi(2)
}

fn rect_intersects_circle(a: Vec2, b: Vec2, c: Vec2, d: Vec2, circle: Vec2, radius: f32) -> bool {
    is_point_in_rect(a, b, c, d, circle)
        || is_point_in_circle(a, circle, radius)
        || is_point_in_circle(b, circle, radius)
        || is_point_in_circle(c, circle, radius)
        || is_point_in_circle(d, circle, radius)
        || line_intersects_circle(a, b, circle, radius)
        || line_intersects_circle(b, c, circle, radius)
        || line_intersects_circle(c, d, circle, radius)
        || line_intersects_circle(d, a, circle, radius)
}

pub fn tilemap_observer_system(
    tilemaps: Query<(&TileMap, &GlobalTransform, Entity)>,
    mut observers: Query<(&mut TileMapObserver, &Transform, Entity)>,
    mut added_event: EventWriter<ChunkObserverAddedEvent>,
    mut removed_event: EventWriter<ChunkObserverRemovedEvent>,
) {
    for (mut observer, observer_transform, observer_entity) in observers.iter_mut() {
        let observer_position = observer_transform.translation.xz();
        let range = observer.view_range;
        for (tilemap, tilemap_transform, tilemap_entity) in tilemaps.iter() {
            let map_size = tilemap.data.size();
            let (a, b, c, d) = chunk_corners(map_size, tilemap_transform);
            if rect_intersects_circle(a, b, c, d, observer_position, range) {
                for (index, _) in tilemap.data.iter_chunks() {
                    let is_observed = observer.observing_chunk(tilemap_entity, index);

                    let position = MapData::position_from_chunk_index(tilemap.data.size, index);
                    let x_offset = (CHUNK_SIZE * position.x) as f32;
                    let y_offset = (CHUNK_SIZE * position.y) as f32;
                    let offset = (tilemap_transform.local_x()
                        * Vec3::new(x_offset, x_offset, x_offset)
                        + tilemap_transform.local_z() * Vec3::new(y_offset, y_offset, y_offset))
                    .xz();
                    let (mut a, mut b, mut c, mut d) = chunk_corners(UVec2::ONE, tilemap_transform);
                    a += offset;
                    b += offset;
                    c += offset;
                    d += offset;

                    if rect_intersects_circle(a, b, c, d, observer_position, range) {
                        if !is_observed {
                            observer.observe_chunk(tilemap_entity, index);
                            added_event.send(ChunkObserverAddedEvent {
                                tilemap_entity,
                                observer: observer_entity,
                                chunk_index: index,
                            });
                        }
                    } else if is_observed {
                        observer.remove_chunk(tilemap_entity, index);
                        removed_event.send(ChunkObserverRemovedEvent {
                            tilemap_entity,
                            observer: observer_entity,
                            chunk_index: index,
                        });
                    }
                }
            } else if let Some(chunks) = observer.remove_tilemap(tilemap_entity) {
                removed_event.send_batch(chunks.iter().map(|&index| ChunkObserverRemovedEvent {
                    tilemap_entity,
                    observer: observer_entity,
                    chunk_index: index,
                }));
            }
        }
    }
}
