//! The Rust half of `tests/contracts/enum-spellings-v1.json`: every case is
//! produced by the core parser, and every enum kind has an unknown-spelling
//! case.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde_json::Value;
use type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic;
use type_bridge_orm::{EnumKind, canonical_enum_spelling};

fn corpus() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/contracts/enum-spellings-v1.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The details of a diagnostic's canonical host JSON form.
fn details_json(diagnostic: &SdkExecutionDiagnostic) -> Value {
    serde_json::to_value(diagnostic.host_json()).unwrap()["details"].clone()
}

#[test]
fn every_case_is_the_core_parser_diagnostic() {
    let corpus = corpus();
    for case in corpus["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let error = canonical_enum_spelling(
            case["kind"].as_str().unwrap(),
            case["value"].as_str().unwrap(),
        )
        .expect_err(id);
        let diagnostic = SdkExecutionDiagnostic::from(error);
        let expected = &case["expected"];
        assert_eq!(diagnostic.category().as_str(), expected["category"], "{id}");
        assert_eq!(diagnostic.code().as_str(), expected["code"], "{id}");
        assert_eq!(details_json(&diagnostic), expected["details"], "{id}");
    }
}

#[test]
fn every_enum_kind_has_an_unknown_spelling_case() {
    let corpus = corpus();
    let covered = corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|case| case["id"].as_str()?.strip_suffix(".unknown"))
        .collect::<BTreeSet<_>>();
    let expected = EnumKind::ACCEPTED
        .iter()
        .copied()
        .chain([EnumKind::KIND])
        .collect::<BTreeSet<_>>();
    assert_eq!(covered, expected);
}
