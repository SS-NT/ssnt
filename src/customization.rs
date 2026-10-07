use std::collections::BTreeMap;

use bevy::{
    asset::{LoadedFolder, RenderAssetUsages},
    camera::{ClearColorConfig, RenderTarget},
    image::Image,
    mesh::morph::MeshMorphWeights,
    platform::collections::HashMap,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages},
    scene::ScenePatch,
};
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass, EguiTextureHandle, EguiUserTextures};
use bevy_rapier3d::prelude::{RigidBody as RapierRigidBody, RigidBodyDisabled, Velocity};
use networking::{
    component::AppExt as ComponentAppExt,
    is_server,
    messaging::{AppExt as MessagingAppExt, MessageEvent, MessageSender},
    scene::NetworkSceneBundle,
    spawning::{ClientControlled, ClientControls},
    variable::{NetworkVar, ServerVar},
    ConnectionId, Networked, Players, UserData,
};
use physics::{Frozen, PhysicsEntityCommands};
use serde::{Deserialize, Serialize};

use crate::{
    body::{build_human_creature, Limb, LimbVisual},
    items::clothes::{Clothing, ClothingHolder},
    job::JobDefinition,
    GameState,
};

/// Where the off-screen preview body lives, far from the game world so the
/// preview camera only ever sees the character against its clear colour.
const PREVIEW_POSITION: Vec3 = Vec3::new(0.0, 1000.0, 0.0);
/// Resolution the preview is rendered at.
const PREVIEW_SIZE: u32 = 512;
/// Camera distance limits for scrolling over the preview.
const PREVIEW_MIN_DISTANCE: f32 = 1.2;
const PREVIEW_MAX_DISTANCE: f32 = 4.0;
const PREVIEW_DEFAULT_DISTANCE: f32 = 2.5;
const CHARACTER_NAME_MAX_CHARS: usize = 32;

/// Job used for the preview's clothing until a job is picked.
const DEFAULT_JOB_ID: &str = "assistant";

/// Fake horizontal speeds fed to the shared locomotion logic to preview each gait.
const IDLE_SPEED: f32 = 0.0;
const WALK_SPEED: f32 = 2.0;
const RUN_SPEED: f32 = 4.5;

/// The morph targets exposed as sliders for the human species, as
/// `(morph target name in the glTF, display label)`.
const HUMAN_MORPHS: &[(&str, &str)] = &[
    ("Female", "Feminine"),
    ("Breasts", "Breasts"),
    ("Fat", "Body fat"),
    ("Muscle", "Muscle"),
    ("Jaw", "Jaw"),
];

pub struct CustomizationPlugin;

impl Plugin for CustomizationPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<AppearanceFeature>()
            .register_type::<AppearanceOptionDefinition>()
            .register_type::<AppearanceOptionHolder>()
            .add_network_message::<SetAppearanceMessage>()
            .add_systems(Startup, load_appearance_option_assets)
            .add_networked_component::<Appearance, AppearanceClient>();

        if is_server(app) {
            app.init_resource::<SelectedAppearances>()
                .add_systems(Update, handle_set_appearance);
        } else {
            app.init_resource::<CharacterCustomization>()
                .init_resource::<SelectedJobClient>()
                .init_resource::<AppearanceOptionCatalog>()
                .add_systems(OnEnter(GameState::Game), setup_preview)
                .add_systems(OnExit(GameState::Game), teardown_preview)
                .add_systems(
                    Update,
                    (
                        prepare_appearance_option_catalog,
                        apply_appearance_morphs,
                        update_appearance_options,
                        reapply_changed_appearance,
                        position_preview_limbs,
                        disable_preview_physics,
                        update_preview_camera,
                        apply_preview_pose,
                        update_preview_clothing,
                        attach_preview_clothing,
                        manage_preview_lifecycle,
                    )
                        .run_if(in_state(GameState::Game)),
                )
                .add_systems(
                    EguiPrimaryContextPass,
                    customization_ui.run_if(in_state(GameState::Game)),
                );
        }
    }
}

/// The species a character belongs to. Only [`Species::Human`] is supported for now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Species {
    #[default]
    Human,
}

impl Species {
    /// Every selectable species, for the menu dropdown.
    const ALL: &'static [Species] = &[Species::Human];

    fn label(self) -> &'static str {
        match self {
            Species::Human => "Human",
        }
    }
}

