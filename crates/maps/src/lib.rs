use adjacency::{AdjacencyInformation, TilemapAdjacency};
use arrayvec::ArrayVec;
use bevy::{
    asset::AssetPathId,
    ecs::system::Command,
    math::{IVec2, UVec2},
    prelude::*,
    reflect::TypeUuid,
    utils::{HashMap, HashSet},
};
use networking::{
    component::AppExt,
    identity::{EntityCommandsExt, NetworkIdentities, NetworkIdentity},
    scene::NetworkSceneBundle,
    spawning::{NetworkedEntityEvent, SpawningSystems},
    transform::NetworkTransform,
    variable::{NetworkVar, ServerVar},
    visibility::{GridAabb, VisibilitySystem, GLOBAL_GRID_CELL_SIZE},
    NetworkManager, Networked,
};
use serde::{Deserialize, Serialize};

pub use enum_map::enum_map;

mod adjacency;
pub use adjacency::Surrounded;

#[derive(Component, Networked)]
#[networked(client = "TileMapClient", priority = 10)]
pub struct TileMap {
    // Size in chunks
    size: UVec2,
    chunks: Vec<Option<Box<Chunk>>>,
    pub job_spawn_positions: HashMap<String, Vec<UVec2>>,
}

impl TileMap {
    pub fn new(size: UVec2) -> Self {
        let mut chunks = Vec::new();
        chunks.resize_with((size.x * size.y) as usize, Default::default);
        Self {
            size,
            chunks,
            job_spawn_positions: Default::default(),
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

    pub fn iter_tiles(&mut self) -> impl Iterator<Item = (UVec2, &TileReference)> {
        let size = self.size;
        self.iter_chunks()
            .map(move |(p, c)| {
                (
                    Self::position_from_chunk_index(size, p) * UVec2::new(CHUNK_SIZE, CHUNK_SIZE),
                    c,
                )
            })
            .flat_map(|(p, c)| {
                c.tiles.iter().enumerate().map(move |(i, t)| {
                    let y = i as u32 / CHUNK_SIZE;
                    let x = match y {
                        0 => i as u32,
                        _ => i as u32 % (y * CHUNK_SIZE),
                    };
                    (p + UVec2::new(x, y), t)
                })
            })
    }

    pub fn tile(&self, position: UVec2) -> Option<&TileReference> {
        let chunk_index = self.index_from_position(position);
        let position_in_chunk = self.position_inside_chunk(position);
        Some(
            self.chunks
                .get(chunk_index)?
                .as_ref()?
                .tile(position_in_chunk),
        )
    }

    pub fn tile_mut(&mut self, position: UVec2) -> Option<&mut TileReference> {
        let chunk_index = self.index_from_position(position);
        let position_in_chunk = self.position_inside_chunk(position);
        self.chunks
            .get_mut(chunk_index)?
            .as_mut()?
            .tile_mut(position_in_chunk)
            .into()
    }

    #[allow(clippy::result_unit_err)]
    pub fn set_tile(&mut self, position: UVec2, data: TileReference) -> Result<(), ()> {
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
}

pub const CHUNK_SIZE: u32 = 16;
const CHUNK_LENGTH: usize = (CHUNK_SIZE * CHUNK_SIZE) as usize;

pub struct Chunk {
    tiles: [TileReference; CHUNK_LENGTH],
    changed_tiles: [bool; CHUNK_LENGTH],
    changed: bool,
}

impl Default for Chunk {
    fn default() -> Self {
        Self {
            tiles: [TileReference::default(); CHUNK_LENGTH],
            changed_tiles: [false; CHUNK_LENGTH],
            changed: false,
        }
    }
}

impl Chunk {
    fn tile(&self, position: UVec2) -> &TileReference {
        let index = Self::index_from_position(position);
        assert!(index < CHUNK_LENGTH);

        &self.tiles[index]
    }

    fn tile_mut(&mut self, position: UVec2) -> &mut TileReference {
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
    let position = position.as_ivec2();
    DIRECTIONS
        .iter()
        .map(move |&dir| {
            let o: IVec2 = dir.into();
            (dir, position + o)
        })
        .filter(|(_, p)| p.x >= 0 && p.y >= 0)
        .map(|(dir, p)| (dir, p.as_uvec2()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    North = 0,
    East,
    South,
    West,
}

impl Direction {
    fn rotate_around(self, axis: Vec3) -> Quat {
        Quat::from_axis_angle(axis, std::f32::consts::FRAC_PI_2 * (self as u8 as f32))
    }
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

impl TryFrom<usize> for Direction {
    type Error = ();

    fn try_from(value: usize) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::North),
            1 => Ok(Self::East),
            2 => Ok(Self::South),
            3 => Ok(Self::West),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TileLayer {
    Turf,
    Furniture,
    HighMount,
}

impl TileLayer {
    fn default_offset(&self) -> Vec3 {
        match self {
            TileLayer::Furniture | TileLayer::Turf => Vec3::ZERO,
            TileLayer::HighMount => Vec3::new(0.0, 2.0, 0.0),
        }
    }
}

/// Uniquely references a tile entity in a [`TileMap`].
///
/// ## Remarks
/// This path is no longer valid if the [tilemap's](TileMap) dimensions change, as it contains an index inside the map.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
struct TileEntityPath {
    position: UVec2,
    layer: TileLayer,
    index_in_layer: Option<u8>,
}

/// Data which can be used to spawn a [`TileMap`]
#[derive(Component)]
pub struct TileMapData {
    /// Size in tiles
    pub size: UVec2,
    pub tiles: Vec<TileData>,
    pub job_spawn_positions: HashMap<String, Vec<UVec2>>,
}

impl TileMapData {
    fn size_in_chunks(&self) -> UVec2 {
        (self.size.as_vec2() / UVec2::new(CHUNK_SIZE, CHUNK_SIZE).as_vec2())
            .ceil()
            .as_uvec2()
    }
}

#[derive(Debug)]
enum TileLayerData<T> {
    Single(Option<T>),
    Directional([Option<T>; DIRECTIONS.len()]),
}

impl<T> From<Option<T>> for TileLayerData<T> {
    fn from(v: Option<T>) -> Self {
        Self::Single(v)
    }
}

impl<T> From<[Option<T>; DIRECTIONS.len()]> for TileLayerData<T> {
    fn from(v: [Option<T>; DIRECTIONS.len()]) -> Self {
        Self::Directional(v)
    }
}

/// The makeup of a tile that can be spawned into the world.
#[derive(Default)]
pub struct TileData {
    pub turf: Option<AssetPathId>,
    pub furniture: Option<AssetPathId>,
    pub high_mounts: [Option<AssetPathId>; 4],
}

impl TileData {
    fn layers(&self) -> impl Iterator<Item = (TileLayer, TileLayerData<AssetPathId>)> {
        [
            (TileLayer::Turf, TileLayerData::Single(self.turf)),
            (TileLayer::Furniture, TileLayerData::Single(self.furniture)),
            (
                TileLayer::HighMount,
                TileLayerData::Directional(self.high_mounts),
            ),
        ]
        .into_iter()
    }
}

/// Points to the entities making up a tile at runtime
#[derive(Default, Clone, Copy)]
pub struct TileReference {
    pub turf: Option<Entity>,
    pub furniture: Option<Entity>,
    pub high_mounts: [Option<Entity>; 4],
}

impl TileReference {
    pub fn position_in_chunk(index: usize) -> UVec2 {
        let y = index as u32 / CHUNK_SIZE;
        let x = match y {
            0 => index as u32,
            _ => index as u32 % (y * CHUNK_SIZE),
        };
        UVec2::new(x, y)
    }

    fn get(&self, layer: TileLayer) -> TileLayerData<Entity> {
        match layer {
            TileLayer::Turf => self.turf.into(),
            TileLayer::Furniture => self.furniture.into(),
            TileLayer::HighMount => self.high_mounts.into(),
        }
    }

    fn set(&mut self, layer: TileLayer, data: TileLayerData<Entity>) {
        match (layer, data) {
            (TileLayer::Turf, TileLayerData::Single(v)) => self.turf = v,
            (TileLayer::Furniture, TileLayerData::Single(v)) => self.furniture = v,
            (TileLayer::HighMount, TileLayerData::Directional(v)) => self.high_mounts = v,
            (layer, data) => panic!(
                "Invalid combination of layer '{:?}' and data format '{:?}'",
                layer, data
            ),
        }
    }

    fn set_index(&mut self, layer: TileLayer, index: usize, value: Option<Entity>) {
        match layer {
            TileLayer::Turf | TileLayer::Furniture => panic!(
                "Can't set index on tile layer '{:?}' with single slot",
                layer
            ),
            TileLayer::HighMount => {
                self.high_mounts[index] = value;
            }
        }
    }

    fn remove_at(&mut self, path: TileEntityPath) {
        match path.index_in_layer {
            Some(i) => {
                self.set_index(path.layer, i as usize, None);
            }
            None => {
                self.set(path.layer, TileLayerData::Single(None));
            }
        }
    }
}

// TODO: Also implement default serializations for some types (like Entity)
/// Attached to an entity that is a part of a tile.
#[derive(Component, Networked)]
#[networked(client = "TileEntityClient")]
struct TileEntity {
    #[networked(
        with = "Self::network_tilemap(Res<'static, NetworkIdentities>) -> NetworkIdentity"
    )]
    tilemap: NetworkVar<Entity>,
    path: NetworkVar<TileEntityPath>,
}

impl TileEntity {
    fn network_tilemap(entity: &Entity, param: Res<NetworkIdentities>) -> NetworkIdentity {
        param
            .get_identity(*entity)
            .expect("Tilemap entity must have network identity")
    }
}

#[derive(Default, Component, TypeUuid, Networked)]
#[uuid = "02de843e-5491-4989-9991-60055d333a4b"]
#[networked(server = "TileEntity")]
struct TileEntityClient {
    #[networked(
        with = "NetworkIdentity -> Self::network_tilemap(Res<'static, NetworkIdentities>)"
    )]
    tilemap: ServerVar<Entity>,

    #[networked(updated = "Self::on_path_update")]
    path: ServerVar<TileEntityPath>,
    old_path: Option<TileEntityPath>,
}

impl TileEntityClient {
    fn network_tilemap(identity: NetworkIdentity, param: Res<NetworkIdentities>) -> Entity {
        param
            .get_entity(identity)
            .unwrap_or_else(|| panic!("Tilemap root network id ({:?}) should exist", identity))
    }

    fn on_path_update(&mut self, _: &TileEntityPath) {
        self.old_path = self.path.get().cloned();
    }
}

/// Creates a tilemap from data and spawns the tile objects into the world
fn spawn_from_data(
    query: Query<(Entity, &TileMapData), Without<TileMap>>,
    mut commands: Commands,
    server: ResMut<AssetServer>,
) {
    for (map_entity, data) in query.iter() {
        let mut map = TileMap::new(data.size_in_chunks());
        map.job_spawn_positions = data.job_spawn_positions.clone();

        for (data_index, tile_data) in data.tiles.iter().enumerate() {
            let y = data_index as u32 / data.size.x;
            let x = data_index as u32 - y * data.size.x;

            let mut tile_ref = TileReference::default();

            // Spawn tile entities for each layer
            for (layer, layer_data) in tile_data.layers() {
                let mut spawn_object =
                    |asset_path, index_in_layer, direction: Direction| -> Entity {
                        let scene = server.get_handle(asset_path);
                        commands.entity(map_entity).add_children(|builder| {
                            builder
                                .spawn((
                                    NetworkSceneBundle {
                                        scene: scene.into(),
                                        transform: Transform {
                                            translation: Vec3::new(x as f32, 0.0, y as f32)
                                                + layer.default_offset(),
                                            rotation: direction.rotate_around(Vec3::Y),
                                            ..Default::default()
                                        },
                                        ..Default::default()
                                    },
                                    TileEntity {
                                        tilemap: map_entity.into(),
                                        path: TileEntityPath {
                                            position: UVec2::new(x, y),
                                            layer,
                                            index_in_layer,
                                        }
                                        .into(),
                                    },
                                ))
                                .networked()
                                .id()
                        })
                    };

                match layer_data {
                    TileLayerData::Single(Some(p)) => {
                        let entity = spawn_object(p, None, Direction::North);
                        tile_ref.set(layer, TileLayerData::Single(Some(entity)));
                    }
                    TileLayerData::Directional(paths) => {
                        let refs = paths
                            .iter()
                            .enumerate()
                            .map(|(i, p)| {
                                p.map(|p| spawn_object(p, Some(i as u8), i.try_into().unwrap()))
                            })
                            .collect::<ArrayVec<_, 4>>()
                            .into_inner()
                            .unwrap();
                        tile_ref.set(layer, TileLayerData::Directional(refs));
                    }
                    _ => continue,
                };
            }

            map.set_tile((x, y).into(), tile_ref).unwrap();
        }

        commands.entity(map_entity).insert((
            map,
            GridAabb::default(),
            SpatialBundle::default(),
            NetworkTransform::default(),
        ));
        info!("Spawned tiles for map (entity={:?})", map_entity);
    }
}

/// Sets the tilemap entities visibility size for networking
fn update_grid_aabb(mut query: Query<(&TileMap, &mut GridAabb), Changed<TileMap>>) {
    for (map, mut aabb) in query.iter_mut() {
        let grid_size = map.size * CHUNK_SIZE / GLOBAL_GRID_CELL_SIZE as u32;
        let half_extents = grid_size / 2u32;
        let new_aabb = GridAabb {
            size: half_extents,
            center: half_extents.as_ivec2(),
        };
        if &new_aabb != aabb.as_ref() {
            *aabb = new_aabb;
        }
    }
}

pub trait MapCommandsExt {
    fn despawn_tile_entity(&mut self, entity: Entity);
}

impl<'w, 's> MapCommandsExt for Commands<'w, 's> {
    fn despawn_tile_entity(&mut self, entity: Entity) {
        self.add(DespawnTileEntityCommand { entity });
        self.entity(entity).despawn_recursive();
    }
}

struct DespawnTileEntityCommand {
    entity: Entity,
}

impl Command for DespawnTileEntityCommand {
    fn write(self, world: &mut World) {
        if let Some(tile) = world.entity_mut(self.entity).remove::<TileEntity>() {
            let path = *tile.path;
            if let Some(mut map) = world.get_mut::<TileMap>(*tile.tilemap) {
                if let Some(reference) = map.tile_mut(path.position) {
                    reference.remove_at(path);
                }
            }
        }
    }
}

// TODO: Remove once scenes support composition
/// Adds some bundles to spawned tile scenes, so we don't need to specify them every time
fn client_initialize_tile_objects(
    new: Query<Entity, Added<TileEntityClient>>,
    children_query: Query<&Children>,
    existing_meshes: Query<(&Handle<Mesh>, Option<&Transform>)>,
    tile_entities: Query<&TileEntityClient>,
    mut tilemaps: Query<&mut TileMapClient>,
    assets: Res<MapAssets>,
    mut commands: Commands,
) {
    let Some(assets) = assets.client.as_ref() else {
        return;
    };

    let mut process_entity = |entity| {
        if let Ok((mesh, transform)) = existing_meshes.get(entity) {
            commands.entity(entity).insert(PbrBundle {
                mesh: mesh.clone(),
                material: assets.default_material.clone(),
                transform: transform.cloned().unwrap_or_default(),
                ..Default::default()
            });
        }
    };

    for root in new.iter() {
        process_entity(root);
        for child in children_query.iter_descendants(root) {
            process_entity(child);
        }

        // Add to dirty tiles for adjacency
        let Ok(tile) = tile_entities.get(root) else {
            continue;
        };
        let mut map = tilemaps.get_mut(*tile.tilemap).unwrap();
        let path = &*tile.path;
        map.dirty_tiles.insert((path.position, path.layer));
    }
}

/// Stores a subset of tile map information on the client.
#[derive(Default, Component, TypeUuid, Networked)]
#[uuid = "9036e9c7-f3c4-478e-81ed-3084e52d2253"]
#[networked(server = "TileMap")]
struct TileMapClient {
    tiles: HashMap<UVec2, TileReference>,
    dirty_tiles: HashSet<(UVec2, TileLayer)>,
}

impl TileMapClient {
    fn remove_at(&mut self, path: TileEntityPath) {
        let Some(entry) = self.tiles.get_mut(&path.position) else {
            return;
        };
        entry.remove_at(path);
        self.dirty_tiles.insert((path.position, path.layer));
    }
}

fn client_update_tile_entities(
    changed_tiles: Query<
        (Entity, &TileEntityClient, Option<&Transform>),
        Changed<TileEntityClient>,
    >,
    mut tilemaps: Query<&mut TileMapClient>,
    mut commands: Commands,
) {
    for (entity, tile_entity, transform) in changed_tiles.iter() {
        let tile_path = *tile_entity.path;
        if Some(tile_path) != tile_entity.old_path {
            // Update position in world
            let mut new_transform = transform.cloned().unwrap_or_default();
            new_transform.translation = tile_path.layer.default_offset();
            new_transform.translation.x += tile_path.position.x as f32;
            new_transform.translation.z += tile_path.position.y as f32;

            let direction: Direction = (tile_path.index_in_layer.unwrap_or_default() as usize)
                .try_into()
                .unwrap();
            // TODO: This will break if object gets moved
            new_transform.rotation *= direction.rotate_around(Vec3::Y);
            commands.entity(entity).insert(new_transform);

            let mut tilemap = tilemaps
                .get_mut(*tile_entity.tilemap)
                .expect("Tilemap component must exist");

            // Remove from old path
            if let Some(old_path) = tile_entity.old_path {
                tilemap.remove_at(old_path);
            }

            // Add to new path
            let entry = tilemap.tiles.entry(tile_path.position).or_default();
            match tile_path.index_in_layer {
                Some(i) => {
                    entry.set_index(tile_path.layer, i as usize, Some(entity));
                }
                None => {
                    entry.set(tile_path.layer, TileLayerData::Single(Some(entity)));
                }
            }

            tilemap
                .dirty_tiles
                .insert((tile_path.position, tile_path.layer));
        }
        // TODO: Handle all changes of tilemap parent and layer

        commands.entity(*tile_entity.tilemap).add_child(entity);
    }
}

fn client_mark_deleted_tile_entities(
    mut events: EventReader<NetworkedEntityEvent>,
    tile_entities: Query<&TileEntityClient>,
    mut tilemaps: Query<&mut TileMapClient>,
) {
    for entity in events.iter().filter_map(|e| match e {
        NetworkedEntityEvent::Spawned(_) => None,
        NetworkedEntityEvent::Despawned(e) => Some(*e),
    }) {
        let Ok(tile) = tile_entities.get(entity) else {
            continue;
        };

        let Ok(mut map) = tilemaps.get_mut(*tile.tilemap) else {
            continue;
        };

        map.remove_at(*tile.path);
    }
}

fn client_update_adjacencies(
    mut tilemaps: Query<&mut TileMapClient>,
    mut adjacents_mut: Query<(&TilemapAdjacency, &mut Handle<Mesh>, &mut Transform)>,
    adjacencies: Query<&TilemapAdjacency>,
) {
    for mut tilemap in tilemaps.iter_mut() {
        let tilemap = tilemap.as_mut();
        for (dirty_position, layer) in tilemap.dirty_tiles.drain() {
            for direction in DIRECTIONS
                .iter()
                .copied()
                .map(IVec2::from)
                .chain(std::iter::once(IVec2::ZERO))
            {
                let position = dirty_position.as_ivec2() + direction;
                // Check for out-of-bounds
                if position.min_element() < 0 {
                    continue;
                }

                let adjacent_tile = match tilemap.tiles.get(&position.as_uvec2()) {
                    Some(t) => t,
                    None => continue,
                };

                let tile_entity = match adjacent_tile.get(layer) {
                    TileLayerData::Single(Some(t)) => t,
                    _ => continue,
                };

                let (adjacency_settings, mut mesh_handle, mut transform) =
                    match adjacents_mut.get_mut(tile_entity) {
                        Ok(q) => q,
                        Err(_) => continue,
                    };

                let mut adjacency_info = AdjacencyInformation::default();
                for direction in DIRECTIONS {
                    let adjacent_position = position + IVec2::from(direction);
                    // Check for out-of-bounds
                    if adjacent_position.min_element() < 0 {
                        continue;
                    }

                    if let Some(tile_ref) = tilemap.tiles.get(&adjacent_position.as_uvec2()) {
                        // TODO: Support cross-layer checks
                        if let TileLayerData::Single(Some(adjacent_entity)) = tile_ref.get(layer) {
                            if let Ok(info) = adjacencies.get(adjacent_entity) {
                                if adjacency_settings.category == info.category {
                                    adjacency_info.add(direction);
                                }
                            }
                        }
                    }
                }

                let (handle, rotation) = adjacency_settings.meshes.get(adjacency_info);
                *mesh_handle = handle;
                transform.rotation = rotation;
            }
        }
    }
}

/// Stores strong references to all tilemap object assets.
/// This is so we can create handles from a path id, which doesn't load the assets by itself.
#[derive(Resource)]
struct MapAssets {
    #[allow(dead_code)]
    definitions: Vec<HandleUntyped>,
    client: Option<ClientMapAssets>,
}

struct ClientMapAssets {
    #[allow(dead_code)]
    models: Vec<HandleUntyped>,
    default_material: Handle<StandardMaterial>,
}

fn load_tilemap_assets(
    mut commands: Commands,
    server: ResMut<AssetServer>,
    network: Res<NetworkManager>,
) {
    let client_assets = network.is_client().then(|| ClientMapAssets {
        models: server
            .load_folder("models/tilemap")
            .expect("assets/models/tilemap is missing"),
        default_material: server.load("models/tilemap/walls windows.glb#Material0"),
    });

    let assets = MapAssets {
        definitions: server
            .load_folder("tilemap")
            .expect("assets/tilemap is missing"),
        client: client_assets,
    };
    commands.insert_resource(assets);
}

pub struct MapPlugin;

impl Plugin for MapPlugin {
    fn build(&self, app: &mut bevy::prelude::App) {
        app.add_startup_system(load_tilemap_assets)
            .register_type::<TilemapAdjacency>()
            .register_type::<adjacency::AdjacencyVariants<Handle<Mesh>>>()
            .add_networked_component::<TileEntity, TileEntityClient>()
            .add_networked_component::<TileMap, TileMapClient>();

        if app
            .world
            .get_resource::<NetworkManager>()
            .unwrap()
            .is_client()
        {
            app.add_system_to_stage(CoreStage::PostUpdate, client_initialize_tile_objects)
                .add_system(client_update_tile_entities)
                .add_system(client_mark_deleted_tile_entities.after(SpawningSystems::Spawn))
                .add_system_to_stage(CoreStage::PostUpdate, client_update_adjacencies);
        } else {
            app.add_system(spawn_from_data).add_system_to_stage(
                CoreStage::PostUpdate,
                update_grid_aabb.before(VisibilitySystem::UpdateGrid),
            );
        }
    }
}
