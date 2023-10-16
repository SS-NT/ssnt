use crate::ron_defs::ReplacementCallbackDef;

use rand::seq::SliceRandom;
use regex::{Captures, Regex};

#[derive(Debug, PartialEq, Clone)]
pub(crate) struct SimpleString {
    body: String,
    char_count: usize,
    is_ascii_only: bool,
    is_ascii_lowercase: bool,
    is_ascii_uppercase: bool,
}

impl SimpleString {
    pub(crate) fn new(body: &str) -> Self {
        Self {
            body: body.to_owned(),
            char_count: body.chars().count(),
            is_ascii_only: body.is_ascii(),
            is_ascii_lowercase: body.chars().all(|c| c.is_ascii_lowercase()),
            is_ascii_uppercase: body.chars().all(|c| c.is_ascii_lowercase()),
        }
    }
}

/// Receives match and provides replacement
#[derive(Debug, PartialEq, Clone)]
pub(crate) enum ReplacementCallback {
    /// Do not replace
    Noop,
    // TODO: either a separate variant or modify Simple to allow formatting using capture groups:
    //       "hello {1}" would insert group 1
    // TODO: implement a bit of serde magic for easier parsing: string would turn into `Simple`,
    //       array into `Any` and map with u64 keys into `Weights`
    /// Puts string as is
    Simple(SimpleString),
    /// Selects random replacement with equal weights
    Any(Vec<ReplacementCallback>),
    /// Selects replacement based on relative weights
    Weights(Vec<(u64, ReplacementCallback)>),
    // TODO: see below
    // Custom(fn taking Caps, severity and maybe other info),
}

impl ReplacementCallback {
    // try to learn something about strings and adjust case accordingly. all logic is currently
    // ascii only
    // tried using Cows but my computer exploded. TODO: try that again
    fn normalize_case(old: &str, new: &SimpleString) -> String {
        let mut body = new.body.clone();

        // assume lowercase ascii is "weakest" form. anything else returns as is
        if !new.is_ascii_lowercase {
            return body;
        }

        // if original was all uppercase we force all uppercase for replacement. this is likely to
        // give false positives on short inputs like "I" or abbreviations
        if old.chars().all(|c| c.is_ascii_uppercase()) {
            return body.to_ascii_uppercase();
        }

        // no constraints if original was all lowercase
        if old.chars().all(|c| !c.is_ascii() || c.is_ascii_lowercase()) {
            return body;
        }

        if old.chars().count() == new.char_count {
            for (i, c_old) in old.chars().enumerate() {
                if c_old.is_ascii_lowercase() {
                    body.get_mut(i..i + 1)
                        .expect("strings have same len")
                        .make_ascii_lowercase()
                } else if c_old.is_ascii_uppercase() {
                    body.get_mut(i..i + 1)
                        .expect("strings have same len")
                        .make_ascii_uppercase()
                }
            }
        }

        body
    }

    fn replace(&self, caps: &Captures, normalize_case: bool) -> String {
        match self {
            Self::Noop => caps[0].to_owned(),
            Self::Simple(string) => {
                if normalize_case {
                    Self::normalize_case(&caps[0], string)
                } else {
                    string.body.clone()
                }
            }
            Self::Any(targets) => {
                let mut rng = rand::thread_rng();

                targets
                    .choose(&mut rng)
                    .expect("empty targets")
                    .replace(caps, normalize_case)
            }
            Self::Weights(items) => {
                let mut rng = rand::thread_rng();

                items
                    .choose_weighted(&mut rng, |item| item.0)
                    .expect("empty targets")
                    .1
                    .replace(caps, normalize_case)
            }
        }
    }
}

impl TryFrom<ReplacementCallbackDef> for ReplacementCallback {
    type Error = String;

    // TODO: these should be in Deserialize implementation when/if it is done
    fn try_from(accent_def: ReplacementCallbackDef) -> Result<Self, Self::Error> {
        Ok(match accent_def {
            ReplacementCallbackDef::Noop => Self::Noop,
            ReplacementCallbackDef::Simple(body) => Self::Simple(SimpleString::new(&body)),
            ReplacementCallbackDef::Any(items) => {
                if items.is_empty() {
                    return Err("Empty Any".to_owned());
                }

                let mut converted = Vec::with_capacity(items.len());
                for item in items {
                    converted.push(item.try_into()?);
                }
                ReplacementCallback::Any(converted)
            }
            ReplacementCallbackDef::Weights(items) => {
                if items.is_empty() {
                    return Err("Empty Weights".to_owned());
                }

                if items.iter().map(|(i, _)| i).sum::<u64>() == 0 {
                    return Err("Weights add up to 0".to_owned());
                }

                let mut converted = Vec::with_capacity(items.len());
                for (weight, item) in items {
                    converted.push((weight, item.try_into()?));
                }
                Self::Weights(converted)
            }
        })
    }
}