/// A fully-resolved character appearance. Morph weights are keyed by glTF morph
/// target name so they apply to whichever meshes expose a matching target.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct CharacterAppearance {
    pub species: Species,
    pub name: String,
    /// IDs of selected scene-defined appearance options, or their asset-defined defaults.
    pub hair: Option<String>,
    pub facial_hair: Option<String>,
    pub eyebrows: Option<String>,
    pub morphs: BTreeMap<String, f32>,
}

/// Networked appearance living on the body root (server side).
#[derive(Component, TypePath, Networked)]
#[networked(client = "AppearanceClient")]
pub struct Appearance {
    data: NetworkVar<CharacterAppearance>,
}

impl Appearance {
    pub fn new(appearance: CharacterAppearance) -> Self {
        Self {
            data: appearance.into(),
        }
    }
}

/// Client-side mirror of [`Appearance`]. Also inserted locally on the preview body.
#[derive(Component, TypePath, Networked, Default)]
#[networked(server = "Appearance")]
pub struct AppearanceClient {
    data: ServerVar<CharacterAppearance>,
}

impl AppearanceClient {
    fn from_appearance(appearance: CharacterAppearance) -> Self {
        Self {
            data: ServerVar::from_default(appearance),
        }
    }

    /// The appearance, or `None` if it hasn't been received from the server yet.
    fn appearance(&self) -> Option<&CharacterAppearance> {
        self.data.get()
    }

    fn set(&mut self, appearance: CharacterAppearance) {
        self.data.set(appearance);
    }
}

/// Sent from a client to choose its appearance before spawning.
#[derive(Serialize, Deserialize)]
pub struct SetAppearanceMessage {
    pub appearance: CharacterAppearance,
}

/// Server-side store of each connection's chosen appearance.
#[derive(Resource, Default)]
pub struct SelectedAppearances {
    selected: HashMap<ConnectionId, CharacterAppearance>,
}

impl SelectedAppearances {
    /// The appearance chosen by a connection, or the default if none was set.
    pub fn get(&self, connection: ConnectionId) -> CharacterAppearance {
        self.selected.get(&connection).cloned().unwrap_or_default()
    }
}

fn handle_set_appearance(
    mut messages: MessageReader<MessageEvent<SetAppearanceMessage>>,
    players: Res<Players>,
    controlled: Res<ClientControls>,
    mut resource: ResMut<SelectedAppearances>,
) {
    for event in messages.read() {
        let Some(player) = players.get(event.connection) else {
            continue;
        };
        // Only allow changes while the player hasn't spawned a body yet.
        if controlled.controlled_entity(player.id).is_some() {
            continue;
        }
        let mut appearance = event.message.appearance.clone();
        appearance.name = appearance
            .name
            .chars()
            .filter(|character| !character.is_control())
            .take(CHARACTER_NAME_MAX_CHARS)
            .collect();
        resource.selected.insert(event.connection, appearance);
    }
}

/// The client's working copy of its appearance, edited by the menu.
#[derive(Resource, Default)]
struct CharacterCustomization {
    appearance: CharacterAppearance,
}

/// The job currently selected in the lobby, mirrored here so the preview can
/// show its starting gear. `None` falls back to the default job's clothing.
#[derive(Resource, Default)]
pub struct SelectedJobClient {
    pub selected: Option<AssetId<JobDefinition>>,
}

/// Marks a limb visual whose morph weights already match its body's appearance.
#[derive(Component)]
struct MorphsApplied;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
#[reflect(Default)]
pub enum AppearanceFeature {
    #[default]
    Hair,
    FacialHair,
    Eyebrows,
}

impl AppearanceFeature {
    const ALL: &'static [Self] = &[Self::Hair, Self::FacialHair, Self::Eyebrows];

    fn label(self) -> &'static str {
        match self {
            Self::Hair => "Hair",
            Self::FacialHair => "Facial hair",
            Self::Eyebrows => "Eyebrows",
        }
    }

    fn selected<'a>(self, appearance: &'a CharacterAppearance) -> Option<&'a str> {
        match self {
            Self::Hair => appearance.hair.as_deref(),
            Self::FacialHair => appearance.facial_hair.as_deref(),
            Self::Eyebrows => appearance.eyebrows.as_deref(),
        }
    }

    fn set_selected(self, appearance: &mut CharacterAppearance, id: String) {
        *match self {
            Self::Hair => &mut appearance.hair,
            Self::FacialHair => &mut appearance.facial_hair,
            Self::Eyebrows => &mut appearance.eyebrows,
        } = Some(id);
    }
}

