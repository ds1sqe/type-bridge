mod common;

use std::collections::BTreeSet;
use std::sync::Mutex;

use common::{CoordinatorProvider, CoordinatorStore, block_on};
use type_bridge_contract::capability::{CapabilityId, CapabilitySet};
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::managed_scope::{
    ManagedScopeId, SemanticProfileBinding,
};
use type_bridge_contract::migration::{
    CONDITIONAL_RESOLUTION_CAPABILITY, MigrationAppLabel, MigrationId,
    MigrationName, MigrationStep, MigrationStepId, SchemaDeltaStep,
};
use type_bridge_contract::migration_assertion::AssertionExpectation;
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId,
    DeclaredSchema, DocumentId, SchemaAnnotationValue, SchemaFact, SourceSpan,
    SourcedSchemaFact, SubFact, SubFactId, TypeFact,
};
use type_bridge_query::{
    MigrationAssertionValidationContext, lower_condition_to_plan,
};
use type_bridge_schema::{
    ManagedDeltaContext, SafetyClass, SafetyDerivationProfile,
    derive_safety_conditions, diff_managed, inverse_delta, managed_schema_state,
    resolve,
};
use type_bridge_schema_migration::{
    AppliedRecord, ExecutionFence, ExecutionScope, GroupEventRecord,
    GroupJournalEventKind, JournalEntry, JournalSequence, LeaseHolderId,
    MigrationApplyApproval, MigrationApplyPlanError, MigrationApplyTarget,
    MigrationExecutionOutcome, MigrationHistoryGraph, MigrationLease,
    MigrationSafetyPolicy, PlanRecord, SafetyPolicyDecision,
    SchemaLoweringBinding, SchemaMigrationDraft, StatementUnit,
    VerifiedMigrationApplyStep, build_verified_manifest,
    build_verified_migration_apply_plan, execute_verified_migration_apply_plan,
    schema_lowering_profile_binding, typedb_3_12_1_profile,
};

fn type_fact(label: &str) -> SchemaFact {
    SchemaFact::Type(
        TypeFact::new(TypeId::new(TypeKind::Entity, label).expect("fixture type"))
            .expect("fixture type fact"),
    )
}

fn declared(labels: &[&str]) -> DeclaredSchema {
    let facts = labels.iter().enumerate().map(|(index, label)| {
        let offset = u64::try_from(index).expect("fixture offset");
        let line = u32::try_from(index + 1).expect("fixture line");
        SourcedSchemaFact::new(
            type_fact(label),
            SourceSpan::new(
                DocumentId::new("apply-plan-fixture").expect("document"),
                offset,
                offset + 1,
                line,
                1,
                line,
                2,
            )
            .expect("source span"),
        )
    });
    DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), facts)
        .expect("declared schema")
}

fn declared_facts(facts: Vec<SchemaFact>) -> DeclaredSchema {
    let sourced = facts.into_iter().enumerate().map(|(index, fact)| {
        let offset = u64::try_from(index).expect("fixture offset");
        let line = u32::try_from(index + 1).expect("fixture line");
        SourcedSchemaFact::new(
            fact,
            SourceSpan::new(
                DocumentId::new("apply-plan-assertion-fixture").expect("document"),
                offset,
                offset + 1,
                line,
                1,
                line,
                2,
            )
            .expect("source span"),
        )
    });
    DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), sourced)
        .expect("declared schema")
}

fn abstract_fact(label: &str) -> SchemaFact {
    let id = TypeId::new(TypeKind::Entity, label).expect("fixture type");
    SchemaFact::Annotation(
        AnnotationFact::new(
            AnnotationFactId::new(
                AnnotationSubjectId::Type(id),
                AnnotationKindId::Abstract,
            ),
            SchemaAnnotationValue::Presence,
        )
        .expect("abstract annotation"),
    )
}

fn context() -> ManagedDeltaContext {
    ManagedDeltaContext::new(
        ManagedScopeId::new("example-schema").expect("scope"),
        SemanticProfileId::new("typedb-3.12.1/v1").expect("profile"),
        typedb_3_12_1_profile().required_capabilities.clone(),
    )
}

