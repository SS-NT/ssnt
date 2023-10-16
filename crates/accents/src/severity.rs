use crate::replacement::ReplacementCallback;
use crate::ron_defs::{SeverityBodyDef, SeverityDef};

#[derive(Debug)]
pub(crate) struct SeverityBody {
    pub(crate) words: Vec<(String, ReplacementCallback)>,
    pub(crate) patterns: Vec<(String, ReplacementCallback)>,
}

/// Either replaces everything from previous severity using `Replace` or adds new words and
/// patterns to the end of previous ones with `Extend`
#[derive(Debug)]
pub(crate) enum Severity {
    Replace(SeverityBody),
    Extend(SeverityBody),
}

impl TryFrom<SeverityBodyDef> for SeverityBody {
    type Error = String;

    // TODO: these should be in Deserialize implementation when/if it is done
    fn try_from(accent_def: SeverityBodyDef) -> Result<Self, Self::Error> {
        let mut words = Vec::with_capacity(accent_def.words.len());
        for (i, (pattern, callback_def)) in accent_def.words.into_iter().enumerate() {
            let callback: ReplacementCallback = match callback_def.try_into() {
                Err(err) => Err(format!("error in word {i}: {pattern}: {err}"))?,
                Ok(callback) => callback,
            };
            words.push((pattern, callback));
        }

        let mut patterns = Vec::with_capacity(accent_def.patterns.len());
        for (i, (pattern, callback_def)) in accent_def.patterns.into_iter().enumerate() {
            let callback: ReplacementCallback = match callback_def.try_into() {
                Err(err) => Err(format!("error in pattern {i}: {pattern}: {err}"))?,
                Ok(callback) => callback,
            };
            patterns.push((pattern, callback));
        }

        Ok(Self { words, patterns })
    }
}

impl TryFrom<SeverityDef> for Severity {
    type Error = String;

    // TODO: these should be in Deserialize implementation when/if it is done
    fn try_from(severity_def: SeverityDef) -> Result<Self, Self::Error> {
        Ok(match severity_def {
            SeverityDef::Replace(body) => Self::Replace(body.try_into()?),
            SeverityDef::Extend(body) => Self::Extend(body.try_into()?),
        })
    }
}
