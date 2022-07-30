#![allow(clippy::type_complexity)]

mod admin;
mod camera;
mod components;
mod items;
mod movement;
mod ui;

use std::net::SocketAddr;
use std::time::Duration;

use admin::AdminPlugin;
use bevy::app::ScheduleRunnerSettings;
use bevy::asset::AssetPlugin;
use bevy::ecs::system::EntityCommands;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
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
use maps::components::{TileMap, TileMapObserver};
use maps::MapData;
use networking::identity::EntityCommandsExt as NetworkingEntityCommandsExt;
use networking::spawning::{ClientControlled, ClientControls, NetworkedEntityEvent, PrefabPath};
use networking::transform::{NetworkTransform, NetworkedTransform};
use networking::visibility::NetworkObserver;
use networking::{ClientEvent, ConnectionId, NetworkRole, NetworkingPlugin, ServerEvent};

/// How many ticks the server runs per second
const SERVER_TPS: u32 = 60;

#[derive(Parser)]
struct Args {
    #[clap(subcommand)]
    command: ArgCommands,
}

#[derive(Subcommand)]
enum ArgCommands {
    /// host a server
    Host {
        /// port to listen on
        #[clap(default_value_t = 33998u16)]
        port: u16,
    },
    /// join a game
    Join { address: SocketAddr },
}

fn main() {
    let args = Args::parse();
    let role = match args.command {
        ArgCommands::Host { .. } => NetworkRole::Server,
        ArgCommands::Join { .. } => NetworkRole::Client,
    };
    let networking_plugin = NetworkingPlugin { role };

    let mut app = App::new();
    match role {
        NetworkRole::Server => {
            app.insert_resource(ScheduleRunnerSettings {
                run_mode: bevy::app::RunMode::Loop {
                    wait: Some(Duration::from_secs_f64(1f64 / SERVER_TPS as f64)),
                },
            })
            .add_plugins(MinimalPlugins)
            .add_plugin(TransformPlugin)
            .add_plugin(AssetPlugin)
            .add_plugin(LogPlugin)
            .add_plugin(networking_plugin)
            .add_system(convert_tgm_map)
            .add_system(create_tilemap_from_converted)
            .add_asset::<byond::tgm::TileMap>()
            .add_asset::<Mesh>() // TODO: remove once no longer needed by rapier
            .add_asset::<Scene>() // TODO: remove once no longer needed by rapier
            .add_asset_loader(TgmLoader)
            .add_startup_system(load_map)
            .add_startup_system(setup_server)
            .add_system(spawn_player_joined);
        }
        NetworkRole::Client => {
            app.add_plugins(DefaultPlugins)
                .add_plugin(networking_plugin)
                .add_plugin(camera::CameraPlugin)
                .add_plugin(ui::UiPlugin)
                .insert_resource(ClearColor(Color::rgb(
                    44.0 / 255.0,
                    68.0 / 255.0,
                    107.0 / 255.0,
                )))
                .add_startup_system(setup_client)
                .add_system_to_stage(CoreStage::PostUpdate, handle_player_spawn)
                .add_system(set_camera_target);
        }
    };
    app.add_plugin(RapierPhysicsPlugin::<NoUserData>::default())
        .add_plugin(movement::MovementPlugin)
        .add_plugin(maps::MapPlugin)
        .add_plugin(AdminPlugin)
        .insert_resource(args)
        .add_startup_system(setup_shared)
        .run();
}

#[derive(Component)]
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

pub struct Map {
    pub handle: Handle<byond::tgm::TileMap>,
    pub spawned: bool,
}

fn setup_shared(mut commands: Commands) {
    // Spawn ground plane
    commands
        .spawn()
        .insert_bundle(TransformBundle::from(Transform::from_xyz(0.0, -1.0, 0.0)))
        .insert(Collider::cuboid(100.0, 0.5, 100.0));
}

fn setup_server(args: Res<Args>, mut commands: Commands) {
    let port = match args.command {
        ArgCommands::Host { port } => port,
        _ => panic!("Missing commandline argument"),
    };
    commands.insert_resource(networking::create_server(port));
}