fn assertion_context() -> ManagedDeltaContext {
    let mut available = typedb_3_12_1_profile().required_capabilities.clone();
    for capability in [
        CONDITIONAL_RESOLUTION_CAPABILITY,
        "query.migration-assertion",
        "query.pattern.has",
        "query.pattern.isa",
        "query.pattern.isa-subtypes",
        "query.pattern.negation",
        "query.pattern.value",
    ] {
        available.insert(
            CapabilityId::new(capability).expect("fixture assertion capability"),
        );
    }
    ManagedDeltaContext::new(
        ManagedScopeId::new("example-schema").expect("scope"),
        SemanticProfileId::new("typedb-3.12.1/v1").expect("profile"),
        available,
    )
}

fn migration_id(name: &str) -> MigrationId {
    MigrationId::from_components(
        MigrationAppLabel::new("example").expect("app"),
        MigrationName::new(name).expect("name"),
    )
}

fn manifest(
    name: &str,
    parents: Vec<MigrationId>,
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    context: &ManagedDeltaContext,
) -> type_bridge_schema_migration::VerifiedSchemaMigrationManifest {
    let delta = diff_managed(source, target, context).expect("delta");
    let reverse = inverse_delta(&delta).expect("inverse");
    let step = SchemaDeltaStep::new(
        MigrationStepId::new(format!("step-{name}")).expect("step id"),
        delta,
        Some(reverse),
    )
    .expect("schema step");
    let draft = SchemaMigrationDraft::new(migration_id(name), parents, vec![step])
        .expect("draft");
    build_verified_manifest(draft, (source, context)).expect("verified manifest")
}

fn derived_assertion_step(
    source: &DeclaredSchema,
    target: &DeclaredSchema,
    context: &ManagedDeltaContext,
    delta: &type_bridge_contract::schema::SchemaDelta,
) -> MigrationStep {
    let safety_profile = SafetyDerivationProfile::new(
        SemanticProfileBinding::resolve(context.semantic_profile().clone())
            .expect("semantic profile binding"),
        schema_lowering_profile_binding().expect("lowering profile binding"),
    )
    .expect("safety profile");
    let derived = derive_safety_conditions(
        0,
        &delta.operations()[0],
        source,
        target,
        &safety_profile,
    )
    .expect("conditional safety condition");
    assert_eq!(derived.conditions().len(), 1);
    let resolved = resolve(source, context.semantic_profile())
        .expect("resolved assertion source");
    let source_state =
        managed_schema_state(source, context).expect("managed assertion source");
    let validation_context =
        MigrationAssertionValidationContext::new(&resolved, &source_state);
    let validated = lower_condition_to_plan(
        &derived.conditions()[0],
        &validation_context,
        StructuralLimits::CANONICAL,
    )
    .expect("derived assertion plan");
    MigrationStep::assertion(
        MigrationStepId::new("assert-person-empty").expect("assertion step ID"),
        validated.plan().clone(),
        AssertionExpectation::NoRows,
    )
    .expect("assertion step")
}

fn additive_policy() -> MigrationSafetyPolicy {
    MigrationSafetyPolicy::default_policy()
        .with_decision(SafetyClass::Conditional, SafetyPolicyDecision::Reject)
        .expect("additive-only policy")
}

