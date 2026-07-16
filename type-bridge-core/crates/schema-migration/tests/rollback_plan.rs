use std::collections::BTreeSet;

use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration::{
    MigrationAppLabel, MigrationId, MigrationName, MigrationStepId, SchemaDeltaStep,
};
use type_bridge_contract::migration_assertion::migration_assertion_capability_vocabulary;
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, SchemaFact, SourceSpan, SourcedSchemaFact, TypeFact,
};
use type_bridge_schema::{
    BUILTIN_SCHEMA_CAPABILITY_IDS, ManagedDeltaContext, SafetyClass, diff_managed,
    inverse_delta,
};
use type_bridge_schema_migration::{
    MigrationApplyApproval, MigrationApplyPlanError, MigrationHistoryGraph,
    MigrationSafetyPolicy, SafetyPolicyDecision, SchemaLoweringBinding,
    SchemaMigrationDraft, VerifiedSchemaMigrationManifest, build_verified_manifest,
    build_verified_migration_rollback_plan, typedb_3_12_1_profile,
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
                    DocumentId::new("rollback-fixture").expect("fixture document"),
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
    with_reverse: bool,
) -> VerifiedSchemaMigrationManifest {
    let delta = diff_managed(source, target, context).expect("fixture delta");
    let reverse = with_reverse
        .then(|| inverse_delta(&delta).expect("fixture inverse"));
    let step = SchemaDeltaStep::new(
        MigrationStepId::new(format!("step-{name}")).expect("fixture step id"),
        delta,
        reverse,
    )
    .expect("fixture step");
    let draft = SchemaMigrationDraft::new(migration_id(name), parents, vec![step])
        .expect("fixture draft");
    build_verified_manifest(draft, (source, context)).expect("fixture manifest")
}

struct Chain {
    context: ManagedDeltaContext,
    lowering: SchemaLoweringBinding,
    graph: MigrationHistoryGraph,
    first: VerifiedSchemaMigrationManifest,
    second: VerifiedSchemaMigrationManifest,
}

fn two_step_chain() -> Chain {
    let base = declared(&[]);
    let middle = declared(&["person"]);
    let top = declared(&["person", "company"]);
    let context = context();
    let first = manifest("0001_person", Vec::new(), &base, &middle, &context, true);
    let second = manifest(
        "0002_company",
        vec![first.id().clone()],
        &middle,
        &top,
        &context,
        true,
    );
    let graph = MigrationHistoryGraph::from_verified([first.clone(), second.clone()])
        .expect("history");
    let lowering =
        SchemaLoweringBinding::current(context.available_capabilities().clone())
            .expect("lowering");
    Chain {
        context,
        lowering,
        graph,
        first,
        second,
    }
}

fn applied_both(chain: &Chain) -> BTreeSet<MigrationId> {
    BTreeSet::from([chain.first.id().clone(), chain.second.id().clone()])
}

#[test]
fn rollback_requires_an_approval_bound_to_the_reverse_transition() {
    let chain = two_step_chain();
    let applied = applied_both(&chain);
    let removals = BTreeSet::from([chain.second.id().clone()]);
    let policy = MigrationSafetyPolicy::default_policy();
    let build = |approvals: &[MigrationApplyApproval]| {
        build_verified_migration_rollback_plan(
            &chain.graph,
            &applied,
            &removals,
            &chain.context,
            &chain.lowering,
            &policy,
            approvals,
        )
    };

    // Rolling back the additive company migration destroys the company type.
    let MigrationApplyPlanError::Contract(unapproved) =
        build(&[]).expect_err("destructive reverse work requires approval")
    else {
        panic!("missing approval must surface as a contract diagnostic");
    };
    assert_eq!(
        unapproved.code().as_str(),
        "migration_rollback_approval_required"
    );

    // A forward approval never authorizes the reverse transition.
    let forward =
        MigrationApplyApproval::for_manifest(&chain.second).expect("forward approval");
    assert!(build(&[forward]).is_err());

    let approval =
        MigrationApplyApproval::for_rollback(&chain.second, SafetyClass::Destructive)
            .expect("rollback approval");
    assert!(
        approval
            .binds_rollback(&chain.second, SafetyClass::Destructive)
            .expect("binding check")
    );
    let plan = build(std::slice::from_ref(&approval)).expect("approved rollback plan");
    assert_eq!(plan.rollbacks().len(), 1);
    let rolled = &plan.rollbacks()[0];
    assert_eq!(rolled.manifest().id(), chain.second.id());
    assert_eq!(rolled.rollback_safety(), SafetyClass::Destructive);
    let statements = rolled.steps()[0]
        .lowering()
        .units()
        .iter()
        .flat_map(|unit| unit.statements())
        .map(|statement| statement.query().to_owned())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(statements.contains("undefine"), "statements: {statements}");
    assert!(statements.contains("company"), "statements: {statements}");
    assert_eq!(plan.remaining_applied(), &[chain.first.id().clone()]);
    assert_eq!(plan.source_state(), chain.second.target_state());
    assert_eq!(plan.target_state(), chain.second.source_state());
}

