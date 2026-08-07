use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use type_bridge_contract::capability::CapabilityId;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{AttributeId, TypeId, TypeKind};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration::{
    MigrationAppLabel, MigrationId, MigrationName, MigrationStepId, SchemaDeltaStep,
};
use type_bridge_contract::migration_assertion::migration_assertion_capability_vocabulary;
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, CanonicalValueRange,
    CanonicalValueSet, DeclaredSchema, DocumentId, OwnsFact, OwnsFactId, RegexPattern,
    SchemaAnnotationValue, SchemaFact, SourceSpan, SourcedSchemaFact, SubFact, SubFactId, TypeFact,
    ValueFact, ValueFactId,
};
use type_bridge_contract::value::{CanonicalValue, Cardinality, ValueTypeTag};
use type_bridge_schema::{
    BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SafetyClass, diff_managed, inverse_delta,
};
use type_bridge_schema_migration::MigrationApplyTarget;
use type_bridge_schema_migration::{
    MigrationDirectory, MigrationGenerationOutcome, MigrationGenerationRequest,
    MigrationHistoryGraph, MigrationSafetyPolicy, SchemaLoweringBinding, SchemaMigrationDraft,
    VerifiedSchemaMigrationManifest, build_verified_manifest, build_verified_migration_apply_plan,
    decode_verified_manifest, discover_verified_migration_chain, generate_next_migration,
    render_migration_preview, try_acquire_migration_authoring_lock, typedb_3_12_1_profile,
    write_generated_migration_under_lock,
};

const APP_LABEL: &str = "example";

fn publish_generated_migration(
    directory: &Path,
    generated: &type_bridge_schema_migration::GeneratedMigration,
    preview: &str,
) -> Result<PathBuf, type_bridge_contract::diagnostic::Diagnostic> {
    let authority = MigrationDirectory::open_ambient(directory).expect("directory authority");
    let lock = try_acquire_migration_authoring_lock(&authority)?;
    let relative = write_generated_migration_under_lock(&lock, generated, preview)?;
    Ok(directory.join(relative))
}

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

fn initial_constrained_schema() -> DeclaredSchema {
    let actor = entity_id("actor");
    let person = entity_id("person");
    let key_id = AttributeId::new("key-id").expect("fixture attribute");
    let unique_id = AttributeId::new("unique-id").expect("fixture attribute");
    let required_name = AttributeId::new("required-name").expect("fixture attribute");
    let score = AttributeId::new("score").expect("fixture attribute");
    let status = AttributeId::new("status").expect("fixture attribute");
    let code = AttributeId::new("code").expect("fixture attribute");
    let key_owns = OwnsFactId::new(person.clone(), key_id.clone()).expect("fixture owns");
    let unique_owns = OwnsFactId::new(person.clone(), unique_id.clone()).expect("fixture owns");
    let required_owns =
        OwnsFactId::new(person.clone(), required_name.clone()).expect("fixture owns");
    let mut facts = vec![
        SchemaFact::Type(TypeFact::new(actor.clone()).expect("fixture actor")),
        SchemaFact::Type(TypeFact::new(person.clone()).expect("fixture person")),
        SchemaFact::Sub(SubFact::new(
            SubFactId::new(person, actor.clone()).expect("fixture sub identity"),
        )),
        SchemaFact::Owns(OwnsFact::new(key_owns.clone())),
        SchemaFact::Owns(OwnsFact::new(unique_owns.clone())),
        SchemaFact::Owns(OwnsFact::new(required_owns.clone())),
        abstract_fact(actor.label().as_str()),
        SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(AnnotationSubjectId::Owns(key_owns), AnnotationKindId::Key),
                SchemaAnnotationValue::Presence,
            )
            .expect("fixture key annotation"),
        ),
        SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(
                    AnnotationSubjectId::Owns(unique_owns),
                    AnnotationKindId::Unique,
                ),
                SchemaAnnotationValue::Presence,
            )
            .expect("fixture unique annotation"),
        ),
        SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(
                    AnnotationSubjectId::Owns(required_owns),
                    AnnotationKindId::Card,
                ),
                SchemaAnnotationValue::Cardinality(
                    Cardinality::new(1, Some(1)).expect("fixture cardinality"),
                ),
            )
            .expect("fixture card annotation"),
        ),
        SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(
                    AnnotationSubjectId::Value(ValueFactId::new(score.clone())),
                    AnnotationKindId::Range,
                ),
                SchemaAnnotationValue::Range(
                    CanonicalValueRange::new(
                        Some(CanonicalValue::Long(0)),
                        Some(CanonicalValue::Long(100)),
                    )
                    .expect("fixture range"),
                ),
            )
            .expect("fixture range annotation"),
        ),
        SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(
                    AnnotationSubjectId::Value(ValueFactId::new(status.clone())),
                    AnnotationKindId::Values,
                ),
                SchemaAnnotationValue::Values(
                    CanonicalValueSet::new([CanonicalValue::Long(1), CanonicalValue::Long(2)])
                        .expect("fixture values"),
                ),
            )
            .expect("fixture values annotation"),
        ),
        SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(
                    AnnotationSubjectId::Value(ValueFactId::new(code.clone())),
                    AnnotationKindId::Regex,
                ),
                SchemaAnnotationValue::Regex(RegexPattern::new("^[A-Z]+$").expect("fixture regex")),
            )
            .expect("fixture regex annotation"),
        ),
    ];
    for (attribute, value_type) in [
        (key_id, ValueTypeTag::String),
        (unique_id, ValueTypeTag::String),
        (required_name, ValueTypeTag::String),
        (score, ValueTypeTag::Long),
        (status, ValueTypeTag::Long),
        (code, ValueTypeTag::String),
    ] {
        facts.push(SchemaFact::Type(
            TypeFact::new(
                TypeId::new(TypeKind::Attribute, attribute.label().as_str())
                    .expect("fixture attribute type"),
            )
            .expect("fixture attribute type fact"),
        ));
        facts.push(SchemaFact::Value(ValueFact::new(
            ValueFactId::new(attribute),
            value_type,
        )));
    }
    declared_facts(facts)
}