#[test]
fn linear_apply_plan_is_deterministic_relowered_and_frontier_bound() {
    let base = declared(&["person"]);
    let middle = declared(&["person", "company"]);
    let target = declared(&["person", "company", "team"]);
    let context = context();
    let first = manifest("0001_company", Vec::new(), &base, &middle, &context);
    let second = manifest(
        "0002_team",
        vec![first.id().clone()],
        &middle,
        &target,
        &context,
    );
    let graph = MigrationHistoryGraph::from_verified([first.clone(), second.clone()])
        .expect("history");
    let lowering = SchemaLoweringBinding::current(context.available_capabilities().clone())
        .expect("lowering");
    let build = || {
        build_verified_migration_apply_plan(
            &graph,
            &BTreeSet::new(),
            &MigrationApplyTarget::DefaultHead,
            &context,
            &lowering,
            &additive_policy(),
            &[],
        )
        .expect("apply plan")
    };
    let plan = build();
    assert_eq!(plan, build());
    assert!(plan.applied_frontier().is_empty());
    assert_eq!(plan.target_frontier(), &[second.id().clone()]);
    assert_eq!(
        plan.source_schema()
            .expect("source schema")
            .declared_identity_fingerprint(),
        base.declared_identity_fingerprint(),
    );
    assert_eq!(
        plan.target_schema()
            .expect("target schema")
            .declared_identity_fingerprint(),
        target.declared_identity_fingerprint(),
    );
    assert_eq!(plan.migrations().len(), 2);
    assert!(plan.migrations().iter().all(|entry| {
        matches!(
            entry.steps(),
            [VerifiedMigrationApplyStep::SchemaDelta { lowering, .. }]
                if !lowering.units().is_empty()
        )
    }));

    let applied = BTreeSet::from([first.id().clone()]);
    let remaining = build_verified_migration_apply_plan(
        &graph,
        &applied,
        &MigrationApplyTarget::Explicit(BTreeSet::from([second.id().clone()])),
        &context,
        &lowering,
        &additive_policy(),
        &[],
    )
    .expect("remaining plan");
    assert_eq!(remaining.applied_frontier(), &[first.id().clone()]);
    assert_eq!(remaining.migrations().len(), 1);
    assert_eq!(
        remaining
            .source_schema()
            .expect("remaining source")
            .declared_identity_fingerprint(),
        middle.declared_identity_fingerprint(),
    );
    assert_eq!(
        remaining
            .target_schema()
            .expect("remaining target")
            .declared_identity_fingerprint(),
        target.declared_identity_fingerprint(),
    );
}

#[test]
fn coordinator_stale_gate_uses_the_full_applied_set_not_only_graph_heads() {
    let base = declared(&["person"]);
    let first_target = declared(&["person", "company"]);
    let second_target = declared(&["person", "company", "team"]);
    let final_target = declared(&["person", "company", "team", "project"]);
    let context = context();
    let first = manifest(
        "0001_company",
        Vec::new(),
        &base,
        &first_target,
        &context,
    );
    let second = manifest(
        "0002_team",
        vec![first.id().clone()],
        &first_target,
        &second_target,
        &context,
    );
    let third = manifest(
        "0003_project",
        vec![second.id().clone()],
        &second_target,
        &final_target,
        &context,
    );
    let graph = MigrationHistoryGraph::from_verified([
        first.clone(),
        second.clone(),
        third.clone(),
    ])
    .expect("history");
    let lowering = SchemaLoweringBinding::current(
        context.available_capabilities().clone(),
    )
    .expect("lowering");
    let complete = build_verified_migration_apply_plan(
        &graph,
        &BTreeSet::new(),
        &MigrationApplyTarget::DefaultHead,
        &context,
        &lowering,
        &additive_policy(),
        &[],
    )
    .expect("complete plan");
    let already_applied = BTreeSet::from([first.id().clone(), second.id().clone()]);
    let remaining = build_verified_migration_apply_plan(
        &graph,
        &already_applied,
        &MigrationApplyTarget::DefaultHead,
        &context,
        &lowering,
        &additive_policy(),
        &[],
    )
    .expect("remaining plan");
    assert_eq!(remaining.applied_migrations().len(), 2);
    assert_eq!(remaining.applied_frontier(), &[second.id().clone()]);

    let store = CoordinatorStore::default();
    let seed_lease = MigrationLease::new(
        ExecutionScope::new(
            remaining.source_state().expect("source").scope().id().clone(),
        ),
        LeaseHolderId::new("seed-ledger").expect("holder"),
        ExecutionFence::new(1).expect("fence"),
    );
    {
        let mut state = store.state.lock().expect("coordinator store");
        state.fence = 1;
        for (index, migration) in complete.migrations()[..2].iter().enumerate() {
            let record = AppliedRecord::from_verified_manifest(&seed_lease, migration)
                .expect("seed applied record");
            let sequence = JournalSequence::new(
                u64::try_from(index + 1).expect("sequence"),
            )
            .expect("sequence");
            state.applied.push(JournalEntry::from_store(sequence, record));
            state.next_sequence = sequence.get();
        }
    }
    let provider = CoordinatorProvider {
        available: context.available_capabilities().clone(),
        calls: Mutex::new(Vec::new()),
        observed: Mutex::new(remaining.source_state().expect("source").clone()),
    };
    let outcome = block_on(execute_verified_migration_apply_plan(
        &store,
        &provider,
        &LeaseHolderId::new("coordinator-applier").expect("holder"),
        &remaining,
    ))
    .expect("execute remaining migration");
    assert!(matches!(outcome, MigrationExecutionOutcome::Applied));
    let state = store.state.lock().expect("coordinator store");
    assert_eq!(state.applied.len(), 3);
    assert_eq!(state.releases, 1);
}