#[test]
fn rollback_rejects_targets_with_remaining_applied_descendants() {
    let chain = two_step_chain();
    let applied = applied_both(&chain);
    let removals = BTreeSet::from([chain.first.id().clone()]);
    let error = build_verified_migration_rollback_plan(
        &chain.graph,
        &applied,
        &removals,
        &chain.context,
        &chain.lowering,
        &MigrationSafetyPolicy::default_policy(),
        &[],
    )
    .expect_err("an applied descendant must block its ancestor's rollback");
    let MigrationApplyPlanError::Contract(diagnostic) = error else {
        panic!("descendant rejection must surface as a contract diagnostic");
    };
    assert_eq!(
        diagnostic.code().as_str(),
        "migration_history_remaining_applied_descendant"
    );
}

#[test]
fn irreversible_manifests_stay_manually_reversible() {
    let base = declared(&[]);
    let top = declared(&["person"]);
    let context = context();
    let irreversible =
        manifest("0001_person", Vec::new(), &base, &top, &context, false);
    let applied = BTreeSet::from([irreversible.id().clone()]);
    let removals = applied.clone();
    let graph =
        MigrationHistoryGraph::from_verified([irreversible]).expect("history");
    let lowering =
        SchemaLoweringBinding::current(context.available_capabilities().clone())
            .expect("lowering");
    let approval_free = build_verified_migration_rollback_plan(
        &graph,
        &applied,
        &removals,
        &context,
        &lowering,
        &MigrationSafetyPolicy::default_policy(),
        &[],
    )
    .expect_err("a manifest without a reverse program cannot roll back");
    let MigrationApplyPlanError::Contract(diagnostic) = approval_free else {
        panic!("irreversible rejection must surface as a contract diagnostic");
    };
    assert_eq!(diagnostic.code().as_str(), "migration_rollback_irreversible");
}

#[test]
fn full_chain_rollback_is_deterministic_and_reverse_topological() {
    let chain = two_step_chain();
    let applied = applied_both(&chain);
    let removals = applied.clone();
    let approvals = [
        MigrationApplyApproval::for_rollback(&chain.first, SafetyClass::Destructive)
            .expect("first approval"),
        MigrationApplyApproval::for_rollback(&chain.second, SafetyClass::Destructive)
            .expect("second approval"),
    ];
    let build = || {
        build_verified_migration_rollback_plan(
            &chain.graph,
            &applied,
            &removals,
            &chain.context,
            &chain.lowering,
            &MigrationSafetyPolicy::default_policy(),
            &approvals,
        )
        .expect("full rollback plan")
    };
    let plan = build();
    assert_eq!(plan, build());
    assert_eq!(
        plan.rollbacks()
            .iter()
            .map(|rollback| rollback.manifest().id().clone())
            .collect::<Vec<_>>(),
        vec![chain.second.id().clone(), chain.first.id().clone()],
    );
    assert!(plan.remaining_applied().is_empty());
    assert_eq!(
        plan.target_schema().declared_identity_fingerprint(),
        declared(&[]).declared_identity_fingerprint(),
    );
}

#[test]
fn rejecting_policy_wins_over_a_valid_rollback_approval() {
    let chain = two_step_chain();
    let applied = applied_both(&chain);
    let removals = BTreeSet::from([chain.second.id().clone()]);
    let approval =
        MigrationApplyApproval::for_rollback(&chain.second, SafetyClass::Destructive)
            .expect("rollback approval");
    let policy = MigrationSafetyPolicy::default_policy()
        .with_decision(SafetyClass::Destructive, SafetyPolicyDecision::Reject)
        .expect("rejecting policy");
    let MigrationApplyPlanError::Contract(rejected) =
        build_verified_migration_rollback_plan(
            &chain.graph,
            &applied,
            &removals,
            &chain.context,
            &chain.lowering,
            &policy,
            &[approval],
        )
        .expect_err("a rejecting policy ignores rollback approvals")
    else {
        panic!("policy rejection must surface as a contract diagnostic");
    };
    assert_eq!(
        rejected.code().as_str(),
        "migration_rollback_safety_policy_rejected"
    );
}
