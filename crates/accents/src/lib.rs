use std::{borrow::Cow, collections::BTreeMap, str::FromStr};

use rand::seq::SliceRandom;
use regex::{Captures, Regex};
use serde::Deserialize;

/// Receives match and provides replacement
#[derive(Debug, Deserialize, PartialEq, Clone)]
enum ReplacementCallback {
    /// Do not replace
    Noop,
    // TODO: either a separate variant or modify Simple to allow formatting using capture groups:
    //       "hello {1}" would insert group 1
    // TODO: implement a bit of serde magic for easier parsing: string would turn into `Simple`,
    //       array into `Any` and map with u64 keys into `Weights`
    /// Puts string as is
    Simple(String),
    /// Selects random replacement with equal weights
    Any(Vec<ReplacementCallback>),
    /// Selects replacement based on relative weights
    Weights(Vec<(u64, ReplacementCallback)>),
}

impl ReplacementCallback {
    /// Checks things to prevent runtime panics. Do not forget to call it!
    ///
    /// TODO: Should probably be done properly by using separate enum and do these checks after
    /// creation
    fn validate_self(&self) -> Result<(), String> {
        match self {
            Self::Any(targets) => {
                if targets.is_empty() {
                    Err("Empty Any".to_owned())
                } else {
                    Ok(())
                }
            }
            Self::Weights(items) => {
                if items.is_empty() {
                    return Err("Empty Weights".to_owned());
                }

                if items.iter().map(|(i, _)| i).sum::<u64>() == 0 {
                    return Err("Weights add up to 0".to_owned());
                }

                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn replace(&self, caps: &Captures) -> String {
        match self {
            Self::Noop => caps[0].to_owned(),
            Self::Simple(target) => target.clone(),
            Self::Any(targets) => {
                let mut rng = rand::thread_rng();

                targets
                    .choose(&mut rng)
                    .expect("empty targets")
                    .replace(caps)
            }
            Self::Weights(items) => {
                let mut rng = rand::thread_rng();

                items
                    .choose_weighted(&mut rng, |item| item.0)
                    .expect("empty targets")
                    .1
                    .replace(caps)
            }
        }
    }
}

/// Maps regex to callback
#[derive(Debug)]
struct Replacement {
    source: Regex,
    cb: ReplacementCallback,
}

impl Replacement {
    // try to learn something about strings and adjust case accordingly. all logic is currently
    // ascii only
    // tried using Cows but my computer exploded. TODO: try that again
    fn normalize_case<'a>(old: &str, mut new: Cow<'a, str>) -> Cow<'a, str> {
        // no constraints if original was all lowercase
        if old.chars().all(|c| c.is_ascii_lowercase()) {
            return new;
        }

        // if first letter is uppercase ascii and the rest are lowercase ascii, uppercase first
        // letter of replacement
        let mut iter = old.chars();
        // TODO: let plus && is unstable, merge into single if when stable
        if let Some(first) = iter.next() {
            if first.is_ascii_uppercase() && iter.all(|c| c.is_ascii_lowercase()) {
                // if there is case variation in replacement, better not touch it
                if !new.chars().all(|c| c.is_ascii_lowercase()) {
                    return new;
                }

                if let Some(r) = new.to_mut().get_mut(..1) {
                    r.make_ascii_uppercase();
                }
            }
        }

        // if original was all uppercase we force all uppercase for replacement. this is likely to
        // give false positives on short inputs like "I" or abbreviations
        if old.chars().all(|c| c.is_ascii_uppercase()) {
            // if there is case variation in replacement, better not touch it
            if !new.chars().all(|c| c.is_ascii_lowercase()) {
                return new;
            }

            return Cow::from(new.to_ascii_uppercase());
        }

        // any other more complex case
        new
    }

    fn apply(&self, text: &str, normalize_case: bool) -> String {
        self.source
            .replace_all(text, |caps: &Captures| {
                let new = self.cb.replace(caps);
                if normalize_case {
                    Self::normalize_case(text, Cow::from(new))
                } else {
                    Cow::from(new)
                }
            })
            .to_string()
    }
}

impl PartialEq for Replacement {
    fn eq(&self, other: &Self) -> bool {
        self.source.as_str() == other.source.as_str() && self.cb == other.cb
    }
}

#[derive(Debug, Deserialize)]
struct WordsAndPatternsDefinition {
    #[serde(default)]
    words: Vec<(String, ReplacementCallback)>,
    #[serde(default)]
    patterns: Vec<(String, ReplacementCallback)>,
}

/// Either replaces everything from previous severity using `Replace` or adds new words and
/// patterns to the end of previous ones with `Extend`
#[derive(Debug, Deserialize)]
enum Severity {
    Replace(WordsAndPatternsDefinition),
    Extend(WordsAndPatternsDefinition),
}

fn default_bool_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct AccentDefinition {
    name: String,
    #[serde(default = "default_bool_true")]
    normalize_case: bool,
    #[serde(default)]
    words: Vec<(String, ReplacementCallback)>,
    #[serde(default)]
    patterns: Vec<(String, ReplacementCallback)>,
    #[serde(default)]
    severities: BTreeMap<u64, Severity>,
}

impl TryFrom<AccentDefinition> for Accent {
    type Error = String;