/// Maps regex to callback
#[derive(Debug)]
pub(crate) struct Replacement {
    pub(crate) source: Regex,
    pub(crate) cb: ReplacementCallback,
}

impl Replacement {
    pub(crate) fn apply(&self, text: &str, normalize_case: bool) -> String {
        self.source
            .replace_all(text, |caps: &Captures| {
                self.cb.replace(caps, normalize_case)
            })
            .into_owned()
    }
}

impl PartialEq for Replacement {
    fn eq(&self, other: &Self) -> bool {
        self.source.as_str() == other.source.as_str() && self.cb == other.cb
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_case_input_lowercase() {
        assert_eq!(
            ReplacementCallback::normalize_case("hello", &SimpleString::new("bye")),
            "bye"
        );
        assert_eq!(
            ReplacementCallback::normalize_case("hello", &SimpleString::new("Bye")),
            "Bye"
        );
        assert_eq!(
            ReplacementCallback::normalize_case("hello", &SimpleString::new("bYE")),
            "bYE"
        );
    }

    // questionable rule, becomes overcomplicated
    // #[test]
    // fn normalize_case_input_titled() {
    //     assert_eq!(
    //         ReplacementCallback::normalize_case("Hello", &SimpleString::new("bye")),
    //         "Bye"
    //     );
    //     // has case variation -- do not touch it
    //     assert_eq!(
    //         ReplacementCallback::normalize_case("Hello", &SimpleString::new("bYe")),
    //         "bYe"
    //     );
    //     // not ascii uppercase
    //     assert_eq!(
    //         ReplacementCallback::normalize_case("Привет", &SimpleString::new("bye")),
    //         "bye"
    //     );
    // }

    #[test]
    fn normalize_case_input_uppercase() {
        assert_eq!(
            ReplacementCallback::normalize_case("HELLO", &SimpleString::new("bye")),
            "BYE"
        );
        // has case variation -- do not touch it
        assert_eq!(
            ReplacementCallback::normalize_case("HELLO", &SimpleString::new("bYE")),
            "bYE"
        );
        // not ascii uppercase
        assert_eq!(
            ReplacementCallback::normalize_case("ПРИВЕТ", &SimpleString::new("bye")),
            "bye"
        );
        assert_eq!(
            ReplacementCallback::normalize_case("HELLO", &SimpleString::new("пока")),
            "пока"
        );
    }

    #[test]
    fn normalize_case_input_different_case() {
        assert_eq!(
            ReplacementCallback::normalize_case("hELLO", &SimpleString::new("bye")),
            "bye"
        );
    }

    #[test]
    fn normalize_case_input_different_case_same_len() {
        assert_eq!(
            ReplacementCallback::normalize_case("hELLO", &SimpleString::new("byeee")),
            "bYEEE"
        );
        assert_eq!(
            ReplacementCallback::normalize_case("hI!", &SimpleString::new("bye")),
            "bYe"
        );
        assert_eq!(
            ReplacementCallback::normalize_case("hI!", &SimpleString::new("Bye")),
            "Bye"
        );
    }

    #[test]
    fn callback_none() {
        let replacement = ReplacementCallback::Noop;

        let bar_capture = Regex::new("bar").unwrap().captures("bar").unwrap();
        let foo_capture = Regex::new("foo").unwrap().captures("foo").unwrap();

        assert_eq!(replacement.replace(&bar_capture, false), "bar".to_owned());
        assert_eq!(replacement.replace(&foo_capture, false), "foo".to_owned());
    }

    #[test]
    fn callback_simple() {
        let replacement = ReplacementCallback::Simple(SimpleString::new("bar"));

        let bar_capture = Regex::new("bar").unwrap().captures("bar").unwrap();
        let foo_capture = Regex::new("foo").unwrap().captures("foo").unwrap();

        assert_eq!(replacement.replace(&bar_capture, false), "bar".to_owned());
        assert_eq!(replacement.replace(&foo_capture, false), "bar".to_owned());
    }

    #[test]
    fn callback_any() {
        let replacement = ReplacementCallback::Any(vec![
            ReplacementCallback::Simple(SimpleString::new("bar")),
            ReplacementCallback::Simple(SimpleString::new("baz")),
        ]);

        let bar_capture = Regex::new("bar").unwrap().captures("bar").unwrap();
        let selected = replacement.replace(&bar_capture, false);

        assert!(vec!["bar".to_owned(), "baz".to_owned()].contains(&selected));
    }

    #[test]
    fn callback_weights() {
        let replacement = ReplacementCallback::Weights(vec![
            (1, ReplacementCallback::Simple(SimpleString::new("bar"))),
            (1, ReplacementCallback::Simple(SimpleString::new("baz"))),
            (0, ReplacementCallback::Simple(SimpleString::new("spam"))),
        ]);

        let bar_capture = Regex::new("bar").unwrap().captures("bar").unwrap();
        let selected = replacement.replace(&bar_capture, false);

        assert!(vec!["bar".to_owned(), "baz".to_owned()].contains(&selected));
    }
}
