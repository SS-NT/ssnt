use bevy::asset::{AssetLoader, LoadContext, LoadedAsset};

use crate::utils::text::truncate;

use super::{parsing, TileMap};

#[derive(Default)]
pub struct TgmLoader;

impl AssetLoader for TgmLoader {
    fn load<'a>(
        &'a self,
        bytes: &'a [u8],
        load_context: &'a mut LoadContext,
    ) -> bevy::asset::BoxedFuture<'a, Result<(), anyhow::Error>> {
        Box::pin(async move { load_tgm(bytes, load_context).await })
    }

    fn extensions(&self) -> &[&str] {
        &["dmm"]
    }
}

#[derive(Debug)]
pub struct TgmError {
    message: String,
}

impl std::fmt::Display for TgmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for TgmError {}

async fn load_tgm<'a, 'b>(
    bytes: &'a [u8],
    load_context: &'a mut LoadContext<'b>,
) -> Result<(), anyhow::Error> {
    let raw_text = std::str::from_utf8(bytes)?;
    let map_text = &raw_text[raw_text.find('\n').unwrap()..raw_text.len()];

    let result = parsing::parse(map_text);
    if let Err(err) = result {
        match err {
            nom::Err::Incomplete(_) => todo!(),
            nom::Err::Error(e) | nom::Err::Failure(e) => {
                let full_error = e.to_string();
                let truncated = truncate(full_error.as_str(), 2000);
                return Err(TgmError {
                    message: truncated.into_owned(),
                }
                .into());
            }
        }
    }

    let (_, (definitions, chunks)) = result.unwrap();

    let tilemap = TileMap::new(
        definitions,
        chunks
            .iter()
            .flat_map(|chunk| {
                chunk.1.split('\n').enumerate().map(move |(offset, mut s)| {
                    if s.ends_with('\r') {
                        let mut chars = s.chars();
                        chars.next_back();
                        s = chars.as_str();
                    }
                    let mut position = chunk.0;
                    position.z += offset as u32;
                    (position, s)
                })
            })
            .collect(),
    );

    load_context.set_default_asset(LoadedAsset::new(tilemap));
    Ok(())
}