    fn try_from(accent_def: AccentDefinition) -> Result<Self, Self::Error> {
        Self::new(
            accent_def.name,
            accent_def.normalize_case,
            accent_def.words,
            accent_def.patterns,
            accent_def.severities,
        )
    }
}

/// Replaces patterns in text according to rules
#[derive(Debug, PartialEq)]
pub struct Accent {
    pub name: String,
    normalize_case: bool,
    // a copy of replacements for each severity level, sorted from lowest to highest
    severities: Vec<(u64, Vec<Replacement>)>,
}

impl Accent {
    fn make_replacements(
        words: Vec<(String, ReplacementCallback)>,
        patterns: Vec<(String, ReplacementCallback)>,
    ) -> Result<Vec<Replacement>, String> {
        let mut replacements = Vec::with_capacity(words.len() + patterns.len());

        for (i, (pattern, replacement)) in words.into_iter().enumerate() {
            // ignorecase only if input is lowercase
            let regex_flags = if pattern.chars().all(|c| c.is_ascii_lowercase()) {
                "mi"
            } else {
                "m"
            };
            let word_regex = Regex::new(&format!(r"(?{regex_flags})\b{pattern}\b"))
                .map_err(|err| format!("bad regex for word {}: {}: {}", i, pattern, err))?;

            replacements.push(Replacement {
                source: word_regex,
                cb: replacement,
            });
        }

        for (i, (pattern, replacement)) in patterns.into_iter().enumerate() {
            // ignorecase only if input is lowercase
            let regex_flags = if pattern.chars().all(|c| c.is_ascii_lowercase()) {
                "mi"
            } else {
                "m"
            };
            let word_regex = Regex::new(&format!(r"(?{regex_flags}){pattern}"))
                .map_err(|err| format!("bad regex for pattern {}: {}: {}", i, pattern, err))?;

            replacements.push(Replacement {
                source: word_regex,
                cb: replacement,
            });
        }

        for (i, replacement) in replacements.iter().enumerate() {
            if let Err(err) = replacement.cb.validate_self() {
                return Err(format!("replacement {} invalid: {}", i, err));
            }
        }

        Ok(replacements)
    }

    // keeps collection order, rewrites left duplicates with right ones
    fn dedup_patterns(
        collection: Vec<(String, ReplacementCallback)>,
        collection_name: &str,
        drop_expected: bool,
    ) -> Vec<(String, ReplacementCallback)> {
        let mut filtered = vec![];
        let mut seen = BTreeMap::<String, usize>::new();

        let mut i = 0;
        for word in collection {
            if let Some(previous) = seen.get(&word.0) {
                filtered[*previous] = word.clone();
                if !drop_expected {
                    log::warn!(
                        "{} already present at position {} in {}",
                        word.0,
                        previous,
                        collection_name,
                    );
                }
            } else {
                seen.insert(word.0.clone(), i);
                filtered.push(word);
                i += 1;
            }
        }

        filtered
    }

