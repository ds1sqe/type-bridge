#[allow(dead_code)]
mod common;
use common::{CoordinatorProvider, CoordinatorStore, block_on, test_diagnostic};
use std::collections::BTreeSet;
use std::sync::Mutex;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::managed_scope::ManagedScopeId;
use type_bridge_contract::migration::{MigrationId, MigrationStep, MigrationStepId};
use type_bridge_contract::migration_data::{DataProgram, DataStep};
use type_bridge_contract::schema::{DeclaredSchema, ManagedSchemaState};
use type_bridge_schema::{ManagedDeltaContext, SafetyClass};
use type_bridge_schema_migration::*;

fn fixture(
    reverse: bool,
) -> (
    ManagedDeltaContext,
    MigrationHistoryGraph,
    VerifiedMigrationApplyPlan,
) {
    let context = ManagedDeltaContext::new(
        ManagedScopeId::new("data-test").unwrap(),
        SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
        typedb_3_12_1_profile().required_capabilities.clone(),
    );
    let schema = DeclaredSchema::from_facts(FormatVersion::V1, CapabilitySet::new(), []).unwrap();
    let step = DataStep::new(
        MigrationStepId::new("derive").unwrap(),
        DataProgram::Typeql {
            text: "insert $p isa person;".into(),
        },
        reverse.then(|| DataProgram::Typeql {
            text: "match $p isa person; delete $p;".into(),
        }),
    )
    .unwrap();
    let manifest = build_verified_manifest(
        SchemaMigrationDraft::new(
            MigrationId::new("example", "0001_data").unwrap(),
            vec![],
            vec![MigrationStep::Data(step)],
        )
        .unwrap(),
        (&schema, &context),
    )
    .unwrap();
    let encoded = encode_verified_manifest(&manifest).unwrap();
    assert_eq!(
        decode_verified_manifest(&encoded, (&schema, &context)).unwrap(),
        manifest
    );
    let approval = MigrationApplyApproval::for_manifest(&manifest).unwrap();
    let graph = MigrationHistoryGraph::from_verified([manifest]).unwrap();
    let plan = build_verified_migration_apply_plan(
        &graph,
        &BTreeSet::new(),
        &MigrationApplyTarget::DefaultHead,
        &context,
        &SchemaLoweringBinding::for_context(&context).unwrap(),
        &MigrationSafetyPolicy::default_policy(),
        &[approval],
    )
    .unwrap();
    (context, graph, plan)
}
struct AtomicProvider {
    base: CoordinatorProvider,
    marker: Mutex<bool>,
    calls: Mutex<usize>,
    failure: Mutex<Option<GroupCommitCertainty>>,
    registered: bool,
}
impl AtomicProvider {
    fn new(context: &ManagedDeltaContext, plan: &VerifiedMigrationApplyPlan) -> Self {
        Self {
            base: CoordinatorProvider {
                available: context.available_capabilities().clone(),
                calls: Mutex::new(vec![]),
                observed: Mutex::new(plan.source_state().unwrap().clone()),
            },
            marker: Mutex::new(false),
            calls: Mutex::new(0),
            failure: Mutex::new(None),
            registered: true,
        }
    }
}
impl MigrationExecutionProvider for AtomicProvider {
    fn available_capabilities(&self) -> &CapabilitySet {
        self.base.available_capabilities()
    }
    fn observe_managed_state<'a>(
        &'a self,
        lease: &'a MigrationLease,
        source: &'a ManagedSchemaState,
        target: &'a ManagedSchemaState,
    ) -> ExecutionFuture<'a, ManagedSchemaState> {
        self.base.observe_managed_state(lease, source, target)
    }
    fn prepare_group<'a>(
        &'a self,
        lease: &'a MigrationLease,
        source: &'a ManagedSchemaState,
        target: &'a ManagedSchemaState,
    ) -> ExecutionFuture<'a, Box<dyn PreparedMigrationGroup + 'a>> {
        self.base.prepare_group(lease, source, target)
    }
    fn validate_data_step(
        &self,
        _step: &DataStep,
        _direction: DataStepDirection,
    ) -> Result<(), Diagnostic> {
        if self.registered {
            Ok(())
        } else {
            Err(test_diagnostic("migration_user_step_unregistered"))
        }
    }
    fn observe_data_step<'a>(
        &'a self,
        _lease: &'a MigrationLease,
        _manifest: &'a VerifiedSchemaMigrationManifest,
        _step: &'a DataStep,
        direction: DataStepDirection,
    ) -> ExecutionFuture<'a, bool> {
        Box::pin(async move {
            Ok(*self.marker.lock().unwrap() == (direction == DataStepDirection::Forward))
        })
    }
    fn execute_data_step<'a>(
        &'a self,
        _lease: &'a MigrationLease,
        _manifest: &'a VerifiedSchemaMigrationManifest,
        _step: &'a DataStep,
        direction: DataStepDirection,
        _control: &'a MigrationExecutionControl,
    ) -> GroupCommitFuture<'a> {
        Box::pin(async move {
            *self.calls.lock().unwrap() += 1;
            let failure = self.failure.lock().unwrap().take();
            if failure != Some(GroupCommitCertainty::DefinitelyAborted) {
                *self.marker.lock().unwrap() = direction == DataStepDirection::Forward;
            }
            match failure {
                Some(certainty) => Err(GroupCommitFailure::new(
                    certainty,
                    test_diagnostic("injected_data_commit_failure"),
                )),
                None => Ok(()),
            }
        })
    }
}
#[test]
fn committed_marker_repairs_a_lost_journal_checkpoint_without_reexecution() {
    let (context, _, plan) = fixture(true);
    let store = CoordinatorStore::default();
    store.state.lock().unwrap().fail_data_event_once = Some(GroupJournalEventKind::Committed);
    let provider = AtomicProvider::new(&context, &plan);
    let holder = LeaseHolderId::new("data-worker").unwrap();
    assert!(
        block_on(execute_verified_migration_apply_plan(
            &store, &provider, &holder, &plan
        ))
        .is_err()
    );
    assert!(*provider.marker.lock().unwrap());
    assert!(matches!(
        block_on(execute_verified_migration_apply_plan(
            &store, &provider, &holder, &plan
        ))
        .unwrap(),
        MigrationExecutionOutcome::Applied { .. }
    ));
    assert_eq!(*provider.calls.lock().unwrap(), 1);
}
#[test]
fn unknown_commit_is_reconciled_from_marker_and_aborted_attempt_retries() {
    for certainty in [
        GroupCommitCertainty::Unknown,
        GroupCommitCertainty::DefinitelyAborted,
    ] {
        let (context, _, plan) = fixture(true);
        let store = CoordinatorStore::default();
        let provider = AtomicProvider::new(&context, &plan);
        *provider.failure.lock().unwrap() = Some(certainty);
        let holder = LeaseHolderId::new("data-worker").unwrap();
        let result = block_on(execute_verified_migration_apply_plan(
            &store, &provider, &holder, &plan,
        ));
        if certainty == GroupCommitCertainty::DefinitelyAborted {
            assert!(result.is_err());
            assert!(!*provider.marker.lock().unwrap());
            block_on(execute_verified_migration_apply_plan(
                &store, &provider, &holder, &plan,
            ))
            .unwrap();
            assert_eq!(*provider.calls.lock().unwrap(), 2);
        } else {
            assert!(result.is_ok());
            assert_eq!(*provider.calls.lock().unwrap(), 1);
        }
    }
}
#[test]
fn reverse_unknown_commit_repairs_from_absence_then_forward_can_run_again() {
    let (context, graph, plan) = fixture(true);
    let store = CoordinatorStore::default();
    let provider = AtomicProvider::new(&context, &plan);
    let holder = LeaseHolderId::new("data-worker").unwrap();
    block_on(execute_verified_migration_apply_plan(
        &store, &provider, &holder, &plan,
    ))
    .unwrap();
    let manifest = plan.migrations()[0].manifest();
    let applied = BTreeSet::from([manifest.id().clone()]);
    let reverse = build_verified_migration_rollback_plan(
        &graph,
        &applied,
        &applied,
        &context,
        &SchemaLoweringBinding::for_context(&context).unwrap(),
        &MigrationSafetyPolicy::default_policy(),
        &[MigrationApplyApproval::for_rollback(manifest, SafetyClass::Opaque).unwrap()],
    )
    .unwrap();
    *provider.failure.lock().unwrap() = Some(GroupCommitCertainty::Unknown);
    block_on(execute_verified_migration_rollback_plan(
        &store, &provider, &holder, &reverse,
    ))
    .unwrap();
    assert!(!*provider.marker.lock().unwrap());
    block_on(execute_verified_migration_apply_plan(
        &store, &provider, &holder, &plan,
    ))
    .unwrap();
    assert_eq!(*provider.calls.lock().unwrap(), 3);
}
#[test]
fn unregistered_step_is_refused_before_acquiring_a_lease() {
    let (context, _, plan) = fixture(true);
    let store = CoordinatorStore::default();
    let mut provider = AtomicProvider::new(&context, &plan);
    provider.registered = false;
    let error = block_on(execute_verified_migration_apply_plan(
        &store,
        &provider,
        &LeaseHolderId::new("worker").unwrap(),
        &plan,
    ))
    .unwrap_err();
    assert_eq!(error.code().as_str(), "migration_user_step_unregistered");
    assert_eq!(store.state.lock().unwrap().fence, 0);
}
#[test]
fn no_reverse_is_visible_in_manifest_and_refused_by_rollback_planning() {
    let (context, graph, plan) = fixture(false);
    let manifest = plan.migrations()[0].manifest();
    assert!(!manifest.reversible());
    assert!(
        render_migration_preview(manifest, &context)
            .unwrap()
            .contains("IRREVERSIBLE")
    );
    let applied = BTreeSet::from([manifest.id().clone()]);
    let error = build_verified_migration_rollback_plan(
        &graph,
        &applied,
        &applied,
        &context,
        &SchemaLoweringBinding::for_context(&context).unwrap(),
        &MigrationSafetyPolicy::default_policy(),
        &[],
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("migration_rollback_irreversible")
    );
}

#[test]
fn aborted_attempt_with_a_completion_marker_is_refused_as_drift() {
    let (context, _, plan) = fixture(true);
    let store = CoordinatorStore::default();
    let provider = AtomicProvider::new(&context, &plan);
    *provider.failure.lock().unwrap() = Some(GroupCommitCertainty::DefinitelyAborted);
    let holder = LeaseHolderId::new("data-worker").unwrap();
    assert!(
        block_on(execute_verified_migration_apply_plan(
            &store, &provider, &holder, &plan,
        ))
        .is_err()
    );
    *provider.marker.lock().unwrap() = true;
    let error = block_on(execute_verified_migration_apply_plan(
        &store, &provider, &holder, &plan,
    ))
    .unwrap_err();
    assert_eq!(error.code().as_str(), "migration_data_marker_drift");
    assert_eq!(*provider.calls.lock().unwrap(), 1);
}
