mod common;

use std::collections::BTreeSet;
use std::sync::Mutex;

use common::{CoordinatorProvider, CoordinatorStore, block_on};
use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::{FormatVersion, to_canonical_json};
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::limits::{MAX_CANONICAL_COLLECTION_LEN, MAX_CANONICAL_STRING_BYTES};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration::{
    MigrationAppLabel, MigrationId, MigrationName, MigrationStep, MigrationStepId, SchemaDeltaStep,
};
use type_bridge_contract::migration_assertion::migration_assertion_capability_vocabulary;
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact,
};
use type_bridge_schema::{
    BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SafetyClass, diff_managed, inverse_delta,
};
use type_bridge_schema_migration::{
    LeaseHolderId, LegacyAppliedSetDigest, LegacyMigrationChecksum, LegacyMigrationId,
    LegacyMigrationReference, MigrationApplyTarget, MigrationExecutionOutcome,
    MigrationHistoryGraph, MigrationSafetyPolicy, SchemaLoweringBinding, SchemaMigrationDraft,
    VerifiedSchemaMigrationManifest, build_legacy_frontier_bridge, build_verified_manifest,
    build_verified_migration_apply_plan, decode_verified_manifest, encode_verified_manifest,
    execute_verified_migration_apply_plan, typedb_3_12_1_profile, verified_manifest_digest,
};

fn migration_id(name: &str) -> MigrationId {
    MigrationId::from_components(
        MigrationAppLabel::new("example").expect("fixture app label"),
        MigrationName::new(name).expect("fixture migration name"),
    )
}

fn legacy_reference(name: &str, checksum: &str) -> LegacyMigrationReference {
    LegacyMigrationReference::new(
        migration_id(name),
        LegacyMigrationChecksum::new(checksum).expect("fixture legacy checksum"),
    )
}

fn legacy_applied_set(references: &[LegacyMigrationReference]) -> LegacyAppliedSetDigest {
    LegacyAppliedSetDigest::compute(references.to_vec()).expect("fixture legacy applied set")
}

fn build_bridge(
    id: MigrationId,
    references: Vec<LegacyMigrationReference>,
    head: &DeclaredSchema,
    context: &ManagedDeltaContext,
) -> Result<VerifiedSchemaMigrationManifest, type_bridge_contract::diagnostic::Diagnostic> {
    let applied_set = legacy_applied_set(&references);
    build_legacy_frontier_bridge(id, references, applied_set, head, context)
}

fn type_fact(label: &str) -> SchemaFact {
    SchemaFact::Type(
        TypeFact::new(TypeId::new(TypeKind::Entity, label).expect("fixture type"))
            .expect("fixture type fact"),
    )
}

fn declared(labels: &[&str]) -> DeclaredSchema {
    let sourced = labels
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let offset = u64::try_from(index).expect("fixture offset");
            let line = u32::try_from(index + 1).expect("fixture line");
            SourcedSchemaFact::new(
                type_fact(label),
                SourceSpan::new(
                    DocumentId::new("legacy-bridge-fixture").expect("fixture document"),
                    offset,
                    offset + 1,
                    line,
                    1,
                    line,
                    2,
                )
                .expect("fixture span"),
            )
        })
        .collect::<Vec<_>>();
    DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
        .expect("fixture schema")
}

fn capabilities() -> CapabilitySet {
    let mut capabilities = typedb_3_12_1_profile().required_capabilities.clone();
    for capability in BUILTIN_SCHEMA_CAPABILITY_IDS {
        capabilities.insert(CapabilityId::new(*capability).expect("builtin capability"));
    }
    for capability in migration_assertion_capability_vocabulary().iter().cloned() {
        capabilities.insert(capability);
    }
    capabilities.insert(
        CapabilityId::new("migration.conditional-resolution")
            .expect("conditional-resolution capability"),
    );
    capabilities
}

fn context() -> ManagedDeltaContext {
    ManagedDeltaContext::new(
        ManagedScopeId::new("example-schema").expect("fixture scope"),
        SemanticProfileId::new("typedb-3.12.1/v1").expect("fixture profile"),
        capabilities(),
    )
}

fn ordinary_manifest(
    name: &str,
    parents: Vec<MigrationId>,
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    context: &ManagedDeltaContext,
) -> VerifiedSchemaMigrationManifest {
    let delta = diff_managed(source, target, context).expect("fixture delta");
    let reverse = inverse_delta(&delta).expect("fixture inverse");
    let step = SchemaDeltaStep::new(
        MigrationStepId::new(format!("step-{name}")).expect("fixture step id"),
        delta,
        Some(reverse),
    )
    .expect("fixture step");
    let draft =
        SchemaMigrationDraft::new(migration_id(name), parents, vec![step]).expect("fixture draft");
    build_verified_manifest(draft, (source, context)).expect("fixture manifest")
}