    fn new(
        name: String,
        normalize_case: bool,
        mut words: Vec<(String, ReplacementCallback)>,
        mut patterns: Vec<(String, ReplacementCallback)>,
        severities_def: BTreeMap<u64, Severity>,
    ) -> Result<Self, String> {
        words = Self::dedup_patterns(words, "words", false);
        patterns = Self::dedup_patterns(patterns, "patterns", false);

        let mut severities = Vec::with_capacity(severities_def.len());

        severities.push((0, Self::make_replacements(words.clone(), patterns.clone())?));

        for (severity, override_or_addition) in severities_def {
            if severity == 0 {
                return Err("Severity cannot be 0 since 0 is base one".to_owned());
            }

            let replacements = match override_or_addition {
                Severity::Replace(overrides) => {
                    words = Self::dedup_patterns(overrides.words, "words", false);
                    patterns = Self::dedup_patterns(overrides.patterns, "patterns", false);

                    Self::make_replacements(words.clone(), patterns.clone())?
                }
                Severity::Extend(additions) => {
                    // no duplicates are allowed inside new definitions
                    let new_words = Self::dedup_patterns(additions.words, "words", false);
                    let new_patterns = Self::dedup_patterns(additions.patterns, "patterns", false);

                    // NOTE: we do not just add everything to the end of `replacements`. words and
                    // patterns maintain relative order where words are always first
                    words.extend(new_words);
                    patterns.extend(new_patterns);

                    // we deduped old and new words separately, now they are merged. dedup again
                    // without warnings. new ones take priority over old while keeping position
                    words = Self::dedup_patterns(words, "words", true);
                    patterns = Self::dedup_patterns(patterns, "patterns", true);

                    Self::make_replacements(words.clone(), patterns.clone())?
                }
            };

            severities.push((severity, replacements));
        }

        Ok(Self {
            name,
            normalize_case,
            severities,
        })
    }

    /// Returns all registered severities in ascending order. Note that there may be gaps
    pub fn severities(&self) -> Vec<u64> {
        self.severities.iter().map(|(k, _)| *k).collect()
    }

    /// Walks rules for given severity from top to bottom and applies them
    pub fn apply(&self, text: &str, severity: u64) -> String {
        // TODO: binary search? probably now worth
        //
        // Go from the end and pick first severity that is less or eaual to requested. This is
        // guaranteed to return something because base severity 0 is always present at the bottom
        // and 0 <= x is true for any u64
        let replacements = &self
            .severities
            .iter()
            .rev()
            .find(|(sev, _)| *sev <= severity)
            .expect("severity 0 is always present")
            .1;

        let mut result = text.to_owned();

        // apply rules from top to bottom
        for replacement in replacements {
            result = replacement.apply(&result, self.normalize_case);
        }

        result
    }
}

impl FromStr for Accent {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(
            ron::from_str::<AccentDefinition>(s)
                .map_err(|err| format!("unable to load accent definition: {}", err))?,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, vec};

    use super::*;

    #[test]
    fn normalize_case_input_lowercase() {
        assert_eq!(
            Replacement::normalize_case("hello", Cow::from("bye")),
            "bye"
        );
        assert_eq!(
            Replacement::normalize_case("hello", Cow::from("Bye")),
            "Bye"
        );
        assert_eq!(
            Replacement::normalize_case("hello", Cow::from("bYE")),
            "bYE"
        );
    }

