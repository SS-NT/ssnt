use bevy::{
    math::{IVec2, Quat, UVec2, Vec3},
    prelude::{Handle, Mesh, StandardMaterial},
};

pub mod components;
pub mod events;
mod spawning;
pub mod systems;

pub struct MapData {
    // Size in chunks
    size: UVec2,
    chunks: Vec<Option<Box<Chunk>>>,
    turf_definitions: Vec<TurfDefinition>,
    pub furniture_definitions: Vec<FurnitureDefinition>,
    pub spawn_position: UVec2,
}

impl MapData {
    pub fn new(size: UVec2) -> Self {
        let mut chunks = Vec::new();
        chunks.resize_with((size.x * size.y) as usize, Default::default);
        Self {
            size,
            chunks,
            turf_definitions: Default::default(),
            furniture_definitions: Default::default(),
            spawn_position: UVec2::ZERO,
        }
    }

    pub fn size(&self) -> UVec2 {
        self.size
    }

    pub fn iter_chunks(&self) -> impl Iterator<Item = (usize, &Chunk)> {
        self.chunks
            .iter()
            .enumerate()
            .filter_map(|(i, x)| x.as_ref().map(|x| (i, x.as_ref())))
    }

    pub fn chunk(&self, index: usize) -> Option<&Chunk> {
        self.chunks.get(index)?.as_deref()
    }

    pub fn chunk_at(&self, position: UVec2) -> Option<&Chunk> {
        let index = self.index_from_chunk_position(position);
        self.chunk(index)
    }

    pub fn chunk_mut(&mut self, index: usize) -> Option<&mut Option<Box<Chunk>>> {
        self.chunks.get_mut(index)
    }

    pub fn chunk_at_mut(&mut self, position: UVec2) -> Option<&mut Option<Box<Chunk>>> {
        let index = self.index_from_chunk_position(position);
        self.chunk_mut(index)
    }

    pub fn iter_tiles(&mut self) -> impl Iterator<Item = (UVec2, &TileData)> {
        let size = self.size;
        self.iter_chunks()
            .map(move |(p, c)| {
                (
                    Self::position_from_chunk_index(size, p) * UVec2::new(CHUNK_SIZE, CHUNK_SIZE),
                    c,
                )
            })
            .flat_map(|(p, c)| {
                c.tiles
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.is_some())
                    .map(move |(i, t)| {
                        let y = i as u32 / CHUNK_SIZE;
                        let x = match y {
                            0 => i as u32,
                            _ => i as u32 % (y * CHUNK_SIZE),
                        };
                        (p + UVec2::new(x, y), t.as_ref().unwrap())
                    })
            })
    }

    pub fn tile(&self, position: UVec2) -> Option<&TileData> {
        let chunk_index = self.index_from_position(position);
        let position_in_chunk = self.position_inside_chunk(position);
        self.chunks
            .get(chunk_index)?
            .as_ref()?
            .tile(position_in_chunk)
            .as_ref()
    }

    pub fn tile_mut(&mut self, position: UVec2) -> Option<&mut Option<TileData>> {
        let chunk_index = self.index_from_position(position);
        let position_in_chunk = self.position_inside_chunk(position);
        self.chunks
            .get_mut(chunk_index)?
            .as_mut()?
            .tile_mut(position_in_chunk)
            .into()
    }

    pub fn set_tile(&mut self, position: UVec2, data: Option<TileData>) -> Result<(), ()> {
        let chunk_index = self.index_from_position(position);
        let position_in_chunk = self.position_inside_chunk(position);
        let chunk = self
            .chunks
            .get_mut(chunk_index)
            .ok_or(())?
            .get_or_insert_with(Default::default);
        *chunk.tile_mut(position_in_chunk) = data;
        Ok(())
    }

    fn position_inside_chunk(&self, position: UVec2) -> UVec2 {
        UVec2::new(position.x % CHUNK_SIZE, position.y % CHUNK_SIZE)
    }

    fn index_from_position(&self, position: UVec2) -> usize {
        let chunk_position = position / UVec2::new(CHUNK_SIZE, CHUNK_SIZE);
        self.index_from_chunk_position(chunk_position)
    }

    pub fn index_from_chunk_position(&self, position: UVec2) -> usize {
        (position.y * self.size.x + position.x) as usize
    }

    pub fn position_from_chunk_index(size: UVec2, index: usize) -> UVec2 {
        let y = index as u32 / size.x;
        let x = match y {
            0 => index as u32,
            _ => index as u32 % (y * size.x),
        };
        UVec2::new(x, y)
    }

    pub fn insert_turf_definition(&mut self, definition: TurfDefinition) -> u32 {
        self.turf_definitions.push(definition);
        (self.turf_definitions.len() - 1) as u32
    }

    pub fn turf_definition(&self, index: u32) -> Option<&TurfDefinition> {
        self.turf_definitions.get(index as usize)
    }

    pub fn insert_furniture_definition(&mut self, definition: FurnitureDefinition) -> u32 {
        self.furniture_definitions.push(definition);
        (self.furniture_definitions.len() - 1) as u32
    }

    pub fn furniture_definition(&self, index: u32) -> Option<&FurnitureDefinition> {
        self.furniture_definitions.get(index as usize)
    }
}

