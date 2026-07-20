use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use type_bridge_contract::capability::CapabilityId;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration::{
    MigrationAppLabel, MigrationId, MigrationName, MigrationStepId, SchemaDeltaStep,
};
use type_bridge_contract::migration_assertion::migration_assertion_capability_vocabulary;
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, DeclaredSchema,
    DocumentId, SchemaAnnotationValue, SchemaFact, SourceSpan, SourcedSchemaFact, SubFact,
    SubFactId, TypeFact,
};
use type_bridge_schema::{
    BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SafetyClass, diff_managed, inverse_delta,
};
use type_bridge_schema_migration::{
    MigrationGenerationOutcome, MigrationGenerationRequest, MigrationHistoryGraph,
    SchemaMigrationDraft, VerifiedSchemaMigrationManifest, build_verified_manifest,
    discover_verified_migration_chain, generate_next_migration, render_migration_preview,
    typedb_3_12_1_profile, write_generated_migration,
};

const APP_LABEL: &str = "example";

fn migration_id(name: &str) -> MigrationId {
    MigrationId::from_components(
        MigrationAppLabel::new(APP_LABEL).expect("fixture app label"),
        MigrationName::new(name).expect("fixture migration name"),
    )
}

fn entity_id(label: &str) -> TypeId {
    TypeId::new(TypeKind::Entity, label).expect("fixture type id")
}

fn type_fact(label: &str) -> SchemaFact {
    SchemaFact::Type(TypeFact::new(entity_id(label)).expect("fixture type fact"))
}

fn sub_fact(child: &str, parent: &str) -> SchemaFact {
    SchemaFact::Sub(SubFact::new(
        SubFactId::new(entity_id(child), entity_id(parent)).expect("fixture sub identity"),
    ))
}

fn abstract_fact(label: &str) -> SchemaFact {
    SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Type(entity_id(label)),
                AnnotationKindId::Abstract,
            ),
            SchemaAnnotationValue::Presence,
        )
        .expect("fixture abstract annotation"),
    )
}

