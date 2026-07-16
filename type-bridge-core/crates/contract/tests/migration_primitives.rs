use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::{FormatVersion, to_canonical_json};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::managed_scope::{
    ManagedScopeBinding, ManagedScopeId, SemanticProfileBinding,
};
use type_bridge_contract::migration::{
    CONDITIONAL_RESOLUTION_CAPABILITY, MIGRATION_FORMAT_V1, MigrationFormat,
    MigrationId, MigrationManifestDigest,
    MigrationPlanFingerprint, MigrationStep, MigrationStepId, RecoveryPolicy, RetryPolicy,
    SchemaDeltaFingerprint, SchemaDeltaStep,
};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, AssertionExpectation, AssertionPattern, BindingId,
    MigrationAssertionPlan, QueryVariable,
};
use type_bridge_contract::schema_delta::{
    ManagedFactSelection, ManagedSchemaState, PatchFormatVersion, SchemaDelta,
};
use type_bridge_contract::schema::{
    DeclaredIdentityFingerprint, DeclaredSchema, DocumentId, SchemaFact, SourceSpan,
    SourcedSchemaFact, TypeFact,
};
use type_bridge_contract::schema_fingerprint::{
    ManagedDeclaredIdentityFingerprint, ManagedSemanticSchemaFingerprint,
};
use type_bridge_contract::schema_lowering::{
    SchemaLoweringProfileBinding,
};

fn capability(value: &str) -> CapabilityId {
    CapabilityId::new(value).unwrap()
}

fn declared_identity() -> DeclaredIdentityFingerprint {
    let fact = SchemaFact::Type(
        TypeFact::new(TypeId::new(TypeKind::Entity, "state-marker").unwrap()).unwrap(),
    );
    DeclaredSchema::from_facts(
        FormatVersion::V1,
        CapabilitySet::new(),
        [SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("migration-primitives-state").unwrap(),
                0,
                1,
                1,
                1,
                1,
                2,
            )
            .unwrap(),
        )],
    )
    .unwrap()
    .declared_identity_fingerprint()
    .clone()
}

fn state(
    scope: ManagedScopeBinding,
    marker: &str,
    capability_id: &str,
) -> ManagedSchemaState {
    ManagedSchemaState::new(
        FormatVersion::V1,
        CapabilitySet::from_iter([capability(capability_id)]),
        scope,
        ManagedFactSelection::empty(),
        declared_identity(),
        ManagedDeclaredIdentityFingerprint::compute(
            format!("declared-{marker}").as_bytes(),
        )
        .unwrap(),
        ManagedSemanticSchemaFingerprint::compute(
            SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
            format!("semantic-{marker}").as_bytes(),
        )
        .unwrap(),
    )
    .unwrap()
}

fn capability_delta() -> SchemaDelta {
    let scope = ManagedScopeBinding::exclusive(
        ManagedScopeId::new("migration-primitives").unwrap(),
    )
    .unwrap();
    SchemaDelta::new(
        PatchFormatVersion::V1,
        state(scope.clone(), "source", "schema.source"),
        state(scope, "target", "schema.target"),
        Vec::new(),
    )
    .unwrap()
}

fn inverse(delta: &SchemaDelta) -> SchemaDelta {
    SchemaDelta::new(
        delta.format(),
        delta.target().clone(),
        delta.source().clone(),
        Vec::new(),
    )
    .unwrap()
}

#[test]
fn compound_id_and_ledger_key_have_exact_canonical_goldens() {
    let id = MigrationId::new("example", "0002_add_display_name").unwrap();
    assert_eq!(
        id.canonical_bytes().unwrap(),
        br#"{"app_label":"example","name":"0002_add_display_name"}"#,
    );
    assert_eq!(
        id.ledger_key()
            .unwrap()
            .as_fingerprint()
            .digest()
            .to_hex(),
        "03d2cd952e323a5d9b6a24ead08182132c7ca8deaaa3cd8168dfb2b5ff251551",
    );
    assert!(
        MigrationId::new("example", "0002_b").unwrap()
            < MigrationId::new("example", "0003_a").unwrap()
    );
    assert!(
        MigrationId::new("alpha", "9999_z").unwrap()
            < MigrationId::new("beta", "0001_a").unwrap()
    );
}