#[test]
fn assertion_apply_step_retains_validated_plan_bound_to_exact_source_state() {
    let source = declared_facts(vec![type_fact("person")]);
    let middle = declared_facts(vec![type_fact("person"), type_fact("company")]);
    let target = declared_facts(vec![
        type_fact("person"),
        type_fact("company"),
        abstract_fact("person"),
    ]);
    let context = assertion_context();
    let additive_delta = diff_managed(&source, &middle, &context).expect("additive delta");
    let additive_reverse = inverse_delta(&additive_delta).expect("additive inverse");
    let additive_step = SchemaDeltaStep::new(
        MigrationStepId::new("add-company").expect("schema step ID"),
        additive_delta,
        Some(additive_reverse),
    )
    .expect("additive schema step");
    let conditional_delta =
        diff_managed(&middle, &target, &context).expect("conditional delta");
    let assertion =
        derived_assertion_step(&middle, &target, &context, &conditional_delta);
    let conditional_source_state = conditional_delta.source().clone();
    let conditional_reverse = inverse_delta(&conditional_delta).expect("conditional inverse");
    let conditional_step = SchemaDeltaStep::new(
        MigrationStepId::new("make-person-abstract").expect("schema step ID"),
        conditional_delta,
        Some(conditional_reverse),
    )
    .expect("conditional schema step");
    let draft = SchemaMigrationDraft::new(
        migration_id("0001_person_abstract"),
        Vec::new(),
        vec![
            MigrationStep::from(additive_step),
            assertion,
            MigrationStep::from(conditional_step),
        ],
    )
    .expect("migration draft");
    let manifest =
        build_verified_manifest(draft, (&source, &context)).expect("verified manifest");
    let graph = MigrationHistoryGraph::from_verified([manifest.clone()])
        .expect("history graph");
    let lowering =
        SchemaLoweringBinding::current(context.available_capabilities().clone())
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
    .expect("verified apply plan");
    let apply_manifest = &plan.migrations()[0];
    let steps = apply_manifest.steps();
    assert_eq!(steps.len(), 3);
    let groups = apply_manifest.transaction_groups();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].ordinal(), 0);
    assert_eq!(groups[0].first_step_index(), 0);
    assert_eq!(groups[0].schema_delta_step_index(), 0);
    assert_eq!(groups[0].assertion_count(), 0);
    assert_eq!(groups[0].end_step_index(), 1);
    assert_eq!(groups[1].ordinal(), 1);
    assert_eq!(groups[1].first_step_index(), 1);
    assert_eq!(groups[1].schema_delta_step_index(), 2);
    assert_eq!(groups[1].assertion_count(), 1);
    assert_eq!(groups[1].end_step_index(), 3);
    let VerifiedMigrationApplyStep::Assertion { step, validated } = &steps[1] else {
        panic!("second positional step must be an execution-ready assertion");
    };
    let (contract, persisted_plan, expectation) =
        step.as_assertion().expect("assertion contract");
    assert_eq!(expectation, AssertionExpectation::NoRows);
    assert_eq!(validated.source_state(), &conditional_source_state);
    assert_ne!(validated.source_state(), manifest.source_state());
    assert_eq!(
        validated.source_state().declared_identity(),
        middle.declared_identity_fingerprint(),
    );
    assert_eq!(
        validated.source_state().managed_semantic_schema(),
        contract.source_semantics(),
    );
    assert_eq!(contract.source_semantics(), contract.target_semantics());
    assert_eq!(
        validated.plan().fingerprint().expect("validated fingerprint"),
        contract.plan_fingerprint().clone(),
    );
    assert_eq!(
        validated.plan().canonical_bytes().expect("validated bytes"),
        persisted_plan.canonical_bytes().expect("persisted bytes"),
    );
    assert_eq!(validated.structural_limits(), StructuralLimits::CANONICAL);
    assert!(matches!(&steps[0], VerifiedMigrationApplyStep::SchemaDelta { .. }));
    assert!(matches!(&steps[2], VerifiedMigrationApplyStep::SchemaDelta { .. }));

    let lease = MigrationLease::new(
        ExecutionScope::new(
            plan.source_state()
                .expect("plan source state")
                .scope()
                .id()
                .clone(),
        ),
        LeaseHolderId::new("apply-plan-test").expect("lease holder"),
        ExecutionFence::new(1).expect("fence"),
    );
    let record = PlanRecord::from_verified_plan(
        &lease,
        &plan,
        plan.applied_migrations(),
        plan.source_state().expect("plan source state"),
    )
    .expect("plan record");
    assert_eq!(record.manifest_digests(), &[apply_manifest.digest()]);
    assert_eq!(record.migration_ids(), &[manifest.id().clone()]);
    assert!(PlanRecord::from_verified_plan(
        &lease,
        &plan,
        &[migration_id("stale_frontier")],
        plan.source_state().expect("plan source state"),
    )
    .is_err());
    assert!(PlanRecord::from_verified_plan(
        &lease,
        &plan,
        plan.applied_migrations(),
        plan.target_state().expect("plan target state"),
    )
    .is_err());

    let first_delta = steps[groups[0].schema_delta_step_index()]
        .step()
        .as_schema_delta()
        .expect("first group delta")
        .delta();
    let before = GroupEventRecord::new(
        &lease,
        apply_manifest,
        &groups[0],
        GroupJournalEventKind::BeforeCommit,
        None,
    )
    .expect("before-commit event");
    assert_eq!(before.first_step_index(), 0);
    assert_eq!(before.end_step_index(), 1);
    assert!(GroupEventRecord::new(
        &lease,
        apply_manifest,
        &groups[0],
        GroupJournalEventKind::Committed,
        None,
    )
    .is_err());
    let committed = GroupEventRecord::new(
        &lease,
        apply_manifest,
        &groups[0],
        GroupJournalEventKind::Committed,
        Some(first_delta.target().managed_semantic_schema().clone()),
    )
    .expect("committed event");
    assert_eq!(
        committed.observed_target(),
        Some(first_delta.target().managed_semantic_schema()),
    );
    let applied = AppliedRecord::from_verified_manifest(&lease, apply_manifest)
        .expect("applied record");
    assert_eq!(applied.migration_id(), manifest.id());
    assert_eq!(applied.manifest_digest(), apply_manifest.digest());

    let store = CoordinatorStore::default();
    let provider = CoordinatorProvider {
        available: context.available_capabilities().clone(),
        calls: Mutex::new(Vec::new()),
        observed: Mutex::new(plan.source_state().expect("source state").clone()),
    };
    let outcome = block_on(execute_verified_migration_apply_plan(
        &store,
        &provider,
        &LeaseHolderId::new("coordinator-test").expect("holder"),
        &plan,
    ))
    .expect("coordinator execution");
    assert!(matches!(outcome, MigrationExecutionOutcome::Applied));
    let calls = provider.calls.lock().expect("provider calls").clone();
    assert_eq!(calls.iter().filter(|call| **call == "prepare").count(), 2);
    assert_eq!(calls.iter().filter(|call| **call == "commit").count(), 2);
    let assertion_position = calls.iter().position(|call| *call == "assertion")
        .expect("assertion call");
    let second_prepare = calls.iter().enumerate()
        .filter(|(_, call)| **call == "prepare")
        .nth(1)
        .map(|(index, _)| index)
        .expect("second prepare");
    let later_statement = calls.iter().enumerate()
        .find(|(index, call)| *index > assertion_position && **call == "statement")
        .map(|(index, _)| index)
        .expect("statement after assertion");
    assert!(second_prepare < assertion_position);
    assert!(assertion_position < later_statement);
    let state = store.state.lock().expect("coordinator store");
    assert_eq!(
        state.event_audit,
        vec![
            GroupJournalEventKind::BeforeCommit,
            GroupJournalEventKind::Committed,
            GroupJournalEventKind::BeforeCommit,
            GroupJournalEventKind::Committed,
        ],
    );
    assert_eq!(state.applied.len(), 1);
    assert!(state.open.is_none());
    assert!(state.active.is_none());
    assert_eq!(state.releases, 1);
}

