mod byond;
mod items;
mod maps;
mod utils;

use bevy::{core::FixedTimestep, prelude::*};
use byond::tgm::TgmLoader;
use byond::tgm::TileMap;
use items::{Item, containers::{Container, ContainerAccessor, ContainerQuery, ContainerWriter, cleanup_removed_items_system}};

fn main() {
    /*let file = File::open("assets/BoxStation.dmm").unwrap();
    let mut reader = BufReader::new(file);

    let mut header_buffer = Vec::new();
    reader.read_until(b'\n', &mut header_buffer).unwrap();
    let header = std::str::from_utf8(&header_buffer).unwrap();
    assert!(header.starts_with("//MAP CONVERTED BY dmm2tgm.py"));
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer).unwrap();
    let map = std::str::from_utf8(&buffer).unwrap();

    let parse_start = Instant::now();
    let result = byond::tgm::parsing::parse(map);
    println!("Took {} milliseconds", parse_start.elapsed().as_millis());

    if let Err(e) = result {

        let err = match e {
            nom::Err::Error(e) | nom::Err::Failure(e) => e,
            _ => return
        };

        const MAX_ERROR_LENGTH: usize = 200;

        for (input, error) in &err.errors {
            let input = truncate(input, MAX_ERROR_LENGTH);
            match error {
              VerboseErrorKind::Nom(e) => println!("{:?} at: {}", e, input),
              VerboseErrorKind::Char(c) => println!("expected '{}' at: {}", c, input),
              VerboseErrorKind::Context(s) => println!("in section '{}', at: {}", s, input),
            }
        }

        let lines = BufReader::new(Cursor::new(&map[0..map.offset(err.errors.first().unwrap().0)])).lines().count();
        println!("Parsed {} lines", lines);

        return;
    }
    let (_, (definitions, chunks)) = result.unwrap();*/

    /*for (name, tile) in &tiles {
        println!("{}", name);
        for object in tile.components.iter() {
            println!("  {}", object.path);
            for var in object.variables.iter() {
                println!("    {} = {:?}", var.name, var.value);
            }
        }
    }*/

    App::build()
        .add_plugins(DefaultPlugins)
        .add_asset::<TileMap>()
        .add_asset_loader(TgmLoader)
        .add_startup_system(load_map.system())
        .add_startup_system(test_containers.system())
        .add_system_set(
            SystemSet::new()
                .with_run_criteria(FixedTimestep::steps_per_second(1f64))
                .with_system(print_containers.system()),
        )
        .add_system(cleanup_removed_items_system.system())
        .run();
}

struct Map(Handle<TileMap>);

fn load_map(mut commands: Commands, server: Res<AssetServer>) {
    let handle = server.load("maps/BoxStation.dmm");
    commands.insert_resource(Map(handle));
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
