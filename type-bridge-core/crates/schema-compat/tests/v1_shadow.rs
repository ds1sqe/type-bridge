use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_schema_compat::{
    ShadowComparison, ShadowCoverageState, ShadowDimension, ShadowLaneOutcome,
    ShadowUnavailableLane, ShadowVerdict, v1_shadow_report,
};

fn profile() -> SemanticProfileId {
    SemanticProfileId::new("typedb-3.12.1/v1").expect("the test profile is frozen")
}

#[test]
fn basic_effective_type_projection_matches_without_claiming_cutover_evidence() {
    let report = v1_shadow_report(
        r#"define
attribute content @abstract, value string;
attribute title @independent, sub content, value string;
entity person @abstract;
entity employee sub person;
relation employment, relates worker;
"#,
        &profile(),
    )
    .expect("shadow setup should succeed");

    assert!(matches!(report.v1_direct(), ShadowLaneOutcome::Accepted(_)));
    assert!(matches!(
        report.v1_effective(),
        ShadowLaneOutcome::Accepted(_)
    ));
    assert!(matches!(
        report.v2_declared(),
        ShadowLaneOutcome::Accepted(_)
    ));
    assert!(matches!(
        report.v2_effective(),
        ShadowLaneOutcome::Accepted(_)
    ));
    assert!(report.v2_declared_fingerprint().is_some());
    assert!(report.v2_semantic_fingerprint().is_some());

    let ShadowComparison::Compared(compared) = report.comparison() else {
        panic!("accepted effective lanes must be compared");
    };
    assert_eq!(compared.verdict(), ShadowVerdict::Matched);
    assert!(compared.findings().is_empty());
    assert!(
        compared
            .coverage()
            .covered()
            .contains(&ShadowDimension::DirectParent)
    );
    assert!(compared.coverage().unimplemented().is_empty());
    assert_eq!(
        compared.coverage().state(ShadowDimension::EffectiveOwns),
        ShadowCoverageState::Compared
    );
    assert_eq!(
        compared
            .coverage()
            .state(ShadowDimension::FunctionBodiesAndAnnotations),
        ShadowCoverageState::NotRepresentable
    );
    assert!(
        compared
            .coverage()
            .not_representable()
            .contains(&ShadowDimension::CardinalityOutsideV1U32)
    );
    assert!(!compared.is_cutover_evidence());
}

#[test]
fn effective_interfaces_functions_and_doc_meta_match() {
    let report = v1_shadow_report(
        r#"define
attribute name @doc("a name") @meta("source", "shared"), value string;
entity person @doc("a person") @meta("kind", "actor"),
  owns name @key @doc("legal name") @meta("column", "name"),
  plays employment:worker @card(0..) @doc("employment participation") @meta("edge", "worker");
relation employment @doc("a job") @meta("kind", "relation"),
  relates worker @card(0..1) @doc("worker role") @meta("endpoint", "worker");
fun people($candidate: person) -> { person }:
  match $candidate isa person;
  return { $candidate };
"#,
        &profile(),
    )
    .expect("shadow setup should succeed");

    let ShadowComparison::Compared(compared) = report.comparison() else {
        panic!("both effective lanes should accept the overlap schema: {report:#?}");
    };
    assert_eq!(compared.verdict(), ShadowVerdict::Matched);
    assert!(compared.findings().is_empty());
    for dimension in [
        ShadowDimension::EffectiveOwns,
        ShadowDimension::EffectiveRelates,
        ShadowDimension::EffectivePlays,
        ShadowDimension::DocumentationAndMetadata,
        ShadowDimension::FunctionSignatures,
    ] {
        assert_eq!(
            compared.coverage().state(dimension),
            ShadowCoverageState::Compared
        );
    }
    assert_eq!(
        compared.coverage().state(ShadowDimension::StructFields),
        ShadowCoverageState::NotRepresentable
    );
    assert!(!compared.coverage().is_complete());
    assert!(!compared.is_cutover_evidence());
}

#[test]
fn dual_rejection_is_not_reported_as_schema_equality() {
    let report = v1_shadow_report("not a define query", &profile())
        .expect("shadow setup should succeed even when lanes reject");

    assert!(matches!(report.v1_direct(), ShadowLaneOutcome::Rejected(_)));
    assert!(matches!(
        report.v1_effective(),
        ShadowLaneOutcome::Rejected(_)
    ));
    assert!(matches!(
        report.v2_declared(),
        ShadowLaneOutcome::Rejected(_)
    ));
    assert!(matches!(
        report.v2_effective(),
        ShadowLaneOutcome::NotRun(_)
    ));
    let ShadowComparison::NotCompared(not_compared) = report.comparison() else {
        panic!("rejecting lanes must not produce a comparison verdict");
    };
    assert!(
        not_compared
            .unavailable_lanes()
            .contains(&ShadowUnavailableLane::V1Effective)
    );
    assert!(
        not_compared
            .unavailable_lanes()
            .contains(&ShadowUnavailableLane::V2Declared)
    );
    assert!(
        not_compared
            .unavailable_lanes()
            .contains(&ShadowUnavailableLane::V2Effective)
    );
}

#[test]
fn report_is_deterministic_for_identical_source() {
    let source = "define\nattribute name, value string;\nentity person, owns name;\n";
    let first = v1_shadow_report(source, &profile()).expect("first report should succeed");
    let second = v1_shadow_report(source, &profile()).expect("second report should succeed");
    assert_eq!(first, second);
}