#[test]
fn explicit_safety_policy_rejects_before_execution_evidence_is_returned() {
    let base = declared(&["person"]);
    let target = declared(&["person", "company"]);
    let context = context();
    let migration = manifest("0001_company", Vec::new(), &base, &target, &context);
    let graph = MigrationHistoryGraph::from_verified([migration]).expect("history");
    let lowering = SchemaLoweringBinding::current(context.available_capabilities().clone())
        .expect("lowering");
    assert!(build_verified_migration_apply_plan(
        &graph,
        &BTreeSet::new(),
        &MigrationApplyTarget::DefaultHead,
        &context,
        &lowering,
        &MigrationSafetyPolicy::default_policy()
            .with_decision(SafetyClass::Additive, SafetyPolicyDecision::Reject)
            .expect("policy rejecting additive"),
        &[],
    )
    .is_err());
}

#[test]
fn divergent_branch_sources_are_not_guessed_into_one_apply_chain() {
    let base = declared(&["person"]);
    let left = declared(&["person", "company"]);
    let right = declared(&["person", "team"]);
    let context = context();
    let first = manifest("0001_left", Vec::new(), &base, &left, &context);
    let second = manifest("0002_right", Vec::new(), &base, &right, &context);
    let targets = BTreeSet::from([first.id().clone(), second.id().clone()]);
    let graph = MigrationHistoryGraph::from_verified([first, second]).expect("history");
    let lowering = SchemaLoweringBinding::current(context.available_capabilities().clone())
        .expect("lowering");
    assert!(build_verified_migration_apply_plan(
        &graph,
        &BTreeSet::new(),
        &MigrationApplyTarget::Explicit(targets),
        &context,
        &lowering,
        &additive_policy(),
        &[],
    )
    .is_err());
}

