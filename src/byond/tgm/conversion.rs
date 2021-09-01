use bevy::{math::UVec2, utils::HashMap};

use super::{Tile, TileMap};
use crate::maps::{MapData, TileData, TurfData, TurfDefinition, CHUNK_SIZE};

type DefinitionLookup<'a> = HashMap<&'a str, u32>;

pub fn to_map_data(tilemap: &TileMap) -> MapData {
    let mut map_data = MapData::new(tilemap.size() / UVec2::new(CHUNK_SIZE, CHUNK_SIZE));
    let mut turf_definitions = DefinitionLookup::default();
    for (index, tile) in tilemap.definitions.iter().enumerate() {
        let tile_data = create_tile_data(tile, &mut map_data, &mut turf_definitions);
        if tile_data.is_none() {
            continue;
        }
        let tile_data = tile_data.unwrap();

        for (position, _) in tilemap.tiles.iter().filter(|(_, i)| **i == index) {
            map_data
                .set_tile(UVec2::new(position.x, position.z), Some(tile_data))
                .unwrap();
        }
    }

    let spawn_definiton = tilemap.definitions.iter().enumerate().find(|(_, tile)| {
        tile.components.iter().any(|c| {
            c.path == "/obj/docking_port/stationary"
                && c.variable("id").map(|v| &v.value)
                    == Some(&super::Value::Literal("arrivals_stationary".to_string()))
        })
    });
    if let Some((spawn_index,_)) = spawn_definiton {
        let (&spawn_position,_) = tilemap.tiles.iter().find(|(_, index)| **index == spawn_index).unwrap();
        map_data.spawn_position = UVec2::new(spawn_position.x, spawn_position.z);
    }

    map_data
}

fn create_tile_data(
    tile: &Tile,
    map: &mut MapData,
    turf_definitions: &mut DefinitionLookup,
) -> Option<TileData> {
    let object = tile
        .components
        .iter()
        .find(|o| o.path.starts_with("/turf"))?;
    let turf_name = match object.path.strip_prefix("/turf/")? {
        "closed/wall" => "wall",
        "closed/wall/r_wall" => "reinforced wall",
        _ => None?,
    };
    let definition_id = *turf_definitions.entry(&turf_name).or_insert_with(|| {
        map.insert_turf_definition(TurfDefinition {
            name: turf_name.into(),
        })
    });
    Some(TileData {
        turf: Some(TurfData { definition_id }),
        ..Default::default()
    })
}