#[test]
fn bridge_builds_round_trips_and_binds_the_frontier() {
    let head = declared(&["person"]);
    let context = context();
    let bridge = build_bridge(
        migration_id("0000_legacy_frontier"),
        vec![
            legacy_reference("0002_addresses", "00c0ffee00c0ffee"),
            legacy_reference("0001_initial", "0123456789abcdef"),
        ],
        &head,
        &context,
    )
    .expect("legacy frontier bridge");

    assert!(bridge.is_legacy_bridge());
    assert!(bridge.steps().is_empty());
    assert!(bridge.parents().is_empty());
    assert_eq!(bridge.safety(), SafetyClass::FormalOnly);
    assert_eq!(bridge.source_state(), bridge.target_state());
    assert_eq!(
        bridge.source_schema().declared_identity_fingerprint(),
        head.declared_identity_fingerprint(),
    );
    // References canonicalize set-like by identity.
    assert_eq!(
        bridge
            .legacy_parents()
            .iter()
            .map(|reference| reference.id().name().as_str())
            .collect::<Vec<_>>(),
        vec!["0001_initial", "0002_addresses"],
    );

    let bytes = encode_verified_manifest(&bridge).expect("bridge encoding");
    let encoded: serde_json::Value = serde_json::from_slice(&bytes).expect("bridge JSON");
    assert_eq!(
        encoded["legacy_parents"][0],
        serde_json::json!({
            "app_label": "example",
            "checksum": {
                "algorithm": "python-source-sha256/16",
                "value": "0123456789abcdef",
            },
            "name": "0001_initial",
        }),
        "portable legacy references retain the original wire shape",
    );
    assert_eq!(
        encoded["legacy_applied_set"],
        serde_json::json!({
            "algorithm": "sha256",
            "canonicalization": "typebridge.legacy-applied-set/v1",
            "digest": bridge.legacy_applied_set().expect("applied set").as_str(),
        }),
        "the complete applied-set binding is mandatory and self-describing",
    );
    let decoded = decode_verified_manifest(&bytes, (&head, &context)).expect("bridge decoding");
    assert_eq!(decoded, bridge);

    let mut missing_applied_set = encoded.clone();
    missing_applied_set
        .as_object_mut()
        .expect("manifest object")
        .remove("legacy_applied_set");
    let error = decode_verified_manifest(
        &to_canonical_json(&missing_applied_set).expect("tampered canonical encoding"),
        (&head, &context),
    )
    .expect_err("bridge wire without its complete applied set must reject");
    assert_eq!(
        error.code().as_str(),
        "migration_manifest_legacy_applied_set_missing"
    );

    let mut foreign_vocabulary = encoded.clone();
    foreign_vocabulary["legacy_applied_set"]["canonicalization"] =
        serde_json::json!("typebridge.legacy-applied-set/v999");
    let error = decode_verified_manifest(
        &to_canonical_json(&foreign_vocabulary).expect("tampered canonical encoding"),
        (&head, &context),
    )
    .expect_err("foreign applied-set vocabulary must reject");
    assert_eq!(
        error.code().as_str(),
        "migration_manifest_legacy_applied_set_contract"
    );

    // The recorded frontier is digest-bound: a different tagged checksum is
    // a different bridge identity.
    let tampered = build_bridge(
        migration_id("0000_legacy_frontier"),
        vec![
            legacy_reference("0002_addresses", "00c0ffee00c0ffee"),
            legacy_reference("0001_initial", "fedcba9876543210"),
        ],
        &head,
        &context,
    )
    .expect("tampered bridge");
    assert_ne!(
        verified_manifest_digest(&bridge).expect("bridge digest"),
        verified_manifest_digest(&tampered).expect("tampered digest"),
    );
}