fn setup_client(
    mut commands: Commands,
    args: Res<Args>,
    mut client_events: EventWriter<ClientEvent>,
) {
    // TODO: Replace with on-station lights
    commands.insert_resource(AmbientLight {
        brightness: 0.2,
        ..Default::default()
    });

    let temporary_camera_target = commands.spawn().insert(GlobalTransform::default()).id();

    commands
        .spawn_bundle(PerspectiveCameraBundle {
            transform: Transform::from_xyz(-2.0, 2.5, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
            ..Default::default()
        })
        .insert(TopDownCamera::new(temporary_camera_target))
        .insert(camera::MainCamera);

    if let ArgCommands::Join { address } = args.command {
        client_events.send(ClientEvent::Join(address));
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
        .insert_bundle(TransformBundle::default())
        .insert(Player::default())
        .insert_bundle(player_rigid_body)
        .insert_bundle(player_collider)
        .id()
}

fn create_player_server(commands: &mut Commands, connection: ConnectionId) -> Entity {
    let player = create_player(&mut commands.spawn());
    commands
        .entity(player)
        .insert(NetworkObserver {
            range: 3,
            connection,
        })
        .insert(TileMapObserver::new(20.0))
        .insert(PrefabPath("player".into()))
        .insert(NetworkTransform::default())
        .networked();
    player
}

fn spawn_player_joined(
    mut server_events: EventReader<ServerEvent>,
    mut controls: ResMut<ClientControls>,
    mut commands: Commands,
) {
    for event in server_events.iter() {
        if let ServerEvent::PlayerConnected(id) = event {
            let player = create_player_server(&mut commands, *id);
            controls.give_control(*id, player);
            info!("Created a player object for new client");
        }
    }
}

fn handle_player_spawn(
    query: Query<&PrefabPath>,
    mut entity_events: EventReader<NetworkedEntityEvent>,
    mut commands: Commands,
    server: ResMut<AssetServer>,
) {
    for event in entity_events.iter() {
        if let NetworkedEntityEvent::Spawned(entity) = event {
            let prefab = query.get(*entity).unwrap();
            if prefab.0 == "player" {
                let player = create_player(&mut commands.entity(*entity));
                let player_model = server.load("models/human.glb#Scene0");
                commands
                    .entity(player)
                    .insert(NetworkedTransform::default())
                    .with_children(|parent| {
                        parent.spawn_scene(player_model);
                    });
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

fn load_map(mut commands: Commands, server: Res<AssetServer>) {
    let handle = server.load("maps/DeltaStation2.dmm");
    commands.insert_resource(Map {
        handle,
        spawned: false,
    });
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

fn convert_tgm_map(
    mut commands: Commands,
    map_resource: Option<ResMut<Map>>,
    tilemaps: Res<Assets<byond::tgm::TileMap>>,
    thread_pool: Res<AsyncComputeTaskPool>,
) {
    if let Some(res) = map_resource {
        if let Some(map) = tilemaps.get(&res.handle) {
            let map_copy = byond::tgm::TileMap::clone(map);
            let task =
                thread_pool.spawn(async move { byond::tgm::conversion::to_map_data(&map_copy) });
            let new_entity = commands.spawn().insert(task).id();
            info!("Scheduled tgm map conversion (entity={:?})", new_entity);
            commands.remove_resource::<Map>();
        }
    }
}

fn create_tilemap_from_converted(
    mut commands: Commands,
    mut map_tasks: Query<(Entity, &mut Task<MapData>)>,
    mut players: Query<&mut Transform, With<Player>>,
) {
    for (entity, mut map_task) in map_tasks.iter_mut() {
        if let Some(map_data) = future::block_on(future::poll_once(&mut *map_task)) {
            for mut player in players.iter_mut() {
                player.translation = Vec3::new(
                    map_data.spawn_position.x as f32,
                    0.0,
                    map_data.spawn_position.y as f32,
                );
            }

            commands
                .entity(entity)
                .remove::<Task<MapData>>()
                .insert(TileMap::new(map_data))
                .insert(Transform::default())
                .insert(GlobalTransform::identity());
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
