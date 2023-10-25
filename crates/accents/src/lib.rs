use serde::Deserialize;

use pink_accents::deserialize::AccentDef as PinkAccentDef;
use pink_accents::Accent as PinkAccent;

#[derive(Debug, Deserialize)]
struct AccentDef {
    name: String,
    description: String,
    rules: PinkAccentDef,
}

#[derive(Debug)]
pub struct Accent {
    pub name: String,
    pub description: String,
    accent: PinkAccent,
}

impl TryFrom<AccentDef> for Accent {
    type Error = String;

    fn try_from(accent_def: AccentDef) -> Result<Self, Self::Error> {
        Ok(Self {
            name: accent_def.name,
            description: accent_def.description,
            accent: accent_def.rules.try_into()?,
        })
    }
}

impl Accent {
    pub fn severities(&self) -> Vec<u64> {
        self.accent.severities()
    }

    pub fn apply(&self, text: &str, severity: u64) -> String {
        self.accent.apply(text, severity)
    }

    pub fn from_ron(s: &str) -> Result<Self, String> {
        Self::try_from(
            ron::from_str::<AccentDef>(s)
                .map_err(|err| format!("unable to load accent definition: {}", err))?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    #[test]
    fn included_accents_can_be_parsed() {
        for file in fs::read_dir("accents").expect("read symlinked accents folder") {
            let filename = file.expect("getting file info").path();
            println!("parsing {}", filename.display());

            let _ = Accent::from_ron(&fs::read_to_string(filename).expect("reading file")).unwrap();
        }
    }
}
