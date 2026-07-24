use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration::{
    MigrationAppLabel, MigrationId, MigrationName, MigrationStepId, SchemaDeltaStep,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact,
};
use type_bridge_schema::{ManagedDeltaContext, diff_managed, inverse_delta};
use type_bridge_schema_migration::{
    MigrationDirectory, MigrationHistoryGraph, SchemaMigrationDraft,
    VerifiedSchemaMigrationManifest, build_verified_manifest, decode_verified_manifest,
    discover_verified_migration_chain, discover_verified_migration_chain_with_evidence_in,
    discover_verified_migrations, encode_verified_manifest,
};

struct Fixture {
    context: ManagedDeltaContext,
    source: DeclaredSchema,
    verified: VerifiedSchemaMigrationManifest,
}

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
            let ordinal = u64::try_from(index).expect("fixture ordinal");
            let line = u32::try_from(index + 1).expect("fixture line");
            SourcedSchemaFact::new(
                type_fact(label),
                SourceSpan::new(
                    DocumentId::new("history-fixture").expect("fixture document"),
                    ordinal,
                    ordinal + 1,
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

fn context() -> ManagedDeltaContext {
    ManagedDeltaContext::new(
        ManagedScopeId::new("example-schema").expect("fixture scope"),
        SemanticProfileId::new("typedb-3.12.1/v1").expect("fixture profile"),
        CapabilitySet::new(),
    )
}

fn fixture(
    name: &str,
    parents: Vec<MigrationId>,
    source_labels: &[&str],
    target_labels: &[&str],
) -> Fixture {
    chained_fixture(name, parents, declared(source_labels), target_labels)
}

fn chained_fixture(
    name: &str,
    parents: Vec<MigrationId>,
    source: DeclaredSchema,
    target_labels: &[&str],
) -> Fixture {
    let target = declared(target_labels);
    let context = context();
    let delta = diff_managed(&source, &target, &context).expect("fixture delta");
    let reverse = inverse_delta(&delta).expect("fixture inverse");
    let step = SchemaDeltaStep::new(
        MigrationStepId::new(format!("step-{name}")).expect("fixture step id"),
        delta,
        Some(reverse),
    )
    .expect("fixture step");
    let draft =
        SchemaMigrationDraft::new(migration_id(name), parents, vec![step]).expect("fixture draft");
    let verified = build_verified_manifest(draft, (&source, &context)).expect("fixture manifest");
    Fixture {
        context,
        source,
        verified,
    }
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "typebridge-history-{}-{sequence}",
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

fn write_manifest(directory: &Path, fixture: &Fixture, file_stem: &str) {
    fs::write(
        directory.join(format!("{file_stem}.tbmigration.json")),
        encode_verified_manifest(&fixture.verified).expect("fixture encoding"),
    )
    .expect("write fixture manifest");
}

#[test]
fn discovery_is_direct_exact_verified_and_filename_bound() {
    let directory = TempDirectory::new();
    let root = fixture("alpha", Vec::new(), &["person"], &["person", "root"]);
    write_manifest(directory.path(), &root, "alpha");
    fs::write(directory.path().join("alpha.typeql"), "preview only")
        .expect("write ignored preview");
    let source = root.source.clone();
    let context = root.context.clone();
    let graph = discover_verified_migrations(directory.path(), move |_, bytes| {
        decode_verified_manifest(bytes, (&source, &context))
    })
    .expect("discover exact direct manifest");
    assert_eq!(graph.len(), 1);
    assert_eq!(graph.heads(), &[migration_id("alpha")]);

    let mismatch_directory = TempDirectory::new();
    write_manifest(mismatch_directory.path(), &root, "wrong-name");
    let source = root.source.clone();
    let context = root.context.clone();
    assert_eq!(
        discover_verified_migrations(mismatch_directory.path(), move |_, bytes| {
            decode_verified_manifest(bytes, (&source, &context))
        })
        .expect_err("filename mismatch")
        .code()
        .as_str(),
        "migration_discovery_filename_manifest_mismatch"
    );

    let unknown_directory = TempDirectory::new();
    fs::write(
        unknown_directory.path().join("future.tbmigration.json"),
        br#"{"format":"typebridge.migration/v99"}"#,
    )
    .expect("write unknown format");
    let called = Cell::new(false);
    assert_eq!(
        discover_verified_migrations(unknown_directory.path(), |_, _| {
            called.set(true);
            panic!("unknown format must fail before verifier")
        })
        .expect_err("unknown format")
        .code()
        .as_str(),
        "migration_discovery_unknown_format"
    );
    assert!(!called.get());

    let nested_directory = TempDirectory::new();
    fs::create_dir(nested_directory.path().join("nested")).expect("nested fixture");
    assert_eq!(
        discover_verified_migrations(nested_directory.path(), |_, _| {
            panic!("nested authority must fail before verifier")
        })
        .expect_err("nested authority")
        .code()
        .as_str(),
        "migration_discovery_nested_authority"
    );
}

#[test]
fn malformed_duplicate_missing_self_and_cycle_histories_fail_closed() {
    let root = fixture("0001_root", Vec::new(), &["person"], &["person", "root"]);
    assert_eq!(
        MigrationHistoryGraph::from_verified([root.verified.clone(), root.verified.clone(),])
            .expect_err("duplicate id")
            .code()
            .as_str(),
        "migration_history_duplicate_id"
    );

    let missing = fixture(
        "0002_missing",
        vec![migration_id("0999_absent")],
        &["person"],
        &["person", "missing"],
    );
    assert_eq!(
        MigrationHistoryGraph::from_verified([missing.verified])
            .expect_err("missing parent")
            .code()
            .as_str(),
        "migration_history_missing_parent"
    );

    let cycle_a = fixture(
        "0003_cycle_a",
        vec![migration_id("0004_cycle_b")],
        &["person"],
        &["person", "a"],
    );
    let cycle_b = fixture(
        "0004_cycle_b",
        vec![migration_id("0003_cycle_a")],
        &["person"],
        &["person", "b"],
    );
    assert_eq!(
        MigrationHistoryGraph::from_verified([cycle_a.verified, cycle_b.verified])
            .expect_err("cycle")
            .code()
            .as_str(),
        "migration_history_cycle"
    );

    let source = declared(&["person"]);
    let target = declared(&["person", "self"]);
    let context = context();
    let delta = diff_managed(&source, &target, &context).expect("self delta");
    let step = SchemaDeltaStep::new(
        MigrationStepId::new("self-step").expect("step id"),
        delta,
        None,
    )
    .expect("self step");
    let id = migration_id("0005_self");
    assert_eq!(
        SchemaMigrationDraft::new(id.clone(), vec![id], vec![step])
            .expect_err("self parent")
            .code()
            .as_str(),
        "migration_manifest_self_parent"
    );
}

fn branch_graph() -> (
    MigrationHistoryGraph,
    MigrationId,
    MigrationId,
    MigrationId,
    MigrationId,
) {
    let root_id = migration_id("0001_root");
    let left_id = migration_id("0002_left");
    let right_id = migration_id("0003_right");
    let merge_id = migration_id("0004_merge");
    let root = fixture("0001_root", Vec::new(), &["person"], &["person", "root"]);
    let left = fixture(
        "0002_left",
        vec![root_id.clone()],
        &["person", "root"],
        &["person", "root", "left"],
    );
    let right = fixture(
        "0003_right",
        vec![root_id.clone()],
        &["person", "root"],
        &["person", "root", "right"],
    );
    let merge = fixture(
        "0004_merge",
        vec![left_id.clone(), right_id.clone()],
        &["person", "root", "left", "right"],
        &["person", "root", "left", "right", "merge"],
    );
    let graph = MigrationHistoryGraph::from_verified([
        merge.verified,
        right.verified,
        root.verified,
        left.verified,
    ])
    .expect("branch graph");
    (graph, root_id, left_id, right_id, merge_id)
}

#[test]
fn branch_merge_frontier_and_apply_closure_are_deterministic() {
    let (graph, root, left, right, merge) = branch_graph();
    assert_eq!(
        graph.topological_order(),
        &[root.clone(), left.clone(), right.clone(), merge.clone()]
    );
    assert_eq!(graph.heads(), std::slice::from_ref(&merge));
    assert_eq!(graph.default_head().expect("single head"), Some(&merge));

    let branched = BTreeSet::from([root.clone(), left.clone(), right.clone()]);
    assert_eq!(
        graph.applied_frontier(&branched).expect("branch frontier"),
        vec![left.clone(), right.clone()]
    );
    let only_root = BTreeSet::from([root.clone()]);
    assert_eq!(
        graph
            .plan_apply(&only_root, &BTreeSet::from([merge.clone()]))
            .expect("merge apply closure"),
        vec![left.clone(), right.clone(), merge.clone()]
    );
    assert_eq!(
        graph
            .plan_apply_to_default_head(&only_root)
            .expect("default apply closure"),
        vec![left.clone(), right.clone(), merge.clone()]
    );
    assert_eq!(
        graph
            .validate_applied(&BTreeSet::from([left.clone()]))
            .expect_err("not downward closed")
            .code()
            .as_str(),
        "migration_history_applied_not_downward_closed"
    );

    let branch_only = MigrationHistoryGraph::from_verified(
        graph
            .manifests()
            .filter(|(id, _)| *id != &merge)
            .map(|(_, manifest)| manifest.clone()),
    )
    .expect("multi-head branch");
    assert_eq!(branch_only.heads(), &[left, right]);
    assert_eq!(
        branch_only
            .default_head()
            .expect_err("ambiguous default")
            .code()
            .as_str(),
        "migration_history_ambiguous_default_head"
    );
}

#[test]
fn rollback_is_reverse_topological_and_preserves_applied_ancestry() {
    let (graph, root, left, right, merge) = branch_graph();
    let applied = BTreeSet::from([root.clone(), left.clone(), right.clone(), merge.clone()]);
    assert_eq!(
        graph
            .plan_rollback(&applied, &BTreeSet::from([left.clone()]))
            .expect_err("remaining descendant")
            .code()
            .as_str(),
        "migration_history_remaining_applied_descendant"
    );
    assert_eq!(
        graph
            .plan_rollback(&applied, &BTreeSet::from([merge.clone(), left.clone()]),)
            .expect("rollback one merged branch"),
        vec![merge.clone(), left.clone()]
    );
    assert_eq!(
        graph
            .plan_rollback(
                &applied,
                &BTreeSet::from([root.clone(), left.clone(), right.clone(), merge.clone(),]),
            )
            .expect("rollback complete graph"),
        vec![merge, left, right, root]
    );
}

#[test]
fn discovery_decodes_multiple_contexts_before_graph_construction() {
    let directory = TempDirectory::new();
    let root_id = migration_id("0001_root");
    let root = fixture("0001_root", Vec::new(), &["person"], &["person", "root"]);
    let child = fixture(
        "0002_child",
        vec![root_id.clone()],
        &["person", "root"],
        &["person", "root", "child"],
    );
    write_manifest(directory.path(), &root, "0001_root");
    write_manifest(directory.path(), &child, "0002_child");
    let contexts = BTreeMap::from([
        (
            "0001_root.tbmigration.json".to_owned(),
            (root.source.clone(), root.context.clone()),
        ),
        (
            "0002_child.tbmigration.json".to_owned(),
            (child.source.clone(), child.context.clone()),
        ),
    ]);
    let graph = discover_verified_migrations(directory.path(), |path, bytes| {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("fixture filename");
        let (source, context) = &contexts[file_name];
        decode_verified_manifest(bytes, (source, context))
    })
    .expect("verified multi-context discovery");
    assert_eq!(
        graph.topological_order(),
        &[root_id, migration_id("0002_child")]
    );
}

#[test]
fn chain_discovery_is_dependency_ordered_not_filename_ordered() {
    let directory = TempDirectory::new();
    let parent = fixture("0002_parent", Vec::new(), &[], &["person"]);
    let child = chained_fixture(
        "0001_child",
        vec![migration_id("0002_parent")],
        parent.verified.target_schema().clone(),
        &["company", "person"],
    );
    write_manifest(directory.path(), &parent, "0002_parent");
    write_manifest(directory.path(), &child, "0001_child");

    let graph = discover_verified_migration_chain(directory.path(), &declared(&[]), &context())
        .expect("chain discovery must decode parents before filename-earlier children");
    assert_eq!(graph.len(), 2);
    assert_eq!(
        graph.topological_order(),
        &[migration_id("0002_parent"), migration_id("0001_child")]
    );
    assert_eq!(graph.heads(), &[migration_id("0001_child")]);
}

#[test]
fn retained_history_evidence_rejects_canonical_swap_delete_and_add() {
    let swapped = TempDirectory::new();
    let original = fixture("0001_root", Vec::new(), &[], &["person"]);
    let replacement = fixture("0001_root", Vec::new(), &[], &["company"]);
    write_manifest(swapped.path(), &original, "0001_root");
    let swapped_authority =
        MigrationDirectory::open_ambient(swapped.path()).expect("retain swapped authority");
    let (_, swapped_evidence) = discover_verified_migration_chain_with_evidence_in(
        &swapped_authority,
        &declared(&[]),
        &context(),
    )
    .expect("discover swapped fixture");
    fs::rename(
        swapped.path().join("0001_root.tbmigration.json"),
        swapped.path().join("0001_root.tbmigration.json.held"),
    )
    .expect("hold original canonical file");
    write_manifest(swapped.path(), &replacement, "0001_root");
    assert_eq!(
        swapped_evidence
            .require_unchanged(&swapped_authority)
            .expect_err("same-name replacement must be rejected")
            .code()
            .as_str(),
        "migration_history_authority_digest_changed"
    );

    let deleted = TempDirectory::new();
    write_manifest(deleted.path(), &original, "0001_root");
    let deleted_authority =
        MigrationDirectory::open_ambient(deleted.path()).expect("retain deleted authority");
    let (_, deleted_evidence) = discover_verified_migration_chain_with_evidence_in(
        &deleted_authority,
        &declared(&[]),
        &context(),
    )
    .expect("discover deleted fixture");
    fs::remove_file(deleted.path().join("0001_root.tbmigration.json"))
        .expect("delete canonical file");
    assert_eq!(
        deleted_evidence
            .require_unchanged(&deleted_authority)
            .expect_err("canonical deletion must be rejected")
            .code()
            .as_str(),
        "migration_history_authority_membership_changed"
    );

    let added = TempDirectory::new();
    write_manifest(added.path(), &original, "0001_root");
    let added_authority =
        MigrationDirectory::open_ambient(added.path()).expect("retain added authority");
    let (_, added_evidence) = discover_verified_migration_chain_with_evidence_in(
        &added_authority,
        &declared(&[]),
        &context(),
    )
    .expect("discover added fixture");
    let child = chained_fixture(
        "0002_child",
        vec![migration_id("0001_root")],
        original.verified.target_schema().clone(),
        &["company", "person"],
    );
    write_manifest(added.path(), &child, "0002_child");
    assert_eq!(
        added_evidence
            .require_unchanged(&added_authority)
            .expect_err("canonical addition must be rejected")
            .code()
            .as_str(),
        "migration_history_authority_membership_changed"
    );
}

#[test]
fn chain_discovery_binds_filename_to_manifest_name() {
    let directory = TempDirectory::new();
    let genesis = fixture("alpha", Vec::new(), &[], &["person"]);
    write_manifest(directory.path(), &genesis, "wrong-name");
    assert_eq!(
        discover_verified_migration_chain(directory.path(), &declared(&[]), &context())
            .expect_err("filename stem must equal the manifest name")
            .code()
            .as_str(),
        "migration_discovery_filename_manifest_mismatch"
    );
}

#[test]
fn chain_discovery_rejects_unknown_parents_before_decoding() {
    let directory = TempDirectory::new();
    let orphan = fixture(
        "0001_orphan",
        vec![migration_id("0000_missing")],
        &["person"],
        &["company", "person"],
    );
    write_manifest(directory.path(), &orphan, "0001_orphan");
    assert_eq!(
        discover_verified_migration_chain(directory.path(), &declared(&[]), &context())
            .expect_err("missing parent file")
            .code()
            .as_str(),
        "migration_discovery_unknown_parent"
    );
}

#[test]
fn chain_discovery_rejects_duplicate_identities_across_files() {
    let directory = TempDirectory::new();
    let genesis = fixture("alpha", Vec::new(), &[], &["person"]);
    write_manifest(directory.path(), &genesis, "alpha");
    write_manifest(directory.path(), &genesis, "beta");
    assert_eq!(
        discover_verified_migration_chain(directory.path(), &declared(&[]), &context())
            .expect_err("two files claiming one identity")
            .code()
            .as_str(),
        "migration_discovery_duplicate_id"
    );
}

#[test]
fn chain_discovery_rejects_parent_cycles_before_decoding() {
    let directory = TempDirectory::new();
    let first = fixture("cycle_a", vec![migration_id("cycle_b")], &[], &["person"]);
    let second = fixture("cycle_b", vec![migration_id("cycle_a")], &[], &["person"]);
    write_manifest(directory.path(), &first, "cycle_a");
    write_manifest(directory.path(), &second, "cycle_b");
    assert_eq!(
        discover_verified_migration_chain(directory.path(), &declared(&[]), &context())
            .expect_err("mutual parents form a cycle")
            .code()
            .as_str(),
        "migration_history_cycle"
    );
}

#[test]
fn chain_discovery_accepts_only_convergent_merge_parents() {
    let convergent = TempDirectory::new();
    let root = fixture("0001_root", Vec::new(), &[], &["person"]);
    let left = chained_fixture(
        "0002_left",
        vec![migration_id("0001_root")],
        root.verified.target_schema().clone(),
        &["company", "person"],
    );
    let right = chained_fixture(
        "0003_right",
        vec![migration_id("0001_root")],
        root.verified.target_schema().clone(),
        &["company", "person"],
    );
    let merge = chained_fixture(
        "0004_merge",
        vec![migration_id("0002_left"), migration_id("0003_right")],
        left.verified.target_schema().clone(),
        &["company", "person", "team"],
    );
    write_manifest(convergent.path(), &root, "0001_root");
    write_manifest(convergent.path(), &left, "0002_left");
    write_manifest(convergent.path(), &right, "0003_right");
    write_manifest(convergent.path(), &merge, "0004_merge");
    let graph = discover_verified_migration_chain(convergent.path(), &declared(&[]), &context())
        .expect("equal-target merge parents are decodable");
    assert_eq!(graph.len(), 4);
    assert_eq!(graph.heads(), &[migration_id("0004_merge")]);

    let divergent = TempDirectory::new();
    let narrow = chained_fixture(
        "0003_right",
        vec![migration_id("0001_root")],
        root.verified.target_schema().clone(),
        &["person", "team"],
    );
    let stale_merge = chained_fixture(
        "0004_merge",
        vec![migration_id("0002_left"), migration_id("0003_right")],
        left.verified.target_schema().clone(),
        &["company", "person", "team"],
    );
    write_manifest(divergent.path(), &root, "0001_root");
    write_manifest(divergent.path(), &left, "0002_left");
    write_manifest(divergent.path(), &narrow, "0003_right");
    write_manifest(divergent.path(), &stale_merge, "0004_merge");
    assert_eq!(
        discover_verified_migration_chain(divergent.path(), &declared(&[]), &context())
            .expect_err("divergent merge parents must fail closed")
            .code()
            .as_str(),
        "migration_discovery_divergent_merge_sources"
    );
}

#[test]
fn discovery_bounds_file_size_before_allocation() {
    let directory = TempDirectory::new();
    // One byte past the canonical document ceiling: discovery must fail
    // with the resource diagnostic instead of retaining the full input.
    let oversized = vec![b'x'; type_bridge_contract::limits::MAX_CANONICAL_BYTES + 1];
    fs::write(directory.path().join("huge.tbmigration.json"), oversized)
        .expect("write oversized candidate");
    let error = discover_verified_migrations(directory.path(), |_, _bytes| {
        panic!("an oversized candidate must never reach decoding")
    })
    .expect_err("oversized candidate rejected");
    assert_eq!(error.code().as_str(), "migration_discovery_file_oversized");
}