#[test]
fn bridge_round_trips_nonportable_released_frontier_identity_losslessly() {
    let head = declared(&["person"]);
    let context = context();
    let legacy_id = LegacyMigrationId::new("Legacy App: Ω", "0002_A:B with space")
        .expect("released UTF-8 identity is bounded but need not use V2 grammar");
    let bridge = build_bridge(
        migration_id("0000_legacy_frontier"),
        vec![LegacyMigrationReference::new(
            legacy_id,
            LegacyMigrationChecksum::new("0123456789abcdef").expect("legacy checksum"),
        )],
        &head,
        &context,
    )
    .expect("nonportable frontier bridge");

    let bytes = encode_verified_manifest(&bridge).expect("bridge encodes");
    let encoded: serde_json::Value = serde_json::from_slice(&bytes).expect("bridge JSON");
    assert_eq!(encoded["legacy_parents"][0]["app_label"], "Legacy App: Ω");
    assert_eq!(encoded["legacy_parents"][0]["name"], "0002_A:B with space");
    let decoded = decode_verified_manifest(&bytes, (&head, &context)).expect("bridge decodes");
    assert_eq!(
        decoded.legacy_parents()[0].id().app_label().as_str(),
        "Legacy App: Ω"
    );
    assert_eq!(
        decoded.legacy_parents()[0].id().name().as_str(),
        "0002_A:B with space"
    );

    assert!(LegacyMigrationId::new("", "0001_initial").is_err());
    assert!(LegacyMigrationId::new("example", "").is_err());
    assert!(
        LegacyMigrationId::new("x".repeat(MAX_CANONICAL_STRING_BYTES + 1), "0001_initial").is_err()
    );
}

#[test]
fn complete_applied_set_digest_is_unambiguous_order_independent_and_bounded() {
    let first = LegacyMigrationReference::new(
        LegacyMigrationId::new("a", "bc").expect("first identity"),
        LegacyMigrationChecksum::new("0123456789abcdef").expect("checksum"),
    );
    let second = LegacyMigrationReference::new(
        LegacyMigrationId::new("ab", "c").expect("second identity"),
        LegacyMigrationChecksum::new("0123456789abcdef").expect("checksum"),
    );
    let unicode = LegacyMigrationReference::new(
        LegacyMigrationId::new("应用 Ω", "迁移 🚀").expect("UTF-8 identity"),
        LegacyMigrationChecksum::new("fedcba9876543210").expect("checksum"),
    );

    let ordered = LegacyAppliedSetDigest::compute([first.clone(), second.clone(), unicode.clone()])
        .expect("ordered digest");
    let reordered = LegacyAppliedSetDigest::compute([unicode, second.clone(), first.clone()])
        .expect("reordered digest");
    assert_eq!(ordered, reordered, "row order is not semantic");
    assert_ne!(
        LegacyAppliedSetDigest::compute([first]).expect("first digest"),
        LegacyAppliedSetDigest::compute([second]).expect("second digest"),
        "length prefixes must distinguish component boundaries",
    );

    let too_many = vec![
        legacy_reference("0001_initial", "0123456789abcdef");
        MAX_CANONICAL_COLLECTION_LEN + 1
    ];
    let error = LegacyAppliedSetDigest::compute(too_many)
        .expect_err("the digest collector must reject an oversized set before sorting");
    assert_eq!(
        error.code().as_str(),
        "migration_legacy_applied_set_too_many_rows"
    );
}

#[test]
fn bridge_constructor_bounds_the_independent_frontier_vector() {
    let parents = vec![
        legacy_reference("0001_initial", "0123456789abcdef");
        MAX_CANONICAL_COLLECTION_LEN + 1
    ];
    let applied_set = legacy_applied_set(&[legacy_reference("0001_initial", "0123456789abcdef")]);
    let error = SchemaMigrationDraft::legacy_bridge(
        migration_id("0000_legacy_frontier"),
        parents,
        applied_set,
    )
    .expect_err("the independently supplied frontier must be bounded before sorting");
    assert_eq!(
        error.code().as_str(),
        "migration_manifest_legacy_frontier_too_large"
    );
}

#[test]
fn bridge_invariants_fail_closed() {
    let head = declared(&["person"]);
    let context = context();

    let fixture_set = legacy_applied_set(&[legacy_reference("0001_initial", "0123456789abcdef")]);
    let empty = SchemaMigrationDraft::legacy_bridge(
        migration_id("0000_legacy_frontier"),
        Vec::new(),
        fixture_set.clone(),
    )
    .expect_err("an empty legacy frontier is not a bridge");
    assert_eq!(
        empty.code().as_str(),
        "migration_manifest_empty_legacy_frontier"
    );

    let duplicated = SchemaMigrationDraft::legacy_bridge(
        migration_id("0000_legacy_frontier"),
        vec![
            legacy_reference("0001_initial", "0123456789abcdef"),
            legacy_reference("0001_initial", "fedcba9876543210"),
        ],
        fixture_set,
    )
    .expect_err("a legacy identity may enter the frontier once");
    assert_eq!(
        duplicated.code().as_str(),
        "migration_manifest_duplicate_legacy_parent"
    );

    let empty_program = build_verified_manifest(
        SchemaMigrationDraft::new(
            migration_id("0001_noop"),
            Vec::new(),
            Vec::<MigrationStep>::new(),
        )
        .expect("empty draft"),
        (&head, &context),
    )
    .expect_err("an ordinary manifest requires at least one step");
    assert_eq!(
        empty_program.code().as_str(),
        "migration_manifest_empty_program"
    );

    for invalid in ["0123456789ABCDEF", "0123", "0123456789abcdef00"] {
        assert!(
            LegacyMigrationChecksum::new(invalid).is_err(),
            "checksum {invalid:?} must be rejected"
        );
    }
}

