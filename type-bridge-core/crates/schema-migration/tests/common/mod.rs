//! Shared coordinator test doubles for apply and rollback execution tests.

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::schema::ManagedSchemaState;
use type_bridge_query::ValidatedMigrationAssertionPlan;
use type_bridge_schema_migration::{
    AppliedRecord, ExecutionFence, ExecutionFuture, ExecutionScope, GroupCommitFuture,
    GroupEventRecord, GroupJournalEventKind, JournalEntry, JournalSequence, LeaseHolderId,
    MigrationExecutionJournal, MigrationExecutionProvider, MigrationLease, MigrationLeaseStore,
    OpenPlanRecord, OpenRollbackPlanRecord, PlanRecord, PreparedMigrationGroup, RollbackPlanRecord,
    RollbackStepEventRecord, RolledBackRecord, StatementUnit, active_applied_entries,
};

#[derive(Default)]
pub struct CoordinatorStoreState {
    pub active: Option<MigrationLease>,
    pub applied: Vec<JournalEntry<AppliedRecord>>,
    pub rolled_back: Vec<JournalEntry<RolledBackRecord>>,
    pub events: Vec<JournalEntry<GroupEventRecord>>,
    pub event_audit: Vec<GroupJournalEventKind>,
    pub rollback_events: Vec<JournalEntry<RollbackStepEventRecord>>,
    pub rollback_event_audit: Vec<GroupJournalEventKind>,
    pub fence: u64,
    pub next_sequence: u64,
    pub open: Option<JournalEntry<PlanRecord>>,
    pub open_rollback: Option<JournalEntry<RollbackPlanRecord>>,
    pub releases: usize,
}

#[derive(Default)]
pub struct CoordinatorStore {
    pub state: Mutex<CoordinatorStoreState>,
}

impl CoordinatorStore {
    fn checked<'a>(
        state: &'a mut CoordinatorStoreState,
        lease: &MigrationLease,
    ) -> Result<&'a mut CoordinatorStoreState, Diagnostic> {
        if state.active.as_ref() != Some(lease) {
            return Err(test_diagnostic("coordinator_stale_lease"));
        }
        Ok(state)
    }

    fn sequence(state: &mut CoordinatorStoreState) -> Result<JournalSequence, Diagnostic> {
        state.next_sequence += 1;
        JournalSequence::new(state.next_sequence)
    }
}

impl MigrationLeaseStore for CoordinatorStore {
    fn acquire<'a>(
        &'a self,
        scope: &'a ExecutionScope,
        holder: &'a LeaseHolderId,
    ) -> ExecutionFuture<'a, MigrationLease> {
        Box::pin(async move {
            let mut state = self.state.lock().expect("coordinator store");
            if state.active.is_some() {
                return Err(test_diagnostic("coordinator_lease_contended"));
            }
            state.fence += 1;
            let lease = MigrationLease::new(
                scope.clone(),
                holder.clone(),
                ExecutionFence::new(state.fence)?,
            );
            state.active = Some(lease.clone());
            Ok(lease)
        })
    }

    fn release<'a>(&'a self, lease: &'a MigrationLease) -> ExecutionFuture<'a, ()> {
        Box::pin(async move {
            let mut state = self.state.lock().expect("coordinator store");
            Self::checked(&mut state, lease)?;
            state.active = None;
            state.releases += 1;
            Ok(())
        })
    }
}

