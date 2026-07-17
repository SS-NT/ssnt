use bevy::asset::{io::Reader, AssetLoader, AsyncReadExt, LoadContext};
use bevy::reflect::TypePath;
use utils::text::truncate;

use super::{parsing, TileMap};

#[derive(Default, TypePath)]
pub struct TgmLoader;

impl AssetLoader for TgmLoader {
    type Asset = TileMap;
    type Settings = ();
    type Error = anyhow::Error;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<TileMap, anyhow::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        load_tgm(&bytes).await
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

async fn load_tgm(bytes: &[u8]) -> Result<TileMap, anyhow::Error> {
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
            .rev()
            .flat_map(|chunk| {
                chunk
                    .1
                    .split('\n')
                    .rev()
                    .enumerate()
                    .map(move |(offset, mut s)| {
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

    Ok(tilemap)
}
