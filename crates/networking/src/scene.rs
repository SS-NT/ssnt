use bevy::{prelude::*, scene::ScenePatch};

use crate::{
    identity::{NetworkCommand, NetworkIdentities, NetworkIdentity},
    spawning::SpawningSet,
    NetworkManager,
};

pub(crate) struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<NetworkSceneSpawner>()
            .add_message::<NetworkSceneEvent>()
            .register_type::<NetworkedChild>()
            .add_systems(
                PreUpdate,
                (
                    ApplyDeferred,
                    queue_network_scenes,
                    spawn_network_scenes,
                    ApplyDeferred,
                )
                    .chain()
                    .in_set(SpawningSet::SpawnScenes),
            );
    }
}

/// A handle to a scene that can be spawned over the network.
#[derive(Component, Default)]
pub struct NetworkScene(pub(crate) Handle<ScenePatch>);

impl From<Handle<ScenePatch>> for NetworkScene {
    fn from(handle: Handle<ScenePatch>) -> Self {
        Self(handle)
    }
}

/// A marker component to identify child objects in scenes.
/// Children with this component will get a network identity assigned.
///
/// Note: Do not use this for detachable children. Instead spawn them normally and nest them at runtime.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct NetworkedChild;

#[derive(Message)]
pub enum NetworkSceneEvent {
    Created(Entity),
}

/// Add to an entity to attach a scene that can be networked.
#[derive(Bundle, Default)]
pub struct NetworkSceneBundle {
    pub scene: NetworkScene,
    pub transform: Transform,
    pub visibility: Visibility,
}

#[derive(Resource, Default)]
struct NetworkSceneSpawner {
    scenes_to_spawn: Vec<(Entity, Handle<ScenePatch>)>,
}

fn queue_network_scenes(
    query: Query<(Entity, &NetworkScene), Added<NetworkScene>>,
    mut spawner: ResMut<NetworkSceneSpawner>,
) {
    for (entity, network_scene) in query.iter() {
        spawner
            .scenes_to_spawn
            .push((entity, network_scene.0.clone()));
    }
}

/// Collects all descendants of `root` marked with [`NetworkedChild`], in depth-first
/// order. This order is deterministic for a given scene, so client and server agree.
fn collect_networked_children(world: &World, root: Entity, out: &mut Vec<Entity>) {
    let children: Vec<Entity> = world
        .entity(root)
        .get::<Children>()
        .map(|c| c.iter().collect())
        .unwrap_or_default();
    for child in children {
        // Don't descend into nested networked scenes (e.g. limbs attached at
        // runtime). They assign their own children's identities independently,
        // and their subtree is not part of *this* scene. Crossing the boundary
        // would over-collect and desync client/server identity assignment,
        // because the live world tree differs between the two (the server nests
        // child scenes synchronously; the client re-parents them asynchronously
        // via transform sync).
        if world.entity(child).contains::<NetworkScene>() {
            continue;
        }
        if world.entity(child).contains::<NetworkedChild>() {
            out.push(child);
        }
        collect_networked_children(world, child, out);
    }
}

// Spawns loaded networked scenes into the world
fn spawn_network_scenes(world: &mut World) {
    world.resource_scope(|world, mut spawner: Mut<NetworkSceneSpawner>| {
        if spawner.scenes_to_spawn.is_empty() {
            return;
        }
        world.resource_scope(|world, scene_assets: Mut<Assets<ScenePatch>>| {
            spawner.scenes_to_spawn.retain(|(entity, scene_handle)| {
                let Some(scene) = scene_assets.get(scene_handle) else {
                    return true;
                };
                // Wait until the scene (and its dependencies) have been resolved.
                if scene.resolved.is_none() {
                    return true;
                }

                // Preserve the entity's transform so it isn't overwritten by the scene root.
                let existing_transform = world.get::<Transform>(*entity).cloned();
                // Remove existing children so we can merge them with any new scene children.
                let existing_children = world.entity_mut(*entity).take::<Children>();

                // Merge the scene's root onto the existing entity (children spawn as new entities).
                {
                    let mut entity_mut = world.entity_mut(*entity);
                    if let Err(err) = scene.apply(&mut entity_mut) {
                        warn!(entity = ?entity, "Error spawning network scene: {}", err);
                        return false;
                    }
                }

                if let Some(transform) = existing_transform {
                    world.entity_mut(*entity).insert(transform);
                }

                // Merge any existing children back in alongside the new scene children.
                if let Some(children) = existing_children {
                    let children: Vec<Entity> = children.iter().collect();
                    world.entity_mut(*entity).add_children(&children);
                }

                let is_server = world.resource::<NetworkManager>().is_server();

                // Ensure entity is networked
                if is_server {
                    NetworkCommand { entity: *entity }.apply(world);
                }

                // Handle children with network identities
                let mut networked_children = Vec::new();
                collect_networked_children(world, *entity, &mut networked_children);
                if !networked_children.is_empty() {
                    if is_server {
                        // Children will get sequential network ids straight after the parent
                        for &child in networked_children.iter() {
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
                        for &child in networked_children.iter() {
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
                    .resource_mut::<Messages<NetworkSceneEvent>>()
                    .write(NetworkSceneEvent::Created(*entity));

                false
            });
        });
    });
}
