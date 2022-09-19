use adjacency::{AdjacencyInformation, TilemapAdjacency};
use bevy::{
    asset::AssetPathId,
    math::{IVec2, UVec2},
    prelude::*,
    reflect::TypeUuid,
    scene::{InstanceId, SceneInstance},
    utils::{HashMap, HashSet},
};
use enum_map::EnumMap;
use networking::{
    component::{AppExt, ComponentSystem},
    identity::EntityCommandsExt,
    transform::NetworkTransform,
    visibility::{GridAabb, VisibilitySystem, GLOBAL_GRID_CELL_SIZE},
    NetworkManager,
};
use serde::{Deserialize, Serialize};

pub use enum_map::enum_map;

mod adjacency;

#[derive(Component)]
pub struct TileMap {
    // Size in chunks
    size: UVec2,
    chunks: Vec<Option<Box<Chunk>>>,
    pub spawn_position: UVec2,
}

impl TileMap {
    pub fn new(size: UVec2) -> Self {
        let mut chunks = Vec::new();
        chunks.resize_with((size.x * size.y) as usize, Default::default);
        Self {
            size,
            chunks,
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

// TODO: Do with proc macro
impl networking::component::NetworkedToClient for TileMap {
    type Param = ();

    fn receiver_matters() -> bool {
        false
    }

    fn serialize(
        &mut self,
        _: &(),
        _: Option<networking::ConnectionId>,
        since_tick: Option<std::num::NonZeroU32>,
    ) -> Option<networking::component::Bytes> {
        // Only serialize once per client
        if since_tick.is_some() {
            None
        } else {
            Some(networking::component::Bytes::new())
        }
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

#[derive(enum_map::Enum, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TileLayer {
    Turf,
    Furniture,
}

/// Data which can be used to spawn a [`TileMap`]
#[derive(Component)]
pub struct TileMapData {
    /// Size in tiles
    pub size: UVec2,
    pub tiles: Vec<TileData>,
    pub spawn_position: UVec2,
}

impl TileMapData {
    fn size_in_chunks(&self) -> UVec2 {
        (self.size.as_vec2() / UVec2::new(CHUNK_SIZE, CHUNK_SIZE).as_vec2())
            .ceil()
            .as_uvec2()
    }
}

#[derive(Default)]
pub struct TileData {
    /// A reference to the turf asset
    //pub turf: Option<AssetPathId>,
    /// A reference to the furniture asset
    //pub furniture: Option<AssetPathId>,
    pub layers: EnumMap<TileLayer, Option<AssetPathId>>,
}

/// Points to the entities making up a tile at runtime
#[derive(Default, Clone, Copy)]
pub struct TileReference {
    pub layers: EnumMap<TileLayer, Option<Entity>>,
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
}

/// Attached to an entity that is a part of a tile.
#[derive(Component)]
struct TileEntity {
    tilemap: networking::component::NetworkVar<Entity>,
    position: networking::component::NetworkVar<UVec2>,
    layer: networking::component::NetworkVar<TileLayer>,
}

// TODO: Do with proc macro
impl networking::component::NetworkedToClient for TileEntity {
    type Param = Res<'static, networking::identity::NetworkIdentities>;

    fn receiver_matters() -> bool {
        false
    }

    fn serialize(
        &mut self,
        param: &bevy::prelude::Res<'_, networking::identity::NetworkIdentities>,
        _: Option<networking::ConnectionId>,
        since_tick: Option<std::num::NonZeroU32>,
    ) -> Option<networking::component::Bytes> {
        let mut writer =
            networking::component::BufMut::writer(networking::component::BytesMut::with_capacity(
                std::mem::size_of::<Option<networking::component::ValueUpdate<Entity>>>(),
            ));
        let mut serializer = networking::component::ComponentSerializer::new(
            &mut writer,
            networking::component::serializer_options(),
        );

        let tilemap_changed = since_tick
            .map(|t| self.tilemap.has_changed_since(t.into()))
            .unwrap_or(true);
        serde::Serialize::serialize(
            &tilemap_changed.then(|| {
                let identity = param
                    .get_identity(*self.tilemap)
                    .expect("Tilemap entity must have network identity");
                networking::component::ValueUpdate::from(identity)
            }),
            &mut serializer,
        )
        .unwrap();

        let position_changed = since_tick
            .map(|t| self.position.has_changed_since(t.into()))
            .unwrap_or(true);
        serde::Serialize::serialize(
            &position_changed.then(|| networking::component::ValueUpdate::from(*self.position)),
            &mut serializer,
        )
        .unwrap();

        let layer_changed = since_tick
            .map(|t| self.layer.has_changed_since(t.into()))
            .unwrap_or(true);
        serde::Serialize::serialize(
            &layer_changed.then(|| networking::component::ValueUpdate::from(*self.layer)),
            &mut serializer,
        )
        .unwrap();

        Some(writer.into_inner().into())
    }
}

#[derive(Default, Component, TypeUuid)]
#[uuid = "02de843e-5491-4989-9991-60055d333a4b"]
struct TileEntityClient {
    tilemap: networking::component::ServerVar<Entity>,
    position: networking::component::ServerVar<UVec2>,
    old_position: Option<UVec2>,
    layer: networking::component::ServerVar<TileLayer>,
}

// TODO: Do with proc macro
impl networking::component::NetworkedFromServer for TileEntityClient {
    type Param = Res<'static, networking::identity::NetworkIdentities>;

    fn deserialize<'w, 's>(
        &mut self,
        param: &<<Self::Param as bevy::ecs::system::SystemParam>::Fetch as bevy::ecs::system::SystemParamFetch<'w, 's>>::Item,
        data: &[u8],
    ) {
        let mut deserializer = networking::component::ComponentDeserializer::with_reader(
            networking::component::Buf::reader(data),
            networking::component::serializer_options(),
        );
        let tilemap_update = Option::<
            networking::component::ValueUpdate<networking::identity::NetworkIdentity>,
        >::deserialize(&mut deserializer)
        .expect("Error deserializing networked component");
        if let Some(tilemap_update) = tilemap_update {
            let identity = tilemap_update.0.into_owned();
            let entity = param
                .get_entity(identity)
                .unwrap_or_else(|| panic!("Tilemap root network id ({:?}) should exist", identity));
            self.tilemap.set(entity);
        }

        let position_update =
            Option::<networking::component::ValueUpdate<UVec2>>::deserialize(&mut deserializer)
                .expect("Error deserializing networked component");
        if let Some(position_update) = position_update {
            self.old_position = self.position.get().cloned();
            self.position.set(position_update.0.into_owned());
        }

        let layer_update =
            Option::<networking::component::ValueUpdate<TileLayer>>::deserialize(&mut deserializer)
                .expect("Error deserializing networked component");
        if let Some(layer_update) = layer_update {
            self.layer.set(layer_update.0.into_owned());
        }
        // TODO: Debug assert that we've consumed all data
    }

    fn default_if_missing() -> Option<Box<Self>> {
        Some(Box::new(Default::default()))
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
        for (data_index, tile_data) in data.tiles.iter().enumerate() {
            let y = data_index as u32 / data.size.x;
            let x = data_index as u32 - y * data.size.x;

            let mut tile_ref = TileReference::default();

            // Spawn tile entities for each layer
            for (layer, asset_path) in tile_data
                .layers
                .iter()
                .filter_map(|(l, o)| o.map(|path| (l, path)))
            {
                let scene = server.get_handle(asset_path);
                let entity = commands.entity(map_entity).add_children(|builder| {
                    builder
                        .spawn_bundle(DynamicSceneBundle {
                            scene,
                            transform: Transform::from_translation(
                                (x as f32, 0.0, y as f32).into(),
                            ),
                            ..Default::default()
                        })
                        .insert(TileEntity {
                            tilemap: map_entity.into(),
                            position: UVec2::new(x, y).into(),
                            layer: layer.into(),
                        })
                        .networked()
                        .id()
                });
                tile_ref.layers[layer] = Some(entity);
            }

            map.set_tile((x, y).into(), tile_ref).unwrap();
        }

        commands
            .entity(map_entity)
            .insert(map)
            .insert(GridAabb::default())
            .insert_bundle(SpatialBundle::default()) // TODO: Remove, just testing
            .insert(NetworkTransform::default());
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

// TODO: Remove once scenes support composition
/// Adds some bundles to spawned tile scenes, so we don't need to specify them every time
fn client_initialize_tile_objects(
    spawned: Query<(&SceneInstance, &Handle<DynamicScene>), Added<SceneInstance>>,
    mut loading: Local<Vec<InstanceId>>,
    spawner: Res<SceneSpawner>,
    existing_meshes: Query<&Handle<Mesh>>,
    assets: Res<MapAssets>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    for (instance, handle) in spawned.iter() {
        // Check if this scene is a tilemap entity
        if !asset_server
            .get_handle_path(handle)
            .map(|p| p.path().starts_with("tilemap/"))
            .unwrap_or(false)
        {
            continue;
        }

        loading.push(**instance);
    }

    loading.retain(|instance| {
        if let Some(entities) = spawner.iter_instance_entities(*instance) {
            if let Some(assets) = assets.client.as_ref() {
                for entity in entities {
                    if let Ok(mesh) = existing_meshes.get(entity) {
                        commands.entity(entity).insert_bundle(PbrBundle {
                            mesh: mesh.clone(),
                            material: assets.default_material.clone(),
                            ..Default::default()
                        });
                    }
                }
            }
            false
        } else {
            true
        }
    });
}

/// Stores a subset of tile map information on the client.
#[derive(Default, Component, TypeUuid)]
#[uuid = "9036e9c7-f3c4-478e-81ed-3084e52d2253"]
struct TileMapClient {
    tiles: HashMap<UVec2, TileReference>,
    dirty_tiles: HashSet<(UVec2, TileLayer)>,
}

// TODO: Do with proc macro
impl networking::component::NetworkedFromServer for TileMapClient {
    type Param = ();

    fn deserialize<'w, 's>(&mut self, _: &(), _: &[u8]) {
        // No data
    }

    fn default_if_missing() -> Option<Box<Self>> {
        Some(Box::new(Default::default()))
    }
}

fn client_update_tile_entities(
    changed_tiles: Query<(Entity, &Parent, &TileEntityClient), Changed<TileEntityClient>>,
    mut tilemaps: Query<&mut TileMapClient>,
    mut commands: Commands,
) {
    for (entity, parent, tile_entity) in changed_tiles.iter() {
        let tile_position = *tile_entity.position;
        if Some(tile_position) != tile_entity.old_position {
            // Update position in world
            commands
                .entity(parent.get())
                .insert_bundle(SpatialBundle::from_transform(Transform::from_translation(
                    Vec3::new(tile_position.x as f32, 0.0, tile_position.y as f32),
                )));

            // Update position in index
            let mut tilemap = tilemaps.get_mut(*tile_entity.tilemap).unwrap();
            tilemap.tiles.entry(tile_position).or_default().layers[*tile_entity.layer] =
                Some(entity);
            tilemap
                .dirty_tiles
                .insert((tile_position, *tile_entity.layer));
            // TODO: Remove from old position
        }
        // TODO: Handle all changes of tilemap parent and layer

        commands
            .entity(*tile_entity.tilemap)
            .add_child(parent.get());
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
            for direction in DIRECTIONS {
                let position = dirty_position.as_ivec2() + IVec2::from(direction);
                // Check for out-of-bounds
                if position.min_element() < 0 {
                    continue;
                }

                let adjacent_tile = match tilemap.tiles.get(&position.as_uvec2()) {
                    Some(t) => t,
                    None => continue,
                };

                let tile_entity = match adjacent_tile.layers[layer] {
                    Some(t) => t,
                    None => continue,
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
                        if let Some(adjacent_entity) = tile_ref.layers[layer] {
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
            app.add_system(client_initialize_tile_objects)
                .add_system_to_stage(
                    CoreStage::PostUpdate,
                    client_update_tile_entities.after(ComponentSystem::Apply),
                )
                .add_system(client_update_adjacencies.after(client_update_tile_entities));
        } else {
            app.add_system(spawn_from_data).add_system_to_stage(
                CoreStage::PostUpdate,
                update_grid_aabb.before(VisibilitySystem::UpdateGrid),
            );
        }
    }
}
