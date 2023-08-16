use bevy::{
    ecs::{
        entity::{EntityMap, MapEntities},
        reflect::ReflectMapEntities,
        system::Command,
    },
    prelude::*,
};
use smallvec::SmallVec;

use crate::{
    identity::{NetworkCommand, NetworkIdentities, NetworkIdentity},
    spawning::SpawningSet,
    NetworkManager,
};

pub(crate) struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkSceneSpawner>()
            .add_event::<NetworkSceneEvent>()
            .register_type::<NetworkedChild>()
            .register_type::<HasNetworkedChildren>()
            .add_systems(
                PreUpdate,
                (
                    apply_deferred,
                    (queue_network_scenes, prepare_loaded_scenes),
                    spawn_network_scenes,
                    apply_deferred,
                )
                    .chain()
                    .in_set(SpawningSet::SpawnScenes),
            );
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

/// A marker component to identify child objects in scenes.
/// Children with this component will get a network identity assigned.
///
/// Note: Do not use this for detachable children. Instead spawn them normally and nest them at runtime.
#[derive(Component, Reflect, Default)]
#[reflect(Component)]
pub struct NetworkedChild;

#[derive(Component, Reflect, Default, Clone)]
#[reflect(Component, MapEntities)]
struct HasNetworkedChildren {
    children: SmallVec<[Entity; 4]>,
}

impl MapEntities for HasNetworkedChildren {
    fn map_entities(&mut self, entity_mapper: &mut bevy::ecs::entity::EntityMapper) {
        for entity in &mut self.children {
            *entity = entity_mapper.get_or_reserve(*entity);
        }
    }
}

#[derive(Event)]
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

fn prepare_loaded_scenes(
    mut scenes: ResMut<Assets<DynamicScene>>,
    mut events: EventReader<AssetEvent<DynamicScene>>,
) {
    for event in events.iter() {
        let AssetEvent::Created { handle } = event else {
            continue;
        };
        let Some(scene) = scenes.get_mut(handle) else {
            continue;
        };

        // Find all entities with `NetworkedChild` component
        let static_children: SmallVec<_> = scene
            .entities
            .iter_mut()
            .filter(|e| {
                e.components
                    .iter()
                    .any(|c| c.represents::<NetworkedChild>())
            })
            .map(|e| e.entity)
            .collect();

        if static_children.is_empty() {
            continue;
        }

        let Some(root) = scene.entities.first_mut() else {
            continue;
        };

        root.components.push(Box::new(HasNetworkedChildren {
            children: static_children,
        }));
    }
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
                // Remove existing children so we can merge them with any potentially new children later
                let existing_children = world.entity_mut(*entity).take::<Children>();

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
                    reflect_component.remove(&mut world.entity_mut(*entity));
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

                // Merge any existing children into the new children
                if let Some(children) = existing_children {
                    world.entity_mut(*entity).push_children(&children);
                }

                // Add back the problematic components
                let read_registry = registry.read();
                for type_id in problematic_components.iter() {
                    let registration = read_registry.get(*type_id).unwrap();
                    let reflect_component = registration.data::<ReflectComponent>().unwrap();
                    reflect_component.copy(&temporary_world, world, temporary_entity, *entity);
                }

                let is_server = world.resource::<NetworkManager>().is_server();

                // Ensure entity is networked
                if is_server {
                    NetworkCommand { entity: *entity }.apply(world);
                }

                // Handle children with network identities
                if let Some(HasNetworkedChildren { children }) =
                    world.entity(*entity).get::<HasNetworkedChildren>().cloned()
                {
                    if is_server {
                        // Children will get sequential network ids straight after the parent
                        for &child in children.iter() {
                            // TODO: DONT INSERT NORMAL GRID COMPONENT AND STUFF!!
                            NetworkCommand { entity: child }.apply(world);
                        }
                    } else {
                        let parent_identity = *world
                            .entity(*entity)
                            .get::<NetworkIdentity>()
                            .expect("network scene should always have a network identity");
                        // On the client we can rely on the child identities being sequential
                        let mut next_identity = parent_identity.next();
                        for &child in children.iter() {
                            world.entity_mut(child).insert(next_identity);
                            world
                                .resource_mut::<NetworkIdentities>()
                                .set_identity(child, next_identity);
                            next_identity = next_identity.next();
                        }
                    }
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
