mod byond;
mod items;
mod maps;
mod utils;

use bevy::tasks::{AsyncComputeTaskPool, Task};
use bevy::{core::FixedTimestep, prelude::*};
use bevy_fly_camera::{FlyCamera, FlyCameraPlugin};
use byond::tgm::TgmLoader;
use futures_lite::future;
use items::{
    containers::{
        cleanup_removed_items_system, Container, ContainerAccessor, ContainerQuery, ContainerWriter,
    },
    Item,
};
use maps::components::{TileMap, TileMapObserver};
use maps::MapData;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugin(FlyCameraPlugin)
        .add_asset::<byond::tgm::TileMap>()
        .add_asset_loader(TgmLoader)
        .add_startup_system(setup.system())
        .add_startup_system(load_map.system())
        .add_startup_system(test_containers.system())
        /*.add_system_set(
            SystemSet::new()
                .with_run_criteria(FixedTimestep::steps_per_second(1f64))
                .with_system(print_containers.system()),
        )*/
        //.add_system(create_map_models.system())
        .add_system(cleanup_removed_items_system.system())
        .add_system(maps::systems::tilemap_observer_check)
        .add_system(convert_tgm_map)
        .add_system(create_tilemap_from_converted)
        .run();
}

struct MainCamera;

struct Map {
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

    //commands.spawn().insert(DirectionalLight::default());

    commands
        .spawn_bundle(PerspectiveCameraBundle {
            transform: Transform::from_xyz(-2.0, 2.5, 5.0).looking_at(Vec3::ZERO, Vec3::Y),
            ..Default::default()
        })
        .insert(MainCamera)
        .insert(TileMapObserver { view_range: 20.0 })
        .insert(FlyCamera::default());
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
    mut camera: Query<&mut Transform, With<MainCamera>>,
) {
    for (entity, mut map_task) in map_tasks.iter_mut() {
        if let Some(map_data) = future::block_on(future::poll_once(&mut *map_task)) {
            camera.single_mut().unwrap().translation = 
             Vec3::new(map_data.spawn_position.x as f32, 0.0, map_data.spawn_position.y as f32);
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

fn create_map_models(
    mut commands: Commands,
    map_resource: Option<ResMut<Map>>,
    tilemaps: Res<Assets<byond::tgm::TileMap>>,
    asset_server: Res<AssetServer>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut cameras: Query<&mut Transform, With<MainCamera>>,
) {
    if let Some(mut res) = map_resource {
        if res.spawned {
            return;
        }

        if let Some(map) = tilemaps.get(&res.handle) {
            let map_middle = map.middle();
            let middle = Vec3::new(map_middle.x as f32, 0.0, map_middle.y as f32);
            let mut cam = middle;
            cam.y = 300.0;
            let mut camera_position = cameras.single_mut().unwrap();
            *camera_position = Transform::from_translation(cam);
            camera_position.rotation = Quat::from_euler(
                bevy::math::EulerRot::YXZ,
                0.0,
                -std::f32::consts::FRAC_PI_2,
                0.0,
            );

            //let floor_handle= asset_server.load("models/tilemap/floors.glb#Mesh0/Primitive0");
            let wall_material_handle = materials.add(StandardMaterial {
                base_color: Color::rgb(0.8, 0.8, 0.8),
                ..Default::default()
            });
            let window_material_handle = materials.add(StandardMaterial {
                base_color: Color::rgb(0.1, 0.1, 0.9),
                ..Default::default()
            });
            let door_material_handle = materials.add(StandardMaterial {
                base_color: Color::rgb(0.0, 1.0, 0.0),
                ..Default::default()
            });
            let mesh_handle = meshes.add(Mesh::from(shape::Cube { size: 1.0 }));

            for (position, tile) in map.iter_tiles() {
                if let Some(tile) = tile {
                    for component in tile.components.iter() {
                        let mut material = match component.path.as_str() {
                            "/turf/closed/wall" | "/turf/closed/wall/r_wall" => {
                                Some(wall_material_handle.clone())
                            }
                            "/obj/structure/window"
                            | "/obj/structure/window/reinforced"
                            | "/obj/effect/spawner/structure/window/reinforced"
                            | "/obj/effect/spawner/structure/window" => {
                                Some(window_material_handle.clone())
                            }
                            _ => None,
                        };
                        if component.path.contains("/obj/machinery/door/airlock") {
                            material = Some(door_material_handle.clone());
                        }
                        if let Some(material) = material {
                            commands.spawn_bundle(PbrBundle {
                                mesh: mesh_handle.clone(),
                                material,
                                transform: Transform::from_translation((*position).as_f32()),
                                ..Default::default()
                            });
                            break;
                        }
                    }
                }
            }

            res.spawned = true;
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
