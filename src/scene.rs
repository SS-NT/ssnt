use bevy::{asset::AssetStage, prelude::*, scene::DynamicScene};

fn modify_loaded_scenes(
    mut scenes: ResMut<Assets<DynamicScene>>,
    mut events: EventReader<AssetEvent<DynamicScene>>,
) {
    for event in events.iter() {
        if let AssetEvent::Created { handle } = event {
            let scene = scenes.get_mut(handle).unwrap();

            // Add a global transform to all entities
            // This will probably change at some point, so we don't add it when it's not needed
            for dynamic_entity in &mut scene.entities {
                dynamic_entity
                    .components
                    .push(Box::<GlobalTransform>::default());
            }
        }
    }
}

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.add_system_to_stage(AssetStage::AssetEvents, modify_loaded_scenes.at_end());
    }
}
