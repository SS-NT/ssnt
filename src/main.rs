mod byond;
mod camera;
mod items;
mod maps;
mod utils;
mod components;
mod movement;
mod ui;

use bevy::tasks::{AsyncComputeTaskPool, Task};
use bevy::prelude::*;
use bevy_fly_camera::{FlyCamera, FlyCameraPlugin};
use byond::tgm::TgmLoader;
use camera::TopDownCamera;
use futures_lite::future;
use items::{
    containers::{
        cleanup_removed_items_system, Container, ContainerAccessor, ContainerQuery, ContainerWriter,
    },
    Item,
};
use maps::components::{TileMap, TileMapObserver};
use maps::MapData;
use components::{Disabled, EntityCommandsExt};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugin(FlyCameraPlugin)
        .add_plugin(camera::CameraPlugin)
        .add_plugin(maps::MapPlugin)
        .add_plugin(movement::MovementPlugin)
        .add_plugin(ui::UiPlugin)
        .insert_resource(ClearColor(Color::rgb(
            44.0 / 255.0,
            68.0 / 255.0,
            107.0 / 255.0,
        )))
        .add_asset::<byond::tgm::TileMap>()
        .add_asset_loader(TgmLoader)
        .add_startup_system(setup.system())
        .add_startup_system(load_map.system())
        .add_startup_system(test_containers.system())
        .add_system(cleanup_removed_items_system.system())
        .add_system(switch_camera_system)
        .add_system(convert_tgm_map)
        .add_system(create_tilemap_from_converted)
        .run();
}

#[derive(Component)]
pub struct Player {
    pub velocity: Vec2,
    pub acceleration: f32,
    pub max_velocity: f32,
    pub friction: f32,
}

impl Default for Player {
    fn default() -> Self {
        Self { max_velocity: 3.0, acceleration: 1.0, friction: 0.5, velocity: Vec2::ZERO }
    }
}

pub struct Map {
    pub handle: Handle<byond::tgm::TileMap>,
    pub spawned: bool,
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn_bundle(PbrBundle {
        mesh: meshes.add(Mesh::from(shape::Cube { size: 2.0 })),
        material: materials.add(Color::rgb(0.8, 0.7, 0.6).into()),
        transform: Transform::from_xyz(0.0, 0.5, 0.0),
        ..Default::default()
    });

    // TODO: Replace with on-station lights
    commands.insert_resource(AmbientLight {
        brightness: 0.2,
        ..Default::default()
    });

    let player = commands
        .spawn()
        .insert(Transform::default())
        .insert(GlobalTransform::default())
        .insert(Player::default())
        .insert(TileMapObserver::new(20.0))
        .id();
    commands
        .spawn_bundle(PerspectiveCameraBundle {
            transform: Transform::from_xyz(-2.0, 2.5, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
            ..Default::default()
        })
        .insert(TopDownCamera::new(player))
        .insert(Disabled(FlyCamera::default()))
        .insert(camera::MainCamera);
}

fn switch_camera_system(
    mut commands: Commands,
    keyboard_input: Res<Input<KeyCode>>,
    fly_cams: Query<(Entity, &FlyCamera), (With<Disabled<TopDownCamera>>, Without<TopDownCamera>)>,
    top_down_cams: Query<(Entity, &TopDownCamera), (With<Disabled<FlyCamera>>, Without<FlyCamera>)>,
) {
    if !keyboard_input.just_pressed(KeyCode::C) {
        return;
    }

    for (entity, _) in fly_cams.iter() {
        commands.entity(entity)
                .disable_component::<FlyCamera>()
                .enable_component::<TopDownCamera>();
    }

    for (entity, _) in top_down_cams.iter() {
        commands.entity(entity)
                .disable_component::<TopDownCamera>()
                .enable_component::<FlyCamera>();
    }
}

fn load_map(mut commands: Commands, server: Res<AssetServer>) {
    let handle = server.load("maps/DeltaStation2.dmm");
    commands.insert_resource(Map {
        handle,
        spawned: false,
    });
}

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
    if let Some(mut res) = map_resource {
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
    mut player: Query<&mut Transform, With<Player>>,
) {
    for (entity, mut map_task) in map_tasks.iter_mut() {
        if let Some(map_data) = future::block_on(future::poll_once(&mut *map_task)) {
            player.single_mut().translation = Vec3::new(
                map_data.spawn_position.x as f32,
                0.0,
                map_data.spawn_position.y as f32,
            );
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
