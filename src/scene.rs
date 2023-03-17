use bevy::{
    asset::AssetStage,
    prelude::*,
    scene::{DynamicEntity, DynamicScene},
};
use networking::is_client;

fn dyn_entity_has_component<T: Reflect>(entity: &DynamicEntity) -> bool {
    entity.components.iter().any(|c| c.represents::<T>())
}

fn modify_loaded_scenes(
    mut scenes: ResMut<Assets<DynamicScene>>,
    mut events: EventReader<AssetEvent<DynamicScene>>,
    client_assets: Option<Res<ClientSceneAssets>>,
) {
    for event in events.iter() {
        if let AssetEvent::Created { handle } = event {
            let scene = scenes.get_mut(handle).unwrap();

            for dynamic_entity in &mut scene.entities {
                // Add a global transform to all entities
                // This will probably change at some point, so we don't add it when it's not needed
                dynamic_entity
                    .components
                    .push(Box::<GlobalTransform>::default());

                // Add some extra components on client
                if let Some(assets) = client_assets.as_ref() {
                    // Add a material if it contains a mesh and doesn't have one
                    if dyn_entity_has_component::<Handle<Mesh>>(dynamic_entity)
                        && !dyn_entity_has_component::<Handle<StandardMaterial>>(dynamic_entity)
                    {
                        dynamic_entity
                            .components
                            .push(Box::new(assets.default_material.clone()));
                    }

                    // Add components for visibility
                    dynamic_entity.components.push(Box::<Visibility>::default());
                    dynamic_entity
                        .components
                        .push(Box::<ComputedVisibility>::default());
                }
            }
        }
    }
}

#[derive(Resource)]
struct ClientSceneAssets {
    default_material: Handle<StandardMaterial>,
}

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.add_system_to_stage(AssetStage::AssetEvents, modify_loaded_scenes.at_end());

        if is_client(app) {
            app.insert_resource(ClientSceneAssets {
                default_material: app
                    .world
                    .resource::<AssetServer>()
                    .load("models/items/wrenches.glb#Material0"),
            });
        }
    }
}
