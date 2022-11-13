#![allow(clippy::type_complexity)]

mod admin;
mod camera;
mod components;
mod config;
mod items;
mod job;
mod movement;
mod physics;
mod round;
mod scene;
mod ui;

use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use admin::AdminPlugin;
use bevy::app::ScheduleRunnerSettings;
use bevy::asset::AssetPlugin;
use bevy::ecs::system::EntityCommands;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use bevy::scene::ScenePlugin;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use bevy_egui::EguiPlugin;
use bevy_inspector_egui::{WorldInspectorParams, WorldInspectorPlugin};
use bevy_rapier3d::plugin::{NoUserData, RapierPhysicsPlugin};
use bevy_rapier3d::prelude::{
    Collider, ColliderMassProperties, Damping, LockedAxes, ReadMassProperties, RigidBody, Velocity,
};
use byond::tgm::TgmLoader;
use camera::TopDownCamera;
use clap::{Parser, Subcommand};
use futures_lite::future;
use items::{
    containers::{Container, ContainerAccessor, ContainerQuery, ContainerWriter},
    Item,
};
use maps::TileMapData;
use networking::identity::EntityCommandsExt as NetworkingEntityCommandsExt;
use networking::spawning::{ClientControlled, NetworkedEntityEvent, PrefabPath};
use networking::transform::NetworkedTransform;
use networking::{ClientEvent, NetworkRole, NetworkingPlugin};

/// How many ticks the server runs per second
const SERVER_TPS: u32 = 60;

#[derive(Parser)]
struct Args {
    #[clap(subcommand)]
    command: Option<ArgCommands>,
}

#[derive(Subcommand)]
enum ArgCommands {
    /// host a server
    Host {
        #[clap(default_value_t = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(0, 0, 0, 0), 33998u16)))]
        bind_address: SocketAddr,
        /// overrides the public address of the server.
        /// set this when hosting behind NAT (ex. a home router)
        #[clap(long)]
        public_address: Option<IpAddr>,
    },
    /// join a game
    Join { address: SocketAddr },
}

fn main() {
    let args = Args::parse();
    let role = match args.command {
        Some(ArgCommands::Host { .. }) => NetworkRole::Server,
        Some(ArgCommands::Join { .. }) | None => NetworkRole::Client,
    };
    let networking_plugin = NetworkingPlugin { role };

    let mut app = App::new();
    match role {
        NetworkRole::Server => {
            match config::load_server_config() {
                Ok(config) => app.insert_resource(config),
                Err(err) => {
                    error!("Error loading server configuration: {}", err);
                    return;
                }
            };

            app.insert_resource(ScheduleRunnerSettings {
                run_mode: bevy::app::RunMode::Loop {
                    wait: Some(Duration::from_secs_f64(1f64 / SERVER_TPS as f64)),
                },
            })
            .add_plugins(MinimalPlugins)
            .add_plugin(TransformPlugin)
            .add_plugin(AssetPlugin)
            .add_plugin(LogPlugin)
            .add_plugin(ScenePlugin)
            .add_plugin(HierarchyPlugin)
            .add_plugin(networking_plugin)
            .add_system(convert_tgm_map)
            .add_system(create_tilemap_from_converted)
            .add_asset::<byond::tgm::TileMap>()
            .add_asset::<Mesh>() // TODO: remove once no longer needed by rapier
            .add_asset::<Scene>() // TODO: remove once no longer needed by rapier
            // Register types used in scenes manually.
            // The server will not do anything with them, but needs it so it can load scene files.
            .register_type::<bevy::pbr::PointLight>()
            .register_type::<bevy::pbr::CubemapVisibleEntities>()
            .register_type::<bevy::pbr::NotShadowCaster>()
            .register_type::<bevy::render::primitives::CubemapFrusta>()
            .register_type::<bevy::render::view::Visibility>()
            .register_type::<bevy::render::view::ComputedVisibility>()
            .add_asset_loader(TgmLoader)
            .add_startup_system(setup_server)
            .add_startup_system(config::server_startup);
        }
        NetworkRole::Client => {
            app.add_plugins(DefaultPlugins)
                .add_plugin(networking_plugin)
                .add_plugin(camera::CameraPlugin)
                .add_plugin(EguiPlugin)
                .insert_resource(WorldInspectorParams {
                    enabled: false,
                    ..Default::default()
                })
                .add_plugin(WorldInspectorPlugin::new())
                .add_plugin(ui::UiPlugin)
                .insert_resource(ClearColor(Color::rgb(
                    44.0 / 255.0,
                    68.0 / 255.0,
                    107.0 / 255.0,
                )))
                .add_startup_system(setup_client)
                .add_system_to_stage(CoreStage::PostUpdate, handle_player_spawn)
                .add_system(set_camera_target)
                .add_system(clean_entities_on_disconnect)
                .add_state(GameState::Splash);
        }
    };
    app.add_plugin(RapierPhysicsPlugin::<NoUserData>::default())
        .add_plugin(physics::PhysicsPlugin)
        .add_plugin(scene::ScenePlugin)
        .add_plugin(movement::MovementPlugin)
        .add_plugin(maps::MapPlugin)
        .add_plugin(AdminPlugin)
        .add_plugin(round::RoundPlugin)
        .add_plugin(job::JobPlugin)
        .insert_resource(args)
        .add_startup_system(setup_shared)
        .register_type::<Player>()
        .run();
}

