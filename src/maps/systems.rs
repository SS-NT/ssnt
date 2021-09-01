use crate::maps::spawning::{apply_chunk, despawn_chunk};

use super::{components::*, spawning::SpawnedChunk, CHUNK_SIZE};
use bevy::{math::{UVec2, Vec2, Vec3, Vec3Swizzles}, prelude::{Assets, Commands, Entity, GlobalTransform, Mesh, Query, ResMut, StandardMaterial, Transform, info}};

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
            UVec2::new(map_size.x - 1, map_size.y - 1) * chunk_size + UVec2::new(CHUNK_SIZE, CHUNK_SIZE),
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
    let a = (end_relative.x - start_relative.x).powi(2) + (end_relative.y - start_relative.y).powi(2);
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

pub fn tilemap_observer_check(
    mut commands: Commands,
    mut tilemaps: Query<(&mut TileMap, &GlobalTransform, Entity)>,
    observers: Query<(&TileMapObserver, &Transform)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    for (observer, observer_transform) in observers.iter() {
        let observer_position = observer_transform.translation.xz();
        let range = observer.view_range;
        for (mut tilemap, tilemap_transform, tilemap_entity) in tilemaps.iter_mut() {
            let map_size = tilemap.data.size();
            let (a, b, c, d) = chunk_corners(map_size, tilemap_transform);
            if rect_intersects_circle(a, b, c, d, observer_position, range) {
                let tilemap = tilemap.into_inner();
                for (position, chunk) in tilemap.data.iter_chunks() {
                    let is_spawned = tilemap.spawned_chunks.contains_key(&position);

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
                    //println!("Corners: a={} b={} c={} d={}", a, b, c, d);

                    if rect_intersects_circle(a, b, c, d, observer_position, range) {
                        if !is_spawned && !tilemap.spawning_chunks.contains_key(&position) {
                            // TODO: Spawn chunk
                            let spawned = apply_chunk(
                                &mut commands,
                                None,
                                chunk,
                                position,
                                &tilemap.data,
                                tilemap_entity,
                                &mut materials,
                                &mut meshes
                            );
                            tilemap
                                .spawned_chunks
                                .insert(position, spawned);
                        }
                    } else if is_spawned {
                        let spawned_chunk = tilemap.spawned_chunks.remove(&position);
                        despawn_chunk(&mut commands, spawned_chunk.unwrap());
                    }
                }
            } else {
                for (_, chunk) in tilemap.spawned_chunks.drain() {
                    despawn_chunk(&mut commands, chunk);
                }
            }
        }
    }
}