/// Metadata stored on the root of each scene-backed appearance option.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct AppearanceOptionDefinition {
    pub id: String,
    pub name: String,
    pub feature: AppearanceFeature,
    pub default: bool,
}

struct AppearanceOptionData {
    id: String,
    name: String,
    scene: String,
    feature: AppearanceFeature,
    default: bool,
}

#[derive(Resource, Default)]
struct AppearanceOptionCatalog {
    definitions: Vec<AppearanceOptionData>,
    loaded: bool,
}

#[derive(Resource)]
struct AppearanceOptionAssets {
    folders: Vec<Handle<LoadedFolder>>,
}

fn load_appearance_option_assets(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(AppearanceOptionAssets {
        folders: ["hair", "facial_hair", "eyebrows"]
            .map(|folder| asset_server.load_folder(folder))
            .into(),
    });
}

fn prepare_appearance_option_catalog(
    option_assets: Res<AppearanceOptionAssets>,
    folders: Res<Assets<LoadedFolder>>,
    patches: Res<Assets<ScenePatch>>,
    type_registry: Res<AppTypeRegistry>,
    mut catalog: ResMut<AppearanceOptionCatalog>,
) {
    if catalog.loaded {
        return;
    }

    let mut definitions = Vec::new();
    for folder_handle in &option_assets.folders {
        let Some(folder) = folders.get(folder_handle) else {
            return;
        };
        for handle in &folder.handles {
            let Some(scene) = handle
                .path()
                .map(|path| path.path().to_string_lossy().into_owned())
            else {
                continue;
            };
            let Ok(id) = handle.id().try_typed::<ScenePatch>() else {
                continue;
            };
            let Some(patch) = patches.get(id) else {
                return;
            };
            let Some(resolved) = &patch.resolved else {
                return;
            };

            let mut scratch = World::new();
            scratch.insert_resource(type_registry.clone());
            let Some((id, name, feature, default)) =
                resolved.spawn(&mut scratch).ok().and_then(|entity| {
                    entity
                        .get::<AppearanceOptionDefinition>()
                        .map(|definition| {
                            (
                                definition.id.clone(),
                                definition.name.clone(),
                                definition.feature,
                                definition.default,
                            )
                        })
                })
            else {
                continue;
            };
            definitions.push(AppearanceOptionData {
                id,
                name,
                scene,
                feature,
                default,
            });
        }
    }

    catalog.definitions = definitions;
    catalog.loaded = true;
}

/// Marks the limb to which scene-backed appearance options are attached.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct AppearanceOptionHolder;

#[derive(Component)]
struct SpawnedAppearanceOption {
    id: String,
    feature: AppearanceFeature,
}

fn selected_appearance_option<'a>(
    feature: AppearanceFeature,
    appearance: &CharacterAppearance,
    catalog: &'a AppearanceOptionCatalog,
) -> Option<&'a AppearanceOptionData> {
    feature
        .selected(appearance)
        .and_then(|selected| {
            catalog
                .definitions
                .iter()
                .find(|definition| definition.feature == feature && definition.id == selected)
        })
        .or_else(|| {
            catalog
                .definitions
                .iter()
                .find(|definition| definition.feature == feature && definition.default)
        })
}

fn update_appearance_options(
    appearances: Query<(Entity, &AppearanceClient)>,
    children: Query<&Children>,
    holders: Query<(), With<AppearanceOptionHolder>>,
    existing: Query<(Entity, &SpawnedAppearanceOption, &ChildOf)>,
    catalog: Res<AppearanceOptionCatalog>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    for (body, appearance) in &appearances {
        let Some(appearance) = appearance.appearance() else {
            continue;
        };
        let Some(holder) = children
            .iter_descendants(body)
            .find(|&entity| holders.contains(entity))
        else {
            continue;
        };

        for &feature in AppearanceFeature::ALL {
            let Some(definition) = selected_appearance_option(feature, appearance, &catalog) else {
                continue;
            };
            let current = existing
                .iter()
                .find(|(_, option, parent)| parent.parent() == holder && option.feature == feature);
            if current.is_some_and(|(_, option, _)| option.id == definition.id) {
                continue;
            }
            if let Some((entity, _, _)) = current {
                commands.entity(entity).try_despawn();
            }
            commands.spawn((
                NetworkSceneBundle {
                    scene: asset_server
                        .load::<ScenePatch>(definition.scene.clone())
                        .into(),
                    ..default()
                },
                ChildOf(holder),
                SpawnedAppearanceOption {
                    id: definition.id.clone(),
                    feature,
                },
            ));
        }
    }
}