pub const CHUNK_SIZE: u32 = 16;
const CHUNK_LENGTH: usize = (CHUNK_SIZE * CHUNK_SIZE) as usize;
const EMPTY_TILE: Option<TileData> = None;

pub struct Chunk {
    tiles: [Option<TileData>; CHUNK_LENGTH],
    changed_tiles: [bool; CHUNK_LENGTH],
    changed: bool,
}

impl Default for Chunk {
    fn default() -> Self {
        Self {
            tiles: [EMPTY_TILE; CHUNK_LENGTH],
            changed_tiles: [false; CHUNK_LENGTH],
            changed: false,
        }
    }
}

impl Chunk {
    fn tile(&self, position: UVec2) -> &Option<TileData> {
        let index = Self::index_from_position(position);
        assert!(index < CHUNK_LENGTH);

        &self.tiles[index]
    }

    fn tile_mut(&mut self, position: UVec2) -> &mut Option<TileData> {
        let index = Self::index_from_position(position);
        assert!(index < CHUNK_LENGTH);

        self.changed = true;
        self.changed_tiles[index] = true;
        &mut self.tiles[index]
    }

    fn index_from_position(position: UVec2) -> usize {
        (position.y * CHUNK_SIZE + position.x) as usize
    }
}

pub fn tile_neighbours(position: UVec2) -> impl Iterator<Item = (Direction, UVec2)> {
    let position = position.as_i32();
    DIRECTIONS
        .iter()
        .map(move |&dir| { let o: IVec2 = dir.into(); (dir, position + o) })
        .filter(|(_, p)| p.x >= 0 && p.y >= 0)
        .map(|(dir, p)| (dir, p.as_u32()))
}

#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
pub enum TilemapMesh {
    Single(Handle<Mesh>),
    Multiple(AdjacencyMeshes),
}

impl From<Handle<Mesh>> for TilemapMesh {
    fn from(handle: Handle<Mesh>) -> Self {
        Self::Single(handle)
    }
}

#[derive(Clone)]
pub struct AdjacencyMeshes {
    pub default: Handle<Mesh>,
    // No neighbours
    pub o: Handle<Mesh>,
    // Connected north
    pub u: Handle<Mesh>,
    // Connected north & south
    pub i: Handle<Mesh>,
    // Connected north & east
    pub l: Handle<Mesh>,
    // Connected north & east & west
    pub t: Handle<Mesh>,
    // Connected in all 4 directions
    pub x: Handle<Mesh>,
}

impl AdjacencyMeshes {
    pub fn mesh_from_adjacency(&self, adjacency: AdjacencyInformation) -> (Handle<Mesh>, Quat) {
        if adjacency.is_o() {
            (self.o.clone(), Quat::IDENTITY)
        } else if let Some(dir) = adjacency.is_u() {
            (self.u.clone(), AdjacencyInformation::rotation_from_dir(dir))
        } else if let Some(dir) = adjacency.is_i() {
            (self.i.clone(), AdjacencyInformation::rotation_from_dir(dir))
        } else if let Some(dir) = adjacency.is_l() {
            (self.l.clone(), AdjacencyInformation::rotation_from_dir(dir))
        } else if let Some(dir) = adjacency.is_t() {
            (self.t.clone(), AdjacencyInformation::rotation_from_dir(dir))
        } else if adjacency.is_x() {
            (self.x.clone(), Quat::IDENTITY)
        } else {
            (self.default.clone(), Quat::IDENTITY)
        }
    }
}

#[derive(Default)]
pub struct AdjacencyInformation {
    directions: [bool; 4],
}

