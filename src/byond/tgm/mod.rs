use bevy::{
    math::{UVec2, UVec3},
    prelude::{AddAsset, App, Plugin},
    reflect::TypeUuid,
    utils::HashMap,
};

pub mod conversion;
mod loader;
pub mod parsing;

pub use self::loader::TgmLoader;

#[derive(Default)]
pub struct TgmPlugin;

impl Plugin for TgmPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset_loader::<TgmLoader>();
    }
}

#[derive(Clone, TypeUuid)]
#[uuid = "b4bcacfa-c562-432a-807a-43a2974cc2d6"]
pub struct TileMap {
    definitions: Vec<Tile>,
    tiles: HashMap<UVec3, usize>,
}

impl TileMap {
    pub fn new(mut definitions: Vec<(&str, Tile)>, positions: Vec<(UVec3, &str)>) -> Self {
        let mut tiles = HashMap::default();
        definitions.sort_unstable_by_key(|&(name, _)| name);

        let mut cached_result = None;
        for (position, name) in positions.into_iter() {
            // Use previous lookup if it was the same definition
            // Improves speed when reading empty space areas
            if let Some((n, index)) = cached_result {
                if n == name {
                    tiles.insert(position, index);
                    continue;
                }
            }

            let search_result = definitions.binary_search_by_key(&name, |&(name, _)| name);
            if let Ok(index) = search_result {
                tiles.insert(position, index);
                cached_result = (name, index).into();
            }
            // NOTE: We ignore missing tile definitions here
        }

        Self {
            definitions: definitions.into_iter().map(|(_, v)| v).collect(),
            tiles,
        }
    }

    pub fn get_tile(&self, position: UVec3) -> Option<&Tile> {
        self.definitions.get(*self.tiles.get(&position)?)
    }

    pub fn iter_tiles(&self) -> impl Iterator<Item = (&UVec3, Option<&Tile>)> {
        self.tiles
            .iter()
            .map(move |(position, &index)| (position, self.definitions.get(index)))
    }

    pub fn middle(&self) -> UVec2 {
        let (biggest, smallest) = self.corners();

        smallest + (biggest - smallest) / UVec2::new(2, 2)
    }

    pub fn size(&self) -> UVec2 {
        let (biggest, smallest) = self.corners();
        biggest - smallest
    }

    fn corners(&self) -> (UVec2, UVec2) {
        let mut biggest = UVec2::default();
        let mut smallest = UVec2::new(u32::MAX, u32::MAX);
        for &position in self.tiles.keys() {
            if position.x > biggest.x {
                biggest.x = position.x;
            }
            if position.z > biggest.y {
                biggest.y = position.z;
            }
            if position.x < smallest.x {
                smallest.x = position.x;
            }
            if position.z < smallest.y {
                smallest.y = position.z;
            }
        }
        (biggest, smallest)
    }
}

#[derive(Clone)]
pub struct Tile {
    pub components: Vec<Object>,
}

impl From<Vec<Object>> for Tile {
    fn from(vec: Vec<Object>) -> Self {
        Self { components: vec }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    pub path: String,
    pub variables: Vec<Variable>,
}

impl Object {
    fn new(name: impl Into<String>, variables: Vec<Variable>) -> Self {
        Self {
            path: name.into(),
            variables,
        }
    }

    fn variable(&self, name: &str) -> Option<&Variable> {
        self.variables.iter().filter(|v| v.name == name).next()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variable {
    pub name: String,
    pub value: Value,
}

impl Variable {
    fn new(name: impl Into<String>, value: Value) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Number(f64),
    Literal(String),
    Object(Object),
    List(Vec<Value>),
    Map(HashMap<String, Value>),
    Null,
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Number(value)
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::Literal(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Literal(value.to_owned())
    }
}

impl From<Object> for Value {
    fn from(value: Object) -> Self {
        Value::Object(value)
    }
}

impl From<Vec<Value>> for Value {
    fn from(values: Vec<Value>) -> Self {
        Self::List(values)
    }
}

impl From<HashMap<&str, Value>> for Value {
    fn from(value: HashMap<&str, Value>) -> Self {
        Self::Map(value.into_iter().map(|(k, b)| (k.to_owned(), b)).collect())
    }
}