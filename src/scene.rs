use bevy::{asset::AssetApp, light::NotShadowCaster, prelude::*};
use networking::is_client;

#[derive(Resource)]
struct ClientSceneAssets {
    default_material: Handle<StandardMaterial>,
}

fn initialize_scene_meshes(
    mut commands: Commands,
    new_meshes: Query<
        (
            Entity,
            Has<MeshMaterial3d<StandardMaterial>>,
            Has<Visibility>,
        ),
        Added<Mesh3d>,
    >,
    assets: Res<ClientSceneAssets>,
) {
    for (entity, has_material, has_visibility) in &new_meshes {
        let mut entity = commands.entity(entity);
        if !has_material {
            entity.insert(MeshMaterial3d(assets.default_material.clone()));
        }
        if !has_visibility {
            entity.insert(Visibility::default());
        }
    }
}

pub struct ScenePlugin;

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        if !app.world().contains_resource::<Assets<Mesh>>() {
            app.init_asset::<Mesh>();
        }
        if !app.world().contains_resource::<Assets<StandardMaterial>>() {
            app.init_asset::<StandardMaterial>();
        }
        app.register_asset_reflect::<Mesh>()
            .register_asset_reflect::<StandardMaterial>()
            .register_type::<Mesh3d>()
            .register_type::<MeshMaterial3d<StandardMaterial>>()
            .register_type::<Visibility>()
            .register_type::<PointLight>()
            .register_type::<NotShadowCaster>()
            .register_type::<Transform>()
            .register_type::<GlobalTransform>()
            .register_type::<Vec3>()
            .register_type::<Quat>()
            .register_type::<UVec2>();

        if is_client(app) {
            let default_material = app
                .world()
                .resource::<AssetServer>()
                .load("models/items/wrenches.glb#Material:Palette05/std");
            app.insert_resource(ClientSceneAssets { default_material })
                .add_systems(Update, initialize_scene_meshes);
        }
    }
}
