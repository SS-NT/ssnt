use bevy::{asset::AssetPathId, math::UVec2, utils::HashMap};

use super::{Tile, TileMap, Value};
use maps::{Direction, TileData, TileMapData, DIRECTIONS};

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

    for index in 0..temporary_tiles.len() {
        let Some(tile) = temporary_tiles.get_mut(index).unwrap() else {
            continue;
        };
        let mounts = std::mem::take(&mut tile.high_mounts);
        for (mount_index, mount) in mounts.iter().enumerate() {
            let Some(mount) = mount else {
                continue;
            };

            let direction = DIRECTIONS[mount_index];
            let target_index = match direction {
                Direction::North => index - size.x as usize,
                Direction::East => index + 1,
                Direction::South => index + size.x as usize,
                Direction::West => index - 1,
            };

            let Some(target_tile) = temporary_tiles.get_mut(target_index) else {
                continue;
            };

            let Some(target_tile) = target_tile else {
                continue;
            };
            target_tile.high_mounts[(-direction) as usize] = Some(*mount);
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
        turf: get_turf_path(tile),
        furniture: get_furniture_path(tile),
        high_mounts: get_high_mounts_path(tile),
    }
}

fn get_turf_path(tile: &Tile) -> Option<AssetPathId> {
    let turf_name = tile
        .components
        .iter()
        .filter_map(|o| {
            let priority = i32::from(o.path.starts_with("/obj"));
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

fn get_high_mounts_path(tile: &Tile) -> [Option<AssetPathId>; 4] {
    let mut mounts = [None; 4];

    for (byond_dir, name) in tile
        .components
        .iter()
        .filter_map(|o| {
            match o.path.as_str() {
                "/obj/machinery/light" => Some("light_tube"),
                _ => None,
            }
            .map(|n| (o, n))
        })
        .filter_map(|(o, n)| match o.variable("dir") {
            Some(Value::Number(dir)) => Some((*dir as u8, n)),
            _ => None,
        })
    {
        if let Some(direction) = Direction::from_byond(byond_dir) {
            mounts[direction as usize] = Some(
                format!("tilemap/wall_mounts/{}.scn.ron", name)
                    .as_str()
                    .into(),
            );
        };
    }

    mounts
}

trait DirectionExt {
    fn from_byond(direction: u8) -> Option<Self>
    where
        Self: Sized;
}

impl DirectionExt for Direction {
    fn from_byond(direction: u8) -> Option<Self> {
        match direction {
            1 => Some(Direction::North),
            2 => Some(Direction::South),
            4 => Some(Direction::East),
            8 => Some(Direction::West),
            _ => None,
        }
    }
}