impl MigrationExecutionJournal for CoordinatorStore {
    fn begin_plan<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: PlanRecord,
    ) -> ExecutionFuture<'a, JournalEntry<PlanRecord>> {
        Box::pin(async move {
            let mut state = self.state.lock().expect("coordinator store");
            Self::checked(&mut state, lease)?;
            if state.open.is_some() || state.open_rollback.is_some() {
                return Err(test_diagnostic("coordinator_plan_already_open"));
            }
            let entry = JournalEntry::from_store(Self::sequence(&mut state)?, record);
            state.open = Some(entry.clone());
            Ok(entry)
        })
    }

    fn record_group_event<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: GroupEventRecord,
    ) -> ExecutionFuture<'a, JournalEntry<GroupEventRecord>> {
        Box::pin(async move {
            let mut state = self.state.lock().expect("coordinator store");
            Self::checked(&mut state, lease)?;
            let entry = JournalEntry::from_store(Self::sequence(&mut state)?, record);
            state.event_audit.push(entry.record().kind());
            state.events.push(entry.clone());
            Ok(entry)
        })
    }

    fn record_applied<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: AppliedRecord,
    ) -> ExecutionFuture<'a, JournalEntry<AppliedRecord>> {
        Box::pin(async move {
            let mut state = self.state.lock().expect("coordinator store");
            Self::checked(&mut state, lease)?;
            let entry = JournalEntry::from_store(Self::sequence(&mut state)?, record);
            state.applied.push(entry.clone());
            let active = active_applied_entries(state.applied.clone(), &state.rolled_back)?;
            let complete = state.open.as_ref().is_some_and(|open| {
                open.record().migration_ids().iter().all(|id| {
                    active
                        .iter()
                        .any(|applied| applied.record().migration_id() == id)
                })
            });
            if complete {
                state.open = None;
                state.events.clear();
            }
            Ok(entry)
        })
    }

    fn load_applied<'a>(
        &'a self,
        lease: &'a MigrationLease,
    ) -> ExecutionFuture<'a, Vec<JournalEntry<AppliedRecord>>> {
        Box::pin(async move {
            let mut state = self.state.lock().expect("coordinator store");
            Self::checked(&mut state, lease)?;
            active_applied_entries(state.applied.clone(), &state.rolled_back)
        })
    }

    fn load_open_plan<'a>(
        &'a self,
        lease: &'a MigrationLease,
    ) -> ExecutionFuture<'a, Option<OpenPlanRecord>> {
        Box::pin(async move {
            let mut state = self.state.lock().expect("coordinator store");
            Self::checked(&mut state, lease)?;
            let Some(open) = state.open.clone() else {
                return Ok(None);
            };
            Ok(Some(OpenPlanRecord::from_store(
                open,
                state.events.clone(),
            )?))
        })
    }

    fn begin_rollback_plan<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: RollbackPlanRecord,
    ) -> ExecutionFuture<'a, JournalEntry<RollbackPlanRecord>> {
        Box::pin(async move {
            let mut state = self.state.lock().expect("coordinator store");
            Self::checked(&mut state, lease)?;
            if state.open.is_some() || state.open_rollback.is_some() {
                return Err(test_diagnostic("coordinator_plan_already_open"));
            }
            let entry = JournalEntry::from_store(Self::sequence(&mut state)?, record);
            state.open_rollback = Some(entry.clone());
            Ok(entry)
        })
    }

    fn record_rollback_step_event<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: RollbackStepEventRecord,
    ) -> ExecutionFuture<'a, JournalEntry<RollbackStepEventRecord>> {
        Box::pin(async move {
            let mut state = self.state.lock().expect("coordinator store");
            Self::checked(&mut state, lease)?;
            if state.open_rollback.is_none() {
                return Err(test_diagnostic("coordinator_no_open_rollback_plan"));
            }
            let entry = JournalEntry::from_store(Self::sequence(&mut state)?, record);
            state.rollback_event_audit.push(entry.record().kind());
            state.rollback_events.push(entry.clone());
            Ok(entry)
        })
    }

    fn record_rolled_back<'a>(
        &'a self,
        lease: &'a MigrationLease,
        record: RolledBackRecord,
    ) -> ExecutionFuture<'a, JournalEntry<RolledBackRecord>> {
        Box::pin(async move {
            let mut state = self.state.lock().expect("coordinator store");
            Self::checked(&mut state, lease)?;
            let plan = state
                .open_rollback
                .as_ref()
                .ok_or_else(|| test_diagnostic("coordinator_no_open_rollback_plan"))?;
            let member = plan
                .record()
                .rollback_ids()
                .iter()
                .zip(plan.record().manifest_digests())
                .any(|(id, digest)| {
                    id == record.migration_id() && *digest == record.manifest_digest()
                });
            if !member {
                return Err(test_diagnostic("coordinator_foreign_retirement"));
            }
            let entry = JournalEntry::from_store(Self::sequence(&mut state)?, record);
            state.rolled_back.push(entry.clone());
            let active = active_applied_entries(state.applied.clone(), &state.rolled_back)?;
            let complete = state.open_rollback.as_ref().is_some_and(|plan| {
                plan.record().rollback_ids().iter().all(|id| {
                    !active
                        .iter()
                        .any(|applied| applied.record().migration_id() == id)
                })
            });
            if complete {
                state.open_rollback = None;
                state.rollback_events.clear();
            }
            Ok(entry)
        })
    }

    fn load_rolled_back<'a>(
        &'a self,
        lease: &'a MigrationLease,
    ) -> ExecutionFuture<'a, Vec<JournalEntry<RolledBackRecord>>> {
        Box::pin(async move {
            let mut state = self.state.lock().expect("coordinator store");
            Self::checked(&mut state, lease)?;
            Ok(state.rolled_back.clone())
        })
    }

    fn load_open_rollback_plan<'a>(
        &'a self,
        lease: &'a MigrationLease,
    ) -> ExecutionFuture<'a, Option<OpenRollbackPlanRecord>> {
        Box::pin(async move {
            let mut state = self.state.lock().expect("coordinator store");
            Self::checked(&mut state, lease)?;
            let Some(open) = state.open_rollback.clone() else {
                return Ok(None);
            };
            Ok(Some(OpenRollbackPlanRecord::from_store(
                open,
                state.rollback_events.clone(),
            )?))
        })
    }
}

