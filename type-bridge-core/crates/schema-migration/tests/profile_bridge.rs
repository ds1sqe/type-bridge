//! Profile-upgrade bridge manifests and profile-aware history discovery.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::{FormatVersion, from_canonical_json, to_canonical_json};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticDetailValue};
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration::{
    MigrationAppLabel, MigrationId, MigrationName, MigrationStepId, SchemaDeltaStep,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact,
};
use type_bridge_schema::{ManagedDeltaContext, SafetyClass, diff_managed, inverse_delta};
use type_bridge_schema_migration::{
    MigrationApplyApproval, MigrationApplyPlanError, MigrationApplyTarget, MigrationDirectory,
    MigrationHistoryGraph, MigrationProfile, MigrationProfileUpgrade, MigrationSafetyPolicy,
    SchemaLoweringBinding, SchemaMigrationDraft, VerifiedSchemaMigrationManifest,
    build_verified_manifest, build_verified_migration_apply_plan,
    build_verified_migration_rollback_plan, decode_verified_manifest,
    discover_history_for_profile_upgrade_in, discover_verified_migration_chain,
    encode_verified_manifest, migration_runtime_capability_vocabulary,
};

const OLD: MigrationProfile = MigrationProfile::Typedb3_12_1;
const NEW: MigrationProfile = MigrationProfile::Typedb3_13_6;

