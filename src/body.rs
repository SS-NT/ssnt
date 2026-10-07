use std::{fmt, time::Duration};

use bevy::{
    animation::{AnimatedBy, AnimationTargetId},
    asset::{AssetId, LoadedFolder},
    ecs::{
        entity::{EntityMapper, MapEntities},
        hierarchy::ChildSpawnerCommands,
        reflect::ReflectMapEntities,
        system::{EntityCommands, SystemParam},
    },
    gltf::{Gltf, GltfNode, GltfSkin},
    mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes},
    platform::collections::{HashMap, HashSet},
    prelude::*,
    reflect::TypePath,
};
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use bevy_rapier3d::prelude::Velocity;
use networking::{
    component::AppExt as ComponentAppExt,
    identity::{NetworkIdentities, NetworkIdentity},
    is_server,
    messaging::{AppExt, MessageEvent, MessageSender},
    scene::NetworkSceneBundle,
    spawning::{ClientControlled, ClientControls},
    variable::{NetworkVar, ServerVar},
    Networked, Players,
};
use physics::{ColliderGroup, Frozen, PhysicsEntityCommands};
use serde::{Deserialize, Serialize};
use utils::task::*;

use crate::{
    customization::{Appearance, CharacterAppearance},
    interaction::{
        ActiveInteraction, GenerateInteractionList, InteractionListEvents, InteractionListRequest,
        InteractionOption, InteractionSpecificity, InteractionStatus,
    },
    items::{
        clothes::{Clothing, ClothingHolder},
        containers::{
            commit_item_move, item_drag_source, item_drop_zone, Container, ContainerDropSet,
            DraggedItem, MoveItem,
        },
        Item, StoredItem, StoredItemClient,
    },
    round::PlayerAssets,
};

mod ghost;
pub mod health;

pub struct BodyPlugin;

impl Plugin for BodyPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Body>()
            .register_type::<LimbSide>()
            .register_type::<Limb>()
            .register_type::<LimbVisual>()
            .register_type::<Hand>()
            .register_type::<Cutting>()
            .register_type::<LocomotionConfig>()
            .register_type::<LocomotionState>()
            .add_network_message::<ChangeHandRequest>()
            .add_networked_component::<Hands, HandsClient>();

        if is_server(app) {
            app.register_type::<PickupInteraction>()
                .register_type::<DropInteraction>()
                .register_type::<CutInteraction>()
                .add_message::<LimbEvent>()
                .init_resource::<Tasks<SpawnCreature>>()
                .add_systems(
                    Update,
                    (
                        pickup_interaction,
                        drop_interaction,
                        cut_interaction,
                        (
                            prepare_pickup_interaction,
                            prepare_drop_interaction,
                            prepare_cut_interaction,
                        )
                            .in_set(GenerateInteractionList),
                        handle_hand_modification,
                        handle_hand_separation,
                        handle_hand_change_request,
                        (process_new_limbs, process_limb_removal, create_creature).chain(),
                    ),
                );
        } else {
            app.add_systems(EguiPrimaryContextPass, hand_ui.before(ContainerDropSet))
                .add_systems(
                    Update,
                    (
                        client_update_limbs,
                        client_hands_keybind,
                        update_limb_visuals,
                        (
                            build_body_skeleton,
                            skin_body_limbs,
                            unskin_limbs,
                            skin_clothing,
                            unskin_clothing,
                            update_hidden_body_parts,
                        )
                            .chain(),
                        drive_locomotion,
                    ),
                );
        }

        app.add_plugins((health::HealthPlugin, ghost::GhostPlugin));

        app.insert_resource(BodyAssets {
            scenes: app
                .world()
                .resource::<AssetServer>()
                .load_folder("creatures"),
        });
    }
}

#[derive(Component, Default, Reflect)]
#[reflect(Component, MapEntities, Default)]
pub struct Body {
    limbs: HashSet<Entity>,
    added_limbs: Vec<Entity>,
    limbs_to_remove: Vec<Entity>,
}

impl MapEntities for Body {
    fn map_entities<E: EntityMapper>(&mut self, entity_mapper: &mut E) {
        self.limbs = self
            .limbs
            .iter()
            .map(|e| entity_mapper.get_mapped(*e))
            .collect();
    }
}

#[derive(Reflect, Default)]
#[reflect(Default)]
pub enum LimbSide {
    #[default]
    Left,
    Right,
}

impl fmt::Display for LimbSide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LimbSide::Left => f.write_str("Left"),
            LimbSide::Right => f.write_str("Right"),
        }
    }
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct Limb {
    attachment_position: Vec3,
    /// Name of the rig bone this limb is primarily bound to.
    bone: String,
}

impl Limb {
    pub(crate) fn local_translation(&self, parent: Option<&Limb>) -> Vec3 {
        self.attachment_position - parent.map_or(Vec3::ZERO, |parent| parent.attachment_position)
    }
}

/// Marks a limb's visual mesh child.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
#[require(Visibility = Visibility::Hidden)]
pub struct LimbVisual;

#[derive(Message)]
struct LimbEvent {
    limb_entity: Entity,
    kind: LimbEventKind,
}

#[derive(PartialEq, Eq)]
enum LimbEventKind {
    Added,
    Removed,
}

