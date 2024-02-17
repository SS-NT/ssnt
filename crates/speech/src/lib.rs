use bevy::{
    prelude::*,
    reflect::{TypePath, TypeUuid},
};
use bevy_common_assets::ron::RonAssetPlugin;
use std::borrow::Cow;

use serde::Deserialize;
pub struct SpeechPlugin;

impl Plugin for SpeechPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(RonAssetPlugin::<AccentDefinition>::new(&["accent.ron"]))
            .add_systems(Startup, load_assets);
    }
}

#[derive(Deserialize, TypeUuid, TypePath)]
#[uuid = "8cdd90cc-96bb-4f7d-97e9-06dccad18d7b"]
pub struct AccentDefinition {
    pub name: String,
    pub description: String,
    accent: sayit::Accent,
}

#[derive(Resource)]
pub struct AccentAssets {
    // Used to keep definitions loaded
    #[allow(dead_code)]
    definitions: Vec<Handle<AccentDefinition>>,
}

fn load_assets(mut commands: Commands, server: ResMut<AssetServer>) {
    let assets = AccentAssets {
        definitions: server
            .load_folder("accents")
            .expect("assets/accents is missing")
            .into_iter()
            .map(HandleUntyped::typed)
            .collect(),
    };
    commands.insert_resource(assets);
}

#[derive(Debug, Deserialize)]
pub struct Accent {
    pub name: String,
    pub description: String,
    accent: sayit::Accent,
}

impl Accent {
    pub fn intensities(&self) -> Vec<u64> {
        self.accent.intensities()
    }

    pub fn apply<'a>(&self, text: &'a str, intensity: u64) -> Cow<'a, str> {
        self.accent.say_it(text, intensity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    #[test]
    fn included_accents_can_be_parsed() {
        for file in std::fs::read_dir("accents").expect("read symlinked accents folder") {
            let path = file.unwrap().path();

            if !path.is_file() {
                continue;
            }

            if !path.extension().is_some_and(|ext| ext == "ron") {
                continue;
            }

            println!("parsing {}", path.display());

            let _ = ron::from_str::<Accent>(&fs::read_to_string(path).expect("reading file"))
                .expect("parsing ron definition");
        }
    }
}
