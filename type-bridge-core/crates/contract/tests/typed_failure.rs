//! Additive typed diagnostics; the released corpus remains untouched.
use std::collections::BTreeMap;
use type_bridge_contract::failure::{AnnotationKind, Code, Failure, LockKind, Phase, Reason};
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::schema::OwnsFactId;
use type_bridge_contract::sdk_diagnostic::{
    FailureContext, SdkCommitFailureOutcome, SdkExecutionDiagnostic, SdkProviderOperation,
};

fn cases() -> BTreeMap<String, serde_json::Value> {
    let mut cases = BTreeMap::new();
    let owner = TypeId::new(TypeKind::Entity, "item").unwrap();
    let field = OwnsFactId::new(owner.clone(), AttributeId::new("score").unwrap()).unwrap();
    for phase in [Phase::Query, Phase::Commit] {
        for reason in [
            Reason::Conflict {
                lock: LockKind::Exclusive,
            },
            Reason::Cardinality,
            Reason::Unique,
            Reason::Constraint {
                annotation: AnnotationKind::Range,
            },
            Reason::Other {
                code: Code::new("DVL1").unwrap(),
            },
        ] {
            let failure = Failure::Rejected {
                phase,
                reason,
                op_index: Some(2),
            };
            let diagnostic = SdkExecutionDiagnostic::from_failure(
                &failure,
                FailureContext::new(Some(owner.clone()), Some(field.clone())),
            );
            assert_eq!(diagnostic.failure(), Some(&failure));
            assert_eq!(
                diagnostic.commit_outcome(),
                if phase == Phase::Commit {
                    Some(SdkCommitFailureOutcome::DefinitelyAborted)
                } else {
                    None
                }
            );
            let value = serde_json::to_value(diagnostic.host_json()).unwrap();
            let name = format!("{}-{}", diagnostic.code(), failure.code().unwrap());
            cases.insert(name, value);
        }
    }
    let failure = Failure::Indeterminate {
        code: Code::new("STC5"),
    };
    let diagnostic = SdkExecutionDiagnostic::from_failure(&failure, Default::default());
    assert_eq!(
        diagnostic.commit_outcome(),
        Some(SdkCommitFailureOutcome::Unknown)
    );
    cases.insert(
        format!("{}-STC5", diagnostic.code()),
        serde_json::to_value(diagnostic.host_json()).unwrap(),
    );
    cases
}
#[test]
fn typed_failure_diagnostic_goldens() {
    let cases = cases();
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/typed-failures.json");
    if std::env::var_os("TYPE_BRIDGE_UPDATE_FAILURE_FIXTURES").is_some() {
        std::fs::write(&path, serde_json::to_vec_pretty(&cases).unwrap()).unwrap();
    }
    let golden: BTreeMap<String, serde_json::Value> =
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(cases, golden);
}
#[test]
fn code_deserialization_cannot_admit_provider_values() {
    assert!(serde_json::from_str::<Code>("\"CNT9/value\"").is_err());
    assert!(serde_json::from_str::<Code>("\"CNT9\"").is_ok());
}
#[test]
fn released_details_omit_only_typed_failure_details() {
    let unique = Failure::Rejected {
        phase: Phase::Query,
        reason: Reason::Unique,
        op_index: None,
    };
    let conflict = Failure::Rejected {
        phase: Phase::Commit,
        reason: Reason::Conflict {
            lock: LockKind::Exclusive,
        },
        op_index: None,
    };
    let indeterminate = Failure::Indeterminate {
        code: Code::new("STC5"),
    };
    for (failure, released) in [
        (
            &unique,
            SdkExecutionDiagnostic::provider_failure(SdkProviderOperation::Write),
        ),
        (
            &conflict,
            SdkExecutionDiagnostic::commit_failure(SdkCommitFailureOutcome::DefinitelyAborted),
        ),
        (
            &indeterminate,
            SdkExecutionDiagnostic::commit_failure(SdkCommitFailureOutcome::Unknown),
        ),
    ] {
        let typed = SdkExecutionDiagnostic::from_failure(failure, Default::default());
        assert!(typed.details().len() > released.details().len());
        assert!(typed.released_details().eq(released.details().iter()));
        // Without a typed failure every detail is released.
        assert!(released.released_details().eq(released.details().iter()));
    }
}
