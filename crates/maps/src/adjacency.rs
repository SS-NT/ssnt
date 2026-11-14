use std::marker::{Send, Sync};
use std::sync::Arc;

use bevy::ecs::{
    error::Result,
    reflect::{ReflectComponent, ReflectFromTemplate, ReflectSharedTemplate, ReflectTemplate},
    template::{FromTemplate, SpecializeFromTemplate, Template, TemplateContext},
};
use bevy::{
    prelude::{Component, Handle, Mesh, Quat, Vec3},
    reflect::{std_traits::ReflectDefault, Reflect},
};

use crate::{Direction, DIRECTIONS};

/// Defines data that depends on what sides a tile is surrounded.
#[derive(Clone, Default, Reflect)]
#[reflect(Default)]
pub struct AdjacencyVariants<T: Reflect + Sync + Send + Default + 'static> {
    pub default: T,
    // No neighbours
    pub o: T,
    // Connected north
    pub u: T,
    // Connected north & south
    pub i: T,
    // Connected north & east
    pub l: T,
    // Connected north & east & west
    pub t: T,
    // Connected in all 4 directions
    pub x: T,
}

impl<T: std::clone::Clone + Reflect + Sync + Send + Default + 'static> AdjacencyVariants<T> {
    fn get_shape(&self, shape: AdjacencyShape) -> &T {
        match shape {
            AdjacencyShape::O => &self.o,
            AdjacencyShape::U => &self.u,
            AdjacencyShape::I => &self.i,
            AdjacencyShape::L => &self.l,
            AdjacencyShape::T => &self.t,
            AdjacencyShape::X => &self.x,
            AdjacencyShape::Default => &self.default,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AdjacencyShape {
    O,
    U,
    I,
    L,
    T,
    X,
    Default,
}

impl AdjacencyShape {
    /// Rotations (in clockwise quarter turns) that map the shape onto itself.
    fn symmetries(self) -> &'static [u8] {
        match self {
            AdjacencyShape::I => &[0, 2],
            AdjacencyShape::X => &[0, 1, 2, 3],
            _ => &[0],
        }
    }
}

/// Sides of a tile, relative to the base orientation of an adjacency variant model.
///
/// North is the model's +z axis, east its -x axis.
#[derive(Clone, Copy, Default, PartialEq, Eq, Reflect)]
#[reflect(Default)]
pub struct Sides {
    pub north: bool,
    pub east: bool,
    pub south: bool,
    pub west: bool,
}

impl Sides {
    fn bits(self) -> u8 {
        self.north as u8 | (self.east as u8) << 1 | (self.south as u8) << 2 | (self.west as u8) << 3
    }
}

/// Rotates a north, east, south, west bitmask clockwise by `turns` quarter turns.
fn rotate_sides(sides: u8, turns: u8) -> u8 {
    ((sides << turns) | (sides >> (4 - turns))) & 0b1111
}

/// A variant of an adjacency shape that is closed off on some of its connected sides.
#[derive(Clone, Default, Reflect)]
#[reflect(Default)]
pub struct CappedMesh {
    pub caps: Sides,
    pub mesh: Handle<Mesh>,
}

#[derive(Clone, Default, Reflect)]
#[reflect(Default)]
pub struct CappedVariants {
    pub u: Vec<CappedMesh>,
    pub i: Vec<CappedMesh>,
    pub l: Vec<CappedMesh>,
    pub t: Vec<CappedMesh>,
    pub x: Vec<CappedMesh>,
}

impl CappedVariants {
    fn get_shape(&self, shape: AdjacencyShape) -> &[CappedMesh] {
        match shape {
            AdjacencyShape::U => &self.u,
            AdjacencyShape::I => &self.i,
            AdjacencyShape::L => &self.l,
            AdjacencyShape::T => &self.t,
            AdjacencyShape::X => &self.x,
            _ => &[],
        }
    }
}

/// Meshes for every shape a tile object can take, including variants with capped sides.
#[derive(Clone, Default, Reflect)]
#[reflect(Default)]
pub struct AdjacencyMeshes {
    pub variants: AdjacencyVariants<Handle<Mesh>>,
    pub capped: CappedVariants,
}

impl AdjacencyMeshes {
    pub fn get(&self, adjacency: AdjacencyInformation) -> (Handle<Mesh>, Quat) {
        let (shape, dir) = adjacency.shape();
        let dir = dir as u8;
        let caps = (0..4u8)
            .filter(|side| adjacency.capped & (1 << ((side + dir) % 4)) != 0)
            .fold(0, |caps, side| caps | 1 << side);

        if caps != 0 {
            for capped in self.capped.get_shape(shape) {
                for &turns in shape.symmetries() {
                    if rotate_sides(capped.caps.bits(), turns) == caps {
                        return (
                            capped.mesh.clone(),
                            AdjacencyInformation::rotation_from_dir(
                                DIRECTIONS[((dir + turns) % 4) as usize],
                            ),
                        );
                    }
                }
            }
        }

        (
            self.variants.get_shape(shape).clone(),
            AdjacencyInformation::rotation_from_dir(DIRECTIONS[dir as usize]),
        )
    }
}

pub struct Surrounded<T> {
    data: [Option<T>; 4],
}

impl<T> Surrounded<T> {
    const fn index(direction: Direction) -> usize {
        match direction {
            Direction::North => 0,
            Direction::East => 1,
            Direction::South => 2,
            Direction::West => 3,
        }
    }

    pub fn get(&self, direction: Direction) -> Option<&T> {
        self.data[Self::index(direction)].as_ref()
    }
}

/// Stores in what directions an object is surrounded.
#[derive(Default, Clone, Copy)]
pub struct AdjacencyInformation {
    /// Represents a bitfield of the neighbours presence.
    /// Starts at the tile right above and goes clockwise.
    directions: u8,
    /// Bitfield of the connected sides that need to be closed off, indexed by [`Direction`].
    capped: u8,
}

macro_rules! match_pattern {
    ($val:expr, $pat:literal) => {{
        const PAT: u8 = $pat;
        const PATR: u8 = AdjacencyInformation::rotate_right(PAT);
        const PATRR: u8 = AdjacencyInformation::rotate_right(PATR);
        const PATRRR: u8 = AdjacencyInformation::rotate_right(PATRR);
        match ($val) & 0b10101010 {
            PAT => Some(Direction::North),
            PATR => Some(Direction::East),
            #[allow(unreachable_patterns)]
            PATRR => Some(Direction::South),
            #[allow(unreachable_patterns)]
            PATRRR => Some(Direction::West),
            _ => None,
        }
    }};
}

impl AdjacencyInformation {
    pub fn add(&mut self, direction: Direction) {
        let add = match direction {
            Direction::North => 0b10000000,
            Direction::East => 0b00100000,
            Direction::South => 0b00001000,
            Direction::West => 0b00000010,
        };
        self.directions |= add;
    }

    pub fn add_capped(&mut self, direction: Direction) {
        self.add(direction);
        self.capped |= 1 << direction as u8;
    }

    /// Returns the shape and the direction its base orientation's north side is facing.
    pub fn shape(&self) -> (AdjacencyShape, Direction) {
        if self.is_o() {
            (AdjacencyShape::O, Direction::South)
        } else if let Some(dir) = self.is_u() {
            (AdjacencyShape::U, dir)
        } else if let Some(dir) = self.is_i() {
            (AdjacencyShape::I, dir)
        } else if let Some(dir) = self.is_l() {
            (AdjacencyShape::L, dir)
        } else if let Some(dir) = self.is_t() {
            (AdjacencyShape::T, dir)
        } else if self.is_x() {
            (AdjacencyShape::X, Direction::South)
        } else {
            (AdjacencyShape::Default, Direction::South)
        }
    }

    pub fn is_o(&self) -> bool {
        // For now we ignore diagonals
        self.directions & 0b10101010 == 0
    }

    pub fn is_u(&self) -> Option<Direction> {
        match_pattern!(self.directions, 0b10000000)
    }

    pub fn is_l(&self) -> Option<Direction> {
        match_pattern!(self.directions, 0b10100000)
    }

    pub fn is_t(&self) -> Option<Direction> {
        match_pattern!(self.directions, 0b10100010)
    }

    pub fn is_i(&self) -> Option<Direction> {
        match_pattern!(self.directions, 0b10001000)
    }

    pub fn is_x(&self) -> bool {
        // For now we ignore diagonals
        self.directions & 0b10101010 == 0b10101010
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

    /// Rotates the directional map by 90 degrees clockwise
    const fn rotate_right(directions: u8) -> u8 {
        // Move the lowest two bits all the way to the left
        ((directions & 0b11) << 6)
            // Combine it with the rest shifted to the right
            | (directions >> 2)
    }
}

/// Defines how a tile object fits together with others.
///
/// The rules are shared between all instances spawned from the same scene.
#[derive(Component, Reflect, Clone)]
#[reflect(Component, Clone, FromTemplate)]
pub(crate) struct TilemapAdjacency(pub Arc<AdjacencyRules>);

impl FromTemplate for TilemapAdjacency {
    type Template = AdjacencyRules;
}

#[derive(Reflect, Clone, Default)]
#[reflect(Default, Template, SharedTemplate)]
pub(crate) struct AdjacencyRules {
    /// Objects of the same category connect seamlessly.
    pub category: String,
    /// Other categories this object connects to, with its connected sides capped.
    /// Connections are mutual, so only one of both sides needs to list the other.
    pub connects_to: Vec<String>,
    pub meshes: AdjacencyMeshes,
}

impl AdjacencyRules {
    /// Returns whether objects of `category` connect to this one, and if so, whether seamlessly.
    pub fn connection(&self, category: &str, connects_to: &[String]) -> Option<Connection> {
        if self.category == category {
            Some(Connection::Seamless)
        } else if self.connects_to.iter().any(|c| c == category)
            || connects_to.contains(&self.category)
        {
            Some(Connection::Capped)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Connection {
    Seamless,
    Capped,
}

// Opts out of the blanket `Template` impl for `Clone + Default` types, like `Handle<T>` does.
impl Unpin for AdjacencyRules where for<'a> [()]: SpecializeFromTemplate {}

impl Template for AdjacencyRules {
    type Output = TilemapAdjacency;

    fn build_template(&self, _context: &mut TemplateContext) -> Result<Self::Output> {
        Ok(TilemapAdjacency(Arc::new(self.clone())))
    }

    fn clone_template(&self) -> Self {
        self.clone()
    }
}

/// Child of a [`TilemapAdjacency`] whose mesh follows the shape chosen for its parent.
#[derive(Component, Reflect, Clone)]
#[reflect(Component, Clone, FromTemplate)]
pub(crate) struct AdjacencyMesh(pub Arc<AdjacencyMeshes>);

impl FromTemplate for AdjacencyMesh {
    type Template = AdjacencyMeshTemplate;
}

#[derive(Reflect, Clone, Default)]
#[reflect(Default, Template, SharedTemplate)]
pub(crate) struct AdjacencyMeshTemplate {
    pub meshes: AdjacencyMeshes,
}

impl Unpin for AdjacencyMeshTemplate where for<'a> [()]: SpecializeFromTemplate {}

impl Template for AdjacencyMeshTemplate {
    type Output = AdjacencyMesh;

    fn build_template(&self, _context: &mut TemplateContext) -> Result<Self::Output> {
        Ok(AdjacencyMesh(Arc::new(self.meshes.clone())))
    }

    fn clone_template(&self) -> Self {
        self.clone()
    }
}

/// Lets tile objects of a category connect to the two sides along this one's local x axis.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub(crate) struct AdjacencyConnector {
    pub category: String,
}

/// Child of an [`AdjacencyConnector`] that is only visible if something connects on its side.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub(crate) struct ConnectorFrame {
    /// Whether the frame is on the local positive x side
    pub positive: bool,
}