pub struct CoordinatorProvider {
    pub available: CapabilitySet,
    pub calls: Mutex<Vec<&'static str>>,
    pub observed: Mutex<ManagedSchemaState>,
}

impl MigrationExecutionProvider for CoordinatorProvider {
    fn available_capabilities(&self) -> &CapabilitySet {
        &self.available
    }

    fn observe_managed_state<'a>(
        &'a self,
        _lease: &'a MigrationLease,
        _source_candidate: &'a ManagedSchemaState,
        _target_candidate: &'a ManagedSchemaState,
    ) -> ExecutionFuture<'a, ManagedSchemaState> {
        Box::pin(async move {
            self.calls.lock().expect("provider calls").push("observe");
            Ok(self.observed.lock().expect("provider state").clone())
        })
    }

    fn prepare_group<'a>(
        &'a self,
        _lease: &'a MigrationLease,
        _source: &'a ManagedSchemaState,
        target: &'a ManagedSchemaState,
    ) -> ExecutionFuture<'a, Box<dyn PreparedMigrationGroup + 'a>> {
        Box::pin(async move {
            self.calls.lock().expect("provider calls").push("prepare");
            Ok(Box::new(CoordinatorTransaction {
                provider: self,
                target,
            }) as Box<dyn PreparedMigrationGroup + 'a>)
        })
    }
}

pub struct CoordinatorTransaction<'a> {
    provider: &'a CoordinatorProvider,
    target: &'a ManagedSchemaState,
}

impl PreparedMigrationGroup for CoordinatorTransaction<'_> {
    fn execute_assertion<'a>(
        &'a mut self,
        _plan: &'a ValidatedMigrationAssertionPlan,
    ) -> ExecutionFuture<'a, ()> {
        Box::pin(async move {
            self.provider
                .calls
                .lock()
                .expect("provider calls")
                .push("assertion");
            Ok(())
        })
    }

    fn execute_statement_unit<'a>(
        &'a mut self,
        _unit: &'a StatementUnit,
    ) -> ExecutionFuture<'a, ()> {
        Box::pin(async move {
            self.provider
                .calls
                .lock()
                .expect("provider calls")
                .push("statement");
            Ok(())
        })
    }

    fn commit<'a>(self: Box<Self>, _lease: &'a MigrationLease) -> GroupCommitFuture<'a>
    where
        Self: 'a,
    {
        Box::pin(async move {
            self.provider
                .calls
                .lock()
                .expect("provider calls")
                .push("commit");
            *self.provider.observed.lock().expect("provider state") = self.target.clone();
            Ok(())
        })
    }

    fn rollback<'a>(self: Box<Self>) -> ExecutionFuture<'a, ()>
    where
        Self: 'a,
    {
        Box::pin(async move {
            self.provider
                .calls
                .lock()
                .expect("provider calls")
                .push("rollback");
            Ok(())
        })
    }
}

pub fn test_diagnostic(code: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::Integrity,
        DiagnosticCode::new(code).expect("test code"),
        "coordinator test failure",
    )
}

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

pub fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}
