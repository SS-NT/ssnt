use bevy::{asset::LoadedFolder, math::UVec2, prelude::*, reflect::TypePath};
use networking::{
    component::AppExt,
    identity::{NetworkIdentities, NetworkIdentity},
    is_server,
    variable::{NetworkVar, ServerVar},
    NetworkManager, Networked,
};

use self::{clothes::ClothingPlugin, containers::ContainerPlugin};

pub mod clothes;
pub mod containers;

pub struct ItemPlugin;

impl Plugin for ItemPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Item>()
            .add_networked_component::<StoredItem, StoredItemClient>()
            .add_systems(Startup, load_item_assets);

        if !is_server(app) {
            app.add_systems(
                Update,
                (
                    client_initialize_spawned_items,
                    client_update_item_visibility,
                ),
            );
        }
        app.add_plugins((ContainerPlugin, ClothingPlugin));
    }
}

#[derive(Component, Reflect)]
#[reflect(Component, Default)]
pub struct Item {
    pub name: String,
    pub size: UVec2,
}

impl Default for Item {
    fn default() -> Self {
        Self {
            name: "Default item name".to_string(),
            size: UVec2::ONE,
        }
    }
}

#[derive(Component, TypePath, Networked)]
#[networked(client = "StoredItemClient")]
pub struct StoredItem {
    #[networked(
        with = "Self::network_container(Res<'static, NetworkIdentities>) -> NetworkIdentity"
    )]
    container: NetworkVar<Entity>,
    slot: NetworkVar<UVec2>,
    visible: NetworkVar<bool>,
}

impl StoredItem {
    fn network_container(entity: &Entity, param: Res<NetworkIdentities>) -> NetworkIdentity {
        param
            .get_identity(*entity)
            .expect("Container entity must have network identity")
    }

    pub fn container(&self) -> Entity {
        *self.container
    }
}

#[derive(Component, Default, Networked, TypePath)]
#[networked(server = "StoredItem")]
pub struct StoredItemClient {
    container: ServerVar<NetworkIdentity>,
    slot: ServerVar<UVec2>,
    visible: ServerVar<bool>,
}

/// Stores strong references to all item assets.
/// This is so we can create handles from a path id, which doesn't load the assets by itself.
#[derive(Resource)]
pub struct ItemAssets {
    pub definitions: Handle<LoadedFolder>,
    client: Option<ClientItemAssets>,
}

struct ClientItemAssets {
    #[allow(dead_code)]
    models: Handle<LoadedFolder>,
    default_material: Handle<StandardMaterial>,
}

fn load_item_assets(
    mut commands: Commands,
    server: ResMut<AssetServer>,
    network: Res<NetworkManager>,
) {
    let client_assets = network.is_client().then(|| ClientItemAssets {
        models: server.load_folder("models/items"),
        default_material: server.load("models/items/wrenches.glb#Material0/std"),
    });

    let assets = ItemAssets {
        definitions: server.load_folder("items"),
        client: client_assets,
    };
    commands.insert_resource(assets);
}

// TODO: Remove once scenes support composition
/// Adds some bundles to spawned tile scenes, so we don't need to specify them every time
fn client_initialize_spawned_items(
    new: Query<Entity, Added<Item>>,
    children_query: Query<&Children>,
    existing_meshes: Query<
        (&Mesh3d, Option<&Transform>),
        Without<MeshMaterial3d<StandardMaterial>>,
    >,
    assets: Res<ItemAssets>,
    mut commands: Commands,
) {
    let Some(assets) = assets.client.as_ref() else {
        return;
    };

    let mut process_entity = |entity| {
        if let Ok((mesh, transform)) = existing_meshes.get(entity) {
            commands.entity(entity).insert((
                mesh.clone(),
                MeshMaterial3d(assets.default_material.clone()),
                transform.cloned().unwrap_or_default(),
            ));
        }
    };

    for root in new.iter() {
        process_entity(root);
        for child in children_query.iter_descendants(root) {
            process_entity(child);
        }
    }
}

fn client_update_item_visibility(
    mut query: Query<(&mut Visibility, &StoredItemClient), Changed<StoredItemClient>>,
    mut removed: RemovedComponents<StoredItemClient>,
    mut vis_query: Query<&mut Visibility, Without<StoredItemClient>>,
) {
    for (mut visibility, item) in query.iter_mut() {
        match (*item.visible, *visibility) {
            (true, Visibility::Hidden) => *visibility = Visibility::Inherited,
            (false, Visibility::Inherited) => *visibility = Visibility::Hidden,
            _ => {}
        }
    }

    for entity in removed.read() {
        if let Ok(mut visibility) = vis_query.get_mut(entity) {
            if *visibility == Visibility::Hidden {
                *visibility = Visibility::Inherited;
            }
        }
    }
}