fn process_new_limbs(
    mut bodies: Query<&mut Body, Changed<Body>>,
    limbs: Query<&Limb>,
    parents: Query<&ChildOf>,
    mut transforms: Query<&mut Transform>,
    mut writer: MessageWriter<LimbEvent>,
    mut commands: Commands,
) {
    for mut body in bodies.iter_mut() {
        body.added_limbs.retain(|&limb_entity| {
            let Ok(limb) = limbs.get(limb_entity) else {
                return true;
            };
            // attachment_position is body-relative; nest the limb via its transform
            // relative to its parent limb
            let parent = parents
                .get(limb_entity)
                .ok()
                .and_then(|child_of| limbs.get(child_of.parent()).ok());
            let Ok(mut transform) = transforms.get_mut(limb_entity) else {
                return true;
            };
            transform.translation = limb.local_translation(parent);
            commands
                .entity(limb_entity)
                .freeze(Some(ColliderGroup::AttachedLimbs));
            writer.write(LimbEvent {
                limb_entity,
                kind: LimbEventKind::Added,
            });
            false
        });
    }
}

fn process_limb_removal(
    mut bodies: Query<&mut Body, Changed<Body>>,
    mut transforms: Query<(&mut Transform, &GlobalTransform)>,
    mut writer: MessageWriter<LimbEvent>,
    mut commands: Commands,
) {
    for mut body in bodies.iter_mut() {
        let body = body.as_mut();
        for limb_entity in body.limbs_to_remove.drain(..) {
            if !body.limbs.remove(&limb_entity) {
                continue;
            }
            if let Ok((mut transform, global_transform)) = transforms.get_mut(limb_entity) {
                *transform = global_transform.compute_transform();
            }
            commands
                .entity(limb_entity)
                .remove::<ChildOf>()
                .unfreeze(Some(ColliderGroup::Default));
            writer.write(LimbEvent {
                limb_entity,
                kind: LimbEventKind::Removed,
            });
        }
    }
}

fn client_update_limbs(
    mut added_limbs: Query<(Entity, &ChildOf), (Or<(Added<Limb>, Changed<ChildOf>)>,)>,
    parents: Query<&ChildOf>,
    hands: Query<(), With<Hand>>,
    mut bodies: Query<&mut Body, With<ClientControlled>>,
) {
    for (limb_entity, limb_parent) in added_limbs.iter_mut() {
        // HACK: assume limb is handled as item if nested under hands
        if hands.contains(limb_parent.parent()) {
            continue;
        }

        let Some(body_entity) = parents
            .iter_ancestors(limb_entity)
            .find(|&e| bodies.contains(e))
        else {
            continue;
        };
        let mut body = bodies.get_mut(body_entity).unwrap();
        body.limbs.insert(limb_entity);
    }
    // TODO: removed limbs
}

/// Offset each limb's visual child by the negated (body-relative) attachment, centring
/// the shared-model-space mesh on the limb body.
fn update_limb_visuals(
    visuals: Query<(Entity, &ChildOf), With<LimbVisual>>,
    limbs: Query<&Limb>,
    mut transforms: Query<&mut Transform>,
) {
    for (visual, child_of) in &visuals {
        let Ok(limb) = limbs.get(child_of.parent()) else {
            continue;
        };
        let target = -limb.attachment_position;
        if let Ok(mut transform) = transforms.get_mut(visual) {
            if transform.translation != target {
                transform.translation = target;
            }
        }
    }
}

/// Shared skeleton for one body instance; every attached limb's `SkinnedMesh`
/// references these same joints.
#[derive(Component)]
struct BodySkeleton {
    joints: Vec<Entity>,
    inverse_bindposes: Handle<SkinnedMeshInverseBindposes>,
}

/// A body whose skeleton still needs building once its rig glTF has loaded.
#[derive(Component)]
struct PendingSkeleton(Handle<Gltf>);