fn declared_facts(facts: Vec<SchemaFact>) -> DeclaredSchema {
    let sourced = facts
        .into_iter()
        .enumerate()
        .map(|(index, fact)| {
            let offset = u64::try_from(index).expect("fixture offset");
            let line = u32::try_from(index + 1).expect("fixture line");
            SourcedSchemaFact::new(
                fact,
                SourceSpan::new(
                    DocumentId::new("generate-fixture").expect("fixture document"),
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

fn declared(labels: &[&str]) -> DeclaredSchema {
    declared_facts(labels.iter().map(|label| type_fact(label)).collect())
}

fn generation_capabilities() -> CapabilitySet {
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
        generation_capabilities(),
    )
}

fn committed_manifest(
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

fn empty_graph() -> MigrationHistoryGraph {
    MigrationHistoryGraph::from_verified(Vec::new()).expect("empty graph")
}

fn request<'a>(
    base_name: &'a str,
    genesis_source: &'a DeclaredSchema,
    desired: &'a DeclaredSchema,
    context: &'a ManagedDeltaContext,
) -> MigrationGenerationRequest<'a> {
    MigrationGenerationRequest {
        app_label: APP_LABEL,
        base_name,
        genesis_source,
        desired,
        context,
    }
}

fn generated(
    graph: &MigrationHistoryGraph,
    request: &MigrationGenerationRequest<'_>,
) -> type_bridge_schema_migration::GeneratedMigration {
    match generate_next_migration(graph, request).expect("generation succeeds") {
        MigrationGenerationOutcome::Generated(generated) => *generated,
        MigrationGenerationOutcome::UpToDate => panic!("expected a generated migration"),
    }
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "typebridge-generate-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create fixture directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn genesis_generation_is_deterministic_and_round_trips_through_discovery() {
    let genesis = declared_facts(Vec::new());
    let desired = declared(&["person"]);
    let context = context();
    let graph = empty_graph();
    let request = request("init", &genesis, &desired, &context);

    let first = generated(&graph, &request);
    let second = generated(&graph, &request);
    assert_eq!(first.canonical_bytes(), second.canonical_bytes());
    assert_eq!(first.manifest().id(), &migration_id("0001_init"));
    assert!(first.manifest().parents().is_empty());
    assert_eq!(first.manifest().safety(), SafetyClass::Additive);
    assert_eq!(first.file_name(), "0001_init.tbmigration.json");
    assert_eq!(first.preview_file_name(), "0001_init.typeql");

    let preview = render_migration_preview(first.manifest(), &context).expect("preview renders");
    assert!(preview.contains("define"), "preview: {preview}");
    assert!(preview.contains("entity person"), "preview: {preview}");

    let directory = TempDirectory::new();
    write_generated_migration(directory.path(), &first, &preview)
        .expect("write generated migration");
    let discovered = discover_verified_migration_chain(directory.path(), &genesis, &context)
        .expect("discover generated migration");
    assert_eq!(discovered.len(), 1);

    // The committed head now equals the desired schema.
    let again = generate_next_migration(&discovered, &request).expect("regenerate");
    assert!(matches!(again, MigrationGenerationOutcome::UpToDate));
}

#[test]
fn incremental_generation_chains_from_the_sole_head() {
    let genesis = declared_facts(Vec::new());
    let first_target = declared(&["person"]);
    let context = context();
    let committed =
        committed_manifest("0001_person", Vec::new(), &genesis, &first_target, &context);
    let head_id = committed.id().clone();
    let graph = MigrationHistoryGraph::from_verified(vec![committed]).expect("graph");

    // The #190 shape: a new subtype under an already-committed parent.
    let desired = declared_facts(vec![
        type_fact("employee"),
        type_fact("person"),
        sub_fact("employee", "person"),
    ]);
    let request = request("add_employee", &genesis, &desired, &context);
    let next = generated(&graph, &request);

    assert_eq!(next.manifest().id(), &migration_id("0002_add_employee"));
    assert_eq!(next.manifest().parents(), &[head_id]);
    assert_eq!(
        next.manifest()
            .source_schema()
            .declared_identity_fingerprint(),
        first_target.declared_identity_fingerprint(),
    );
    // The proven condition-free sub edge needs no assertion steps.
    assert_eq!(next.manifest().steps().len(), 1);
    assert_eq!(next.manifest().safety(), SafetyClass::Conditional);
}

#[test]
fn conditional_operations_receive_derived_assertion_steps() {
    let genesis = declared_facts(Vec::new());
    let head_target = declared(&["person"]);
    let context = context();
    let committed = committed_manifest("0001_person", Vec::new(), &genesis, &head_target, &context);
    let graph = MigrationHistoryGraph::from_verified(vec![committed]).expect("graph");

    let desired = declared_facts(vec![type_fact("person"), abstract_fact("person")]);
    let request = request("person_abstract", &genesis, &desired, &context);
    let next = generated(&graph, &request);

    assert_eq!(next.manifest().safety(), SafetyClass::Conditional);
    let steps = next.manifest().steps();
    assert_eq!(steps.len(), 2, "expected one assertion and one delta step");
    assert!(steps[0].as_assertion().is_some());
    assert!(steps[1].as_schema_delta().is_some());
}

#[test]
fn reverse_requiring_assertions_downgrades_to_an_irreversible_manifest() {
    let genesis = declared_facts(Vec::new());
    let first_target = declared(&["person"]);
    let head_target = declared_facts(vec![type_fact("person"), abstract_fact("person")]);
    let context = context();
    let first = committed_manifest("0001_person", Vec::new(), &genesis, &first_target, &context);
    let second = generated(
        &MigrationHistoryGraph::from_verified(vec![first.clone()]).expect("first graph"),
        &request("person_abstract", &genesis, &head_target, &context),
    );
    let graph = MigrationHistoryGraph::from_verified(vec![first, second.manifest().clone()])
        .expect("graph");

    // Removing the abstract annotation is forward-safe, but its structural
    // inverse (re-adding @abstract) would require assertions, so the verifier
    // refuses to record it as a real rollback program.
    let desired = declared(&["person"]);
    let request = request("person_concrete", &genesis, &desired, &context);
    let next = generated(&graph, &request);
    assert!(!next.manifest().reversible());
}

#[test]
fn ambiguous_heads_are_refused() {
    let genesis = declared_facts(Vec::new());
    let context = context();
    let left = committed_manifest(
        "0001_left",
        Vec::new(),
        &genesis,
        &declared(&["left"]),
        &context,
    );
    let right = committed_manifest(
        "0002_right",
        Vec::new(),
        &genesis,
        &declared(&["right"]),
        &context,
    );
    let graph = MigrationHistoryGraph::from_verified(vec![left, right]).expect("graph");

    let desired = declared(&["left", "merged"]);
    let request = request("merged", &genesis, &desired, &context);
    let error = generate_next_migration(&graph, &request).expect_err("ambiguous heads");
    assert_eq!(
        error.code().as_str(),
        "migration_history_ambiguous_default_head"
    );
}

#[test]
fn foreign_app_label_fails_closed() {
    let genesis = declared_facts(Vec::new());
    let context = context();
    let committed = committed_manifest(
        "0001_person",
        Vec::new(),
        &genesis,
        &declared(&["person"]),
        &context,
    );
    let graph = MigrationHistoryGraph::from_verified(vec![committed]).expect("graph");

    let desired = declared(&["person", "company"]);
    let request = MigrationGenerationRequest {
        app_label: "other",
        base_name: "company",
        genesis_source: &genesis,
        desired: &desired,
        context: &context,
    };
    let error = generate_next_migration(&graph, &request).expect_err("foreign lineage");
    assert_eq!(
        error.code().as_str(),
        "migration_generation_foreign_app_label"
    );
}

#[test]
fn ordinal_allocation_continues_past_gaps_and_ignores_non_numeric_names() {
    let genesis = declared_facts(Vec::new());
    let first_target = declared(&["person"]);
    let second_target = declared(&["person", "company"]);
    let context = context();
    let first = committed_manifest("0001_person", Vec::new(), &genesis, &first_target, &context);
    let second = committed_manifest(
        "0007_company",
        vec![first.id().clone()],
        &first_target,
        &second_target,
        &context,
    );
    let graph = MigrationHistoryGraph::from_verified(vec![first, second]).expect("graph");

    let desired = declared(&["person", "company", "office"]);
    let request = request("office", &genesis, &desired, &context);
    let next = generated(&graph, &request);
    assert_eq!(next.manifest().id(), &migration_id("0008_office"));
}

#[test]
fn write_refuses_existing_generated_files() {
    let genesis = declared_facts(Vec::new());
    let desired = declared(&["person"]);
    let context = context();
    let graph = empty_graph();
    let request = request("init", &genesis, &desired, &context);
    let generated = generated(&graph, &request);
    let preview = render_migration_preview(generated.manifest(), &context).expect("preview");

    let directory = TempDirectory::new();
    write_generated_migration(directory.path(), &generated, &preview)
        .expect("first write succeeds");
    let error = write_generated_migration(directory.path(), &generated, &preview)
        .expect_err("second write conflicts");
    assert_eq!(error.code().as_str(), "migration_generation_write_conflict");
}

#[test]
fn destructive_generation_is_honest_and_previews_without_approval() {
    let genesis = declared_facts(Vec::new());
    let head_target = declared(&["person", "company"]);
    let context = context();
    let committed = committed_manifest(
        "0001_person_company",
        Vec::new(),
        &genesis,
        &head_target,
        &context,
    );
    let graph = MigrationHistoryGraph::from_verified(vec![committed]).expect("graph");

    let desired = declared(&["person"]);
    let request = request("drop_company", &genesis, &desired, &context);
    let next = generated(&graph, &request);

    assert_eq!(next.manifest().safety(), SafetyClass::Destructive);
    // Destructive guard conditions never become assertions: the approved
    // intent is data loss, not refuse-if-populated.
    assert_eq!(next.manifest().steps().len(), 1);
    assert!(next.manifest().steps()[0].as_schema_delta().is_some());

    // The review-only preview renders the destructive statements so the
    // operator can inspect exactly what an approval would execute.
    let preview = render_migration_preview(next.manifest(), &context).expect("preview renders");
    assert!(preview.contains("undefine"), "preview: {preview}");
}

#[test]
fn writes_publish_atomically_and_recover_orphaned_previews() {
    let genesis = declared_facts(Vec::new());
    let desired = declared(&["person"]);
    let context = context();
    let graph = empty_graph();
    let request = request("init", &genesis, &desired, &context);
    let generated = generated(&graph, &request);
    let preview = render_migration_preview(generated.manifest(), &context).expect("preview");

    // A preview left behind by an interrupted earlier publication (its
    // manifest never appeared) must not wedge every later generation.
    let directory = TempDirectory::new();
    std::fs::write(
        directory.path().join(generated.preview_file_name()),
        "-- partial",
    )
    .expect("orphan preview");
    let manifest_path = write_generated_migration(directory.path(), &generated, &preview)
        .expect("orphaned preview is replaced, not a conflict");
    assert_eq!(
        std::fs::read_to_string(directory.path().join(generated.preview_file_name()))
            .expect("published preview"),
        preview,
    );
    assert!(manifest_path.exists());

    // Publication leaves no temporaries behind under any name.
    let leftovers: Vec<_> = std::fs::read_dir(directory.path())
        .expect("read directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .into_string()
                .expect("utf8")
        })
        .filter(|name| name.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
