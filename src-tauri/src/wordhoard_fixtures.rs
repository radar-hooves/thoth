//! Wordhoard's shared conformance fixtures (version 1), run against Thoth's own
//! apply path: every entry in list order, then the canonical terms, as the
//! pipeline does. Thoth is the reference implementation, so all cases run,
//! snapping ones included.
//!
//! `tests/fixtures/wordhoard/apply-v1.json` is vendored from wordhoard
//! (`backend/src/wordhoard/api/dictionary/fixtures/apply-v1.json`, 3a4652da).

use crate::canonical::{CanonicalTerm, apply_list};
use crate::dictionary::DictionaryEntry;
use serde::Deserialize;

const FIXTURES: &str = include_str!("../tests/fixtures/wordhoard/apply-v1.json");

#[derive(Deserialize)]
struct Fixtures {
    version: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    list: List,
    text: String,
    expected: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct List {
    entries: Vec<DictionaryEntry>,
    canonical_terms: Vec<CanonicalTerm>,
}

#[test]
fn wordhoard_apply_fixtures_v1() {
    let fixtures: Fixtures = serde_json::from_str(FIXTURES).expect("fixtures parse");
    assert_eq!(fixtures.version, "1");
    let mut failed = Vec::new();
    for c in &fixtures.cases {
        let got = apply_list(&c.list.entries, &c.list.canonical_terms, &c.text);
        if got != c.expected {
            failed.push(format!(
                "{}: {:?} -> {:?}, expected {:?}",
                c.id, c.text, got, c.expected
            ));
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {} fixture cases failed:\n{}",
        failed.len(),
        fixtures.cases.len(),
        failed.join("\n")
    );
}
