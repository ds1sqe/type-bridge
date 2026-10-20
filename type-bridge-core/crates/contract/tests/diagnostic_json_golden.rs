//! The Rust half of `tests/contracts/diagnostic-json.json`: the contract
//! serializers produce each case Rust can build on its own.

use std::path::PathBuf;

use serde_json::Value;
use type_bridge_contract::codec::from_canonical_json;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCode, SdkDiagnosticDetailValue, SdkDiagnosticMessage, SdkDiagnosticName,
    SdkDiagnosticPathSegment, SdkExecutionDiagnostic,
};

fn golden(id: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/contracts/diagnostic-json.json");
    let corpus: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    corpus["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == id)
        .unwrap_or_else(|| panic!("golden has no case {id}"))["diagnostic"]
        .clone()
}

fn sdk_json(diagnostic: &SdkExecutionDiagnostic) -> Value {
    serde_json::to_value(diagnostic.host_json()).unwrap()
}

#[test]
fn canonical_limit_refusal_matches_the_golden() {
    let refusal = Diagnostic::new(
        DiagnosticCategory::ResourceLimit,
        DiagnosticCode::new("canonical_json_too_deep").unwrap(),
        "nested too deeply",
    );
    let diagnostic = SdkExecutionDiagnostic::from_canonical_limit(&refusal, true).unwrap();
    assert_eq!(sdk_json(&diagnostic), golden("canonical_limit.depth"));
    let other = Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new("malformed_canonical_json").unwrap(),
        "not JSON",
    );
    assert!(SdkExecutionDiagnostic::from_canonical_limit(&other, true).is_none());
}

#[test]
fn integer_details_are_decimal_strings_at_their_extremes() {
    let name = |value: &str| SdkDiagnosticName::new(value).unwrap();
    let diagnostic = SdkExecutionDiagnostic::resource_limit(
        SdkDiagnosticCode::new("fixture_integer_range").unwrap(),
        SdkDiagnosticMessage::new("Fixture integer range").unwrap(),
    )
    .try_with_path_prefix([
        SdkDiagnosticPathSegment::Argument(name("rows")),
        SdkDiagnosticPathSegment::Index(4_095),
        SdkDiagnosticPathSegment::QueryBinding(u16::MAX),
        SdkDiagnosticPathSegment::QueryOutputSlot(3),
    ])
    .and_then(|d| d.try_with_detail(name("count"), SdkDiagnosticDetailValue::Count(u64::MAX)))
    .and_then(|d| d.try_with_detail(name("bytes"), SdkDiagnosticDetailValue::ByteCount(u64::MAX)))
    .and_then(|d| {
        d.try_with_detail(
            name("signed_min"),
            SdkDiagnosticDetailValue::Signed(i64::MIN),
        )
    })
    .and_then(|d| {
        d.try_with_detail(
            name("signed_max"),
            SdkDiagnosticDetailValue::Signed(i64::MAX),
        )
    })
    .unwrap();
    assert_eq!(sdk_json(&diagnostic), golden("integer_range.extremes"));
}

#[test]
fn contract_diagnostic_serde_form_matches_the_golden() {
    let error = from_canonical_json::<Value>(b"{").unwrap_err();
    assert_eq!(
        serde_json::to_value(&error).unwrap(),
        golden("contract.malformed_canonical_json")
    );
}
