use bevy::{math::{IVec2, UVec2}, prelude::{Handle, Mesh, StandardMaterial}};

pub mod components;
pub mod events;
mod spawning;
pub mod systems;

pub struct MapData {
    // Size in chunks
    size: UVec2,
    chunks: Vec<Option<Box<Chunk>>>,
    turf_definitions: Vec<TurfDefinition>,
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

    pub fn iter_tiles(&self) -> impl Iterator<Item = (UVec2, &TileData)> {
        self.iter_chunks()
            .map(move |(p, c)| {
                (
                    Self::position_from_chunk_index(self.size, p)
                        * UVec2::new(CHUNK_SIZE, CHUNK_SIZE),
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

#[derive(Clone)]
pub enum TurfMesh {
    Single(Handle<Mesh>),
    Multiple(TurfMeshes),
}

impl From<Handle<Mesh>> for TurfMesh {
    fn from(handle: Handle<Mesh>) -> Self {
        Self::Single(handle)
    }
}

#[derive(Clone)]
pub struct TurfMeshes {
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

#[derive(Clone)]
pub struct TurfDefinition {
    pub name: String,
    pub category: String,
    pub mesh: Option<TurfMesh>,
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

#[derive(Clone, Copy)]
pub enum Direction {
    North = 0,
    East,
    South,
    West,
}

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

#[derive(Clone, Copy)]
pub struct FurnitureData {
    pub definition_id: u32,
    pub direction: Direction,
}