fn single_key_schema(with_key: bool) -> DeclaredSchema {
    let person = entity_id("person");
    let identifier = AttributeId::new("identifier").expect("fixture attribute");
    let owns = OwnsFactId::new(person.clone(), identifier.clone()).expect("fixture owns");
    let mut facts = vec![
        SchemaFact::Type(TypeFact::new(person).expect("fixture person")),
        SchemaFact::Type(
            TypeFact::new(
                TypeId::new(TypeKind::Attribute, identifier.label().as_str())
                    .expect("fixture attribute type"),
            )
            .expect("fixture attribute type fact"),
        ),
        SchemaFact::Value(ValueFact::new(
            ValueFactId::new(identifier),
            ValueTypeTag::String,
        )),
        SchemaFact::Owns(OwnsFact::new(owns.clone())),
    ];
    if with_key {
        facts.push(SchemaFact::Annotation(
            AnnotationFact::new(
                AnnotationFactId::new(AnnotationSubjectId::Owns(owns), AnnotationKindId::Key),
                SchemaAnnotationValue::Presence,
            )
            .expect("fixture key annotation"),
        ));
    }
    declared_facts(facts)
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
    publish_generated_migration(directory.path(), &first, &preview)
        .expect("write generated migration");
    let discovered = discover_verified_migration_chain(directory.path(), &genesis, &context)
        .expect("discover generated migration");
    assert_eq!(discovered.len(), 1);

    // The committed head now equals the desired schema.
    let again = generate_next_migration(&discovered, &request).expect("regenerate");
    assert!(matches!(again, MigrationGenerationOutcome::UpToDate));
}

#[test]
fn genesis_constraints_on_new_types_are_proven_data_free() {
    let genesis = declared_facts(Vec::new());
    let desired = initial_constrained_schema();
    let context = context();
    let next = generated(
        &empty_graph(),
        &request("constrained_init", &genesis, &desired, &context),
    );

    assert_eq!(next.manifest().safety(), SafetyClass::Conditional);
    assert_eq!(
        next.manifest().steps().len(),
        1,
        "the empty source needs no assertion or backfill step",
    );
    let preview = render_migration_preview(next.manifest(), &context)
        .expect("condition-free initial constraints lower");
    for constraint in [
        "@abstract",
        "@key",
        "@unique",
        "@card",
        "@range",
        "@values",
        "@regex",
    ] {
        assert!(
            preview.contains(constraint),
            "preview lacks {constraint}: {preview}"
        );
    }

    let decoded = decode_verified_manifest(next.canonical_bytes(), (&genesis, &context))
        .expect("canonical constrained genesis decodes");
    assert_eq!(decoded, *next.manifest());
    let directory = TempDirectory::new();
    publish_generated_migration(directory.path(), &next, &preview)
        .expect("publish constrained genesis");
    let discovered = discover_verified_migration_chain(directory.path(), &genesis, &context)
        .expect("discover constrained genesis");
    assert_eq!(discovered.manifests().count(), 1);

    let lowering = SchemaLoweringBinding::current(context.available_capabilities().clone())
        .expect("lowering binding");
    let plan = build_verified_migration_apply_plan(
        &discovered,
        &BTreeSet::new(),
        &MigrationApplyTarget::DefaultHead,
        &context,
        &lowering,
        &MigrationSafetyPolicy::default_policy(),
        &[],
    )
    .expect("condition-free constrained genesis builds an apply plan");
    let migration = &plan.migrations()[0];
    assert_eq!(migration.transaction_groups().len(), 1);
    assert_eq!(migration.transaction_groups()[0].assertion_count(), 0);
    let lowering = migration.steps()[0]
        .lowering()
        .expect("the only generated step is a schema delta");
    assert_eq!(
        lowering
            .units()
            .iter()
            .filter(|unit| unit.safety() == SafetyClass::BackfillRequired)
            .count(),
        3,
        "key, unique, and required cardinality retain raw unit safety",
    );
}