    #[test]
    fn normalize_case_input_titled() {
        assert_eq!(
            Replacement::normalize_case("Hello", Cow::from("bye")),
            "Bye"
        );
        // has case variation -- do not touch it
        assert_eq!(
            Replacement::normalize_case("Hello", Cow::from("bYe")),
            "bYe"
        );
        // not ascii uppercase
        assert_eq!(
            Replacement::normalize_case("Привет", Cow::from("bye")),
            "bye"
        );
    }

    #[test]
    fn normalize_case_input_uppercase() {
        assert_eq!(
            Replacement::normalize_case("HELLO", Cow::from("bye")),
            "BYE"
        );
        // has case variation -- do not touch it
        assert_eq!(
            Replacement::normalize_case("HELLO", Cow::from("bYE")),
            "bYE"
        );
        // not ascii uppercase
        assert_eq!(
            Replacement::normalize_case("ПРИВЕТ", Cow::from("bye")),
            "bye"
        );
        assert_eq!(
            Replacement::normalize_case("HELLO", Cow::from("пока")),
            "пока"
        );
    }

    #[test]
    fn callback_none() {
        let replacement = ReplacementCallback::Noop;

        let bar_capture = Regex::new("bar").unwrap().captures("bar").unwrap();
        let foo_capture = Regex::new("foo").unwrap().captures("foo").unwrap();

        assert_eq!(replacement.replace(&bar_capture), "bar".to_owned());
        assert_eq!(replacement.replace(&foo_capture), "foo".to_owned());
    }

    #[test]
    fn callback_simple() {
        let replacement = ReplacementCallback::Simple("bar".to_owned());

        let bar_capture = Regex::new("bar").unwrap().captures("bar").unwrap();
        let foo_capture = Regex::new("foo").unwrap().captures("foo").unwrap();

        assert_eq!(replacement.replace(&bar_capture), "bar".to_owned());
        assert_eq!(replacement.replace(&foo_capture), "bar".to_owned());
    }

    #[test]
    fn callback_any() {
        let replacement = ReplacementCallback::Any(vec![
            ReplacementCallback::Simple("bar".to_owned()),
            ReplacementCallback::Simple("baz".to_owned()),
        ]);

        let bar_capture = Regex::new("bar").unwrap().captures("bar").unwrap();
        let selected = replacement.replace(&bar_capture);

        assert!(vec!["bar".to_owned(), "baz".to_owned()].contains(&selected));
    }

    #[test]
    fn callback_weights() {
        let replacement = ReplacementCallback::Weights(vec![
            (1, ReplacementCallback::Simple("bar".to_owned())),
            (1, ReplacementCallback::Simple("baz".to_owned())),
            (0, ReplacementCallback::Simple("spam".to_owned())),
        ]);

        let bar_capture = Regex::new("bar").unwrap().captures("bar").unwrap();
        let selected = replacement.replace(&bar_capture);

        assert!(vec!["bar".to_owned(), "baz".to_owned()].contains(&selected));
    }

    #[test]
    fn e() {
        let e = Accent::new(
            "E".to_owned(),
            false,
            vec![],
            vec![
                (
                    r"(?-i)[a-z]".to_owned(),
                    ReplacementCallback::Simple("e".to_owned()),
                ),
                (
                    r"(?-i)[A-Z]".to_owned(),
                    ReplacementCallback::Simple("E".to_owned()),
                ),
            ],
            BTreeMap::new(),
        )
        .unwrap();

        assert_eq!(e.apply("Hello World!", 0), "Eeeee Eeeee!");
    }

