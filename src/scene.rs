use bevy::{
    prelude::{Added, App, Commands, GlobalTransform, Local, Plugin, Query, Res, Transform, With},
    scene::{InstanceId, SceneInstance, SceneSpawner},
};

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
        if let Some(entities) = spawner.iter_instance_entities(*instance) {
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
        app.add_system(post_process_scenes);
    }
}