#[test]
fn constraints_on_an_existing_anchor_keep_the_backfill_gate_closed() {
    let genesis = declared_facts(Vec::new());
    let base = single_key_schema(false);
    let desired = single_key_schema(true);
    let context = context();
    let first = committed_manifest("0001_base", Vec::new(), &genesis, &base, &context);
    let graph = MigrationHistoryGraph::from_verified([first]).expect("base history");

    let error = generate_next_migration(
        &graph,
        &request("add_existing_key", &genesis, &desired, &context),
    )
    .expect_err("an existing owner domain still requires backfill");
    assert_eq!(
        error.code().as_str(),
        "migration_manifest_unresolved_safety"
    );
}

#[test]
fn condition_free_proof_does_not_cross_schema_step_transaction_boundaries() {
    let genesis = declared_facts(Vec::new());
    let base = single_key_schema(false);
    let constrained = single_key_schema(true);
    let context = context();
    let create = diff_managed(&genesis, &base, &context).expect("create delta");
    let create_reverse = inverse_delta(&create).expect("create reverse");
    let constrain = diff_managed(&base, &constrained, &context).expect("constraint delta");
    let constrain_reverse = inverse_delta(&constrain).expect("constraint reverse");
    let draft = SchemaMigrationDraft::new(
        migration_id("0001_split_constraint"),
        Vec::new(),
        vec![
            SchemaDeltaStep::new(
                MigrationStepId::new("create-schema").expect("step id"),
                create,
                Some(create_reverse),
            )
            .expect("create step"),
            SchemaDeltaStep::new(
                MigrationStepId::new("add-key").expect("step id"),
                constrain,
                Some(constrain_reverse),
            )
            .expect("constraint step"),
        ],
    )
    .expect("split draft");

    let error = build_verified_manifest(draft, (&genesis, &context))
        .expect_err("proof from the first transaction must not discharge the second");
    assert_eq!(
        error.code().as_str(),
        "migration_manifest_unresolved_safety"
    );
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
    publish_generated_migration(directory.path(), &generated, &preview)
        .expect("first write succeeds");
    let error = publish_generated_migration(directory.path(), &generated, &preview)
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
    let manifest_path = publish_generated_migration(directory.path(), &generated, &preview)
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

#[test]
fn concurrent_writers_use_one_locked_no_replace_publication() {
    use std::sync::{Arc, Barrier};

    let genesis = declared_facts(Vec::new());
    let desired = declared(&["person"]);
    let context = context();
    let graph = empty_graph();
    let request = request("concurrent", &genesis, &desired, &context);
    let generated = generated(&graph, &request);
    let preview = render_migration_preview(generated.manifest(), &context).expect("preview");
    let directory = TempDirectory::new();
    let root = Arc::new(directory.path().to_path_buf());
    let barrier = Arc::new(Barrier::new(2));

    let handles = (0..2)
        .map(|_| {
            let root = Arc::clone(&root);
            let barrier = Arc::clone(&barrier);
            let generated = generated.clone();
            let preview = preview.clone();
            std::thread::spawn(move || {
                barrier.wait();
                publish_generated_migration(&root, &generated, &preview)
            })
        })
        .collect::<Vec<_>>();
    let results = handles
        .into_iter()
        .map(|handle| handle.join().expect("writer does not panic"))
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
    assert_eq!(
        std::fs::read(root.join(generated.file_name())).expect("manifest published"),
        generated.canonical_bytes(),
    );
    assert_eq!(
        std::fs::read_to_string(root.join(generated.preview_file_name()))
            .expect("preview published"),
        preview,
    );
    assert!(
        std::fs::read_dir(root.as_ref())
            .expect("directory reads")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp"))
    );
}
