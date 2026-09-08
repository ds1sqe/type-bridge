use type_bridge_schema::SafetyClass;
use type_bridge_schema_migration::{MigrationSafetyPolicy, SafetyPolicyDecision};

#[test]
fn default_policy_gates_destructive_and_backfill_work_and_rejects_unsupported() {
    let policy = MigrationSafetyPolicy::default_policy();
    assert_eq!(
        policy.decision(SafetyClass::FormalOnly),
        SafetyPolicyDecision::Allow
    );
    assert_eq!(
        policy.decision(SafetyClass::SchemaMetadata),
        SafetyPolicyDecision::Allow
    );
    assert_eq!(
        policy.decision(SafetyClass::Additive),
        SafetyPolicyDecision::Allow
    );
    assert_eq!(
        policy.decision(SafetyClass::Conditional),
        SafetyPolicyDecision::Allow
    );
    assert_eq!(
        policy.decision(SafetyClass::Destructive),
        SafetyPolicyDecision::RequireApproval
    );
    assert_eq!(
        policy.decision(SafetyClass::Opaque),
        SafetyPolicyDecision::RequireApproval
    );
    assert_eq!(
        policy.decision(SafetyClass::BackfillRequired),
        SafetyPolicyDecision::RequireApproval
    );
    assert_eq!(
        policy.decision(SafetyClass::Unsupported),
        SafetyPolicyDecision::Reject
    );
}

#[test]
fn standing_allowance_for_destructive_opaque_or_backfill_work_is_invalid() {
    for class in [
        SafetyClass::Destructive,
        SafetyClass::Opaque,
        SafetyClass::BackfillRequired,
    ] {
        let error = MigrationSafetyPolicy::default_policy()
            .with_decision(class, SafetyPolicyDecision::Allow)
            .expect_err("a permanent force-style allowance is invalid");
        assert_eq!(error.code().as_str(), "migration_policy_forbidden_allow");
    }
}

#[test]
fn unresolvable_classes_cannot_be_admitted_by_policy() {
    for decision in [
        SafetyPolicyDecision::Allow,
        SafetyPolicyDecision::RequireApproval,
    ] {
        let error = MigrationSafetyPolicy::default_policy()
            .with_decision(SafetyClass::Unsupported, decision)
            .expect_err("unsupported work cannot be admitted");
        assert_eq!(error.code().as_str(), "migration_policy_unresolvable_class");
    }
}

#[test]
fn policy_tightening_is_recorded_per_class() {
    let policy = MigrationSafetyPolicy::default_policy()
        .with_decision(SafetyClass::Conditional, SafetyPolicyDecision::Reject)
        .expect("tightened policy")
        .with_decision(SafetyClass::Destructive, SafetyPolicyDecision::Reject)
        .expect("tightened policy");
    assert_eq!(
        policy.decision(SafetyClass::Conditional),
        SafetyPolicyDecision::Reject
    );
    assert_eq!(
        policy.decision(SafetyClass::Destructive),
        SafetyPolicyDecision::Reject
    );
    assert_eq!(
        policy.decision(SafetyClass::Additive),
        SafetyPolicyDecision::Allow
    );
}
