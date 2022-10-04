use bevy::{asset::AssetPathId, math::UVec2, utils::HashMap};

use super::{Tile, TileMap};
use maps::{TileData, TileLayer, TileMapData};

pub fn to_map_data(tilemap: &TileMap) -> TileMapData {
    let size = tilemap.size();

    let mut temporary_tiles = Vec::new();
    temporary_tiles.resize_with(size.x as usize * size.y as usize, Default::default);
    let mut job_spawns = HashMap::<String, Vec<UVec2>>::default();

    // Loop through all positions and convert the tile format
    for (position, &definition_index) in tilemap.tiles.iter() {
        let index = position.x + position.z * size.x;
        let definition = tilemap.definitions.get(definition_index).unwrap();
        // TODO: Cache this conversion (indexed by definition id)
        let tile_data = tile_to_data(definition);
        *temporary_tiles.get_mut(index as usize).unwrap() = Some(tile_data);

        // Find job spawn on tile
        for object in definition
            .components
            .iter()
            .filter(|c| c.path.starts_with("/obj/effect/landmark/start/"))
        {
            let job_name = object.path.rsplit_once('/').unwrap().1;
            if job_name.is_empty() {
                continue;
            }
            job_spawns
                .entry_ref(job_name)
                .or_default()
                .push(UVec2::new(position.x, position.z));
        }
    }

    TileMapData {
        size,
        tiles: temporary_tiles
            .into_iter()
            .map(|t| t.unwrap_or_default())
            .collect(),
        job_spawn_positions: job_spawns,
    }
}

fn tile_to_data(tile: &Tile) -> TileData {
    TileData {
        layers: maps::enum_map! {
            TileLayer::Turf => get_turf_path(tile),
            TileLayer::Furniture => get_furniture_path(tile),
            _ => None,
        },
    }
}

fn get_turf_path(tile: &Tile) -> Option<AssetPathId> {
    let turf_name = tile
        .components
        .iter()
        .filter_map(|o| {
            let priority = if o.path.starts_with("/obj") { 1 } else { 0 };
            let mut name = match o.path.as_str() {
                "/turf/closed/wall" => Some("wall"),
                "/turf/closed/wall/r_wall" => Some("reinforced wall"),
                "/obj/structure/grille" => Some("grille"),
                "/obj/structure/plasticflaps/opaque" => Some("wall"),
                "/obj/effect/spawner/structure/window" => Some("window"),
                "/obj/effect/spawner/structure/window/reinforced" => Some("reinforced window"),
                "/obj/effect/spawner/structure/window/reinforced/tinted" => {
                    Some("reinforced window")
                }
                "/turf/open/floor/plasteel" => Some("floor"),
                "/turf/open/floor/plasteel/white" => Some("white floor"),
                "/turf/open/floor/plasteel/white/corner" => Some("white floor"),
                "/turf/open/floor/plasteel/dark" => Some("dark floor"),
                "/turf/open/floor/plasteel/grimy" => Some("floor"),
                "/turf/open/floor/plating" => Some("plating"),
                "/turf/open/floor/wood" => Some("wood floor"),
                _ => None,
            };
            // Fallback for all floors
            if name.is_none() && o.path.starts_with("/turf/open/floor") {
                name = Some("floor");
            }

            Some((priority, name?))
        })
        .max_by_key(|x| x.0)?
        .1;

    Some(
        format!("tilemap/turfs/{}.scn.ron", turf_name)
            .as_str()
            .into(),
    )
}

fn get_furniture_path(tile: &Tile) -> Option<AssetPathId> {
    let furniture_name = tile
        .components
        .iter()
        .filter_map(|o| {
            if o.path.contains("door/airlock") {
                if o.path.contains("maintenance") {
                    Some("airlock maintenance")
                } else if o.path.contains("command") {
                    Some("airlock command")
                } else if o.path.contains("mining") {
                    Some("airlock supply")
                } else if o.path.contains("security") {
                    Some("airlock security")
                } else if o.path.contains("engineering") {
                    Some("airlock engineering")
                } else if o.path.contains("atmos") {
                    Some("airlock atmospherics")
                } else if o.path.contains("research") {
                    Some("airlock research")
                } else if o.path.contains("medical") {
                    Some("airlock medical")
                } else {
                    Some("airlock")
                }
            } else if o.path.starts_with("/obj/structure/table") {
                Some("table")
            } else if o.path.starts_with("/obj/structure/chair") {
                Some("chair")
            } else {
                None
            }
        })
        .next()?;

    Some(
        format!("tilemap/furniture/{}.scn.ron", furniture_name)
            .as_str()
            .into(),
    )
}
