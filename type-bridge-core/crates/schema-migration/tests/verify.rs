use std::collections::BTreeSet;

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration::{
    MigrationAppLabel, MigrationId, MigrationName, MigrationStepId,
    SchemaDeltaStep,
};
use type_bridge_contract::migration_assertion::migration_assertion_capability_vocabulary;
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, ManagedSchemaState, SchemaFact, SourceSpan,
    SourcedSchemaFact, TypeFact,
};
use type_bridge_schema::{
    BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, diff_managed,
    inverse_delta, managed_schema_state,
};
use type_bridge_schema_migration::{
    MigrationDriftFinding, MigrationHistoryGraph, SchemaMigrationDraft,
    VerifiedSchemaMigrationManifest, build_verified_manifest,
    typedb_3_12_1_profile, verify_migration_state,
};

fn migration_id(name: &str) -> MigrationId {
    MigrationId::from_components(
        MigrationAppLabel::new("example").expect("fixture app label"),
        MigrationName::new(name).expect("fixture migration name"),
    )
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
                    DocumentId::new("verify-fixture").expect("fixture document"),
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

fn manifest(
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
    let draft = SchemaMigrationDraft::new(migration_id(name), parents, vec![step])
        .expect("fixture draft");
    build_verified_manifest(draft, (source, context)).expect("fixture manifest")
}

struct Triad {
    context: ManagedDeltaContext,
    genesis: DeclaredSchema,
    middle: DeclaredSchema,
    top: DeclaredSchema,
    graph: MigrationHistoryGraph,
    first: VerifiedSchemaMigrationManifest,
    second: VerifiedSchemaMigrationManifest,
}

fn triad() -> Triad {
    let genesis = declared(&[]);
    let middle = declared(&["person"]);
    let top = declared(&["company", "person"]);
    let context = context();
    let first = manifest("0001_person", Vec::new(), &genesis, &middle, &context);
    let second = manifest(
        "0002_company",
        vec![first.id().clone()],
        &middle,
        &top,
        &context,
    );
    let graph = MigrationHistoryGraph::from_verified([first.clone(), second.clone()])
        .expect("history");
    Triad {
        context,
        genesis,
        middle,
        top,
        graph,
        first,
        second,
    }
}

fn state(schema: &DeclaredSchema, context: &ManagedDeltaContext) -> ManagedSchemaState {
    managed_schema_state(schema, context).expect("fixture managed state")
}

#[test]
fn a_coherent_triad_verifies_clean() {
    let triad = triad();
    let applied =
        BTreeSet::from([triad.first.id().clone(), triad.second.id().clone()]);
    let live = state(&triad.top, &triad.context);
    let report = verify_migration_state(
        &triad.graph,
        &applied,
        &triad.genesis,
        Some(&triad.top),
        Some(&live),
        &triad.context,
    )
    .expect("verification report");
    assert!(report.is_clean(), "findings: {:?}", report.findings());
    assert_eq!(report.applied_frontier(), &[triad.second.id().clone()]);
    assert_eq!(
        report.frontier_semantics(),
        Some(triad.second.target_state().managed_semantic_schema()),
    );
    assert_eq!(
        report.observed_semantics(),
        Some(live.managed_semantic_schema()),
    );
}

#[test]
fn each_drift_category_is_reported_without_repair() {
    let triad = triad();

    // A non-downward-closed ledger is invalid before any comparison.
    let orphaned = BTreeSet::from([triad.second.id().clone()]);
    let report = verify_migration_state(
        &triad.graph,
        &orphaned,
        &triad.genesis,
        None,
        None,
        &triad.context,
    )
    .expect("ledger report");
    assert!(matches!(
        report.findings(),
        [
            MigrationDriftFinding::AppliedLedger { .. },
            MigrationDriftFinding::PendingMigrations { .. },
        ] | [MigrationDriftFinding::AppliedLedger { .. }]
    ));

    // Live semantics behind the recorded frontier target is drift.
    let applied =
        BTreeSet::from([triad.first.id().clone(), triad.second.id().clone()]);
    let stale_live = state(&triad.middle, &triad.context);
    let report = verify_migration_state(
        &triad.graph,
        &applied,
        &triad.genesis,
        None,
        Some(&stale_live),
        &triad.context,
    )
    .expect("live drift report");
    let [MigrationDriftFinding::LiveSemantics { recorded, observed }] =
        report.findings()
    else {
        panic!("expected exactly one live-semantics finding: {report:?}");
    };
    assert_eq!(recorded, triad.second.target_state().managed_semantic_schema());
    assert_eq!(observed, stale_live.managed_semantic_schema());

    // Desired schema ahead of the committed head is divergence, not intent.
    let desired = declared(&["company", "person", "team"]);
    let report = verify_migration_state(
        &triad.graph,
        &applied,
        &triad.genesis,
        Some(&desired),
        None,
        &triad.context,
    )
    .expect("desired divergence report");
    let [MigrationDriftFinding::DesiredDivergence { head, desired: declared }] =
        report.findings()
    else {
        panic!("expected exactly one desired-divergence finding: {report:?}");
    };
    assert_eq!(head, triad.second.target_state().managed_semantic_schema());
    assert_eq!(
        declared,
        state(&desired, &triad.context).managed_semantic_schema(),
    );

    // Verified history beyond the frontier is pending work.
    let partial = BTreeSet::from([triad.first.id().clone()]);
    let report = verify_migration_state(
        &triad.graph,
        &partial,
        &triad.genesis,
        None,
        None,
        &triad.context,
    )
    .expect("pending report");
    let [MigrationDriftFinding::PendingMigrations { pending }] = report.findings()
    else {
        panic!("expected exactly one pending finding: {report:?}");
    };
    assert_eq!(pending, &[triad.second.id().clone()]);
}

#[test]
fn an_empty_lineage_verifies_against_its_genesis() {
    let context = context();
    let genesis = declared(&[]);
    let graph =
        MigrationHistoryGraph::from_verified([]).expect("empty history");
    let live = state(&genesis, &context);
    let report = verify_migration_state(
        &graph,
        &BTreeSet::new(),
        &genesis,
        Some(&genesis),
        Some(&live),
        &context,
    )
    .expect("empty-lineage report");
    assert!(report.is_clean(), "findings: {:?}", report.findings());

    let desired = declared(&["person"]);
    let report = verify_migration_state(
        &graph,
        &BTreeSet::new(),
        &genesis,
        Some(&desired),
        Some(&live),
        &context,
    )
    .expect("ungenerated-desired report");
    assert!(matches!(
        report.findings(),
        [MigrationDriftFinding::DesiredDivergence { .. }]
    ));
}
