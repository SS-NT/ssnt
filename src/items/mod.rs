use bevy::{math::UVec2, prelude::*, reflect::TypeUuid};
use networking::{
    component::AppExt,
    identity::{NetworkIdentities, NetworkIdentity},
    is_server,
    variable::{NetworkVar, ServerVar},
    NetworkManager, Networked,
};

use self::containers::ContainerPlugin;

pub mod containers;

pub struct ItemPlugin;

impl Plugin for ItemPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Item>()
            .add_networked_component::<StoredItem, StoredItemClient>()
            .add_startup_system(load_item_assets);

        if !is_server(app) {
            app.add_system(client_initialize_spawned_items);
        }
        app.add_plugin(ContainerPlugin);
    }
}

#[derive(Component, Reflect)]
#[reflect(Component)]
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

#[derive(Component, Networked)]
#[networked(client = "StoredItemClient")]
pub struct StoredItem {
    #[networked(
        with = "Self::network_container(Res<'static, NetworkIdentities>) -> NetworkIdentity"
    )]
    container: NetworkVar<Entity>,
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

#[derive(Component, Default, Networked, TypeUuid)]
#[uuid = "7a30823e-ab38-4bca-ba3a-4bab1328d2df"]
#[networked(server = "StoredItem")]
struct StoredItemClient {
    container: ServerVar<NetworkIdentity>,
}

/// Stores strong references to all item assets.
/// This is so we can create handles from a path id, which doesn't load the assets by itself.
#[derive(Resource)]
pub struct ItemAssets {
    pub definitions: Vec<Handle<DynamicScene>>,
    client: Option<ClientItemAssets>,
}

struct ClientItemAssets {
    #[allow(dead_code)]
    models: Vec<HandleUntyped>,
    default_material: Handle<StandardMaterial>,
}

fn load_item_assets(
    mut commands: Commands,
    server: ResMut<AssetServer>,
    network: Res<NetworkManager>,
) {
    let client_assets = network.is_client().then(|| ClientItemAssets {
        models: server
            .load_folder("models/items")
            .expect("assets/models/items is missing"),
        default_material: server.load("models/items/wrenches.glb#Material0"),
    });

    let assets = ItemAssets {
        definitions: server
            .load_folder("items")
            .expect("assets/items is missing")
            .into_iter()
            .map(|h| h.typed::<DynamicScene>())
            .collect(),
        client: client_assets,
    };
    commands.insert_resource(assets);
}

// TODO: Remove once scenes support composition
/// Adds some bundles to spawned tile scenes, so we don't need to specify them every time
fn client_initialize_spawned_items(
    new: Query<Entity, Added<Item>>,
    children_query: Query<&Children>,
    existing_meshes: Query<(&Handle<Mesh>, Option<&Transform>)>,
    assets: Res<ItemAssets>,
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
    }
}
