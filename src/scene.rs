use bevy::{
    asset::AssetStage,
    prelude::*,
    scene::{DynamicScene, InstanceId, SceneInstance, SceneSpawner},
};

fn modify_loaded_scenes(
    mut scenes: ResMut<Assets<DynamicScene>>,
    mut events: EventReader<AssetEvent<DynamicScene>>,
) {
    for event in events.iter() {
        if let AssetEvent::Created { handle } = event {
            println!("Hewwo");
            let scene = scenes.get_mut(handle).unwrap();

            // Add a global transform to all entities
            // This will probably change at some point, so we don't add it when it's not needed
            for dynamic_entity in &mut scene.entities {
                dynamic_entity
                    .components
                    .push(Box::new(GlobalTransform::default()));
            }
        }
    }
}

fn post_process_scenes(
    created: Query<&SceneInstance, Added<SceneInstance>>,
    mut loading: Local<Vec<InstanceId>>,
    spawner: Res<SceneSpawner>,
    with_transforms: Query<(), With<Transform>>,
    mut commands: Commands,
) {
    for instance in created.iter() {
        loading.push(**instance);
    }

    loading.retain(|instance| {
        if spawner.instance_is_ready(*instance) {
            let entities = spawner.iter_instance_entities(*instance);
            for entity in entities {
                // Automatically add a global transform
                if with_transforms.get(entity).is_ok() {
                    commands.entity(entity).insert(GlobalTransform::default());
                }
            }

            false
        } else {
            true
        }
    });
}

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.add_system(post_process_scenes)
            .add_system_to_stage(AssetStage::AssetEvents, modify_loaded_scenes.at_end());
    }
}
