use std::borrow::Cow;

use serde::Deserialize;

use pink_accents::Accent as PinkAccent;

#[derive(Debug, Deserialize)]
pub struct Accent {
    pub name: String,
    pub description: String,
    rules: PinkAccent,
}

impl Accent {
    pub fn severities(&self) -> Vec<u64> {
        self.rules.severities()
    }

    pub fn apply<'a>(&self, text: &'a str, severity: u64) -> Cow<'a, str> {
        self.rules.apply(text, severity)
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

            let _ = ron::from_str::<Accent>(&fs::read_to_string(filename).expect("reading file"))
                .expect("parsing ron definition");
        }
    }
}
