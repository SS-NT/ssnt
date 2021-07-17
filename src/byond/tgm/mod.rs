use bevy::{
    math::UVec3,
    prelude::{AddAsset, AppBuilder, Plugin},
    utils::{HashMap, StableHashMap},
};
use bevy_reflect::TypeUuid;

mod loader;
pub mod parsing;

pub use self::loader::TgmLoader;

#[derive(Default)]
pub struct TgmPlugin;

impl Plugin for TgmPlugin {
    fn build(&self, app: &mut AppBuilder) {
        app.init_asset_loader::<TgmLoader>();
    }
}

#[derive(TypeUuid)]
#[uuid = "b4bcacfa-c562-432a-807a-43a2974cc2d6"]
pub struct TileMap {
    definitions: Vec<Tile>,
    tiles: HashMap<UVec3, usize>,
}

impl TileMap {
    pub fn new(
        definitions: StableHashMap<&str, Tile>,
        positions: Vec<(UVec3, &str)>,
    ) -> Self {
        let mut tiles = HashMap::default();
        for (position, name) in positions.into_iter() {
            let def_result = definitions
                    .iter()
                    .enumerate()
                    .find(|(_, (k, _))| **k == name);
            if let Some(definition) = def_result {
                tiles.insert(
                    position,
                    definition.0,
                );
            } else {
                //println!("Missing tile definition {}", name);
            }
        }

        Self { definitions: definitions.into_iter().map(|(_, v)| v).collect(), tiles }
    }

    pub fn get_tile(&self, position: UVec3) -> Option<&Tile> {
        self.definitions.get(self.tiles.get(&position)?.clone())
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

#[derive(Debug, Clone)]
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
}

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
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