#[derive(Clone, Eq, PartialEq, Debug, Hash)]
enum GameState {
    Splash,
    MainMenu,
    Joining,
    Game,
}

/// A component that prevents an entity from being deleted when joining or leaving a server.
#[derive(Component)]
#[component(storage = "SparseSet")]
struct KeepOnServerChange;

#[derive(Component, Reflect)]
#[reflect(Component)]
pub struct Player {
    pub target_velocity: Vec2,
    pub acceleration: f32,
    pub max_acceleration_force: f32,
    pub max_velocity: f32,
    pub target_direction: Vec2,
}

impl Default for Player {
    fn default() -> Self {
        Self {
            max_velocity: 5.0,
            acceleration: 20.0,
            max_acceleration_force: 1000.0,
            target_velocity: Vec2::ZERO,
            target_direction: Vec2::ZERO,
        }
    }
}

#[derive(Clone)]
pub struct Map {
    pub handle: Handle<byond::tgm::TileMap>,
    pub spawned: bool,
}

fn setup_shared(mut commands: Commands) {
    // Spawn ground plane
    commands
        .spawn()
        .insert_bundle(TransformBundle::from(Transform::from_xyz(0.0, -0.5, 0.0)))
        .insert(Collider::cuboid(1000.0, 0.5, 1000.0))
        .insert(KeepOnServerChange);
}

fn setup_server(args: Res<Args>, mut commands: Commands) {
    match args.command.as_ref().unwrap() {
        &ArgCommands::Host {
            bind_address,
            public_address,
        } => {
            commands.insert_resource(networking::create_server(bind_address, public_address));
        }
        _ => panic!("Missing commandline argument"),
    };
}