fn apply_appearance_morphs(
    visuals: Query<
        (Entity, &Mesh3d),
        (
            Or<(With<LimbVisual>, With<Clothing>)>,
            Without<MorphsApplied>,
        ),
    >,
    parents: Query<&ChildOf>,
    appearances: Query<&AppearanceClient>,
    meshes: Res<Assets<Mesh>>,
    mut commands: Commands,
) {
    for (entity, mesh) in &visuals {
        let appearance = parents
            .iter_ancestors(entity)
            .find_map(|ancestor| appearances.get(ancestor).ok())
            .and_then(|client| client.appearance());
        let Some(appearance) = appearance else {
            continue; // no appearance available yet, retry next frame
        };
        let Some(mesh) = meshes.get(&mesh.0) else {
            continue; // mesh asset not loaded yet
        };
        let Some(names) = mesh.morph_target_names() else {
            // This mesh has no morph targets; nothing to drive.
            commands.entity(entity).insert(MorphsApplied);
            continue;
        };
        let weights = names
            .iter()
            .map(|name| appearance.morphs.get(name).copied().unwrap_or(0.0))
            .collect();
        commands
            .entity(entity)
            .insert((MeshMorphWeights::Value { weights }, MorphsApplied));
    }
}

/// Re-applies morphs when a body's appearance changes.
fn reapply_changed_appearance(
    changed: Query<Entity, Changed<AppearanceClient>>,
    children: Query<&Children>,
    visuals: Query<(), Or<(With<LimbVisual>, With<Clothing>)>>,
    mut commands: Commands,
) {
    for body in &changed {
        for descendant in children.iter_descendants(body) {
            if visuals.contains(descendant) {
                commands.entity(descendant).remove::<MorphsApplied>();
            }
        }
    }
}

/// Marks the root of the client-only preview body.
#[derive(Component)]
struct PreviewCharacter;

/// Marks a clothing item spawned for the preview body.
#[derive(Component)]
struct PreviewClothing;

/// Handles to the preview render target, camera, light and body.
#[derive(Resource)]
struct CharacterPreview {
    image: Handle<Image>,
    texture_id: egui::TextureId,
    camera: Entity,
    light: Entity,
    body: Option<Entity>,
    /// Orbit angle around the character, in radians.
    yaw: f32,
    /// Distance from the camera to the character.
    distance: f32,
    /// Horizontal speed fed to locomotion so the menu can preview each gait.
    pose_speed: f32,
    /// Clothing item names currently equipped on the preview body.
    equipped_clothing: Vec<String>,
    /// Whether the preview is still being shown (false once the player spawns in).
    active: bool,
}

