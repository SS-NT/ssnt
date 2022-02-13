use bevy::{math::UVec2, utils::HashMap};

use super::{Tile, TileMap};
use maps::{
    tile_neighbours, AdjacencyInformation, Direction, FurnitureData, FurnitureDefinition,
    FurnitureKind, MapData, TileData, TurfData, TurfDefinition, CHUNK_SIZE,
};

type DefinitionLookup<'a> = HashMap<&'a str, u32>;

pub fn to_map_data(tilemap: &TileMap) -> MapData {
    let mut map_data = MapData::new(tilemap.size() / UVec2::new(CHUNK_SIZE, CHUNK_SIZE));
    let mut turf_definitions = DefinitionLookup::default();
    let mut furniture_definitions = DefinitionLookup::default();
    for (index, tile) in tilemap.definitions.iter().enumerate() {
        let tile_definitions = create_tile_definitions(
            tile,
            &mut map_data,
            &mut turf_definitions,
            &mut furniture_definitions,
        );
        if tile_definitions.is_none() {
            continue;
        }
        let (turf_definition, furniture_definition) = tile_definitions.unwrap();

        for (position, &definition_id) in tilemap.tiles.iter().filter(|(_, i)| **i == index) {
            let original_tile = tilemap.definitions.get(definition_id).unwrap();
            let tile_data = create_tile_data(original_tile, turf_definition, furniture_definition);
            map_data
                .set_tile(UVec2::new(position.x, position.z), Some(tile_data))
                .unwrap();
        }
    }
    let positions: Vec<UVec2> = map_data.iter_tiles().map(|(p, _)| p).collect();
    for position in positions {
        let tile_data = map_data.tile(position).unwrap();
        if let Some(furniture_data) = tile_data.furniture {
            let definition = map_data
                .furniture_definition(furniture_data.definition_id)
                .unwrap();
            if definition.kind == FurnitureKind::Door {
                let mut adjacency = AdjacencyInformation::default();
                for (dir, pos) in tile_neighbours(position) {
                    if let Some(turf_data) = map_data.tile(pos).and_then(|t| t.turf) {
                        let turf_definition =
                            map_data.turf_definition(turf_data.definition_id).unwrap();
                        if turf_definition.category == "wall" {
                            adjacency.add(dir);
                            continue;
                        }
                    }
                    if let Some(furniture_data) = map_data.tile(pos).and_then(|t| t.furniture) {
                        let furniture_definition = map_data
                            .furniture_definition(furniture_data.definition_id)
                            .unwrap();
                        if furniture_definition.kind == FurnitureKind::Door
                            || furniture_definition.kind == FurnitureKind::Table
                        {
                            adjacency.add(dir);
                            continue;
                        }
                    }
                }
                if let Some(direction) = adjacency.is_i() {
                    map_data
                        .tile_mut(position)
                        .unwrap()
                        .as_mut()
                        .unwrap()
                        .furniture
                        .as_mut()
                        .unwrap()
                        .direction = Some(direction);
                }
            }
        }
    }

    let spawn_definiton = tilemap.definitions.iter().enumerate().find(|(_, tile)| {
        tile.components.iter().any(|c| {
            c.path == "/obj/docking_port/stationary"
                && c.variable("id").map(|v| &v.value)
                    == Some(&super::Value::Literal("arrivals_stationary".to_string()))
        })
    });
    if let Some((spawn_index, _)) = spawn_definiton {
        let (&spawn_position, _) = tilemap
            .tiles
            .iter()
            .find(|(_, index)| **index == spawn_index)
            .unwrap();
        map_data.spawn_position = UVec2::new(spawn_position.x, spawn_position.z);
    }

    map_data
}

fn create_tile_definitions(
    tile: &Tile,
    map: &mut MapData,
    turf_definitions: &mut DefinitionLookup,
    furniture_definitions: &mut DefinitionLookup,
) -> Option<(Option<u32>, Option<u32>)> {
    let turf = create_turf_definition(tile, map, turf_definitions);
    let furniture = create_furniture_definition(tile, map, furniture_definitions);
    if turf.is_none() && furniture.is_none() {
        return None;
    }

    Some((turf, furniture))
}

