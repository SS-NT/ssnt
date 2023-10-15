use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Deserialize)]
pub(crate) enum ReplacementCallbackDef {
    Noop,
    Simple(String),
    Any(Vec<ReplacementCallbackDef>),
    Weights(Vec<(u64, ReplacementCallbackDef)>),
}

#[derive(Debug, Deserialize)]
pub(crate) struct SeverityBodyDef {
    #[serde(default)]
    pub(crate) words: Vec<(String, ReplacementCallbackDef)>,
    #[serde(default)]
    pub(crate) patterns: Vec<(String, ReplacementCallbackDef)>,
}

#[derive(Debug, Deserialize)]
pub(crate) enum SeverityDef {
    Replace(SeverityBodyDef),
    Extend(SeverityBodyDef),
}

fn default_bool_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub(crate) struct AccentDef {
    pub(crate) name: String,
    #[serde(default = "default_bool_true")]
    pub(crate) normalize_case: bool,
    #[serde(default)]
    pub(crate) words: Vec<(String, ReplacementCallbackDef)>,
    #[serde(default)]
    pub(crate) patterns: Vec<(String, ReplacementCallbackDef)>,
    #[serde(default)]
    pub(crate) severities: BTreeMap<u64, SeverityDef>,
}