    #[test]
    fn ron_minimal() {
        let _ = Accent::from_str(r#"(name: "myname")"#).unwrap();
    }

    #[test]
    fn ron_empty() {
        let _ = Accent::from_str(r#"(name: "foobar", words: [], patterns: [], severities: {})"#)
            .unwrap();
    }

    #[test]
    fn ron_extend_extends() {
        let parsed = Accent::from_str(
            r#"
(
    name: "extend",
    words: [("a", Noop)],
    patterns: [("1", Noop)],
    severities: {
        1: Extend(
            (
                words: [("b", Noop)],
                patterns: [("2", Noop)],
            )

        ),
    },
)
"#,
        )
        .unwrap();

        let manual = Accent {
            name: "extend".to_owned(),
            normalize_case: true,
            severities: vec![
                (
                    0,
                    vec![
                        Replacement {
                            source: Regex::new(r"(?mi)\ba\b").unwrap(),
                            cb: ReplacementCallback::Noop,
                        },
                        Replacement {
                            source: Regex::new("(?m)1").unwrap(),
                            cb: ReplacementCallback::Noop,
                        },
                    ],
                ),
                (
                    1,
                    vec![
                        Replacement {
                            source: Regex::new(r"(?mi)\ba\b").unwrap(),
                            cb: ReplacementCallback::Noop,
                        },
                        Replacement {
                            source: Regex::new(r"(?mi)\bb\b").unwrap(),
                            cb: ReplacementCallback::Noop,
                        },
                        Replacement {
                            source: Regex::new("(?m)1").unwrap(),
                            cb: ReplacementCallback::Noop,
                        },
                        Replacement {
                            source: Regex::new("(?m)2").unwrap(),
                            cb: ReplacementCallback::Noop,
                        },
                    ],
                ),
            ],
        };

        assert_eq!(parsed, manual);
        assert_eq!(parsed.severities(), manual.severities());
    }

    #[test]
    fn ron_replace_replaces() {
        let parsed = Accent::from_str(
            r#"
(
    name: "replace",
    words: [("a", Noop)],
    patterns: [("1", Noop)],
    severities: {
        1: Replace(
            (
                words: [("b", Noop)],
                patterns: [("2", Noop)],
            )

        ),
    },
)
"#,
        )
        .unwrap();

        let manual = Accent {
            name: "replace".to_owned(),
            normalize_case: true,
            severities: vec![
                (
                    0,
                    vec![
                        Replacement {
                            source: Regex::new(r"(?mi)\ba\b").unwrap(),
                            cb: ReplacementCallback::Noop,
                        },
                        Replacement {
                            source: Regex::new("(?m)1").unwrap(),
                            cb: ReplacementCallback::Noop,
                        },
                    ],
                ),
                (
                    1,
                    vec![
                        Replacement {
                            source: Regex::new(r"(?mi)\bb\b").unwrap(),
                            cb: ReplacementCallback::Noop,
                        },
                        Replacement {
                            source: Regex::new("(?m)2").unwrap(),
                            cb: ReplacementCallback::Noop,
                        },
                    ],
                ),
            ],
        };

        assert_eq!(parsed, manual);
    }

    #[test]
    fn ron_invalid_callback_any() {
        assert!(Accent::from_str(
            r#"
(
    name: "foobar",
    patterns:
        [
            ("a", Any([]))
        ]
)
"#
        )
        .err()
        .unwrap()
        .contains("Empty Any"));
    }

    #[test]
    fn ron_invalid_callback_weighted() {
        assert!(Accent::from_str(
            r#"
(
    name: "foobar",
    patterns:
        [
            ("a", Weights([]))
        ]
)
"#
        )
        .err()
        .unwrap()
        .contains("Empty Weights"));

        assert!(Accent::from_str(
            r#"
(
    name: "foobar",
    patterns:
        [
            ("a", Weights(
                [
                    (0, Noop),
                    (0, Noop),
                ]
            ))
        ]
)
"#
        )
        .err()
        .unwrap()
        .contains("Weights add up to 0"));
    }

    #[test]
    fn ron_severity_starts_from_0() {
        assert!(
            Accent::from_str(r#"(name: "0", severities: { 0: Extend(()) })"#)
                .err()
                .unwrap()
                .contains("Severity cannot be 0")
        );
    }

    #[test]
    fn ron_malformed() {
        assert!(Accent::from_str(r#"(name: "borken..."#).is_err());
    }

    #[test]
    fn ron_all_features() {
        let ron_string = r#"
(
    name: "test",
    normalize_case: true,
    words: [
        ("test", Simple("Testing in progress; Please ignore ...")),
        ("badword", Simple("")),
        ("dupe", Simple("0")),
    ],
    patterns: [
        // lowercase letters are replaced with e
        ("[a-z]", Simple("e")),
        // uppercase letters are replaced with 50% uppercase "E" and 10% for each of the cursed "E"
        ("[A-Z]", Weights(
            [
                (5, Simple("E")),
                (1, Simple("Ē")),
                (1, Simple("Ê")),
                (1, Simple("Ë")),
                (1, Simple("È")),
                (1, Simple("É")),
            ],
        )),
        // numbers are replaced with 6 or 9 or are left untouched
        // excessive nesting that does nothing
        ("[0-9]", Any(
            [
                Weights(
                    [
                        (1, Any(
                            [
                              Simple("6"),
                              Simple("9"),
                              Noop,
                            ],
                        )),
                    ],
                ),
            ],
        )),
    ],
    severities: {
        1: Replace(
            (
                words: [
                    ("replaced", Simple("words")),
                    ("dupe", Simple("1")),
                    ("Windows", Simple("Linux")),
                ],
                patterns: [
                    ("a+", Simple("multiple A's")),
                    ("^", Simple("start")),
                ],
            )
        ),
        2: Extend(
            (
                words: [
                    ("dupe", Simple("2")),
                    ("added", Simple("words")),
                ],
                patterns: [
                    ("b+", Simple("multiple B's")),
                    ("$", Simple("end")),
                ],
            )
        ),
    },
)
"#;

        let parsed = Accent::from_str(ron_string).unwrap();
        let manual = Accent {
            name: "test".to_owned(),
            normalize_case: true,
            severities: vec![
                (
                    0,
                    vec![
                        Replacement {
                            source: Regex::new(r"(?mi)\btest\b").unwrap(),
                            cb: ReplacementCallback::Simple(
                                "Testing in progress; Please ignore ...".to_owned(),
                            ),
                        },
                        Replacement {
                            source: Regex::new(r"(?mi)\bbadword\b").unwrap(),
                            cb: ReplacementCallback::Simple("".to_owned()),
                        },
                        Replacement {
                            source: Regex::new(r"(?mi)\bdupe\b").unwrap(),
                            cb: ReplacementCallback::Simple("0".to_owned()),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)[a-z]").unwrap(),
                            cb: ReplacementCallback::Simple("e".to_owned()),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)[A-Z]").unwrap(),
                            cb: ReplacementCallback::Weights(vec![
                                (5, ReplacementCallback::Simple("E".to_owned())),
                                (1, ReplacementCallback::Simple("Ē".to_owned())),
                                (1, ReplacementCallback::Simple("Ê".to_owned())),
                                (1, ReplacementCallback::Simple("Ë".to_owned())),
                                (1, ReplacementCallback::Simple("È".to_owned())),
                                (1, ReplacementCallback::Simple("É".to_owned())),
                            ]),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)[0-9]").unwrap(),
                            cb: ReplacementCallback::Any(vec![ReplacementCallback::Weights(vec![
                                (
                                    1,
                                    ReplacementCallback::Any(vec![
                                        ReplacementCallback::Simple("6".to_owned()),
                                        ReplacementCallback::Simple("9".to_owned()),
                                        ReplacementCallback::Noop,
                                    ]),
                                ),
                            ])]),
                        },
                    ],
                ),
                (
                    1,
                    vec![
                        Replacement {
                            source: Regex::new(r"(?mi)\breplaced\b").unwrap(),
                            cb: ReplacementCallback::Simple("words".to_owned()),
                        },
                        Replacement {
                            source: Regex::new(r"(?mi)\bdupe\b").unwrap(),
                            cb: ReplacementCallback::Simple("1".to_owned()),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)\bWindows\b").unwrap(),
                            cb: ReplacementCallback::Simple("Linux".to_owned()),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)a+").unwrap(),
                            cb: ReplacementCallback::Simple("multiple A's".to_owned()),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)^").unwrap(),
                            cb: ReplacementCallback::Simple("start".to_owned()),
                        },
                    ],
                ),
                (
                    2,
                    vec![
                        Replacement {
                            source: Regex::new(r"(?mi)\breplaced\b").unwrap(),
                            cb: ReplacementCallback::Simple("words".to_owned()),
                        },
                        Replacement {
                            source: Regex::new(r"(?mi)\bdupe\b").unwrap(),
                            cb: ReplacementCallback::Simple("2".to_owned()),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)\bWindows\b").unwrap(),
                            cb: ReplacementCallback::Simple("Linux".to_owned()),
                        },
                        Replacement {
                            source: Regex::new(r"(?mi)\badded\b").unwrap(),
                            cb: ReplacementCallback::Simple("words".to_owned()),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)a+").unwrap(),
                            cb: ReplacementCallback::Simple("multiple A's".to_owned()),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)^").unwrap(),
                            cb: ReplacementCallback::Simple("start".to_owned()),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)b+").unwrap(),
                            cb: ReplacementCallback::Simple("multiple B's".to_owned()),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)$").unwrap(),
                            cb: ReplacementCallback::Simple("end".to_owned()),
                        },
                    ],
                ),
            ],
        };
        assert_eq!(manual, parsed);

        // TODO: either patch rand::thread_rng somehow or change interface to pass rng directly?
        // let test_string = "Hello World! test 12 23";
        // for severity in manual.severities() {
        //     assert_eq!(parsed.apply(test_string, severity), manual.apply(test_string, severity));
        //  }
    }

    #[test]
    fn duplicates_eliminated() {
        let parsed = Accent::from_str(
            r#"
(
    name: "dupes",
    words: [
        ("dupew", Simple("0")),
        ("dupew", Simple("1")),
        ("dupew", Simple("2")),
    ],
    patterns: [
        ("dupep", Simple("0")),
        ("dupep", Simple("1")),
        ("dupep", Simple("2")),
    ],
)
"#,
        )
        .unwrap();

        let manual = Accent {
            name: "dupes".to_owned(),
            normalize_case: true,
            severities: vec![(
                0,
                vec![
                    Replacement {
                        source: Regex::new(r"(?mi)\bdupew\b").unwrap(),
                        cb: ReplacementCallback::Simple("2".to_owned()),
                    },
                    Replacement {
                        source: Regex::new(r"(?mi)dupep").unwrap(),
                        cb: ReplacementCallback::Simple("2".to_owned()),
                    },
                ],
            )],
        };

        assert_eq!(parsed, manual);
    }

    #[test]
    fn severity_selection() {
        let accent = Accent::from_str(
            r#"
(
    name: "severity",
    words: [("severity", Simple("0"))],
    severities: {
        1: Replace(
            (
                words: [("severity", Simple("1"))],
            )

        ),
        5: Replace(
            (
                words: [("severity", Simple("5"))],
            )

        ),
    },
)
"#,
        )
        .unwrap();

        assert_eq!(accent.apply("severity", 0), "0");
        assert_eq!(accent.apply("severity", 1), "1");
        assert_eq!(accent.apply("severity", 4), "1");
        assert_eq!(accent.apply("severity", 5), "5");
        assert_eq!(accent.apply("severity", 9000 + 1), "5");
    }

    #[test]
    fn included_accents() {
        let sample_text = fs::read_to_string("tests/sample_text.txt").expect("reading sample text");

        for file in fs::read_dir("accents").expect("read symlinked accents folder") {
            let filename = file.expect("getting file info").path();
            println!("parsing {}", filename.display());

            let accent =
                Accent::from_str(&fs::read_to_string(filename).expect("reading file")).unwrap();

            for severity in accent.severities() {
                let _ = accent.apply(&sample_text, severity);
            }
        }
    }
}