fn setup_preview(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut egui_textures: ResMut<EguiUserTextures>,
    asset_server: Res<AssetServer>,
    mut customization: ResMut<CharacterCustomization>,
    user_data: Res<UserData>,
) {
    if customization.appearance.name.is_empty() {
        customization.appearance.name = user_data.username.clone();
    }

    let size = Extent3d {
        width: PREVIEW_SIZE,
        height: PREVIEW_SIZE,
        depth_or_array_layers: 1,
    };
    let mut image = Image::new_fill(
        size,
        TextureDimension::D2,
        &[0, 0, 0, 0],
        TextureFormat::Bgra8UnormSrgb,
        RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT;
    let image = images.add(image);
    let texture_id = egui_textures.add_image(EguiTextureHandle::Strong(image.clone()));

    let camera = commands
        .spawn((
            Camera3d::default(),
            Camera {
                clear_color: ClearColorConfig::Custom(Color::srgb(0.12, 0.12, 0.14)),
                // Render before the main camera so the image is ready for egui.
                order: -1,
                ..default()
            },
            RenderTarget::Image(image.clone().into()),
            Transform::from_translation(PREVIEW_POSITION + Vec3::new(0.0, 1.2, 2.5))
                .looking_at(PREVIEW_POSITION + Vec3::Y * 0.9, Vec3::Y),
        ))
        .id();

    // Attenuated point light so it stays local to the preview and never touches the map.
    let light = commands
        .spawn((
            PointLight {
                intensity: 1_000_000.0,
                range: 20.0,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::from_translation(PREVIEW_POSITION + Vec3::new(2.0, 3.0, 3.0)),
        ))
        .id();

    let body = build_human_creature(&mut commands, asset_server.as_ref(), true);
    commands.entity(body).insert((
        Transform::from_translation(PREVIEW_POSITION),
        PreviewCharacter,
        AppearanceClient::from_appearance(customization.appearance.clone()),
        Velocity::default(),
    ));

    commands.insert_resource(CharacterPreview {
        image,
        texture_id,
        camera,
        light,
        body: Some(body),
        yaw: 0.0,
        distance: PREVIEW_DEFAULT_DISTANCE,
        pose_speed: IDLE_SPEED,
        equipped_clothing: Vec::new(),
        active: true,
    });
}

/// The server positions attached limbs in `process_new_limbs`, which does not run for the
/// client-only preview. Mirror that positioning so mesh and clothing bounds match the skeleton.
fn position_preview_limbs(
    roots: Query<Entity, With<PreviewCharacter>>,
    children: Query<&Children>,
    limbs: Query<(&Limb, Option<&ChildOf>)>,
    mut transforms: Query<&mut Transform>,
) {
    for root in &roots {
        for entity in children.iter_descendants(root) {
            let Ok((limb, parent)) = limbs.get(entity) else {
                continue;
            };
            let parent = parent.and_then(|parent| limbs.get(parent.parent()).ok());
            let target = limb.local_translation(parent.map(|(limb, _)| limb));
            if let Ok(mut transform) = transforms.get_mut(entity) {
                if transform.translation != target {
                    transform.translation = target;
                }
            }
        }
    }
}

fn teardown_preview(
    mut commands: Commands,
    preview: Option<Res<CharacterPreview>>,
    clothing: Query<Entity, With<PreviewClothing>>,
    mut egui_textures: ResMut<EguiUserTextures>,
) {
    let Some(preview) = preview else {
        return;
    };
    commands.entity(preview.camera).try_despawn();
    commands.entity(preview.light).try_despawn();
    if let Some(body) = preview.body {
        commands.entity(body).try_despawn();
    }
    for entity in &clothing {
        commands.entity(entity).try_despawn();
    }
    egui_textures.remove_image(preview.image.id());
    commands.remove_resource::<CharacterPreview>();
}

/// Physics is meaningless for the static preview; the skinned meshes follow the
/// skeleton regardless. Disable rigid bodies as their scenes finish spawning so
/// nothing falls or drifts.
fn disable_preview_physics(
    roots: Query<Entity, With<PreviewCharacter>>,
    children: Query<&Children>,
    active_bodies: Query<(), (With<RapierRigidBody>, Without<RigidBodyDisabled>)>,
    // Clothing starts detached from the body, so disable it directly too, before it
    // can free-fall.
    loose_clothing: Query<
        Entity,
        (
            With<PreviewClothing>,
            With<RapierRigidBody>,
            Without<RigidBodyDisabled>,
        ),
    >,
    mut commands: Commands,
) {
    for root in &roots {
        if active_bodies.contains(root) {
            commands.entity(root).disable_physics();
        }
        for descendant in children.iter_descendants(root) {
            if active_bodies.contains(descendant) {
                commands.entity(descendant).disable_physics();
            }
        }
    }
    for clothing in &loose_clothing {
        commands.entity(clothing).disable_physics();
    }
}

fn update_preview_camera(
    preview: Option<Res<CharacterPreview>>,
    mut transforms: Query<&mut Transform>,
) {
    let Some(preview) = preview else {
        return;
    };
    if !preview.active {
        return;
    }
    let target = PREVIEW_POSITION + Vec3::Y * 0.9;
    let orbit = Quat::from_rotation_y(preview.yaw);
    let offset = orbit * Vec3::new(0.0, 0.0, preview.distance) + Vec3::Y * 0.3;
    if let Ok(mut transform) = transforms.get_mut(preview.camera) {
        transform.translation = target + offset;
        transform.look_at(target, Vec3::Y);
    }
    if let Ok(mut transform) = transforms.get_mut(preview.light) {
        transform.translation = target + offset + orbit * Vec3::new(2.0, 1.8, 0.5);
    }
}

/// Once the player spawns into the world, stop rendering the preview.
fn manage_preview_lifecycle(
    controlled: Query<(), With<ClientControlled>>,
    preview: Option<ResMut<CharacterPreview>>,
    clothing: Query<Entity, With<PreviewClothing>>,
    mut cameras: Query<&mut Camera>,
    mut commands: Commands,
) {
    let Some(mut preview) = preview else {
        return;
    };
    if !preview.active || controlled.is_empty() {
        return;
    }
    if let Ok(mut camera) = cameras.get_mut(preview.camera) {
        camera.is_active = false;
    }
    if let Some(body) = preview.body.take() {
        commands.entity(body).try_despawn();
    }
    for entity in &clothing {
        commands.entity(entity).try_despawn();
    }
    preview.active = false;
}

/// Feeds the selected gait's speed into the preview body so the shared
/// locomotion logic plays the matching idle/walk/run animation.
fn apply_preview_pose(
    preview: Option<Res<CharacterPreview>>,
    mut velocities: Query<&mut Velocity, With<PreviewCharacter>>,
) {
    let Some(preview) = preview else {
        return;
    };
    if let Ok(mut velocity) = velocities.single_mut() {
        let target = Vec3::new(0.0, 0.0, preview.pose_speed);
        if velocity.linear != target {
            velocity.linear = target;
        }
    }
}

/// Keeps the preview body wearing the selected job's starting gear (or the
/// default job's until one is chosen). Spawned items are attached to the body
/// by [`attach_preview_clothing`].
fn update_preview_clothing(
    preview: Option<ResMut<CharacterPreview>>,
    selected: Res<SelectedJobClient>,
    jobs: Res<Assets<JobDefinition>>,
    asset_server: Res<AssetServer>,
    existing: Query<Entity, With<PreviewClothing>>,
    mut commands: Commands,
) {
    let Some(mut preview) = preview else {
        return;
    };
    if !preview.active || preview.body.is_none() {
        return;
    }

    let desired = match selected.selected.and_then(|id| jobs.get(id)) {
        Some(job) => job.clothing.clone(),
        // Fall back to the default job until the player picks one.
        None => match jobs.iter().find(|(_, job)| job.id == DEFAULT_JOB_ID) {
            Some((_, job)) => job.clothing.clone(),
            None => return, // job assets not loaded yet, retry next frame
        },
    };

    if preview.equipped_clothing == desired {
        return;
    }

    for entity in &existing {
        commands.entity(entity).try_despawn();
    }
    for name in &desired {
        commands.spawn((
            NetworkSceneBundle {
                scene: asset_server.load(format!("items/{name}.bsn")).into(),
                // Spawn at the preview so the brief moment before it attaches to the
                // body (and gets its physics disabled) never touches the game world.
                transform: Transform::from_translation(PREVIEW_POSITION),
                ..default()
            },
            PreviewClothing,
            Frozen,
        ));
    }
    preview.equipped_clothing = desired;
}

/// Parents each spawned preview clothing item under the body's matching clothing
/// slot once its scene has resolved, so the shared skinning logic renders it.
fn attach_preview_clothing(
    unattached: Query<(Entity, &Clothing), (With<PreviewClothing>, Without<ChildOf>)>,
    bodies: Query<Entity, With<PreviewCharacter>>,
    children: Query<&Children>,
    holders: Query<&ClothingHolder>,
    mut commands: Commands,
) {
    let Ok(body) = bodies.single() else {
        return;
    };
    for (clothing_entity, clothing) in &unattached {
        let holder = children.iter_descendants(body).find(|&entity| {
            holders
                .get(entity)
                .is_ok_and(|holder| holder.clothing_type() == clothing.clothing_type())
        });
        if let Some(holder) = holder {
            // Sit at the holder (identity transform), like an equipped item in-game.
            // The mesh is skinned so this only matters for its cull AABB.
            commands
                .entity(clothing_entity)
                .insert((ChildOf(holder), Transform::default()));
        }
    }
}

fn customization_ui(
    mut contexts: EguiContexts,
    mut customization: ResMut<CharacterCustomization>,
    preview: Option<ResMut<CharacterPreview>>,
    controlled: Query<(), With<ClientControlled>>,
    mut preview_bodies: Query<&mut AppearanceClient, With<PreviewCharacter>>,
    option_catalog: Res<AppearanceOptionCatalog>,
    mut sender: MessageSender,
) {
    // Only show the menu while choosing a character (not yet in a body).
    if !controlled.is_empty() {
        return;
    }
    let Some(mut preview) = preview else {
        return;
    };
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    let texture_id = preview.texture_id;
    let species = customization.appearance.species;
    let mut changed = false;

    egui::Window::new("Character")
        .anchor(egui::Align2::LEFT_CENTER, egui::vec2(30.0, 0.0))
        .resizable(false)
        .show(ctx, |ui| {
            egui::Grid::new("character_identity")
                .num_columns(2)
                .show(ui, |ui| {
                    ui.label("Name");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut customization.appearance.name)
                                .char_limit(CHARACTER_NAME_MAX_CHARS),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                    ui.end_row();

                    ui.label("Species");
                    egui::ComboBox::from_id_salt("species")
                        .selected_text(species.label())
                        .show_ui(ui, |ui| {
                            for &option in Species::ALL {
                                if ui
                                    .selectable_value(
                                        &mut customization.appearance.species,
                                        option,
                                        option.label(),
                                    )
                                    .changed()
                                {
                                    changed = true;
                                }
                            }
                        });
                    ui.end_row();

                    for &feature in AppearanceFeature::ALL {
                        ui.label(feature.label());
                        let selected = selected_appearance_option(
                            feature,
                            &customization.appearance,
                            &option_catalog,
                        );
                        let selected_id = selected.map(|definition| definition.id.as_str());
                        let selected_name = selected
                            .map(|definition| definition.name.as_str())
                            .unwrap_or("Loading...");
                        egui::ComboBox::from_id_salt(feature.label())
                            .selected_text(selected_name)
                            .show_ui(ui, |ui| {
                                let mut definitions: Vec<_> = option_catalog
                                    .definitions
                                    .iter()
                                    .filter(|definition| definition.feature == feature)
                                    .collect();
                                definitions.sort_by(|a, b| a.name.cmp(&b.name));
                                for definition in definitions {
                                    let is_selected = selected_id == Some(definition.id.as_str());
                                    if ui.selectable_label(is_selected, &definition.name).clicked()
                                        && !is_selected
                                    {
                                        feature.set_selected(
                                            &mut customization.appearance,
                                            definition.id.clone(),
                                        );
                                        changed = true;
                                    }
                                }
                            });
                        ui.end_row();
                    }
                });

            // Drag to orbit; scroll while hovered to zoom.
            let preview_width = ui.available_width();
            let image = egui::Image::new(egui::load::SizedTexture::new(
                texture_id,
                egui::vec2(PREVIEW_SIZE as f32, PREVIEW_SIZE as f32),
            ))
            .fit_to_exact_size(egui::vec2(preview_width, preview_width))
            .sense(egui::Sense::drag());
            let response = ui.add(image);
            if response.dragged() {
                preview.yaw -= response.drag_delta().x * 0.01;
            }
            if response.hovered() {
                let scroll = ui.input(|input| input.smooth_scroll_delta().y);
                preview.distance = (preview.distance - scroll * 0.005)
                    .clamp(PREVIEW_MIN_DISTANCE, PREVIEW_MAX_DISTANCE);
            }

            // Animation gait selection.
            ui.horizontal(|ui| {
                for (label, speed) in [
                    ("Idle", IDLE_SPEED),
                    ("Walk", WALK_SPEED),
                    ("Run", RUN_SPEED),
                ] {
                    if ui
                        .selectable_label(preview.pose_speed == speed, label)
                        .clicked()
                    {
                        preview.pose_speed = speed;
                    }
                }
            });

            ui.separator();
            for &(name, label) in HUMAN_MORPHS {
                let value = customization
                    .appearance
                    .morphs
                    .entry(name.to_string())
                    .or_insert(0.0);
                if ui
                    .add(egui::Slider::new(value, 0.0..=1.0).text(label))
                    .changed()
                {
                    changed = true;
                }
            }
        });

    if changed {
        // Update the live preview and inform the server.
        if let Ok(mut client) = preview_bodies.single_mut() {
            client.set(customization.appearance.clone());
        }
        sender.send_to_server(&SetAppearanceMessage {
            appearance: customization.appearance.clone(),
        });
    }
}