#[test]
fn identity_and_format_constructors_fail_closed() {
    for (app, name) in [
        ("", "0001_initial"),
        ("Example", "0001_initial"),
        ("example/app", "0001_initial"),
        ("example", ""),
        ("example", "../0001_initial"),
        ("example", "Initial"),
    ] {
        assert!(MigrationId::new(app, name).is_err());
    }
    assert!(MigrationStepId::new("define-display-name").is_ok());
    assert!(MigrationStepId::new("Define").is_err());
    assert_eq!(MigrationFormat::new(MIGRATION_FORMAT_V1).unwrap(), MigrationFormat::V1);
    assert_eq!(
        to_canonical_json(&MigrationFormat::V1).unwrap(),
        br#""typebridge.migration/v1""#,
    );
    assert_eq!(
        MigrationFormat::new("typebridge.migration/v2")
            .unwrap_err()
            .code()
            .as_str(),
        "unsupported_migration_format",
    );
}

#[test]
fn external_manifest_digest_is_raw_full_sha256() {
    let bytes = br#"{"format":"typebridge.migration/v1"}"#;
    let digest = MigrationManifestDigest::compute(bytes);
    assert_eq!(
        digest.to_hex(),
        "6972e8747ee242596db118e30e1369dee62d77a6180f6fb00e780164c28ab5d9",
    );
    assert_eq!(
        MigrationManifestDigest::from_hex(&digest.to_hex()).unwrap(),
        digest,
    );
    assert!(MigrationManifestDigest::from_hex("ABC").is_err());
}

#[test]
fn registry_owned_profile_bindings_are_exact_and_closed() {
    let semantic = SemanticProfileBinding::typedb_3_12_1().unwrap();
    assert_eq!(semantic.id().as_str(), "typedb-3.12.1/v1");
    assert_eq!(
        semantic
            .fingerprint()
            .as_fingerprint()
            .domain()
            .as_str(),
        "typebridge.schema.semantic-profile",
    );

    let lowering = SchemaLoweringProfileBinding::from_canonical_profile_bytes(
        br#"{"id":"typedb-3.12.1-schema-lowering/v1","rules":[]}"#,
    )
    .unwrap();
    assert_eq!(
        lowering.id().as_str(),
        "typedb-3.12.1-schema-lowering/v1",
    );
    assert!(SchemaLoweringProfileBinding::from_canonical_profile_bytes(
        br#"{"id":"typedb-3.12.0-schema-lowering/v1","rules":[]}"#,
    )
    .is_err());
    assert_eq!(
        lowering
            .fingerprint()
            .as_fingerprint()
            .domain()
            .as_str(),
        "typebridge.schema.lowering-profile",
    );

    let unknown = SemanticProfileId::new("typedb-9.9.9/v1").unwrap();
    assert!(SemanticProfileBinding::resolve(unknown).is_err());
}

#[test]
fn schema_step_derives_contract_and_checks_exact_inverse() {
    let delta = capability_delta();
    let reverse = inverse(&delta);
    let step = SchemaDeltaStep::new(
        MigrationStepId::new("capability-transition").unwrap(),
        delta.clone(),
        Some(reverse.clone()),
    )
    .unwrap();

    assert_eq!(step.delta(), &delta);
    assert_eq!(step.contract().retry(), RetryPolicy::Never);
    assert_eq!(
        step.contract().recovery(),
        RecoveryPolicy::OperatorRequired,
    );
    assert_eq!(step.contract().reverse(), Some(&reverse));
    assert_eq!(
        step.contract().required_capabilities(),
        delta.required_capabilities(),
    );
    assert_eq!(
        step.contract().source_semantics(),
        delta.source().managed_semantic_schema(),
    );
    assert_eq!(
        step.contract().target_semantics(),
        delta.target().managed_semantic_schema(),
    );
    assert_eq!(
        step.contract().delta_fingerprint(),
        &SchemaDeltaFingerprint::compute(&delta).unwrap(),
    );

    let contract = to_canonical_json(step.contract()).unwrap();
    let delta_bytes = delta.canonical_bytes().unwrap();
    let expected = format!(
        "{{\"contract\":{},\"delta\":{},\"kind\":\"schema_delta\"}}",
        String::from_utf8(contract).unwrap(),
        String::from_utf8(delta_bytes).unwrap(),
    )
    .into_bytes();
    assert_eq!(step.canonical_bytes().unwrap(), expected);

    let wrong_reverse = capability_delta();
    assert_eq!(
        SchemaDeltaStep::new(
            MigrationStepId::new("bad-reverse").unwrap(),
            delta,
            Some(wrong_reverse),
        )
        .unwrap_err()
        .code()
        .as_str(),
        "schema_delta_step_inverse_mismatch",
    );
}