fn migration_id(name: &str) -> MigrationId {
    MigrationId::from_components(
        MigrationAppLabel::new("example").expect("fixture app label"),
        MigrationName::new(name).expect("fixture migration name"),
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
                SchemaFact::Type(
                    TypeFact::new(TypeId::new(TypeKind::Entity, *label).expect("fixture type"))
                        .expect("fixture type fact"),
                ),
                SourceSpan::new(
                    DocumentId::new("profile-bridge-fixture").expect("fixture document"),
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

fn context(profile: MigrationProfile) -> ManagedDeltaContext {
    ManagedDeltaContext::new(
        ManagedScopeId::new("example-schema").expect("fixture scope"),
        profile.semantic_profile_id(),
        migration_runtime_capability_vocabulary().expect("runtime capabilities"),
    )
}

fn step_manifest(
    name: &str,
    parents: Vec<MigrationId>,
    source: &DeclaredSchema,
    target_labels: &[&str],
    profile: MigrationProfile,
) -> VerifiedSchemaMigrationManifest {
    let context = context(profile);
    let target = declared(target_labels);
    let delta = diff_managed(source, &target, &context).expect("fixture delta");
    let reverse = inverse_delta(&delta).expect("fixture inverse");
    let step = SchemaDeltaStep::new(
        MigrationStepId::new(format!("step-{name}")).expect("fixture step id"),
        delta,
        Some(reverse),
    )
    .expect("fixture step");
    let draft =
        SchemaMigrationDraft::new(migration_id(name), parents, vec![step]).expect("fixture draft");
    build_verified_manifest(draft, (source, &context)).expect("fixture manifest")
}

fn bridge_manifest(
    name: &str,
    parent: &VerifiedSchemaMigrationManifest,
    from: MigrationProfile,
    to: MigrationProfile,
) -> Result<VerifiedSchemaMigrationManifest, Diagnostic> {
    let draft =
        SchemaMigrationDraft::profile_bridge(migration_id(name), parent.id().clone(), from)?;
    build_verified_manifest(draft, (parent.target_schema(), &context(to)))
}

/// `0001_root` and `0002_more` on 3.12.1, then the bridge to 3.13.6.
fn bridged_history() -> [VerifiedSchemaMigrationManifest; 3] {
    let root = step_manifest(
        "0001_root",
        Vec::new(),
        &declared(&["person"]),
        &["person", "root"],
        OLD,
    );
    let more = step_manifest(
        "0002_more",
        vec![root.id().clone()],
        root.target_schema(),
        &["person", "root", "more"],
        OLD,
    );
    let bridge = bridge_manifest("0003_profile_upgrade", &more, OLD, NEW).expect("bridge");
    [root, more, bridge]
}

fn code(diagnostic: &Diagnostic) -> &str {
    diagnostic.code().as_str()
}

fn encode_value(value: &Value) -> Vec<u8> {
    to_canonical_json(value).expect("canonical fixture bytes")
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "typebridge-profile-bridge-{}-{sequence}",
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

fn write_history(manifests: &[&VerifiedSchemaMigrationManifest]) -> TempDirectory {
    let directory = TempDirectory::new();
    for manifest in manifests {
        fs::write(
            directory.path().join(format!(
                "{}.tbmigration.json",
                manifest.id().name().as_str()
            )),
            encode_verified_manifest(manifest).expect("fixture encoding"),
        )
        .expect("write fixture manifest");
    }
    directory
}

fn discover(
    directory: &TempDirectory,
    workspace: MigrationProfile,
) -> Result<MigrationHistoryGraph, Diagnostic> {
    discover_verified_migration_chain(
        directory.path(),
        &declared(&["person"]),
        &context(workspace),
    )
}

#[test]
fn ordinary_manifest_bytes_carry_no_bridge_field() {
    let [root, ..] = bridged_history();
    let bytes = encode_verified_manifest(&root).unwrap();
    let wire = from_canonical_json::<Value>(&bytes).unwrap();
    assert!(wire.get("profile_bridge").is_none());
    assert!(!root.is_profile_bridge());
    let decoded =
        decode_verified_manifest(&bytes, (&declared(&["person"]), &context(OLD))).unwrap();
    assert_eq!(decoded, root);
    assert_eq!(encode_verified_manifest(&decoded).unwrap(), bytes);
}

#[test]
fn bridge_round_trips_and_moves_only_the_profile() {
    let [_, more, bridge] = bridged_history();
    assert!(bridge.is_profile_bridge());
    assert!(bridge.steps().is_empty());
    assert_eq!(bridge.parents(), std::slice::from_ref(more.id()));
    assert_eq!(bridge.safety(), SafetyClass::FormalOnly);
    assert!(!bridge.reversible());
    assert_eq!(bridge.semantic_profile().id(), &NEW.semantic_profile_id());
    let from = bridge.profile_bridge().unwrap();
    assert_eq!(from.from(), OLD);
    assert_eq!(from.from_semantic_profile(), more.semantic_profile());
    assert_eq!(from.from_lowering_profile(), more.lowering_profile());

    // Source is the parent's target under the old profile; the target is the
    // same declared schema under the new profile.
    assert_eq!(bridge.source_state(), more.target_state());
    assert_ne!(bridge.target_state(), bridge.source_state());
    assert_eq!(bridge.source_schema(), bridge.target_schema());
    assert_eq!(
        bridge.target_state().managed_declared_identity(),
        bridge.source_state().managed_declared_identity()
    );
    assert_ne!(
        bridge.target_state().managed_semantic_schema(),
        bridge.source_state().managed_semantic_schema()
    );

    let bytes = encode_verified_manifest(&bridge).unwrap();
    let wire = from_canonical_json::<Value>(&bytes).unwrap();
    assert_eq!(
        wire["profile_bridge"]["from_semantic_profile"]["id"],
        "typedb-3.12.1/v1"
    );
    let decoded = decode_verified_manifest(&bytes, (more.target_schema(), &context(NEW))).unwrap();
    assert_eq!(decoded, bridge);
    // Replaying under the old profile cannot reproduce the recorded claims.
    let error =
        decode_verified_manifest(&bytes, (more.target_schema(), &context(OLD))).unwrap_err();
    assert_eq!(
        code(&error),
        "migration_manifest_profile_bridge_not_upgrade"
    );
}

#[test]
fn bridge_shape_and_direction_are_validated() {
    let [root, more, bridge] = bridged_history();
    let bytes = encode_verified_manifest(&bridge).unwrap();
    let source = (more.target_schema(), &context(NEW));

    let mut with_steps = from_canonical_json::<Value>(&bytes).unwrap();
    let root_wire =
        from_canonical_json::<Value>(&encode_verified_manifest(&root).unwrap()).unwrap();
    with_steps["steps"] = root_wire["steps"].clone();
    let error = decode_verified_manifest(&encode_value(&with_steps), source).unwrap_err();
    assert_eq!(code(&error), "migration_manifest_profile_bridge_shape");

    let mut two_parents = from_canonical_json::<Value>(&bytes).unwrap();
    let parent = two_parents["parents"][0].clone();
    let mut other = parent.clone();
    other["name"] = Value::from("0001_root");
    two_parents["parents"] = Value::Array(vec![other, parent]);
    let error = decode_verified_manifest(&encode_value(&two_parents), source).unwrap_err();
    assert_eq!(code(&error), "migration_manifest_profile_bridge_shape");

    let mut no_parent = from_canonical_json::<Value>(&bytes).unwrap();
    no_parent["parents"] = Value::Array(Vec::new());
    let error = decode_verified_manifest(&encode_value(&no_parent), source).unwrap_err();
    assert_eq!(code(&error), "migration_manifest_profile_bridge_shape");

    let mut unknown = from_canonical_json::<Value>(&bytes).unwrap();
    unknown["profile_bridge"]["from_semantic_profile"]["id"] = Value::from("typedb-9.9.9/v1");
    let error = decode_verified_manifest(&encode_value(&unknown), source).unwrap_err();
    assert_eq!(
        code(&error),
        "migration_manifest_profile_bridge_unsupported_profile"
    );

    let new_root = step_manifest(
        "0001_root",
        Vec::new(),
        &declared(&["person"]),
        &["person", "root"],
        NEW,
    );
    let downgrade = bridge_manifest("0002_down", &new_root, NEW, OLD).unwrap_err();
    assert_eq!(
        code(&downgrade),
        "migration_manifest_profile_bridge_not_upgrade"
    );
    let same = bridge_manifest("0003_same", &more, OLD, OLD).unwrap_err();
    assert_eq!(code(&same), "migration_manifest_profile_bridge_not_upgrade");

    let self_parent =
        SchemaMigrationDraft::profile_bridge(migration_id("0002_more"), more.id().clone(), OLD)
            .unwrap_err();
    assert_eq!(code(&self_parent), "migration_manifest_self_parent");
}

#[test]
fn history_crosses_one_bridge_into_the_workspace_profile() {
    let [root, more, bridge] = bridged_history();
    let after = step_manifest(
        "0004_after",
        vec![bridge.id().clone()],
        bridge.target_schema(),
        &["person", "root", "more", "after"],
        NEW,
    );
    // The first manifest on the new profile chains from the bridge target.
    assert_eq!(after.source_state(), bridge.target_state());

    let directory = write_history(&[&root, &more, &bridge, &after]);
    let graph = discover(&directory, NEW).expect("bridged history");
    assert_eq!(
        graph.topological_order(),
        &[
            root.id().clone(),
            more.id().clone(),
            bridge.id().clone(),
            after.id().clone()
        ]
    );
    assert_eq!(graph.heads(), std::slice::from_ref(after.id()));
    assert_eq!(graph.manifest(bridge.id()), Some(&bridge));

    // The bridge itself may be the head.
    let directory = write_history(&[&root, &more, &bridge]);
    let graph = discover(&directory, NEW).expect("bridge head");
    assert_eq!(graph.heads(), std::slice::from_ref(bridge.id()));

    // A bridged history is not on an older workspace profile.
    let error = discover(&directory, OLD).unwrap_err();
    assert_eq!(code(&error), "migration_history_profile_not_workspace");
}

#[test]
fn old_profile_history_requires_an_upgrade_bridge() {
    let [root, more, _] = bridged_history();
    let directory = write_history(&[&root, &more]);
    discover(&directory, OLD).expect("history on its own profile");
    let error = discover(&directory, NEW).unwrap_err();
    assert_eq!(code(&error), "migration_history_profile_upgrade_required");
    assert_eq!(
        error.details().get("hint"),
        Some(&DiagnosticDetailValue::Text(
            "run `migration upgrade-profile --to typedb-3.13.6/v1`".to_owned()
        ))
    );
    assert_eq!(
        error.details().get("history_profile"),
        Some(&DiagnosticDetailValue::Text("typedb-3.12.1/v1".to_owned()))
    );
}

#[test]
fn profile_changes_outside_the_bridge_are_refused() {
    let [root, more, bridge] = bridged_history();

    let unbridged = step_manifest(
        "0003_unbridged",
        vec![more.id().clone()],
        more.target_schema(),
        &["person", "root", "more", "next"],
        NEW,
    );
    let directory = write_history(&[&root, &more, &unbridged]);
    let error = discover(&directory, NEW).unwrap_err();
    assert_eq!(
        code(&error),
        "migration_history_profile_change_without_bridge"
    );

    let second = bridge_manifest("0003_second_upgrade", &more, OLD, NEW).unwrap();
    let error =
        MigrationHistoryGraph::from_verified([root.clone(), more.clone(), bridge.clone(), second])
            .unwrap_err();
    assert_eq!(code(&error), "migration_history_multiple_profile_bridges");

    // An old-profile branch beside the bridge never crossed it.
    let stray = step_manifest(
        "0003_stray",
        vec![more.id().clone()],
        more.target_schema(),
        &["person", "root", "more", "stray"],
        OLD,
    );
    let directory = write_history(&[&root, &more, &bridge, &stray]);
    let error = discover(&directory, NEW).unwrap_err();
    assert_eq!(
        code(&error),
        "migration_history_old_profile_not_bridge_ancestor"
    );

    // A new-profile root beside the bridge is a second lineage.
    let new_root = step_manifest(
        "0009_new_root",
        Vec::new(),
        &declared(&["person"]),
        &["person", "other"],
        NEW,
    );
    let error = MigrationHistoryGraph::from_verified([
        root.clone(),
        more.clone(),
        bridge,
        new_root.clone(),
    ])
    .unwrap_err();
    assert_eq!(code(&error), "migration_history_root_beside_profile_bridge");

    // A bridge must move from its parent's exact profile.
    let misplaced = bridge_manifest("0010_misplaced", &new_root, OLD, NEW).unwrap();
    let error = MigrationHistoryGraph::from_verified([new_root, misplaced]).unwrap_err();
    assert_eq!(
        code(&error),
        "migration_history_profile_bridge_parent_mismatch"
    );
}

fn lowering(profile: MigrationProfile) -> SchemaLoweringBinding {
    SchemaLoweringBinding::for_semantic_profile(
        &profile.semantic_profile_id(),
        migration_runtime_capability_vocabulary().expect("runtime capabilities"),
    )
    .expect("fixture lowering binding")
}

fn plan_code(error: MigrationApplyPlanError) -> String {
    let MigrationApplyPlanError::Contract(diagnostic) = error else {
        panic!("expected a contract refusal, got {error:?}");
    };
    diagnostic.code().as_str().to_owned()
}

/// The bridged history plus `0004_after`, a new-profile step past the bridge.
fn bridged_history_with_descendant() -> [VerifiedSchemaMigrationManifest; 4] {
    let [root, more, bridge] = bridged_history();
    let after = step_manifest(
        "0004_after",
        vec![bridge.id().clone()],
        bridge.target_schema(),
        &["person", "root", "more", "after"],
        NEW,
    );
    [root, more, bridge, after]
}

fn ids(manifests: &[&VerifiedSchemaMigrationManifest]) -> BTreeSet<MigrationId> {
    manifests
        .iter()
        .map(|manifest| manifest.id().clone())
        .collect()
}

#[test]
fn apply_plans_cross_the_bridge_only_forward_from_its_parent() {
    let [root, more, bridge, after] = bridged_history_with_descendant();
    let graph = MigrationHistoryGraph::from_verified([
        root.clone(),
        more.clone(),
        bridge.clone(),
        after.clone(),
    ])
    .expect("bridged graph");
    let policy = MigrationSafetyPolicy::default_policy();

    // From the old head, the plan applies the zero-step bridge then the
    // new-profile step. It starts at the bridge target: the old head's
    // schema observed under the new profile.
    let plan = build_verified_migration_apply_plan(
        &graph,
        &ids(&[&root, &more]),
        &MigrationApplyTarget::DefaultHead,
        &context(NEW),
        &lowering(NEW),
        &policy,
        &[],
    )
    .expect("plan across the bridge");
    let planned = plan
        .migrations()
        .iter()
        .map(|migration| migration.manifest().id().clone())
        .collect::<Vec<_>>();
    assert_eq!(planned, vec![bridge.id().clone(), after.id().clone()]);
    assert!(plan.migrations()[0].steps().is_empty());
    assert!(plan.migrations()[0].transaction_groups().is_empty());
    assert_eq!(plan.source_state(), Some(bridge.target_state()));
    assert_ne!(plan.source_state(), Some(more.target_state()));
    assert_eq!(plan.target_state(), Some(after.target_state()));

    // The bridge alone is a zero-operation plan whose endpoints coincide.
    let bridge_only = build_verified_migration_apply_plan(
        &graph,
        &ids(&[&root, &more]),
        &MigrationApplyTarget::Explicit(ids(&[&bridge])),
        &context(NEW),
        &lowering(NEW),
        &policy,
        &[],
    )
    .expect("bridge-only plan");
    assert_eq!(bridge_only.source_state(), bridge_only.target_state());

    // Past the bridge, planning is wholly on the new profile.
    let past = build_verified_migration_apply_plan(
        &graph,
        &ids(&[&root, &more, &bridge]),
        &MigrationApplyTarget::DefaultHead,
        &context(NEW),
        &lowering(NEW),
        &policy,
        &[],
    )
    .expect("plan past the bridge");
    assert_eq!(past.source_state(), Some(bridge.target_state()));

    // Old-profile manifests never execute on the new line.
    let error = build_verified_migration_apply_plan(
        &graph,
        &ids(&[&root]),
        &MigrationApplyTarget::DefaultHead,
        &context(NEW),
        &lowering(NEW),
        &policy,
        &[],
    )
    .expect_err("an old-profile pending manifest is refused");
    assert_eq!(
        plan_code(error),
        "migration_apply_lowering_profile_mismatch"
    );
}

#[test]
fn rollback_plans_never_cross_the_bridge() {
    let [root, more, bridge, after] = bridged_history_with_descendant();
    let graph = MigrationHistoryGraph::from_verified([
        root.clone(),
        more.clone(),
        bridge.clone(),
        after.clone(),
    ])
    .expect("bridged graph");
    let policy = MigrationSafetyPolicy::default_policy();
    let applied = ids(&[&root, &more, &bridge, &after]);
    let approval = MigrationApplyApproval::for_rollback(&after, SafetyClass::Destructive)
        .expect("rollback approval");

    let plan = build_verified_migration_rollback_plan(
        &graph,
        &applied,
        &ids(&[&after]),
        &context(NEW),
        &lowering(NEW),
        &policy,
        std::slice::from_ref(&approval),
    )
    .expect("a new-profile step past the bridge rolls back");
    assert_eq!(plan.target_state(), bridge.target_state());

    let error = build_verified_migration_rollback_plan(
        &graph,
        &applied,
        &ids(&[&bridge, &after]),
        &context(NEW),
        &lowering(NEW),
        &policy,
        std::slice::from_ref(&approval),
    )
    .expect_err("the bridge is not rolled back");
    assert_eq!(plan_code(error), "migration_rollback_profile_bridge");

    // With the bridge pending, the old head is on a line the workspace left.
    let error = build_verified_migration_rollback_plan(
        &graph,
        &ids(&[&root, &more]),
        &ids(&[&more]),
        &context(NEW),
        &lowering(NEW),
        &policy,
        &[],
    )
    .expect_err("an old-profile manifest is not rolled back on the new line");
    assert_eq!(plan_code(error), "migration_rollback_profile_bridge");
}

fn upgrade_discovery(
    directory: &TempDirectory,
    workspace: MigrationProfile,
) -> Result<MigrationProfileUpgrade, Diagnostic> {
    let directory = MigrationDirectory::open_ambient(directory.path()).expect("fixture directory");
    discover_history_for_profile_upgrade_in(&directory, &declared(&["person"]), &context(workspace))
}

#[test]
fn upgrade_discovery_reports_the_head_to_bridge_or_the_bridge() {
    let [root, more, bridge] = bridged_history();

    let old = write_history(&[&root, &more]);
    let MigrationProfileUpgrade::Pending {
        graph,
        from,
        parent,
    } = upgrade_discovery(&old, NEW).expect("old history under a newer workspace")
    else {
        panic!("an unbridged old history is pending");
    };
    assert_eq!(from, OLD);
    assert_eq!(&parent, more.id());
    assert_eq!(graph.len(), 2);
    // The CLI authors the bridge from exactly these values.
    let authored = bridge_manifest(
        bridge.id().name().as_str(),
        graph.manifest(&parent).expect("parent"),
        from,
        NEW,
    )
    .expect("bridge from the pending discovery");
    assert_eq!(authored, bridge);

    let bridged = write_history(&[&root, &more, &bridge]);
    let MigrationProfileUpgrade::Bridged { bridge: found, .. } =
        upgrade_discovery(&bridged, NEW).expect("bridged history")
    else {
        panic!("a bridged history reports its bridge");
    };
    assert_eq!(&found, bridge.id());

    let error = upgrade_discovery(&old, OLD).expect_err("same profile");
    assert_eq!(code(&error), "migration_upgrade_profile_same_profile");

    let new_root = step_manifest(
        "0001_root",
        Vec::new(),
        &declared(&["person"]),
        &["person", "root"],
        NEW,
    );
    let newer = write_history(&[&new_root]);
    let error = upgrade_discovery(&newer, OLD).expect_err("downgrade");
    assert_eq!(code(&error), "migration_upgrade_profile_downgrade");

    let empty = write_history(&[]);
    let error = upgrade_discovery(&empty, NEW).expect_err("empty history");
    assert_eq!(code(&error), "migration_upgrade_profile_empty_history");

    let sibling = step_manifest(
        "0002_sibling",
        vec![root.id().clone()],
        root.target_schema(),
        &["person", "root", "sibling"],
        OLD,
    );
    let forked = write_history(&[&root, &more, &sibling]);
    let error = upgrade_discovery(&forked, NEW).expect_err("divergent heads");
    assert_eq!(code(&error), "migration_upgrade_profile_divergent_heads");
}
