use bevy::{
    ecs::{entity::EntityMap, reflect::ReflectMapEntities},
    prelude::*,
};

use crate::identity::NetworkIdentity;

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

/// A component on the scene root that lists the children that have their own network ids.
/// Counterpart of [`NetworkSceneIdentities`].
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub struct NetworkSceneChildren {
    pub networked_children: Vec<Entity>,
}

/// A component listing the network identities of the children an object has.
/// Counterpart of [`NetworkSceneChildren`].
#[derive(Component)]
pub struct NetworkSceneIdentities {
    pub child_identities: Vec<NetworkIdentity>,
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
        if spawner.scenes_to_spawn.is_empty() {
            return;
        }
        world.resource_scope(|world, scene_assets: Mut<Assets<DynamicScene>>| {
            let registry = world.resource::<AppTypeRegistry>().clone();
            spawner.scenes_to_spawn.retain(|(entity, scene_handle)| {
                let Some(scene) = scene_assets.get(scene_handle) else {
                    return true;
                };

                // TODO: Verify scene only has one root entity

                // Preserve transform so it doesn't get overwritten
                let existing_transform = world.get::<Transform>(*entity).cloned();

                // HACK: Remove and store components that would be remapped by the scene system
                let mut temporary_world = World::new();
                let temporary_entity = temporary_world.spawn_empty().id();
                let read_registry = registry.read();
                let problematic_components = world
                    .entity(*entity)
                    .archetype()
                    .components()
                    .filter_map(|c| {
                        world
                            .components()
                            .get_info(c)
                            .and_then(|info| info.type_id())
                    })
                    .filter(|id| {
                        read_registry
                            .get(*id)
                            .and_then(|ty| ty.data::<ReflectMapEntities>())
                            .is_some()
                    })
                    .collect::<Vec<_>>();
                for type_id in problematic_components.iter() {
                    let registration = read_registry.get(*type_id).unwrap();
                    let reflect_component = registration.data::<ReflectComponent>().unwrap();
                    reflect_component.copy(world, &mut temporary_world, *entity, temporary_entity);
                    reflect_component.remove(world, *entity);
                }
                drop(read_registry);

                // Make the scene entity #0 add components onto our existing entity
                let mut entity_map = EntityMap::default();
                entity_map.insert(Entity::from_raw(0), *entity);

                if let Err(err) = scene.write_to_world(world, &mut entity_map) {
                    warn!(entity = ?entity, "Error spawning network scene: {}", err);
                    return false;
                }

                if let Some(transform) = existing_transform {
                    world.entity_mut(*entity).insert(transform);
                }

                // Add back the problematic components
                let read_registry = registry.read();
                for type_id in problematic_components.iter() {
                    let registration = read_registry.get(*type_id).unwrap();
                    let reflect_component = registration.data::<ReflectComponent>().unwrap();
                    reflect_component.copy(&temporary_world, world, temporary_entity, *entity);
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