#[test]
fn new_subtype_migration_lowers_without_assertion_coverage() {
    // A subtype introduced by its own migration derives zero safety
    // conditions, so the conditional lowering gate is discharged by proof:
    // the plan builds with no assertion steps and renders the sub edge.
    let context = context();
    let genesis = declared_facts(vec![type_fact("person")]);
    let person = TypeId::new(TypeKind::Entity, "person").expect("person type");
    let employee = TypeId::new(TypeKind::Entity, "employee").expect("employee type");
    let target = declared_facts(vec![
        type_fact("employee"),
        type_fact("person"),
        SchemaFact::Sub(SubFact::new(
            SubFactId::new(employee, person).expect("sub identity"),
        )),
    ]);
    let verified = manifest(
        "0001_employee_sub_person",
        Vec::new(),
        &genesis,
        &target,
        &context,
    );
    let graph = MigrationHistoryGraph::from_verified([verified])
        .expect("verified history");
    let lowering = SchemaLoweringBinding::current(
        context.available_capabilities().clone(),
    )
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
    .expect("a proven condition-free subtype transition needs no assertion");

    let steps = plan.migrations()[0].steps();
    assert!(
        steps
            .iter()
            .all(|step| !matches!(step, VerifiedMigrationApplyStep::Assertion { .. })),
        "no assertion evidence may be required"
    );
    let rendered_sub = steps.iter().any(|step| match step {
        VerifiedMigrationApplyStep::SchemaDelta { lowering, .. } => lowering
            .units()
            .iter()
            .flat_map(StatementUnit::statements)
            .any(|statement| statement.query().contains("sub person")),
        VerifiedMigrationApplyStep::Assertion { .. } => false,
    });
    assert!(rendered_sub, "lowered statements must define the sub edge");
}