fn create_tile_data(
    original_tile: &Tile,
    turf_definition: Option<u32>,
    furniture_definition: Option<u32>,
) -> TileData {
    let mut furniture_dir = None;
    if furniture_definition.is_some() {
        if let Some(dir) = original_tile
            .components
            .iter()
            .find_map(|c| c.variable("dir"))
        {
            if let super::Value::Number(n) = dir.value {
                furniture_dir = match n as i64 {
                    1 => Some(Direction::North),
                    2 => Some(Direction::South),
                    4 => Some(Direction::East),
                    8 => Some(Direction::West),
                    _ => Some(Direction::North),
                }
            }
        }
    }

    TileData {
        turf: turf_definition.map(|i| TurfData { definition_id: i }),
        furniture: furniture_definition.map(|i| FurnitureData {
            definition_id: i,
            direction: furniture_dir,
        }),
    }
}

fn create_turf_definition(
    tile: &Tile,
    map: &mut MapData,
    turf_definitions: &mut DefinitionLookup,
) -> Option<u32> {
    let turf_description = tile
        .components
        .iter()
        .filter_map(|o| {
            let priority = if o.path.starts_with("/obj") { 1 } else { 0 };
            let mut name = match o.path.as_str() {
                "/turf/closed/wall" => Some(("wall", "wall")),
                "/turf/closed/wall/r_wall" => Some(("reinforced wall", "wall")),
                "/obj/structure/grille" => Some(("grille", "grille")),
                "/obj/structure/plasticflaps/opaque" => Some(("wall", "wall")),
                "/obj/effect/spawner/structure/window" => Some(("window", "wall")),
                "/obj/effect/spawner/structure/window/reinforced" => {
                    Some(("reinforced window", "wall"))
                }
                "/obj/effect/spawner/structure/window/reinforced/tinted" => {
                    Some(("reinforced window", "wall"))
                }
                "/turf/open/floor/plasteel" => Some(("floor", "floor")),
                "/turf/open/floor/plasteel/white" => Some(("white floor", "floor")),
                "/turf/open/floor/plasteel/white/corner" => Some(("white floor", "floor")),
                "/turf/open/floor/plasteel/dark" => Some(("dark floor", "floor")),
                "/turf/open/floor/plasteel/grimy" => Some(("floor", "floor")),
                "/turf/open/floor/plating" => Some(("plating", "floor")),
                "/turf/open/floor/wood" => Some(("wood floor", "floor")),
                _ => None,
            };
            // Fallback for all floors
            if name.is_none() && o.path.starts_with("/turf/open/floor") {
                name = Some(("floor", "floor"));
            }

            Some((priority, name?))
        })
        .max_by_key(|x| x.0)?
        .1;

    let definition_id = *turf_definitions
        .entry(turf_description.0)
        .or_insert_with(|| {
            map.insert_turf_definition(TurfDefinition::new(turf_description.0, turf_description.1))
        });

    Some(definition_id)
}

fn create_furniture_definition(
    tile: &Tile,
    map: &mut MapData,
    furniture_definitions: &mut DefinitionLookup,
) -> Option<u32> {
    let furniture_definition = tile
        .components
        .iter()
        .filter_map(|o| {
            if o.path.contains("door/airlock") {
                if o.path.contains("maintenance") {
                    Some(("airlock maintenance", FurnitureKind::Door))
                } else if o.path.contains("command") {
                    Some(("airlock command", FurnitureKind::Door))
                } else if o.path.contains("mining") {
                    Some(("airlock supply", FurnitureKind::Door))
                } else if o.path.contains("security") {
                    Some(("airlock security", FurnitureKind::Door))
                } else if o.path.contains("engineering") {
                    Some(("airlock engineering", FurnitureKind::Door))
                } else if o.path.contains("atmos") {
                    Some(("airlock atmospherics", FurnitureKind::Door))
                } else if o.path.contains("research") {
                    Some(("airlock research", FurnitureKind::Door))
                } else if o.path.contains("medical") {
                    Some(("airlock medical", FurnitureKind::Door))
                } else {
                    Some(("airlock", FurnitureKind::Door))
                }
            } else if o.path.starts_with("/obj/structure/table") {
                Some(("table", FurnitureKind::Table))
            } else if o.path.starts_with("/obj/structure/chair") {
                Some(("chair", FurnitureKind::Chair))
            } else {
                None
            }
        })
        .next()?;

    let definition_id = *furniture_definitions
        .entry(furniture_definition.0)
        .or_insert_with(|| {
            map.insert_furniture_definition(FurnitureDefinition::new(
                furniture_definition.0,
                furniture_definition.1,
            ))
        });

    Some(definition_id)
}