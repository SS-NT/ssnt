use bevy::{asset::{AssetLoader, LoadContext, LoadedAsset}, utils::StableHashMap};

use super::{parsing, TileMap};

#[derive(Default)]
pub struct TgmLoader;

impl AssetLoader for TgmLoader {
    fn load<'a>(
        &'a self,
        bytes: &'a [u8],
        load_context: &'a mut LoadContext,
    ) -> bevy::asset::BoxedFuture<'a, Result<(), anyhow::Error>> {
        Box::pin(async move { Ok(load_tgm(bytes, load_context).await?) })
    }

    fn extensions(&self) -> &[&str] {
        &["dmm"]
    }
}

#[derive(Debug)]
pub struct TgmError {
    line: u32,
    character: u32,
    message: String,
}

impl std::fmt::Display for TgmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Error: {} at line {} character {}",
            self.message, self.line, self.character
        )
    }
}

impl std::error::Error for TgmError {}

async fn load_tgm<'a, 'b>(
    bytes: &'a [u8],
    load_context: &'a mut LoadContext<'b>,
) -> Result<(), anyhow::Error> {
    let raw_text = std::str::from_utf8(bytes)?;
    let map_text = &raw_text[raw_text.find('\n').unwrap()..raw_text.len()];
    // TODO: don't panic on invalid map
    let (_, (definitions, chunks)) = parsing::parse(map_text).unwrap();

    let tilemap = TileMap::new(
        definitions
            .into_iter()
            .fold(StableHashMap::default(), |mut map, pair| {
                map.insert(pair.0, pair.1);
                map
            }),
        chunks.iter().flat_map(|chunk| {
            chunk.1.split("\n").map(move |s| (chunk.0, s))
        }).collect(),
    );

    load_context.set_default_asset(LoadedAsset::new(tilemap));
    Ok(())
}