#[test]
fn destructive_manifest_requires_an_identity_bound_approval() {
    let base = declared(&["person", "company"]);
    let target = declared(&["person"]);
    let context = context();
    let migration = manifest("0001_drop_company", Vec::new(), &base, &target, &context);
    assert_eq!(migration.safety(), SafetyClass::Destructive);
    let graph =
        MigrationHistoryGraph::from_verified([migration.clone()]).expect("history");
    let lowering = SchemaLoweringBinding::current(context.available_capabilities().clone())
        .expect("lowering");
    let policy = MigrationSafetyPolicy::default_policy();
    let build = |approvals: &[MigrationApplyApproval]| {
        build_verified_migration_apply_plan(
            &graph,
            &BTreeSet::new(),
            &MigrationApplyTarget::DefaultHead,
            &context,
            &lowering,
            &policy,
            approvals,
        )
    };

    let MigrationApplyPlanError::Contract(unapproved) =
        build(&[]).expect_err("destructive work requires approval")
    else {
        panic!("missing approval must surface as a contract diagnostic");
    };
    assert_eq!(unapproved.code().as_str(), "migration_apply_approval_required");

    // An approval bound to a different verified transition never matches.
    let other = manifest(
        "0002_other",
        Vec::new(),
        &declared(&["person"]),
        &declared(&["person", "team"]),
        &context,
    );
    let foreign =
        MigrationApplyApproval::for_manifest(&other).expect("foreign approval");
    let MigrationApplyPlanError::Contract(still_unapproved) =
        build(&[foreign]).expect_err("a foreign approval must not match")
    else {
        panic!("foreign approval must surface as a contract diagnostic");
    };
    assert_eq!(
        still_unapproved.code().as_str(),
        "migration_apply_approval_required"
    );

    // The exact binding admits the manifest and its destructive lowering.
    let approval =
        MigrationApplyApproval::for_manifest(&migration).expect("bound approval");
    assert!(approval.binds(&migration).expect("binding check"));
    let plan = build(std::slice::from_ref(&approval)).expect("approved plan");
    let rendered_undefine = plan.migrations()[0].steps().iter().any(|step| match step {
        VerifiedMigrationApplyStep::SchemaDelta { lowering, .. } => lowering
            .units()
            .iter()
            .flat_map(StatementUnit::statements)
            .any(|statement| statement.query().contains("undefine")),
        VerifiedMigrationApplyStep::Assertion { .. } => false,
    });
    assert!(rendered_undefine, "approved destructive work must lower");
}

#[test]
fn destructive_reject_policy_wins_over_a_valid_approval() {
    let base = declared(&["person", "company"]);
    let target = declared(&["person"]);
    let context = context();
    let migration = manifest("0001_drop_company", Vec::new(), &base, &target, &context);
    let approval =
        MigrationApplyApproval::for_manifest(&migration).expect("bound approval");
    let graph = MigrationHistoryGraph::from_verified([migration]).expect("history");
    let lowering = SchemaLoweringBinding::current(context.available_capabilities().clone())
        .expect("lowering");
    let policy = MigrationSafetyPolicy::default_policy()
        .with_decision(SafetyClass::Destructive, SafetyPolicyDecision::Reject)
        .expect("rejecting policy");
    let MigrationApplyPlanError::Contract(rejected) = build_verified_migration_apply_plan(
        &graph,
        &BTreeSet::new(),
        &MigrationApplyTarget::DefaultHead,
        &context,
        &lowering,
        &policy,
        &[approval],
    )
    .expect_err("a rejecting policy ignores approvals") else {
        panic!("policy rejection must surface as a contract diagnostic");
    };
    assert_eq!(
        rejected.code().as_str(),
        "migration_apply_safety_policy_rejected"
    );
}
