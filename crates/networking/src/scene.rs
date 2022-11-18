use bevy::{ecs::entity::EntityMap, prelude::*};

pub(crate) struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkSceneSpawner>()
            .add_event::<NetworkSceneEvent>()
            .add_system_to_stage(CoreStage::PreUpdate, queue_network_scenes)
            .add_system_to_stage(CoreStage::PreUpdate, spawn_network_scenes.at_end());
    }
}

/// A handle to a scene that can be spawned over the network.
#[derive(Component, Default)]
pub struct NetworkScene(pub(crate) Handle<DynamicScene>);

impl From<Handle<DynamicScene>> for NetworkScene {
    fn from(handle: Handle<DynamicScene>) -> Self {
        Self(handle)
    }
}

pub enum NetworkSceneEvent {
    Created(Entity),
}

/// Add to an entity to attach a scene that can be networked.
#[derive(Bundle, Default)]
pub struct NetworkSceneBundle {
    pub scene: NetworkScene,
    pub transform: Transform,
    pub global_transform: GlobalTransform,
    pub visibility: Visibility,
    pub computed_visibility: ComputedVisibility,
}

#[derive(Resource, Default)]
struct NetworkSceneSpawner {
    scenes_to_spawn: Vec<(Entity, Handle<DynamicScene>)>,
}

fn queue_network_scenes(
    query: Query<(Entity, &NetworkScene), Added<NetworkScene>>,
    mut spawner: ResMut<NetworkSceneSpawner>,
) {
    for (entity, network_scene) in query.iter() {
        spawner
            .scenes_to_spawn
            .push((entity, network_scene.0.clone_weak()));
    }
}

// Spawns loaded networked scenes into the world
fn spawn_network_scenes(world: &mut World) {
    world.resource_scope(|world, mut spawner: Mut<NetworkSceneSpawner>| {
        world.resource_scope(|world, scene_assets: Mut<Assets<DynamicScene>>| {
            spawner.scenes_to_spawn.retain(|(entity, scene_handle)| {
                let Some(scene) = scene_assets.get(scene_handle) else {
                    return true;
                };

                // TODO: Verify scene only has one root entity

                // Preserve some components that should not be overwritten
                let existing_parent = world.get::<Parent>(*entity).map(|p| **p);
                let existing_transform = world.get::<Transform>(*entity).cloned();

                // Make the scene entity #0 add components onto our existing entity
                let mut entity_map = EntityMap::default();
                entity_map.insert(Entity::from_raw(0), *entity);

                if let Err(err) = scene.write_to_world(world, &mut entity_map) {
                    warn!(entity = ?entity, "Error spawning network scene: {}", err);
                    return false;
                }

                if let Some(parent_entity) = existing_parent {
                    let mut parent = world.get_mut::<Parent>(*entity).unwrap();
                    // Hack: Use reflection to set parent entity
                    // Because people were messing up their hierarchies, we can't change the value in Parent components anymore.
                    // Sucks for us. May get a new method with a scary name in the future.
                    *parent.get_field_mut(0).unwrap() = parent_entity;
                }
                if let Some(transform) = existing_transform {
                    world.entity_mut(*entity).insert(transform);
                }

                // Emit scene event
                world
                    .resource_mut::<Events<NetworkSceneEvent>>()
                    .send(NetworkSceneEvent::Created(*entity));

                false
            });
        });
    });
}