impl AdjacencyInformation {
    pub fn add(&mut self, direction: Direction) {
        self.directions[direction as usize] = true;
    }

    pub fn is_o(&self) -> bool {
        self.directions == [false, false, false, false]
    }

    pub fn is_u(&self) -> Option<Direction> {
        match self.directions {
            [true, false, false, false] => Some(Direction::North),
            [false, true, false, false] => Some(Direction::East),
            [false, false, true, false] => Some(Direction::South),
            [false, false, false, true] => Some(Direction::West),
            _ => None,
        }
    }

    pub fn is_l(&self) -> Option<Direction> {
        match self.directions {
            [true, true, false, false] => Some(Direction::North),
            [false, true, true, false] => Some(Direction::East),
            [false, false, true, true] => Some(Direction::South),
            [true, false, false, true] => Some(Direction::West),
            _ => None,
        }
    }

    pub fn is_t(&self) -> Option<Direction> {
        match self.directions {
            [true, true, false, true] => Some(Direction::North),
            [true, true, true, false] => Some(Direction::East),
            [false, true, true, true] => Some(Direction::South),
            [true, false, true, true] => Some(Direction::West),
            _ => None,
        }
    }

    pub fn is_i(&self) -> Option<Direction> {
        match self.directions {
            [true, false, true, false] => Some(Direction::North),
            [false, true, false, true] => Some(Direction::East),
            _ => None,
        }
    }

    pub fn is_x(&self) -> bool {
        self.directions == [true, true, true, true]
    }

    pub fn rotation_from_dir(direction: Direction) -> Quat {
        let corners = match direction {
            Direction::North => 2,
            Direction::East => 1,
            Direction::South => 0,
            Direction::West => 3,
        };
        Quat::from_axis_angle(Vec3::Y, std::f32::consts::FRAC_PI_2 * (corners as f32))
    }
}

#[derive(Clone)]
pub struct TurfDefinition {
    pub name: String,
    pub category: String,
    pub mesh: Option<TilemapMesh>,
    pub material: Option<Handle<StandardMaterial>>,
}

impl TurfDefinition {
    pub fn new(name: impl Into<String>, category: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            category: category.into(),
            mesh: None,
            material: None,
        }
    }
}

#[derive(Clone, PartialEq)]
pub enum FurnitureKind {
    Door,
    Table,
}

#[derive(Clone)]
pub struct FurnitureDefinition {
    pub name: String,
    pub mesh: Option<TilemapMesh>,
    pub material: Option<Handle<StandardMaterial>>,
    pub kind: FurnitureKind,
    // TODO: get rid of this
    pub connector_mesh: Option<Handle<Mesh>>,
}

impl FurnitureDefinition {
    pub fn new(name: impl Into<String>, kind: FurnitureKind) -> Self {
        Self {
            name: name.into(),
            mesh: None,
            material: None,
            kind,
            connector_mesh: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Direction {
    North = 0,
    East,
    South,
    West,
}

pub const DIRECTIONS: [Direction; 4] = [
    Direction::North,
    Direction::South,
    Direction::East,
    Direction::West,
];

impl From<IVec2> for Direction {
    fn from(vec: IVec2) -> Self {
        if vec.x > 0 {
            Self::East
        } else if vec.x < 0 {
            Self::West
        } else if vec.y > 0 {
            Self::South
        } else {
            Self::North
        }
    }
}

impl From<Direction> for IVec2 {
    fn from(val: Direction) -> Self {
        match val {
            Direction::North => -IVec2::Y,
            Direction::South => IVec2::Y,
            Direction::East => IVec2::X,
            Direction::West => -IVec2::X,
        }
    }
}

#[derive(Default, Clone, Copy)]
pub struct TileData {
    pub turf: Option<TurfData>,
    pub furniture: Option<FurnitureData>,
}

impl TileData {
    pub fn position_in_chunk(index: usize) -> UVec2 {
        let y = index as u32 / CHUNK_SIZE;
        let x = match y {
            0 => index as u32,
            _ => index as u32 % (y * CHUNK_SIZE),
        };
        UVec2::new(x, y)
    }
}

#[derive(Clone, Copy, PartialEq)]
pub struct TurfData {
    pub definition_id: u32,
}

#[derive(Clone, Copy, PartialEq)]
pub struct FurnitureData {
    pub definition_id: u32,
    pub direction: Option<Direction>,
}
