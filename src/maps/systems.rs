use super::{CHUNK_SIZE, MapData, components::*, events::*, spawning::{apply_chunk, despawn_chunk}};
use bevy::{math::{UVec2, Vec2, Vec3, Vec3Swizzles}, prelude::{Added, AssetServer, Assets, Commands, Entity, EventReader, EventWriter, GlobalTransform, Query, Res, ResMut, StandardMaterial, Transform}};

pub fn tilemap_mesh_loading_system(
    mut tilemaps: Query<&mut TileMap, Added<TileMap>>,
    asset_server: Res<AssetServer>,
) {
    for mut tilemap in tilemaps.iter_mut() {
        for definition in tilemap
            .data
            .turf_definitions
            .iter_mut()
            .filter(|d| d.mesh.is_none())
        {
            definition.mesh = Some(match definition.name.as_str() {
                "wall" => asset_server
                    .load("models/tilemap/walls windows.glb#Mesh29/Primitive0")
                    .into(),
                "reinforced wall" => asset_server
                    .load("models/tilemap/walls windows.glb#Mesh24/Primitive0")
                    .into(),
                "grille" => asset_server
                    .load("models/tilemap/girders.glb#Mesh1/Primitive0")
                    .into(),
                "window" => asset_server
                    .load("models/tilemap/walls windows.glb#Mesh22/Primitive0")
                    .into(),
                "reinforced window" => asset_server
                    .load("models/tilemap/walls windows.glb#Mesh39/Primitive0")
                    .into(),
                _ => continue,
            });
        }
    }
}

pub fn tilemap_spawning_system(
    mut commands: Commands,
    mut tilemaps: Query<(&mut TileMap, Entity)>,
    mut added_event: EventReader<ChunkObserverAddedEvent>,
    mut materials: ResMut<Assets<StandardMaterial>>,
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

        let spawned = apply_chunk(&mut commands, None, chunk_index, &tilemap.data, tilemap_entity, &mut materials);
        tilemap.spawned_chunks.insert(chunk_index, spawned);
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
                removed_event.send_batch(chunks.iter().map(|&index| {
                    ChunkObserverRemovedEvent {
                        tilemap_entity,
                        observer: observer_entity,
                        chunk_index: index,
                    }
                }));
            }
        }
    }
}