/// Build a body's shared skeleton from its rig's glTF skin.
fn build_body_skeleton(
    pending: Query<(Entity, &PendingSkeleton)>,
    gltfs: Res<Assets<Gltf>>,
    skins: Res<Assets<GltfSkin>>,
    nodes: Res<Assets<GltfNode>>,
    configs: Query<&LocomotionConfig>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut commands: Commands,
) {
    for (body, PendingSkeleton(handle)) in &pending {
        let Some(gltf) = gltfs.get(handle) else {
            continue;
        };
        let Some(skin_handle) = gltf.skins.first() else {
            continue;
        };
        let Some(skin) = skins.get(skin_handle) else {
            continue;
        };

        // Bail until every joint node is loaded so we never spawn a partial skeleton.
        let Some(node_refs) = skin
            .joints
            .iter()
            .map(|handle| nodes.get(handle))
            .collect::<Option<Vec<&GltfNode>>>()
        else {
            continue;
        };

        // One entity per joint, in skin order (matches the meshes' JOINTS_0 indices).
        let mut node_to_entity: HashMap<AssetId<GltfNode>, Entity> = HashMap::default();
        let mut joint_names: HashMap<AssetId<GltfNode>, Name> = HashMap::default();
        let mut joints = Vec::with_capacity(node_refs.len());
        for (handle, node) in skin.joints.iter().zip(&node_refs) {
            let name = Name::new(node.name.clone());
            let entity = commands.spawn((node.transform, name.clone())).id();
            node_to_entity.insert(handle.id(), entity);
            joint_names.insert(handle.id(), name);
            joints.push(entity);
        }

        // Rebuild the bone hierarchy from each node's children.
        let mut is_child: HashSet<AssetId<GltfNode>> = HashSet::default();
        let mut parent_of: HashMap<AssetId<GltfNode>, AssetId<GltfNode>> = HashMap::default();
        for (handle, node) in skin.joints.iter().zip(&node_refs) {
            let parent = node_to_entity[&handle.id()];
            for child in &node.children {
                if let Some(&child_entity) = node_to_entity.get(&child.id()) {
                    commands.entity(child_entity).insert(ChildOf(parent));
                    is_child.insert(child.id());
                    parent_of.insert(child.id(), handle.id());
                }
            }
        }

        // Joint transforms are relative to the armature node, which may carry a rotation,
        // so we create an armature entity to put all the bones on
        let root_joints: HashSet<AssetId<GltfNode>> = skin
            .joints
            .iter()
            .map(|handle| handle.id())
            .filter(|id| !is_child.contains(id))
            .collect();
        let armature_node = gltf
            .nodes
            .iter()
            .filter_map(|handle| nodes.get(handle))
            .find(|node| {
                node.children
                    .iter()
                    .any(|child| root_joints.contains(&child.id()))
            });
        let armature_transform = armature_node.map_or(Transform::IDENTITY, |node| node.transform);
        let armature_name =
            Name::new(armature_node.map_or_else(String::new, |node| node.name.clone()));
        let armature = commands.spawn((armature_transform, ChildOf(body))).id();
        for id in &root_joints {
            commands
                .entity(node_to_entity[id])
                .insert(ChildOf(armature));
        }

        // Tag every bone as an animation target of the body's player. The target id hashes the
        // bone's name path from the armature.
        for id in skin.joints.iter().map(|handle| handle.id()) {
            let mut path = vec![joint_names[&id].clone()];
            let mut cursor = parent_of.get(&id).copied();
            while let Some(parent) = cursor {
                path.push(joint_names[&parent].clone());
                cursor = parent_of.get(&parent).copied();
            }
            path.push(armature_name.clone());
            path.reverse();
            commands
                .entity(node_to_entity[&id])
                .insert((AnimationTargetId::from_names(path.iter()), AnimatedBy(body)));
        }

        // Locomotion graph
        let config = configs.get(body).cloned().unwrap_or_default();
        let mut graph = AnimationGraph::new();
        let mut states: Vec<ResolvedState> = config
            .states
            .iter()
            .filter_map(|state| {
                let Some(clip) = gltf.named_animations.get(state.animation.as_str()) else {
                    warn!("Locomotion clip {:?} missing from rig", state.animation);
                    return None;
                };
                Some(ResolvedState {
                    clip: graph.add_clip(clip.clone(), 1.0, graph.root),
                    min_speed: state.min_speed,
                    reference_speed: state.reference_speed,
                })
            })
            .collect();
        if !states.is_empty() {
            states.sort_by(|a, b| a.min_speed.total_cmp(&b.min_speed));
            commands.entity(body).insert((
                AnimationPlayer::default(),
                AnimationGraphHandle(graphs.add(graph)),
                AnimationTransitions::new(),
                LocomotionAnimations {
                    states,
                    blend: Duration::from_secs_f32(config.blend_seconds),
                },
            ));
        }

        commands
            .entity(body)
            .remove::<PendingSkeleton>()
            .insert(BodySkeleton {
                joints,
                inverse_bindposes: skin.inverse_bind_matrices.clone(),
            });
    }
}

/// Skin attached limbs' visual meshes, kicking off the skeleton build lazily on first
/// need. Also handles reattaching a limb to another body.
fn skin_body_limbs(
    visuals: Query<(Entity, &ChildOf), (With<LimbVisual>, With<Mesh3d>, Without<SkinnedMesh>)>,
    limbs: Query<&Limb>,
    parents: Query<&ChildOf>,
    hands: Query<(), With<Hand>>,
    bodies: Query<(), With<Body>>,
    skeletons: Query<&BodySkeleton>,
    pending: Query<(), With<PendingSkeleton>>,
    assets: Res<PlayerAssets>,
    mut commands: Commands,
) {
    for (visual_entity, visual_parent) in &visuals {
        let limb_entity = visual_parent.parent();
        let Ok(limb) = limbs.get(limb_entity) else {
            continue;
        };
        if limb.bone.is_empty() {
            continue; // not rigged for skinning
        }
        if parents
            .get(limb_entity)
            .is_ok_and(|child_of| hands.contains(child_of.parent()))
        {
            continue; // held item, not an attached limb
        }
        let Some(body) = parents
            .iter_ancestors(limb_entity)
            .find(|&e| bodies.contains(e))
        else {
            continue;
        };

        match skeletons.get(body) {
            // All limbs share the rig, so joints + bindposes are cloned wholesale.
            Ok(skeleton) => {
                commands.entity(visual_entity).insert(SkinnedMesh {
                    inverse_bindposes: skeleton.inverse_bindposes.clone(),
                    joints: skeleton.joints.clone(),
                });
            }
            // No skeleton yet: build one for this body's rig.
            Err(_) => {
                if !pending.contains(body) {
                    if let Some(rig) = assets.player_model.clone() {
                        commands.entity(body).insert(PendingSkeleton(rig));
                    }
                }
            }
        }
    }
}

/// Drop skinning from a limb's visual meshes once the limb is no longer attached.
fn unskin_limbs(
    mut detached: RemovedComponents<ChildOf>,
    reparented: Query<Entity, (With<Limb>, Changed<ChildOf>)>,
    limbs: Query<(), With<Limb>>,
    children: Query<&Children>,
    skinned_visuals: Query<(), (With<LimbVisual>, With<SkinnedMesh>)>,
    parents: Query<&ChildOf>,
    hands: Query<(), With<Hand>>,
    skeletons: Query<(), With<BodySkeleton>>,
    mut commands: Commands,
) {
    let mut seen: HashSet<Entity> = HashSet::default();
    for limb_entity in detached.read().chain(reparented.iter()) {
        if !seen.insert(limb_entity) || !limbs.contains(limb_entity) {
            continue;
        }

        let parent = parents.get(limb_entity).ok().map(ChildOf::parent);
        let under_hand = parent.is_some_and(|p| hands.contains(p));
        let attached = !under_hand
            && parents
                .iter_ancestors(limb_entity)
                .any(|e| skeletons.contains(e));
        if attached {
            continue; // still an attached limb, keep it skinned
        }

        let Ok(limb_children) = children.get(limb_entity) else {
            continue;
        };
        for &child in limb_children {
            if skinned_visuals.contains(child) {
                commands.entity(child).remove::<SkinnedMesh>();
            }
        }
    }
}