#[test]
fn bridged_lineage_admits_no_root_beside_the_bridge() {
    let head = declared(&["person"]);
    let target = declared(&["company", "person"]);
    let context = context();
    let bridge = build_bridge(
        migration_id("0000_legacy_frontier"),
        vec![legacy_reference("0001_initial", "0123456789abcdef")],
        &head,
        &context,
    )
    .expect("legacy frontier bridge");
    let child = ordinary_manifest(
        "0001_company",
        vec![bridge.id().clone()],
        &head,
        &target,
        &context,
    );

    let graph = MigrationHistoryGraph::from_verified([bridge.clone(), child.clone()])
        .expect("bridged lineage");
    assert_eq!(graph.default_head().expect("head"), Some(child.id()));

    let orphan = ordinary_manifest("0001_orphan", Vec::new(), &head, &target, &context);
    let beside = MigrationHistoryGraph::from_verified([bridge.clone(), orphan])
        .expect_err("a second root beside the bridge splits the frontier");
    assert_eq!(
        beside.code().as_str(),
        "migration_history_root_beside_legacy_bridge"
    );

    let second_bridge = build_bridge(
        migration_id("0000_second_frontier"),
        vec![legacy_reference("0009_other", "fedcba9876543210")],
        &head,
        &context,
    )
    .expect("second bridge");
    let doubled = MigrationHistoryGraph::from_verified([bridge, second_bridge])
        .expect_err("one lineage carries one bridge");
    assert_eq!(
        doubled.code().as_str(),
        "migration_history_multiple_legacy_bridges"
    );
}

#[test]
fn bridge_applies_as_a_pure_ledger_checkpoint() {
    let head = declared(&["person"]);
    let target = declared(&["company", "person"]);
    let context = context();
    let bridge = build_bridge(
        migration_id("0000_legacy_frontier"),
        vec![legacy_reference("0001_initial", "0123456789abcdef")],
        &head,
        &context,
    )
    .expect("legacy frontier bridge");
    let child = ordinary_manifest(
        "0001_company",
        vec![bridge.id().clone()],
        &head,
        &target,
        &context,
    );
    let graph = MigrationHistoryGraph::from_verified([bridge.clone(), child.clone()])
        .expect("bridged lineage");
    let lowering = SchemaLoweringBinding::current(context.available_capabilities().clone())
        .expect("lowering binding");
    let plan = build_verified_migration_apply_plan(
        &graph,
        &BTreeSet::new(),
        &MigrationApplyTarget::DefaultHead,
        &context,
        &lowering,
        &MigrationSafetyPolicy::default_policy(),
        &[],
    )
    .expect("bridged apply plan");
    assert_eq!(plan.migrations().len(), 2);
    assert!(
        plan.migrations()[0].transaction_groups().is_empty(),
        "the bridge executes nothing"
    );

    let store = CoordinatorStore::default();
    let provider = CoordinatorProvider {
        available: context.available_capabilities().clone(),
        calls: Mutex::new(Vec::new()),
        observed: Mutex::new(plan.source_state().expect("source state").clone()),
    };
    let outcome = block_on(execute_verified_migration_apply_plan(
        &store,
        &provider,
        &LeaseHolderId::new("legacy-import").expect("holder"),
        &plan,
    ))
    .expect("bridged apply execution");
    assert!(matches!(outcome, MigrationExecutionOutcome::Applied));
    let calls = provider.calls.lock().expect("provider calls").clone();
    assert_eq!(
        calls.iter().filter(|call| **call == "prepare").count(),
        1,
        "only the ordinary child opens a provider transaction: {calls:?}"
    );
    let state = store.state.lock().expect("coordinator store");
    assert_eq!(state.applied.len(), 2);
    assert_eq!(state.applied[0].record().migration_id(), bridge.id());
    assert_eq!(state.applied[1].record().migration_id(), child.id());
}