fn setup_client(
    mut commands: Commands,
    args: Res<Args>,
    mut client_events: EventWriter<ClientEvent>,
    mut state: ResMut<State<GameState>>,
) {
    // TODO: Replace with on-station lights
    commands.insert_resource(AmbientLight {
        brightness: 0.01,
        ..Default::default()
    });

    let temporary_camera_target = commands.spawn().insert(GlobalTransform::default()).id();

    commands
        .spawn_bundle(Camera3dBundle {
            transform: Transform::from_xyz(-2.0, 2.5, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
            ..Default::default()
        })
        .insert(TopDownCamera::new(temporary_camera_target))
        .insert(camera::MainCamera)
        .insert(KeepOnServerChange);

    if let Some(ArgCommands::Join { address }) = args.command {
        state.overwrite_set(GameState::MainMenu).unwrap();
        client_events.send(ClientEvent::Join(address));
    }
}

/// Delete all entities when leaving a server, except entities with [`KeepOnServerChange`].
fn clean_entities_on_disconnect(
    mut events: EventReader<ClientEvent>,
    to_delete: Query<Entity, (Without<Parent>, Without<KeepOnServerChange>)>,
    mut commands: Commands,
) {
    let has_disconnected = events
        .iter()
        .any(|e| matches!(e, ClientEvent::Disconnected(_)));
    if !has_disconnected {
        return;
    }

    // TODO: Optimize deletion?
    for entity in to_delete.iter() {
        commands.entity(entity).despawn_recursive();
    }
}

fn create_player(commands: &mut EntityCommands) -> Entity {
    let player_rigid_body = (
        RigidBody::Dynamic,
        LockedAxes::ROTATION_LOCKED,
        Damping {
            linear_damping: 0.0,
            angular_damping: 0.0,
        },
        Velocity::default(),
    );
    let player_collider = (
        Collider::capsule(Vec3::ZERO, (0.0, 1.0, 0.0).into(), 0.2),
        ColliderMassProperties::Density(5.0),
        ReadMassProperties::default(),
    );
    commands
        .insert_bundle(TransformBundle::from_transform(
            Transform::from_translation((0.0, 1.0, 0.0).into()),
        ))
        .insert(Player::default())
        .insert_bundle(player_rigid_body)
        .insert_bundle(player_collider)
        .id()
}

// TODO: replace with spawning scene
fn handle_player_spawn(
    query: Query<&PrefabPath>,
    mut entity_events: EventReader<NetworkedEntityEvent>,
    mut commands: Commands,
    server: ResMut<AssetServer>,
) {
    for event in entity_events.iter() {
        if let NetworkedEntityEvent::Spawned(entity) = event {
            if let Ok(prefab) = query.get(*entity) {
                if prefab.0 == "player" {
                    let player = create_player(&mut commands.entity(*entity));
                    let player_model = server.load("models/human.glb#Scene0");
                    commands
                        .entity(player)
                        .insert(NetworkedTransform::default())
                        .insert_bundle(SceneBundle {
                            scene: player_model,
                            ..Default::default()
                        });
                }
            }
        }
    }
}

fn set_camera_target(
    query: Query<Entity, Added<ClientControlled>>,
    mut camera: Query<&mut TopDownCamera, Without<ClientControlled>>,
) {
    for entity in query.iter() {
        if let Ok(mut camera) = camera.get_single_mut() {
            camera.target = entity;
        }
    }
}

#[allow(dead_code)]
fn test_containers(mut commands: Commands, q: ContainerQuery) {
    let mut item = Item::new("Toolbox".into(), UVec2::new(2, 1));
    let item_entity = commands.spawn().id();

    let mut container = Container::new(UVec2::new(5, 5));
    let mut container_builder = commands.spawn();
    let container_entity = container_builder.id();
    let mut container_writer = ContainerWriter::new(&mut container, container_entity, &q);
    container_writer.insert_item(&mut item, item_entity, UVec2::new(0, 0));

    container_builder.insert(container);
    commands.entity(item_entity).insert(item);
}

#[derive(Component)]
struct ConvertByondMap(Task<TileMapData>);

fn convert_tgm_map(
    mut commands: Commands,
    map_resource: Option<ResMut<Map>>,
    tilemaps: Res<Assets<byond::tgm::TileMap>>,
) {
    if let Some(res) = map_resource {
        if let Some(map) = tilemaps.get(&res.handle) {
            let map_copy = byond::tgm::TileMap::clone(map);
            let thread_pool = AsyncComputeTaskPool::get();
            let task =
                thread_pool.spawn(async move { byond::tgm::conversion::to_map_data(&map_copy) });
            let new_entity = commands.spawn().insert(ConvertByondMap(task)).id();
            info!("Scheduled tgm map conversion (entity={:?})", new_entity);
            commands.remove_resource::<Map>();
        }
    }
}

fn create_tilemap_from_converted(
    mut commands: Commands,
    mut map_tasks: Query<(Entity, &mut ConvertByondMap)>,
) {
    for (entity, mut map_task) in map_tasks.iter_mut() {
        if let Some(map_data) = future::block_on(future::poll_once(&mut map_task.0)) {
            commands
                .entity(entity)
                .remove::<ConvertByondMap>()
                .insert(map_data)
                .insert_bundle(SpatialBundle::default())
                .networked();
            info!("Map conversion finished and applied (entity={:?})", entity);
        }
    }
}

#[allow(dead_code)]
fn print_containers(containers: Query<(&Container, Entity)>, container_query: ContainerQuery) {
    for (container, entity) in containers.iter() {
        println!("Container Entity: {}", entity.id());
        let accessor = ContainerAccessor::new(container, &container_query);
        for (position, item) in accessor.items() {
            println!("  {}", item.name);
            println!("    Size:     {}", item.size);
            println!("    Position: {}", position);
        }
    }
}