/// Skin equipped clothing to the wearer's shared skeleton.
fn skin_clothing(
    clothing: Query<(Entity, &ChildOf), (With<Clothing>, With<Mesh3d>, Without<SkinnedMesh>)>,
    holders: Query<(), With<ClothingHolder>>,
    parents: Query<&ChildOf>,
    bodies: Query<(), With<Body>>,
    skeletons: Query<&BodySkeleton>,
    pending: Query<(), With<PendingSkeleton>>,
    assets: Res<PlayerAssets>,
    mut commands: Commands,
) {
    for (clothing_entity, clothing_parent) in &clothing {
        if !holders.contains(clothing_parent.parent()) {
            continue; // not worn in a slot
        }
        let Some(body) = parents
            .iter_ancestors(clothing_entity)
            .find(|&e| bodies.contains(e))
        else {
            continue;
        };

        match skeletons.get(body) {
            Ok(skeleton) => {
                commands.entity(clothing_entity).insert(SkinnedMesh {
                    inverse_bindposes: skeleton.inverse_bindposes.clone(),
                    joints: skeleton.joints.clone(),
                });
            }
            // No skeleton yet: build one for this body's rig.
            Err(_) => {
                if !pending.contains(body) {
                    if let Some(rig) = assets.player_model.clone() {
                        commands.entity(body).insert(PendingSkeleton(rig));
                    }
                }
            }
        }
    }
}

/// Drop skinning from clothing once it's no longer worn.
fn unskin_clothing(
    mut detached: RemovedComponents<ChildOf>,
    reparented: Query<Entity, (With<Clothing>, Changed<ChildOf>)>,
    skinned: Query<(), (With<Clothing>, With<SkinnedMesh>)>,
    holders: Query<(), With<ClothingHolder>>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    let mut seen: HashSet<Entity> = HashSet::default();
    for clothing_entity in detached.read().chain(reparented.iter()) {
        if !seen.insert(clothing_entity) || !skinned.contains(clothing_entity) {
            continue;
        }
        let worn = parents
            .get(clothing_entity)
            .is_ok_and(|child_of| holders.contains(child_of.parent()));
        if worn {
            continue; // still worn, keep it skinned
        }
        commands.entity(clothing_entity).remove::<SkinnedMesh>();
    }
}

/// Hide limb visuals covered by worn clothing so skin doesn't clip through clothes.
///
/// Each worn [`Clothing`] lists the [`Limb::bone`] names it covers; a limb is hidden when
/// any worn item covers it, and revealed once nothing does. Recomputed from scratch per body
/// whenever clothing is equipped, moved, or removed, so overlapping items compose correctly.
fn update_hidden_body_parts(
    changed_clothing: Query<(), (With<Clothing>, Or<(Added<Clothing>, Changed<ChildOf>)>)>,
    added_limb_visuals: Query<(), Added<LimbVisual>>,
    mut removed_children: RemovedComponents<ChildOf>,
    bodies: Query<Entity, With<Body>>,
    children: Query<&Children>,
    clothing: Query<(&Clothing, &ChildOf)>,
    holders: Query<(), With<ClothingHolder>>,
    limb_visuals: Query<&ChildOf, With<LimbVisual>>,
    limbs: Query<&Limb>,
    mut visibility: Query<&mut Visibility>,
) {
    // Scene-backed clothing and limb visuals can resolve in either order during spawning.
    let any_removed = removed_children.read().count() > 0;
    if changed_clothing.is_empty() && added_limb_visuals.is_empty() && !any_removed {
        return;
    }

    for body in &bodies {
        // Union of bones covered by every item currently worn in a slot on this body.
        let mut hidden: HashSet<String> = HashSet::default();
        for descendant in children.iter_descendants(body) {
            if let Ok((cloth, parent)) = clothing.get(descendant) {
                if holders.contains(parent.parent()) {
                    hidden.extend(cloth.hidden_body_parts().iter().cloned());
                }
            }
        }

        // Apply the result to each limb visual under this body.
        for descendant in children.iter_descendants(body) {
            let Ok(visual_parent) = limb_visuals.get(descendant) else {
                continue;
            };
            let hide = limbs
                .get(visual_parent.parent())
                .is_ok_and(|limb| hidden.contains(&limb.bone));
            let target = if hide {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            };
            if let Ok(mut vis) = visibility.get_mut(descendant) {
                if *vis != target {
                    *vis = target;
                }
            }
        }
    }
}

#[derive(Reflect, Clone, Default)]
#[reflect(Default)]
pub struct LocomotionState {
    /// Name of the animation clip in the rig's glTF.
    pub animation: String,
    /// Minimum horizontal speed (m/s) at which this state becomes active.
    pub min_speed: f32,
    /// Speed the clip was authored at; playback scales `speed / reference_speed` so footfalls
    /// track the ground.
    pub reference_speed: f32,
}

#[derive(Component, Reflect, Clone)]
#[reflect(Component, Default)]
pub struct LocomotionConfig {
    pub states: Vec<LocomotionState>,
    /// Crossfade duration (seconds) between states.
    pub blend_seconds: f32,
}