#[test]
fn plan_fingerprint_is_golden_and_order_sensitive() {
    let empty = MigrationPlanFingerprint::compute(&[]).unwrap();
    assert_eq!(
        MigrationPlanFingerprint::canonical_plan_bytes(&[]).unwrap(),
        br#"{"steps":[]}"#,
    );
    assert_eq!(
        empty.as_fingerprint().digest().to_hex(),
        "b0b5986e5e35ff131b40bd99632ffb5e92c90728008c1c3b94d0af9a34d6462e",
    );

    let delta = capability_delta();
    let first = SchemaDeltaStep::new(
        MigrationStepId::new("first").unwrap(),
        delta.clone(),
        None,
    )
    .unwrap();
    let second = SchemaDeltaStep::new(
        MigrationStepId::new("second").unwrap(),
        delta,
        None,
    )
    .unwrap();
    let forward = MigrationPlanFingerprint::compute(&[
        MigrationStep::from(first.clone()),
        MigrationStep::from(second.clone()),
    ])
    .unwrap();
    let reverse = MigrationPlanFingerprint::compute(&[
        MigrationStep::from(second),
        MigrationStep::from(first),
    ])
    .unwrap();
    assert_ne!(
        forward.as_fingerprint().digest(),
        reverse.as_fingerprint().digest(),
    );
}

#[test]
fn assertion_step_derives_closed_contract_and_heterogeneous_plan_identity() {
    let delta = capability_delta();
    let binding = BindingId::new(0).unwrap();
    let plan = MigrationAssertionPlan::new(
        vec![AssertionBinding::new(
            binding,
            QueryVariable::new("instance").unwrap(),
        )],
        vec![AssertionPattern::Isa {
            binding,
            include_subtypes: false,
            type_id: TypeId::new(TypeKind::Entity, "state-marker").unwrap(),
        }],
        vec![binding],
        Vec::new(),
        delta.source().managed_semantic_schema().clone(),
        AssertionExpectation::NoRows,
    )
    .unwrap();
    let assertion = MigrationStep::assertion(
        MigrationStepId::new("assert-empty").unwrap(),
        plan.clone(),
        AssertionExpectation::NoRows,
    )
    .unwrap();
    let (contract, persisted, expected) = assertion.as_assertion().unwrap();
    assert_eq!(persisted, &plan);
    assert_eq!(expected, AssertionExpectation::NoRows);
    assert_eq!(contract.source_semantics(), contract.target_semantics());
    assert_eq!(contract.plan_fingerprint(), &plan.fingerprint().unwrap());
    assert_eq!(contract.retry(), RetryPolicy::Never);
    assert_eq!(contract.recovery(), RecoveryPolicy::OperatorRequired);
    assert_eq!(contract.reverse(), None);
    assert!(contract.required_capabilities().contains(
        &CapabilityId::new(CONDITIONAL_RESOLUTION_CAPABILITY).unwrap()
    ));
    let value: serde_json::Value =
        serde_json::from_slice(&assertion.canonical_bytes().unwrap()).unwrap();
    assert_eq!(value["kind"], "assertion");
    assert_eq!(value["expected"], "no_rows");

    let schema = MigrationStep::from(
        SchemaDeltaStep::new(MigrationStepId::new("schema").unwrap(), delta, None)
            .unwrap(),
    );
    assert_ne!(
        MigrationPlanFingerprint::compute(&[assertion.clone(), schema.clone()]).unwrap(),
        MigrationPlanFingerprint::compute(&[schema, assertion]).unwrap(),
    );
}
