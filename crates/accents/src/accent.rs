use crate::replacement::{Replacement, ReplacementCallback};
use crate::ron_defs::AccentDef;
use crate::severity::Severity;

use std::{collections::BTreeMap, str::FromStr};

use regex::Regex;

impl TryFrom<AccentDef> for Accent {
    type Error = String;

    // NOTE: this should all go away with custom Deserialize hopefully?
    fn try_from(accent_def: AccentDef) -> Result<Self, Self::Error> {
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

        let mut severities = BTreeMap::new();
        for (severity_level, severity_def) in accent_def.severities.into_iter() {
            let severity: Severity = match severity_def.try_into() {
                Err(err) => Err(format!("error in severity {severity_level}: {err}"))?,
                Ok(callback) => callback,
            };

            severities.insert(severity_level, severity);
        }

        Self::new(
            accent_def.name,
            accent_def.normalize_case,
            words,
            patterns,
            severities,
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

    pub(crate) fn new(
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
            ron::from_str::<AccentDef>(s)
                .map_err(|err| format!("unable to load accent definition: {}", err))?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::replacement::SimpleString;

    use std::{fs, vec};

    #[test]
    fn e() {
        let e = Accent::new(
            "E".to_owned(),
            false,
            vec![],
            vec![
                (
                    r"(?-i)[a-z]".to_owned(),
                    ReplacementCallback::Simple(SimpleString::new("e")),
                ),
                (
                    r"(?-i)[A-Z]".to_owned(),
                    ReplacementCallback::Simple(SimpleString::new("E")),
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
                            cb: ReplacementCallback::Simple(SimpleString::new(
                                "Testing in progress; Please ignore ...",
                            )),
                        },
                        Replacement {
                            source: Regex::new(r"(?mi)\bbadword\b").unwrap(),
                            cb: ReplacementCallback::Simple(SimpleString::new("")),
                        },
                        Replacement {
                            source: Regex::new(r"(?mi)\bdupe\b").unwrap(),
                            cb: ReplacementCallback::Simple(SimpleString::new("0")),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)[a-z]").unwrap(),
                            cb: ReplacementCallback::Simple(SimpleString::new("e")),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)[A-Z]").unwrap(),
                            cb: ReplacementCallback::Weights(vec![
                                (5, ReplacementCallback::Simple(SimpleString::new("E"))),
                                (1, ReplacementCallback::Simple(SimpleString::new("Ē"))),
                                (1, ReplacementCallback::Simple(SimpleString::new("Ê"))),
                                (1, ReplacementCallback::Simple(SimpleString::new("Ë"))),
                                (1, ReplacementCallback::Simple(SimpleString::new("È"))),
                                (1, ReplacementCallback::Simple(SimpleString::new("É"))),
                            ]),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)[0-9]").unwrap(),
                            cb: ReplacementCallback::Any(vec![ReplacementCallback::Weights(vec![
                                (
                                    1,
                                    ReplacementCallback::Any(vec![
                                        ReplacementCallback::Simple(SimpleString::new("6")),
                                        ReplacementCallback::Simple(SimpleString::new("9")),
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
                            cb: ReplacementCallback::Simple(SimpleString::new("words")),
                        },
                        Replacement {
                            source: Regex::new(r"(?mi)\bdupe\b").unwrap(),
                            cb: ReplacementCallback::Simple(SimpleString::new("1")),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)\bWindows\b").unwrap(),
                            cb: ReplacementCallback::Simple(SimpleString::new("Linux")),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)a+").unwrap(),
                            cb: ReplacementCallback::Simple(SimpleString::new("multiple A's")),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)^").unwrap(),
                            cb: ReplacementCallback::Simple(SimpleString::new("start")),
                        },
                    ],
                ),
                (
                    2,
                    vec![
                        Replacement {
                            source: Regex::new(r"(?mi)\breplaced\b").unwrap(),
                            cb: ReplacementCallback::Simple(SimpleString::new("words")),
                        },
                        Replacement {
                            source: Regex::new(r"(?mi)\bdupe\b").unwrap(),
                            cb: ReplacementCallback::Simple(SimpleString::new("2")),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)\bWindows\b").unwrap(),
                            cb: ReplacementCallback::Simple(SimpleString::new("Linux")),
                        },
                        Replacement {
                            source: Regex::new(r"(?mi)\badded\b").unwrap(),
                            cb: ReplacementCallback::Simple(SimpleString::new("words")),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)a+").unwrap(),
                            cb: ReplacementCallback::Simple(SimpleString::new("multiple A's")),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)^").unwrap(),
                            cb: ReplacementCallback::Simple(SimpleString::new("start")),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)b+").unwrap(),
                            cb: ReplacementCallback::Simple(SimpleString::new("multiple B's")),
                        },
                        Replacement {
                            source: Regex::new(r"(?m)$").unwrap(),
                            cb: ReplacementCallback::Simple(SimpleString::new("end")),
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
                        cb: ReplacementCallback::Simple(SimpleString::new("2")),
                    },
                    Replacement {
                        source: Regex::new(r"(?mi)dupep").unwrap(),
                        cb: ReplacementCallback::Simple(SimpleString::new("2")),
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