impl Default for LocomotionConfig {
    fn default() -> Self {
        Self {
            states: vec![LocomotionState {
                animation: "HumanIdle".to_string(),
                min_speed: 0.0,
                reference_speed: 0.0,
            }],
            blend_seconds: 0.25,
        }
    }
}

/// A resolved locomotion state: graph node plus the speeds copied from its [`LocomotionState`].
struct ResolvedState {
    clip: AnimationNodeIndex,
    min_speed: f32,
    reference_speed: f32,
}

/// Locomotion states resolved against a body's animation graph, sorted by `min_speed`.
#[derive(Component)]
struct LocomotionAnimations {
    states: Vec<ResolvedState>,
    blend: Duration,
}

// Bounds on speed-matched playback so extreme velocities don't play a clip absurdly fast/slow.
const PLAYBACK_MIN: f32 = 0.5;
const PLAYBACK_MAX: f32 = 1.6;

/// Drive each rigged body's animation from its horizontal speed, picking the fastest state it
/// has reached and scaling that clip's playback so footfalls roughly track the ground.
fn drive_locomotion(
    mut bodies: Query<(
        &Velocity,
        &LocomotionAnimations,
        &mut AnimationPlayer,
        &mut AnimationTransitions,
    )>,
) {
    for (velocity, anims, mut player, mut transitions) in &mut bodies {
        let speed = velocity.linear.xz().length();
        // States are sorted ascending, so the last match is the fastest reached state
        let state = anims
            .states
            .iter()
            .rev()
            .find(|state| speed >= state.min_speed)
            .unwrap_or(&anims.states[0]);
        let playback = if state.reference_speed > 0.0 {
            (speed / state.reference_speed).clamp(PLAYBACK_MIN, PLAYBACK_MAX)
        } else {
            1.0
        };

        if transitions.get_main_animation() != Some(state.clip) {
            transitions
                .play(&mut player, state.clip, anims.blend)
                .repeat();
        }
        if let Some(active) = player.animation_mut(state.clip) {
            active.set_speed(playback);
        }
    }
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct Hand {
    pub side: LimbSide,
    order: u32,
}

#[derive(Component, TypePath, Networked)]
#[networked(client = "HandsClient")]
pub struct Hands {
    #[networked(
        with = "Self::network_active_hand(Res<'static, NetworkIdentities>) -> NetworkIdentity"
    )]
    active_hand: NetworkVar<Entity>,
}

impl Hands {
    pub fn active_hand(&self) -> Entity {
        *self.active_hand
    }

    fn network_active_hand(entity: &Entity, param: Res<NetworkIdentities>) -> NetworkIdentity {
        param
            .get_identity(*entity)
            .expect("Hand entity must have network identity")
    }
}

#[derive(Component, Networked, TypePath, Default)]
#[networked(server = "Hands")]
pub struct HandsClient {
    active_hand: ServerVar<NetworkIdentity>,
}

impl HandsClient {
    pub fn active_hand(&self) -> NetworkIdentity {
        *self.active_hand
    }
}

/// Get the item currently held by the player with their active hand
#[derive(SystemParam)]
pub struct ClientHeldItem<'w, 's> {
    client_body: Query<'w, 's, &'static HandsClient, With<ClientControlled>>,
    child_query: Query<'w, 's, &'static Children>,
    items: Query<'w, 's, Entity, With<StoredItemClient>>,
    identities: Res<'w, NetworkIdentities>,
}

impl<'w, 's> ClientHeldItem<'w, 's> {
    pub fn get(&self) -> Option<Entity> {
        let hands = self.client_body.single().ok()?;
        let active_hand = self.identities.get_entity(hands.active_hand())?;
        let children = self.child_query.get(active_hand).ok()?;
        self.items.iter_many(children.iter()).next()
    }
}

/// Returns the controlled body's active hand if it's free.
#[derive(SystemParam)]
pub struct ClientFreeHand<'w, 's> {
    client_body: Query<'w, 's, &'static HandsClient, With<ClientControlled>>,
    hands: Query<'w, 's, (Entity, &'static NetworkIdentity), With<Hand>>,
    children: Query<'w, 's, &'static Children>,
    items: Query<'w, 's, (), With<StoredItemClient>>,
    identities: Res<'w, NetworkIdentities>,
}

impl ClientFreeHand<'_, '_> {
    pub fn get_active_empty(&self) -> Option<(Entity, NetworkIdentity)> {
        let hands = self.client_body.single().ok()?;
        let active = self.identities.get_entity(hands.active_hand())?;
        let (entity, &identity) = self.hands.get(active).ok()?;
        let free = match self.children.get(entity) {
            Ok(children) => self.items.iter_many(children).next().is_none(),
            Err(_) => true,
        };
        free.then_some((entity, identity))
    }
}

/// Updates the selected hand when limbs of a body get changed
fn handle_hand_modification(
    mut bodies: Query<(Entity, &Body, Option<&mut Hands>), Changed<Body>>,
    hands: Query<(), With<Hand>>,
    mut commands: Commands,
) {
    for (body_entity, body, existing_hands) in bodies.iter_mut() {
        // TODO: We should only check added and removed limbs
        let current_hands: HashSet<_> = body
            .limbs
            .iter()
            .copied()
            .filter(|entity| hands.contains(*entity))
            .collect();
        if let Some(mut hands) = existing_hands {
            // We still have the hand that's currently active, nothing to change
            if current_hands.contains(&*(hands.active_hand)) {
                continue;
            }
            // If we lost that hand, choose a random one or remove hands entirely
            match current_hands.iter().next() {
                Some(&hand) => *hands.active_hand = hand,
                None => {
                    commands.entity(body_entity).remove::<Hands>();
                }
            };
        } else if let Some(&first_hand) = current_hands.iter().next() {
            commands.entity(body_entity).insert(Hands {
                active_hand: first_hand.into(),
            });
        }
    }
}

fn handle_hand_separation(
    mut events: MessageReader<LimbEvent>,
    hands: Query<&Container, With<Hand>>,
    mut move_items: ResMut<Tasks<MoveItem>>,
) {
    for event in events.read() {
        if event.kind != LimbEventKind::Removed {
            continue;
        }
        let Ok(container) = hands.get(event.limb_entity) else {
            continue;
        };
        if container.is_empty() {
            continue;
        }
        // Drop all items this hand is holding
        // TODO: This should be handled in health system (ex. no nerve signal)
        for item in container.iter().map(|(_, i)| *i) {
            move_items.create_ignore(MoveItem {
                item,
                container: None,
                position: None,
            });
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ChangeHandRequest {
    identity: NetworkIdentity,
}

#[allow(clippy::too_many_arguments)]
fn hand_ui(
    mut contexts: EguiContexts,
    mut bodies: Query<(&Body, &mut HandsClient), With<ClientControlled>>,
    hands: Query<(Entity, &NetworkIdentity, &Hand, Option<&Children>)>,
    mut items: Query<(Entity, &Item, &NetworkIdentity, &mut StoredItemClient)>,
    mut ordered_hands: Local<Vec<(Entity, u32)>>,
    mut dragged: ResMut<DraggedItem>,
    mut sender: MessageSender,
    mut commands: Commands,
) {
    let Ok((body, hand_data)) = bodies.single_mut() else {
        return;
    };

    egui::Window::new("hands")
        .title_bar(false)
        .anchor(egui::Align2::CENTER_BOTTOM, egui::Vec2::ZERO)
        .resizable(false)
        .show(contexts.ctx_mut().unwrap(), |ui| {
            ui.horizontal_wrapped(|ui| {
                // Order hands for display
                ordered_hands.clear();
                ordered_hands.extend(
                    hands
                        .iter_many(&body.limbs)
                        .map(|(entity, .., hand, _)| (entity, hand.order)),
                );
                ordered_hands.sort_unstable_by_key(|(_, k)| *k);

                let hand_entities: Vec<Entity> =
                    ordered_hands.iter().map(|(entity, _)| *entity).collect();
                for hand_entity in hand_entities {
                    let Ok((_, &identity, hand, children)) = hands.get(hand_entity) else {
                        continue;
                    };

                    // Copy out the held item so we no longer borrow `items` below.
                    let held_item = children.and_then(|children| {
                        items
                            .iter_many(children)
                            .next()
                            .map(|(entity, item, &id, _)| {
                                (entity, id, item.name.clone(), item.size)
                            })
                    });

                    let name = held_item
                        .as_ref()
                        .map(|(_, _, name, _)| name.as_str())
                        .unwrap_or("empty");
                    let response = ui
                        .selectable_label(
                            identity == *hand_data.active_hand,
                            format!("{}: {}", hand.side, name),
                        )
                        .interact(egui::Sense::click_and_drag());

                    if let Some((item_entity, _, item_name, item_size)) = &held_item {
                        item_drag_source(
                            ui,
                            &response,
                            &mut dragged,
                            *item_entity,
                            *item_size,
                            item_name,
                        );
                    }

                    let occupied_by_other = held_item.as_ref().is_some_and(|(entity, ..)| {
                        dragged
                            .info
                            .as_ref()
                            .is_some_and(|info| *entity != info.entity)
                    });
                    if item_drop_zone(ui, response.rect, &mut dragged, !occupied_by_other) {
                        let dragged_entity = dragged.info.as_ref().unwrap().entity;
                        if let Ok((item_entity, _, &item_identity, mut stored)) =
                            items.get_mut(dragged_entity)
                        {
                            commit_item_move(
                                &mut dragged,
                                &mut sender,
                                &mut commands,
                                item_entity,
                                item_identity,
                                &mut stored,
                                hand_entity,
                                identity,
                                UVec2::ZERO,
                            );
                        }
                    }

                    // A plain click switches hand; right-click opens the item's interactions.
                    if response.clicked() {
                        sender.send_to_server(&ChangeHandRequest { identity });
                    } else if response.clicked_by(egui::PointerButton::Secondary) {
                        if let Some((_, item_identity, ..)) = &held_item {
                            sender.send_to_server(&InteractionListRequest {
                                target: *item_identity,
                            });
                        }
                    }
                }
            });
        });
}

fn client_hands_keybind(
    keyboard_input: Res<ButtonInput<KeyCode>>,
    mut bodies: Query<(&Body, &mut HandsClient), With<ClientControlled>>,
    hands: Query<&NetworkIdentity, With<Hand>>,
    mut sender: MessageSender,
) {
    if !keyboard_input.just_pressed(KeyCode::KeyX) {
        return;
    }

    let Ok((body, hand_data)) = bodies.single_mut() else {
        return;
    };

    let mut previous_was_active_hand = false;
    for &identity in hands.iter_many(&body.limbs) {
        if previous_was_active_hand {
            sender.send_to_server(&ChangeHandRequest { identity });
            return;
        }

        if *hand_data.active_hand == identity {
            previous_was_active_hand = true;
        }
    }
    // If we get here we haven't changed hands
    // Try to just change to first hand
    if let Some(&identity) = hands.iter_many(&body.limbs).next() {
        if *hand_data.active_hand == identity {
            return;
        }
        sender.send_to_server(&ChangeHandRequest { identity });
    }
}

fn handle_hand_change_request(
    mut events: MessageReader<MessageEvent<ChangeHandRequest>>,
    players: Res<Players>,
    controls: Res<ClientControls>,
    identities: Res<NetworkIdentities>,
    mut hands: Query<&mut Hands>,
) {
    for event in events.read() {
        let Some(controlled) = players
            .get(event.connection)
            .and_then(|player| controls.controlled_entity(player.id))
        else {
            continue;
        };
        let Ok(mut hands) = hands.get_mut(controlled) else {
            continue;
        };
        let Some(hand_entity) = identities.get_entity(event.message.identity) else {
            continue;
        };
        // TODO: Validate object is actually hand
        *hands.active_hand = hand_entity;
    }
}

#[derive(Resource)]
struct BodyAssets {
    // Used to keep strong handles to prevent asset unloading
    #[allow(dead_code)]
    scenes: Handle<LoadedFolder>,
}

/// Task to create the body of a given creature archetype
pub struct SpawnCreature {
    pub archetype: String,
    /// The customized appearance to apply and network to observers.
    pub appearance: CharacterAppearance,
}

impl Task for SpawnCreature {
    type Result = SpawnCreatureResult;
}

pub struct SpawnCreatureResult {
    pub root: Entity,
}

fn spawn_limb<'a>(
    builder: &'a mut ChildSpawnerCommands,
    server: &AssetServer,
    name: &str,
) -> EntityCommands<'a> {
    builder.spawn(NetworkSceneBundle {
        scene: server.load(format!("creatures/{}.bsn", name)).into(),
        ..Default::default()
    })
}

/// Spawns the full limb hierarchy for a human body and returns the root entity.
pub(crate) fn build_human_creature(
    commands: &mut Commands,
    server: &AssetServer,
    frozen: bool,
) -> Entity {
    let mut creature = commands.spawn(NetworkSceneBundle {
        scene: server.load("creatures/player.bsn").into(),
        ..Default::default()
    });
    let mut limbs = HashSet::default();
    creature.with_children(|builder| {
        let torso = spawn_limb(builder, server, "human_torso")
            .with_children(|builder| {
                // Head
                let head = spawn_limb(builder, server, "human_head")
                    .with_children(|builder| {
                        let brain = spawn_limb(builder, server, "organic_brain").id();
                        limbs.insert(brain);
                    })
                    .id();
                limbs.insert(head);

                // Arms
                let arm_left = spawn_limb(builder, server, "human_arm_left")
                    .with_children(|builder| {
                        let hand_left = spawn_limb(builder, server, "human_hand_left").id();
                        limbs.insert(hand_left);
                    })
                    .id();
                limbs.insert(arm_left);
                let arm_right = spawn_limb(builder, server, "human_arm_right")
                    .with_children(|builder| {
                        let hand_right = spawn_limb(builder, server, "human_hand_right").id();
                        limbs.insert(hand_right);
                    })
                    .id();
                limbs.insert(arm_right);

                // Legs
                let leg_left = spawn_limb(builder, server, "human_leg_left")
                    .with_children(|builder| {
                        let foot_left = spawn_limb(builder, server, "human_foot_left").id();
                        limbs.insert(foot_left);
                    })
                    .id();
                limbs.insert(leg_left);
                let leg_right = spawn_limb(builder, server, "human_leg_right")
                    .with_children(|builder| {
                        let foot_right = spawn_limb(builder, server, "human_foot_right").id();
                        limbs.insert(foot_right);
                    })
                    .id();
                limbs.insert(leg_right);

                let heart = spawn_limb(builder, server, "organic_heart").id();
                limbs.insert(heart);
                let lung = spawn_limb(builder, server, "organic_lung").id();
                limbs.insert(lung);
            })
            .id();
        limbs.insert(torso);
    });
    let limb_list: Vec<Entity> = limbs.iter().copied().collect();
    creature.insert(Body {
        limbs,
        added_limbs: limb_list.clone(),
        ..Default::default()
    });
    let creature_entity = creature.id();

    if frozen {
        commands.entity(creature_entity).insert(Frozen);
        for limb in limb_list {
            commands.entity(limb).insert(Frozen);
        }
    }

    creature_entity
}

fn create_creature(
    mut tasks: ResMut<Tasks<SpawnCreature>>,
    server: Res<AssetServer>,
    mut commands: Commands,
) {
    tasks.process(|data| {
        // TODO: Replace with species configuration in assets
        let root = match data.archetype.as_str() {
            "human" => build_human_creature(&mut commands, server.as_ref(), false),
            _ => todo!(),
        };
        commands
            .entity(root)
            .insert(Appearance::new(data.appearance));

        bevy::log::info!("Created creature");
        SpawnCreatureResult { root }
    });
}

#[derive(Component, Reflect)]
#[reflect(Component)]
#[component(storage = "SparseSet")]
struct PickupInteraction {
    #[reflect(ignore)]
    move_task: Option<TaskId<MoveItem>>,
}

impl PickupInteraction {
    fn new() -> Self {
        Self { move_task: None }
    }
}

// Dummy implementation for reflection
impl FromWorld for PickupInteraction {
    fn from_world(_: &mut World) -> Self {
        Self::new()
    }
}

fn prepare_pickup_interaction(
    interaction_lists: Res<InteractionListEvents>,
    items: Query<&Item>,
    bodies: Query<(&Body, &Hands)>,
    hand_query: Query<(&Hand, &Container)>,
) {
    for event in interaction_lists.events.iter() {
        let Ok(_) = items.get(event.target) else {
            continue;
        };

        let Ok((body, hands)) = bodies.get(event.source) else {
            continue;
        };

        let hand_entity = *hands.active_hand;
        if !body.limbs.contains(&hand_entity) {
            continue;
        }

        let Ok((_, hand_container)) = hand_query.get(hand_entity) else {
            continue;
        };

        if !hand_container.is_empty() {
            continue;
        }

        event.add_interaction(InteractionOption {
            text: "Pick Up".into(),
            interaction: Box::new(PickupInteraction::new()),
            specificity: InteractionSpecificity::Generic,
        });
    }
}

fn pickup_interaction(
    mut query: Query<(Entity, &mut PickupInteraction, &mut ActiveInteraction)>,
    items: Query<&Item>,
    hands: Query<&Hands>,
    hand_query: Query<(Entity, &Hand, &Container)>,
    mut item_moves: ResMut<Tasks<MoveItem>>,
) {
    for (source, mut interaction, mut active) in query.iter_mut() {
        if interaction.move_task.is_some() {
            continue;
        }

        let Ok(_) = items.get(active.target) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        let Ok(hands) = hands.get(source) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        let Ok((hand_entity, _, hand_container)) = hand_query.get(*hands.active_hand) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        if !hand_container.is_empty() {
            active.status = InteractionStatus::Canceled;
            continue;
        }

        // Creating a task to move the target item
        let id = item_moves.create(MoveItem {
            item: active.target,
            container: Some(hand_entity),
            position: Some(UVec2::ZERO),
        });
        interaction.move_task = Some(id);
    }

    // Check for completed container moves
    for (_, interaction, mut active) in query.iter_mut() {
        let Some(task) = interaction.move_task else {
            continue;
        };
        if let Some(result) = item_moves.result(task) {
            active.status = if result.was_success() {
                InteractionStatus::Completed
            } else {
                InteractionStatus::Canceled
            };
        }
    }
}

#[derive(Component, Reflect)]
#[reflect(Component)]
#[component(storage = "SparseSet")]
struct DropInteraction {
    #[reflect(ignore)]
    move_task: Option<TaskId<MoveItem>>,
}

impl DropInteraction {
    fn new() -> Self {
        Self { move_task: None }
    }
}

// Dummy implementation for reflection
impl FromWorld for DropInteraction {
    fn from_world(_: &mut World) -> Self {
        Self::new()
    }
}

fn prepare_drop_interaction(
    interaction_list: Res<InteractionListEvents>,
    items: Query<&StoredItem>,
    bodies: Query<&Body>,
    hand_query: Query<Entity, With<Hand>>,
) {
    for event in interaction_list.events.iter() {
        let Ok(stored) = items.get(event.target) else {
            continue;
        };
        let container_entity = stored.container();

        let Ok(body) = bodies.get(event.source) else {
            continue;
        };

        let Some(_) = hand_query
            .iter_many(&body.limbs)
            .find(|entity| container_entity == *entity)
        else {
            continue;
        };

        event.add_interaction(InteractionOption {
            text: "Drop".into(),
            interaction: Box::new(DropInteraction::new()),
            specificity: InteractionSpecificity::Generic,
        });
    }
}

fn drop_interaction(
    mut query: Query<(Entity, &mut DropInteraction, &mut ActiveInteraction)>,
    items: Query<&StoredItem>,
    bodies: Query<&Body>,
    hand_query: Query<Entity, With<Hand>>,
    mut item_moves: ResMut<Tasks<MoveItem>>,
) {
    for (source, mut interaction, mut active) in query.iter_mut() {
        if interaction.move_task.is_some() {
            continue;
        }

        let Ok(stored) = items.get(active.target) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };
        let container_entity = stored.container();

        let Ok(body) = bodies.get(source) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        let Some(_) = hand_query
            .iter_many(&body.limbs)
            .find(|entity| container_entity == *entity)
        else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        // Creating a task to move the item held
        let id = item_moves.create(MoveItem {
            item: active.target,
            container: None,
            position: None,
        });
        interaction.move_task = Some(id);
    }

    // Check for completed container moves
    for (_, interaction, mut active) in query.iter_mut() {
        let Some(task) = interaction.move_task else {
            continue;
        };
        if let Some(result) = item_moves.result(task) {
            active.status = if result.was_success() {
                InteractionStatus::Completed
            } else {
                InteractionStatus::Canceled
            };
        }
    }
}

// NOTE: This is just for funny content

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Cutting {}

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
#[component(storage = "SparseSet")]
struct CutInteraction {}

fn prepare_cut_interaction(
    interaction_list: Res<InteractionListEvents>,
    cutting_items: Query<(), (With<Item>, With<Cutting>)>,
) {
    for event in interaction_list.events.iter() {
        let Some(item) = event.item_in_hand else {
            continue;
        };

        if !cutting_items.contains(item) {
            continue;
        }

        event.add_interaction(InteractionOption {
            text: "Cut".into(),
            interaction: Box::<CutInteraction>::default(),
            specificity: InteractionSpecificity::Specific,
        });
    }
}

fn cut_interaction(
    mut query: Query<(&mut CutInteraction, &mut ActiveInteraction)>,
    mut bodies: Query<&mut Body>,
) {
    for (_, mut active) in query.iter_mut() {
        let Ok(mut body) = bodies.get_mut(active.target) else {
            active.status = InteractionStatus::Canceled;
            continue;
        };

        let body = body.as_mut();
        body.limbs_to_remove.extend(body.limbs.iter().copied());
        active.status = InteractionStatus::Completed;
    }
}
