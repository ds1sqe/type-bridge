//! Atomic binding-neutral execution for homogeneous projected mutation batches.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;

use serde_json::Value;
use type_bridge_contract::id::{RoleId, TypeId, TypeKind, is_canonical_thing_iid};
use type_bridge_contract::projection::{ModelProjection, ProjectedModelForm, ReadRoleProjection};
use type_bridge_contract::schema::{AnnotationKindId, OwnsFactId};
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCategory, SdkDiagnosticCode, SdkDiagnosticDetailValue, SdkDiagnosticMessage,
    SdkDiagnosticName, SdkDiagnosticPathSegment, SdkExecutionDiagnostic, SdkProviderOperation,
};
use type_bridge_contract::split::SplitLimits;
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};

use crate::_dynamic::{
    DynamicAttributeMap, DynamicEntityRow, DynamicRelationRow, DynamicRolePlayer,
};
use crate::_manager::hydration::{hydrate_dynamic_entity, hydrate_dynamic_relation};
use crate::OrmError;
use crate::execution_diagnostic::lower_orm_error;
use crate::plan_builders::batch::{split_lookups, split_stage};
use crate::projected_batch::{
    PreparedProjectedBatchInvocation, ProjectedBatch, ProjectedBatchInvocationControl,
    ProjectedBatchOperation, ProjectedBatchRow,
};
use crate::projected_model::{
    ProjectedAttributeValue, ProjectedCreate, ProjectedReference, ProjectedRolePlayer,
    ProjectedThing,
};
use crate::query_execution_limits::{QueryExecutionDeadline, QueryExecutionResourceLimits};
use crate::runtime_projection::InstalledRuntimeProjection;
use crate::session::backend::{
    AnswerConsumer, AnswerControl, AnswerItem, AnswerLimits, BoundedAnswerLimits,
    BoundedAnswerReader, BoundedAnswerStats, GivenRowsSpec, GivenValue,
};
use crate::session::context::TransactionContextMutationLease;
use crate::session::database::DatabaseExecutionIdentity;
use crate::session::{Database, TransactionContext, TxType};
use crate::value::AttributeValue;

#[path = "projected_batch_native.rs"]
mod native;

const COMPILER_CARDINALITY_MAX: u64 = u16::MAX as u64;
const MAX_PROVIDER_IID_BYTES: u64 = (2 + type_bridge_contract::id::MAX_THING_IID_HEX_DIGITS) as u64;

enum MappedExecutionFailure {
    Diagnostic(SdkExecutionDiagnostic),
    Panic(Box<dyn std::any::Any + Send + 'static>),
}

impl From<SdkExecutionDiagnostic> for MappedExecutionFailure {
    fn from(diagnostic: SdkExecutionDiagnostic) -> Self {
        Self::Diagnostic(diagnostic)
    }
}

/// Result of one homogeneous projected mutation batch.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProjectedBatchResult {
    /// Complete post-mutation projected things in input ordinal order.
    Things(crate::BatchWriteResult<ProjectedThing>),
    /// Every requested delete was applied as an idempotent present-or-absent operation.
    Deleted,
}

/// Binding-neutral atomic executor for [`ProjectedBatch`].
pub struct ProjectedBatchExecutor<'projection> {
    installed: &'projection InstalledRuntimeProjection,
    return_deleted: bool,
    #[cfg(test)]
    fail_hydration_allocation: bool,
}

impl<'projection> ProjectedBatchExecutor<'projection> {
    /// Create an executor over one verified installed runtime projection.
    #[must_use]
    pub const fn new(installed: &'projection InstalledRuntimeProjection) -> Self {
        Self {
            installed,
            return_deleted: false,
            #[cfg(test)]
            fail_hydration_allocation: false,
        }
    }

    /// Capture complete deleted things before mutation, in input order.
    ///
    /// Only delete batches may use this mode. Missing IIDs are omitted.
    #[must_use]
    pub const fn returning_deleted(mut self) -> Self {
        self.return_deleted = true;
        self
    }

    /// Construct the shared redacted diagnostic for one binding
    /// materialization failure after projected hydration.
    #[doc(hidden)]
    #[must_use]
    pub fn binding_materialization_failure(ordinal: u64) -> SdkExecutionDiagnostic {
        SdkExecutionDiagnostic::integrity(
            code("generated_model_materialization_failed"),
            message("The generated model could not materialize validated projected evidence"),
        )
        .try_at(SdkDiagnosticPathSegment::Argument(name("rows")))
        .and_then(|diagnostic| diagnostic.try_at(SdkDiagnosticPathSegment::Index(ordinal)))
        .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure())
    }

    fn binding_materialization_panic() -> SdkExecutionDiagnostic {
        SdkExecutionDiagnostic::integrity(
            code("generated_model_materialization_failed"),
            message("The generated model could not materialize validated projected evidence"),
        )
        .try_at(SdkDiagnosticPathSegment::Argument(name("rows")))
        .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure())
    }

    #[cfg(test)]
    fn failing_hydration_allocation_for_test(mut self) -> Self {
        self.fail_hydration_allocation = true;
        self
    }

    /// Execute a batch on the connection-selected target with the same mapper.
    ///
    /// Empty batches validate admission without dispatch or materialization.
    /// Direct owned writes retain pre-commit mapping; borrowed writes retain
    /// their mutation lease. Proxy mapping preserves the staged commit outcome.
    #[doc(hidden)]
    pub async fn execute_on_target_mapped<T, F>(
        &self,
        target: crate::QueryTarget<'_>,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
        empty: T,
        mapper: F,
    ) -> std::result::Result<T, crate::QueryTargetError>
    where
        F: FnOnce(ProjectedBatchResult) -> std::result::Result<T, SdkExecutionDiagnostic>,
    {
        match target {
            crate::QueryTarget::Database(database) => self
                .execute_mapped(database, batch, control, empty, mapper)
                .await
                .map_err(Into::into),
            crate::QueryTarget::Transaction(transaction) => self
                .execute_in_transaction_mapped(transaction, batch, control, empty, mapper)
                .await
                .map_err(Into::into),
            crate::QueryTarget::Proxy(proxy) => {
                self.execute_on_proxy_mapped(proxy, batch, control, empty, mapper)
                    .await
            }
        }
    }

    /// Execute and commit one batch in an owned write transaction.
    pub async fn execute(
        &self,
        database: &Database,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
    ) -> std::result::Result<ProjectedBatchResult, SdkExecutionDiagnostic> {
        self.execute_mapped(
            database,
            batch,
            control,
            if self.return_deleted {
                ProjectedBatchResult::Things(crate::BatchWriteResult::default())
            } else {
                empty_result(batch.operation())
            },
            Ok,
        )
        .await
    }

    /// Execute, synchronously map the fully hydrated result while atomicity is
    /// still armed, and commit one batch in an owned write transaction.
    ///
    /// An authority-valid empty batch returns `empty` without invoking the
    /// mapper. For a nonempty batch, the mapper receives no transaction or
    /// provider handle and cannot yield. It must not synchronously re-enter a
    /// captured transaction or provider API: the mutation lease remains held
    /// during mapping, so re-entry could self-deadlock.
    /// Its exact diagnostic is treated as a pre-commit failure: owned execution
    /// rolls back, while borrowed execution through
    /// [`Self::execute_in_transaction_mapped`] becomes rollback-only.
    /// A mapper panic is recorded as the same redacted materialization failure;
    /// the original panic resumes only after rollback or rollback-only latching.
    #[doc(hidden)]
    pub async fn execute_mapped<T, F>(
        &self,
        database: &Database,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
        empty: T,
        mapper: F,
    ) -> std::result::Result<T, SdkExecutionDiagnostic>
    where
        F: FnOnce(ProjectedBatchResult) -> std::result::Result<T, SdkExecutionDiagnostic>,
    {
        if self.return_deleted && batch.operation() != ProjectedBatchOperation::Delete {
            return Err(invalid_at_type(
                "delete_returning_operation",
                "Deletion snapshots require a delete batch",
                batch.model(),
            ));
        }
        let control = control.constrained_by(database.answer_limits());
        let prepared = PreparedProjectedBatchInvocation::try_new(self.installed, batch, control)?;
        if batch.is_empty() {
            return Ok(empty);
        }
        prepared.check_control()?;
        require_native_provider(database.server_version())?;
        let identity = database.execution_identity();
        let plan = BatchPlan::compile(
            self.installed,
            &prepared,
            &identity,
            self.return_deleted,
            database.shared_query_limits(),
        )?;
        prepared.check_control()?;

        let transaction = await_controlled(
            database.transaction_context(TxType::Write),
            prepared.deadline(),
            prepared.cancellation(),
        )
        .await
        .map_err(|error| match error {
            ControlledAwaitError::Interrupted(diagnostic) => diagnostic,
            ControlledAwaitError::Inner(error) => at_type(
                lower_orm_error(&error, SdkProviderOperation::OpenWriteTransaction),
                batch.model(),
            ),
        })?;

        let result = self
            .execute_prepared_in_transaction(&transaction, &prepared, plan, &identity, mapper)
            .await;
        match result {
            Ok(value) => {
                // The mutation lease has been consumed before this point. Once
                // commit is attempted, its outcome is terminal and rollback is
                // never dispatched.
                if let Err(primary) = prepared.check_control() {
                    let _ = transaction.rollback().await;
                    return Err(primary);
                }
                transaction
                    .commit_sdk()
                    .await
                    .map_err(|diagnostic| at_type(diagnostic, batch.model()))?;
                Ok(value)
            }
            Err(primary) => {
                let _ = transaction.rollback().await;
                match primary {
                    MappedExecutionFailure::Diagnostic(diagnostic) => Err(diagnostic),
                    MappedExecutionFailure::Panic(payload) => resume_unwind(payload),
                }
            }
        }
    }

    /// Execute one batch inside a caller-owned write transaction.
    pub async fn execute_in_transaction(
        &self,
        transaction: &TransactionContext,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
    ) -> std::result::Result<ProjectedBatchResult, SdkExecutionDiagnostic> {
        self.execute_in_transaction_mapped(
            transaction,
            batch,
            control,
            if self.return_deleted {
                ProjectedBatchResult::Things(crate::BatchWriteResult::default())
            } else {
                empty_result(batch.operation())
            },
            Ok,
        )
        .await
    }

    /// Execute and synchronously map one fully hydrated batch result inside a
    /// caller-owned write transaction while its mutation lease remains armed.
    ///
    /// An authority-valid empty batch returns `empty` without invoking the
    /// mapper. For a nonempty batch, the mapper receives no transaction or
    /// provider handle and cannot yield. It must not synchronously re-enter a
    /// captured transaction or provider API: the mutation lease remains held
    /// during mapping, so re-entry could self-deadlock.
    /// Its exact diagnostic becomes the transaction's first rollback-only
    /// cause after mutation dispatch.
    /// A mapper panic is recorded as a redacted materialization failure before
    /// the original panic resumes.
    #[doc(hidden)]
    pub async fn execute_in_transaction_mapped<T, F>(
        &self,
        transaction: &TransactionContext,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
        empty: T,
        mapper: F,
    ) -> std::result::Result<T, SdkExecutionDiagnostic>
    where
        F: FnOnce(ProjectedBatchResult) -> std::result::Result<T, SdkExecutionDiagnostic>,
    {
        if self.return_deleted && batch.operation() != ProjectedBatchOperation::Delete {
            return Err(invalid_at_type(
                "delete_returning_operation",
                "Deletion snapshots require a delete batch",
                batch.model(),
            ));
        }
        let control = control.constrained_by(transaction.answer_limits());
        let prepared = PreparedProjectedBatchInvocation::try_new(self.installed, batch, control)?;
        if batch.is_empty() {
            return Ok(empty);
        }
        if transaction.tx_type() != TxType::Write {
            return Err(invalid_at_type(
                "transaction_type_mismatch",
                "Projected batch execution requires a borrowed write transaction",
                batch.model(),
            ));
        }
        prepared.check_control()?;
        require_native_provider(transaction.server_version())?;
        let identity = transaction.execution_identity().clone();
        let plan = BatchPlan::compile(
            self.installed,
            &prepared,
            &identity,
            self.return_deleted,
            transaction.shared_query_limits(),
        )?;
        prepared.check_control()?;
        match self
            .execute_prepared_in_transaction(transaction, &prepared, plan, &identity, mapper)
            .await
        {
            Ok(value) => Ok(value),
            Err(MappedExecutionFailure::Diagnostic(diagnostic)) => Err(diagnostic),
            Err(MappedExecutionFailure::Panic(payload)) => resume_unwind(payload),
        }
    }

    async fn execute_prepared_in_transaction<T, F>(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedProjectedBatchInvocation<'_>,
        mut plan: BatchPlan<'_>,
        identity: &DatabaseExecutionIdentity,
        mapper: F,
    ) -> std::result::Result<T, MappedExecutionFailure>
    where
        F: FnOnce(ProjectedBatchResult) -> std::result::Result<T, SdkExecutionDiagnostic>,
    {
        let mut lease = await_controlled(
            transaction.acquire_mutation_lease(),
            prepared.deadline(),
            prepared.cancellation(),
        )
        .await
        .map_err(|error| controlled_orm(error, SdkProviderOperation::Write, plan.model.id()))?;
        let result = match self
            .run_native(&mut lease, prepared, &mut plan, identity)
            .await
        {
            Err(diagnostic) => Err(MappedExecutionFailure::Diagnostic(diagnostic)),
            Ok(batch_result) => match catch_unwind(AssertUnwindSafe(|| mapper(batch_result))) {
                Ok(Err(diagnostic)) => Err(MappedExecutionFailure::Diagnostic(diagnostic)),
                Ok(Ok(value)) => prepared
                    .check_control()
                    .map(|()| value)
                    .map_err(MappedExecutionFailure::Diagnostic),
                Err(payload) => Err(MappedExecutionFailure::Panic(payload)),
            },
        };
        match result {
            Ok(value) => {
                lease.complete_success();
                Ok(value)
            }
            Err(MappedExecutionFailure::Diagnostic(diagnostic)) => {
                lease.record_failure(&diagnostic);
                Err(MappedExecutionFailure::Diagnostic(diagnostic))
            }
            Err(MappedExecutionFailure::Panic(payload)) => {
                lease.record_failure(&Self::binding_materialization_panic());
                Err(MappedExecutionFailure::Panic(payload))
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn inline_batch_plan_bytes_for_test(
    installed: &InstalledRuntimeProjection,
    batch: &ProjectedBatch,
    identity: &DatabaseExecutionIdentity,
    returning: bool,
) -> std::result::Result<Vec<u8>, SdkExecutionDiagnostic> {
    let prepared = PreparedProjectedBatchInvocation::try_new(
        installed,
        batch,
        ProjectedBatchInvocationControl::capture(Default::default(), Default::default()),
    )?;
    let mut plan = BatchPlan::compile(installed, &prepared, identity, returning, None)?;
    let native = if let Some((mut program, _)) = plan.write_program.take() {
        assert_eq!(program.statement_count(), 1);
        program
            .next()
            .map_err(|error| {
                lower_orm_error(
                    &crate::plan_builders::manager::diagnostic(error),
                    SdkProviderOperation::Write,
                )
            })?
            .expect("one inline direct write")
            .plan
    } else if returning {
        let iids = plan
            .rows
            .iter()
            .map(|(_, row)| row_iid(row).unwrap().to_owned())
            .collect::<Vec<_>>();
        let plans = crate::plan_builders::manager::ManagerPlans::new(
            installed
                .manager_registry()
                .map_err(|error| lower_orm_error(&error, SdkProviderOperation::Write))?,
        );
        plans
            .delete_many(plan.model.id().label().as_str(), &iids, true, true)
            .map_err(|error| lower_orm_error(&error, SdkProviderOperation::Write))?
    } else {
        plan.native_delete(plan.delete_rows()?)?
    };
    native.local().canonical_bytes().map_err(|error| {
        lower_orm_error(
            &crate::plan_builders::manager::diagnostic(error),
            SdkProviderOperation::Write,
        )
    })
}

fn empty_result(operation: ProjectedBatchOperation) -> ProjectedBatchResult {
    if operation == ProjectedBatchOperation::Delete {
        ProjectedBatchResult::Deleted
    } else {
        ProjectedBatchResult::Things(crate::BatchWriteResult::default())
    }
}

pub(crate) fn require_native_provider(
    version: Option<type_bridge_core_lib::version::Version>,
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    if version
        .is_none_or(|version| version >= type_bridge_core_lib::version::Version::new(3, 12, 0))
    {
        Ok(())
    } else {
        Err(SdkExecutionDiagnostic::unsupported_capability(
            code("projected_batch_transport_unsupported"),
            message("Projected batch execution requires TypeDB 3.12 or newer"),
        ))
    }
}

enum ControlledAwaitError<E> {
    Interrupted(SdkExecutionDiagnostic),
    Inner(E),
}

async fn await_controlled<F, T, E>(
    future: F,
    deadline: QueryExecutionDeadline,
    cancellation: &crate::session::backend::AnswerCancellation,
) -> std::result::Result<T, ControlledAwaitError<E>>
where
    F: Future<Output = std::result::Result<T, E>>,
{
    tokio::pin!(future);
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(ControlledAwaitError::Interrupted(SdkExecutionDiagnostic::data_operation_cancelled())),
        _ = deadline.elapsed() => Err(ControlledAwaitError::Interrupted(SdkExecutionDiagnostic::data_operation_deadline_exceeded())),
        result = &mut future => result.map_err(ControlledAwaitError::Inner),
    }
}

fn controlled_orm(
    error: ControlledAwaitError<OrmError>,
    operation: SdkProviderOperation,
    type_id: &TypeId,
) -> SdkExecutionDiagnostic {
    match error {
        ControlledAwaitError::Interrupted(diagnostic) => at_type(diagnostic, type_id),
        ControlledAwaitError::Inner(error) => at_type(lower_orm_error(&error, operation), type_id),
    }
}

struct FieldRoute {
    field: OwnsFactId,
    variable: String,
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
struct RoleRouteKey {
    role: RoleId,
    player_type: TypeId,
}

struct RoleRoute {
    key: RoleRouteKey,
    variable: String,
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
enum ReferenceRouteKey {
    Iid(RoleId, TypeId),
    Key(RoleId, TypeId, OwnsFactId),
}

struct ReferenceRoute {
    key: ReferenceRouteKey,
    candidate_types: Vec<TypeId>,
    key_variable: Option<String>,
}

struct RoleReference<'a> {
    owner_ordinal: usize,
    reference_ordinal: usize,
    role: RoleId,
    reference: &'a ProjectedReference,
}

struct BatchPlan<'a> {
    installed: &'a InstalledRuntimeProjection,
    authority: Arc<crate::plan_builders::authority::PlanAuthority>,
    model: &'a ModelProjection,
    operation: ProjectedBatchOperation,
    rows: Vec<(usize, &'a ProjectedBatchRow)>,
    field_routes: Vec<FieldRoute>,
    role_references: Vec<RoleReference<'a>>,
    reference_routes: Vec<ReferenceRoute>,
    return_deleted: bool,
    row_count: usize,
    single_input: bool,
    statement_count: u32,
    preflight_bytes: u64,
    capture_role_snapshots: bool,
    write_program: Option<(
        crate::plan_builders::bulk_write::WriteProgram,
        ExecutionLedger,
    )>,
}

impl<'a> BatchPlan<'a> {
    fn compile(
        installed: &'a InstalledRuntimeProjection,
        prepared: &PreparedProjectedBatchInvocation<'a>,
        identity: &DatabaseExecutionIdentity,
        return_deleted: bool,
        query_limits: Option<&Arc<type_bridge_contract::limits::QueryLimits>>,
    ) -> std::result::Result<Self, SdkExecutionDiagnostic> {
        let batch = prepared.batch();
        if return_deleted && batch.operation() != ProjectedBatchOperation::Delete {
            return Err(invalid_at_type(
                "delete_returning_operation",
                "Deletion snapshots require a delete batch",
                batch.model(),
            ));
        }
        let model = installed
            .projection()
            .models()
            .get(batch.model())
            .ok_or_else(|| {
                integrity_at_type(
                    "runtime_projection_mismatch",
                    "The projected batch model disappeared from the installed projection",
                    batch.model(),
                )
            })?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(batch.len())
            .map_err(|_| allocation_failure())?;
        for index in 0..batch.len() {
            rows.push(batch.row_at(index).expect("normalized batch row exists"));
        }
        validate_reference_origins(&rows, identity)?;
        validate_provider_write_inputs(&rows)?;

        let field_routes = compile_field_routes(installed, model)?;
        let (role_routes, role_references) = compile_role_routes(installed, model, &rows)?;
        let reference_routes = compile_reference_routes(installed, model, &role_references)?;
        preflight_route_shape(
            prepared,
            model,
            batch.operation(),
            &rows,
            &field_routes,
            &role_routes,
            &reference_routes,
            &role_references,
        )?;
        let authority = installed
            .plan_authority_with_query_limits(query_limits)
            .map_err(|error| {
                lower_orm_error(
                    &crate::plan_builders::manager::diagnostic(error),
                    SdkProviderOperation::Write,
                )
            })?;
        if batch.operation() != ProjectedBatchOperation::Delete {
            let capture_role_snapshots = batch.operation() == ProjectedBatchOperation::Update
                && role_references.iter().any(|reference| {
                    reference.reference.iid().is_none()
                        && is_same_or_subtype(installed, model.id(), reference.reference.type_id())
                });
            let mut plan = Self {
                installed,
                authority,
                model,
                operation: batch.operation(),
                rows,
                field_routes,
                role_references,
                reference_routes,
                return_deleted,
                row_count: batch.len(),
                single_input: batch.is_single_write(),
                statement_count: batch.resource_measure().statements(),
                preflight_bytes: 0,
                capture_role_snapshots,
                write_program: None,
            };
            let program = plan
                .complete_write_plans()?
                .prepare_static()
                .map_err(|error| {
                    lower_orm_error(
                        &crate::plan_builders::manager::diagnostic(error),
                        SdkProviderOperation::Write,
                    )
                })?;
            let statements =
                u32::try_from(program.statement_count()).map_err(|_| complexity_overflow())?;
            let mut ledger = ExecutionLedger::new(
                prepared
                    .limits()
                    .for_adapter_statements(program.statement_count()),
                batch.resource_measure().bytes(),
            );
            ledger.charge_preflight(0, statements)?;
            plan.statement_count = statements;
            plan.write_program = Some((program, ledger));
            return Ok(plan);
        }
        let attach = batch.operation() != ProjectedBatchOperation::Delete;
        let statement_count = batch.resource_measure().statements();
        let preflight_bytes = {
            preflight_transport_bytes(
                prepared,
                model,
                &rows,
                &field_routes,
                &role_routes,
                &role_references,
                attach,
            )?
        };
        let (statement_count, preflight_bytes) = if return_deleted {
            let rows = preflight_mutation_rows(batch.operation(), &rows, model)?;
            let bytes = given_rows_bytes(&rows)?;
            (
                statement_count + 1,
                checked_add_u64(preflight_bytes, bytes)?,
            )
        } else {
            (statement_count, preflight_bytes)
        };
        Ok(Self {
            installed,
            authority,
            model,
            operation: batch.operation(),
            rows,
            field_routes,
            role_references,
            reference_routes,
            return_deleted,
            row_count: batch.len(),
            single_input: batch.is_single_write(),
            statement_count,
            preflight_bytes,
            capture_role_snapshots: false,
            write_program: None,
        })
    }

    fn delete_rows(&self) -> std::result::Result<GivenRowsSpec, SdkExecutionDiagnostic> {
        let mut rows = Vec::new();
        rows.try_reserve_exact(self.row_count)
            .map_err(|_| allocation_failure())?;
        for (ordinal, row) in &self.rows {
            rows.push(vec![
                ordinal_value(*ordinal)?,
                GivenValue::String(
                    row_iid(row)
                        .expect("normalized delete IID")
                        .to_ascii_lowercase(),
                ),
            ]);
        }
        Ok(GivenRowsSpec {
            variables: vec!["ordinal".to_owned(), "target-iid".to_owned()],
            rows,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn preflight_route_shape(
    prepared: &PreparedProjectedBatchInvocation<'_>,
    model: &ModelProjection,
    operation: ProjectedBatchOperation,
    rows: &[(usize, &ProjectedBatchRow)],
    fields: &[FieldRoute],
    roles: &[RoleRoute],
    reference_routes: &[ReferenceRoute],
    references: &[RoleReference<'_>],
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    let put_columns = if operation == ProjectedBatchOperation::Put {
        model.reference_read().key_fields().len()
    } else {
        0
    };
    let reference_key_columns = reference_routes
        .iter()
        .filter(|route| route.key_variable.is_some())
        .count();
    let prerequisite_columns = 5_usize
        .checked_add(put_columns)
        .and_then(|value| value.checked_add(reference_key_columns))
        .ok_or_else(complexity_overflow)?;
    let reference_branches = reference_routes
        .iter()
        .try_fold(0_usize, |total, route| {
            total.checked_add(route.candidate_types.len())
        })
        .ok_or_else(complexity_overflow)?;
    let attach_columns = 2_usize
        .checked_add(fields.len())
        .and_then(|value| value.checked_add(roles.len()))
        .ok_or_else(complexity_overflow)?;
    for (dimension, actual) in [
        ("given_columns", prerequisite_columns.max(attach_columns)),
        (
            "branches",
            reference_branches
                .saturating_add(usize::from(operation == ProjectedBatchOperation::Put)),
        ),
    ] {
        let actual = u64::try_from(actual).unwrap_or(u64::MAX);
        if actual > COMPILER_CARDINALITY_MAX {
            return Err(resource_limit(
                "projected_batch_compilation_complexity_limit",
                "Projected batch schema routing exceeds the provider compiler cardinality",
                dimension,
                actual,
                COMPILER_CARDINALITY_MAX,
            ));
        }
    }

    let prerequisite_rows = references
        .len()
        .checked_add(if operation == ProjectedBatchOperation::Put {
            rows.len()
        } else {
            0
        })
        .ok_or_else(complexity_overflow)?;
    let mutation_columns = match operation {
        ProjectedBatchOperation::Insert => 1,
        ProjectedBatchOperation::Update | ProjectedBatchOperation::Delete => 2,
        ProjectedBatchOperation::Put => 1_usize
            .checked_add(model.reference_read().key_fields().len())
            .ok_or_else(complexity_overflow)?,
    };
    let attach_rows = if operation == ProjectedBatchOperation::Delete {
        0
    } else {
        rows.iter()
            .try_fold(0_usize, |total, (_, row)| {
                let create = row_create(row).expect("non-delete batch row has create");
                let events = create
                    .fields()
                    .values()
                    .try_fold(0_usize, |count, values| count.checked_add(values.len()))?
                    .checked_add(
                        create
                            .roles()
                            .values()
                            .try_fold(0_usize, |count, values| count.checked_add(values.len()))?,
                    )?
                    .max(1);
                total.checked_add(events)
            })
            .ok_or_else(complexity_overflow)?
    };
    let matrix_bytes = [
        (prerequisite_rows, prerequisite_columns),
        (rows.len(), mutation_columns),
        (attach_rows, attach_columns),
    ]
    .into_iter()
    .try_fold(0_usize, |total, (row_count, column_count)| {
        total.checked_add(dense_matrix_allocation_bytes(row_count, column_count)?)
    })
    .ok_or_else(complexity_overflow)?;
    let iid_string_count = attach_rows
        .checked_add(references.len())
        .and_then(|value| value.checked_add(references.len()))
        .and_then(|value| {
            value.checked_add(
                if matches!(
                    operation,
                    ProjectedBatchOperation::Update | ProjectedBatchOperation::Delete
                ) {
                    rows.len()
                } else {
                    0
                },
            )
        })
        .ok_or_else(complexity_overflow)?;
    let iid_heap_bytes = iid_string_count
        .checked_mul(MAX_PROVIDER_IID_BYTES as usize)
        .ok_or_else(complexity_overflow)?;
    let output_reservation_bytes = rows
        .len()
        .checked_mul(
            std::mem::size_of::<Option<String>>()
                + std::mem::size_of::<String>()
                + std::mem::size_of::<Option<ProjectedThing>>()
                + std::mem::size_of::<ProjectedThing>(),
        )
        .ok_or_else(complexity_overflow)?;
    // Scalar strings/temporals are cloned into the synthetic sizing rows and
    // later into the one retained provider matrix. Charging two additional
    // input-sized regions is conservative and keeps preparation beneath the
    // same captured allocation ceiling even at hard V3 shapes.
    let scalar_clone_bytes = usize::try_from(prepared.batch().resource_measure().bytes())
        .unwrap_or(usize::MAX)
        .checked_mul(2)
        .ok_or_else(complexity_overflow)?;
    let allocation_bytes = matrix_bytes
        .checked_add(iid_heap_bytes)
        .and_then(|value| value.checked_add(output_reservation_bytes))
        .and_then(|value| value.checked_add(scalar_clone_bytes))
        .ok_or_else(complexity_overflow)?;
    let actual = u64::try_from(allocation_bytes).unwrap_or(u64::MAX);
    let ceiling = prepared
        .limits()
        .bytes
        .saturating_sub(prepared.batch().resource_measure().bytes());
    if actual > ceiling {
        return Err(resource_limit(
            "projected_batch_preparation_allocation_limit",
            "Projected batch GivenRows preparation exceeds the captured byte ceiling",
            "bytes",
            actual,
            ceiling,
        ));
    }
    Ok(())
}

fn dense_matrix_allocation_bytes(rows: usize, columns: usize) -> Option<usize> {
    let cells = rows.checked_mul(columns)?;
    cells
        .checked_mul(std::mem::size_of::<GivenValue>())?
        .checked_add(rows.checked_mul(std::mem::size_of::<Vec<GivenValue>>())?)
}

fn empty_event(
    width: usize,
    ordinal: usize,
    iid: Option<&str>,
) -> std::result::Result<Vec<GivenValue>, SdkExecutionDiagnostic> {
    let mut event = Vec::new();
    event
        .try_reserve_exact(width)
        .map_err(|_| allocation_failure())?;
    event.push(ordinal_value(ordinal)?);
    let target_iid = match iid {
        Some(iid) => GivenValue::String(iid.to_owned()),
        None => {
            let mut value = String::new();
            value
                .try_reserve_exact(MAX_PROVIDER_IID_BYTES as usize)
                .map_err(|_| allocation_failure())?;
            GivenValue::String(value)
        }
    };
    event.push(target_iid);
    event.resize(width, GivenValue::Empty);
    Ok(event)
}

fn compile_field_routes(
    installed: &InstalledRuntimeProjection,
    model: &ModelProjection,
) -> std::result::Result<Vec<FieldRoute>, SdkExecutionDiagnostic> {
    let mut routes = Vec::new();
    routes
        .try_reserve_exact(model.create().fields().len())
        .map_err(|_| allocation_failure())?;
    for (index, field) in model
        .create()
        .fields()
        .iter()
        .filter(|field| field.multiplicity().collection_mode().is_unordered())
        .enumerate()
    {
        field_typeql_type(installed, field.token())?;
        routes.push(FieldRoute {
            field: field.token().clone(),
            variable: format!("field-{index}"),
        });
    }
    Ok(routes)
}

fn compile_role_routes<'a>(
    installed: &InstalledRuntimeProjection,
    model: &ModelProjection,
    rows: &[(usize, &'a ProjectedBatchRow)],
) -> std::result::Result<(Vec<RoleRoute>, Vec<RoleReference<'a>>), SdkExecutionDiagnostic> {
    let mut keys = BTreeSet::new();
    let mut references = Vec::new();
    for (owner_ordinal, row) in rows {
        let Some(create) = row_create(row) else {
            continue;
        };
        for (role, players) in create.roles() {
            for player in players {
                let reference_ordinal = references.len();
                for candidate in concrete_role_candidates(installed, model, role, player.type_id())?
                {
                    keys.insert(RoleRouteKey {
                        role: role.clone(),
                        player_type: candidate,
                    });
                }
                references.push(RoleReference {
                    owner_ordinal: *owner_ordinal,
                    reference_ordinal,
                    role: role.clone(),
                    reference: player,
                });
            }
        }
    }
    let mut routes = Vec::new();
    routes
        .try_reserve_exact(keys.len())
        .map_err(|_| allocation_failure())?;
    for (index, key) in keys.into_iter().enumerate() {
        validate_provider_role_descriptor(installed, model, &key.role)?;
        routes.push(RoleRoute {
            key,
            variable: format!("player-iid-{index}"),
        });
    }
    Ok((routes, references))
}

fn concrete_role_candidates(
    installed: &InstalledRuntimeProjection,
    relation: &ModelProjection,
    role: &RoleId,
    referenced_type: &TypeId,
) -> std::result::Result<Vec<TypeId>, SdkExecutionDiagnostic> {
    let projected_role = relation
        .create()
        .roles()
        .get(role)
        .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
    let token = relation
        .query_tokens()
        .roles()
        .get(role)
        .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
    let mut candidates = installed
        .projection()
        .models()
        .values()
        .filter(|candidate| {
            matches!(candidate.id().kind(), TypeKind::Entity | TypeKind::Relation)
                && !candidate.declaration().is_abstract()
                && candidate.declaration().is_constructible()
                && is_same_or_subtype(installed, candidate.id(), referenced_type)
                && projected_role
                    .players()
                    .iter()
                    .any(|allowed| is_same_or_subtype(installed, candidate.id(), allowed.id()))
                && token
                    .accepted_players()
                    .iter()
                    .any(|allowed| is_same_or_subtype(installed, candidate.id(), allowed))
        })
        .map(|candidate| candidate.id().clone())
        .collect::<Vec<_>>();
    candidates.sort();
    if candidates.is_empty() {
        return Err(integrity_at_reference(
            "projected_role_candidate_missing",
            "A validated projected role reference has no concrete provider candidate",
            relation.id(),
            role,
            0,
            0,
            referenced_type,
        ));
    }
    Ok(candidates)
}

fn validate_provider_role_descriptor(
    installed: &InstalledRuntimeProjection,
    model: &ModelProjection,
    role: &RoleId,
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    let descriptor = installed
        .relation_descriptor(model.id())
        .map_err(|error| runtime_projection_error(error, model.id()))?;
    let token = model
        .query_tokens()
        .roles()
        .get(role)
        .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
    let mut matches = descriptor
        .roles
        .iter()
        .filter(|candidate| candidate.role_name == token.role().label().as_str());
    let Some(found) = matches.next() else {
        return Err(integrity_at_role(
            "projected_role_descriptor_mismatch",
            "The projected role has no exact provider descriptor",
            model.id(),
            role,
        ));
    };
    if matches.next().is_some()
        || found.player_type_names.len() != token.accepted_players().len()
        || !token.accepted_players().iter().all(|player| {
            found
                .player_type_names
                .iter()
                .filter(|label| label.as_str() == player.label().as_str())
                .count()
                == 1
        })
    {
        return Err(integrity_at_role(
            "projected_role_descriptor_mismatch",
            "The projected role does not match one exact provider descriptor",
            model.id(),
            role,
        ));
    }
    Ok(())
}

fn compile_reference_routes(
    installed: &InstalledRuntimeProjection,
    model: &ModelProjection,
    references: &[RoleReference<'_>],
) -> std::result::Result<Vec<ReferenceRoute>, SdkExecutionDiagnostic> {
    let mut keys = BTreeMap::<ReferenceRouteKey, BTreeSet<TypeId>>::new();
    for reference in references {
        let key = match reference.reference.iid() {
            Some(_) => ReferenceRouteKey::Iid(
                reference.role.clone(),
                reference.reference.type_id().clone(),
            ),
            None => {
                let Some((field, _)) = reference.reference.keys().first_key_value() else {
                    return Err(SdkExecutionDiagnostic::internal_failure());
                };
                ReferenceRouteKey::Key(
                    reference.role.clone(),
                    reference.reference.type_id().clone(),
                    field.clone(),
                )
            }
        };
        keys.entry(key)
            .or_default()
            .extend(concrete_role_candidates(
                installed,
                model,
                &reference.role,
                reference.reference.type_id(),
            )?);
    }
    let mut routes = Vec::new();
    routes
        .try_reserve_exact(keys.len())
        .map_err(|_| allocation_failure())?;
    for (index, (key, candidate_types)) in keys.into_iter().enumerate() {
        let key_variable = match &key {
            ReferenceRouteKey::Iid(_, _) => None,
            ReferenceRouteKey::Key(_, _, field) => {
                field_typeql_type(installed, field)?;
                Some(format!("reference-key-{index}"))
            }
        };
        routes.push(ReferenceRoute {
            key,
            candidate_types: candidate_types.into_iter().collect(),
            key_variable,
        });
    }
    Ok(routes)
}

struct DeleteIdentityConsumer {
    slots: Vec<Option<String>>,
    deferred: Option<SdkExecutionDiagnostic>,
}

impl DeleteIdentityConsumer {
    fn new(row_count: usize) -> std::result::Result<Self, SdkExecutionDiagnostic> {
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(row_count)
            .map_err(|_| allocation_failure())?;
        slots.resize(row_count, None);
        Ok(Self {
            slots,
            deferred: None,
        })
    }

    fn finish(&self, plan: &BatchPlan<'_>) -> std::result::Result<(), SdkExecutionDiagnostic> {
        if let Some(error) = &self.deferred {
            return Err(error.clone());
        }
        for (ordinal, value) in self.slots.iter().enumerate() {
            if let Some(returned) = value.as_deref().filter(|iid| !iid.is_empty()) {
                let expected = row_iid(plan.rows[ordinal].1)
                    .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                if !is_canonical_thing_iid(returned) || !returned.eq_ignore_ascii_case(expected) {
                    return Err(integrity_at_row(
                        "projected_batch_delete_identity_changed",
                        "A projected delete returned a different or malformed target IID",
                        plan.model.id(),
                        ordinal,
                    ));
                }
            }
        }
        Ok(())
    }

    fn take_deferred(&mut self) -> Option<SdkExecutionDiagnostic> {
        self.deferred.take()
    }
}

impl AnswerConsumer for DeleteIdentityConsumer {
    fn accept(&mut self, item: AnswerItem) -> crate::Result<AnswerControl> {
        if self.deferred.is_some() {
            return Ok(AnswerControl::Continue);
        }
        let result = (|| {
            let AnswerItem::Document(document) = item else {
                return Err(integrity(
                    "projected_batch_answer_kind_invalid",
                    "A projected batch mutation returned rows instead of documents",
                ));
            };
            let ordinal = document_index(&document, "ordinal", self.slots.len())?;
            let iid = document.as_object().and_then(|object| object.get("iid"));
            let mut value = match iid {
                None => String::new(),
                Some(value) => unwrap_scalar(value)
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        integrity(
                            "projected_batch_mutation_identity_invalid",
                            "A projected batch mutation returned a malformed IID field",
                        )
                    })?,
            };
            value.make_ascii_lowercase();
            if self.slots[ordinal].replace(value).is_some() {
                return Err(integrity(
                    "projected_batch_mutation_result_duplicate",
                    "A projected batch mutation repeated one input ordinal",
                ));
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.deferred = Some(error);
        }
        Ok(AnswerControl::Continue)
    }
}

#[allow(clippy::too_many_arguments)]
struct LocallyBoundedConsumer<'a> {
    inner: &'a mut dyn AnswerConsumer,
    reader: BoundedAnswerReader,
}

impl<'a> LocallyBoundedConsumer<'a> {
    fn new(inner: &'a mut dyn AnswerConsumer, limits: BoundedAnswerLimits) -> Self {
        Self {
            inner,
            reader: BoundedAnswerReader::new(limits),
        }
    }

    const fn stats(&self) -> BoundedAnswerStats {
        self.reader.stats()
    }
}

impl AnswerConsumer for LocallyBoundedConsumer<'_> {
    fn accept(&mut self, item: AnswerItem) -> crate::Result<AnswerControl> {
        self.reader.accept(item, self.inner)
    }
}

pub(crate) struct ExecutionLedger {
    limits: QueryExecutionResourceLimits,
    bytes: u64,
    items: u64,
    statements: u32,
}

impl ExecutionLedger {
    pub(crate) const fn new(limits: QueryExecutionResourceLimits, input_bytes: u64) -> Self {
        Self {
            limits,
            bytes: input_bytes,
            items: 0,
            statements: 0,
        }
    }

    pub(crate) fn charge_preflight(
        &mut self,
        request_bytes: u64,
        statements: u32,
    ) -> std::result::Result<(), SdkExecutionDiagnostic> {
        if statements > self.limits.statement_limit() {
            return Err(resource_limit(
                "projected_batch_statement_limit",
                "Projected batch execution exceeds the provider statement ceiling",
                "statements",
                u64::from(statements),
                u64::from(self.limits.statement_limit()),
            ));
        }
        let total = self.bytes.checked_add(request_bytes).ok_or_else(|| {
            resource_limit(
                "projected_batch_transport_byte_limit",
                "Projected batch transport byte accounting overflowed",
                "bytes",
                u64::MAX,
                self.limits.bytes,
            )
        })?;
        if total > self.limits.bytes {
            return Err(resource_limit(
                "projected_batch_transport_byte_limit",
                "Projected batch request transport exceeds the byte ceiling",
                "bytes",
                total,
                self.limits.bytes,
            ));
        }
        self.bytes = total;
        self.statements = statements;
        Ok(())
    }

    pub(crate) fn record_reply(
        &mut self,
        stats: BoundedAnswerStats,
    ) -> std::result::Result<(), SdkExecutionDiagnostic> {
        let items = self.items.saturating_add(stats.processed_items);
        if items > self.limits.items {
            return Err(resource_limit(
                "projected_batch_reply_item_limit",
                "Projected batch cumulative provider answers exceed the item ceiling",
                "items",
                items,
                self.limits.items,
            ));
        }
        self.items = items;
        self.charge_bytes(stats.response_bytes, "projected_batch_reply_byte_limit")
    }

    pub(crate) fn remaining_bytes(&self) -> u64 {
        self.limits.bytes.saturating_sub(self.bytes)
    }

    pub(crate) fn remaining_items(&self) -> u64 {
        self.limits.items.saturating_sub(self.items)
    }

    pub(crate) fn charge_projected(
        &mut self,
        things: &[ProjectedThing],
    ) -> std::result::Result<(), SdkExecutionDiagnostic> {
        let mut bytes = 0_u64;
        let mut graph_nodes = 0_u64;
        let mut attributes = 0_u64;
        let mut members = 0_u64;
        let mut role_players = 0_u64;
        for thing in things {
            bytes = checked_add_u64(
                bytes,
                u64::try_from(thing.resource_measure().bytes()).unwrap_or(u64::MAX),
            )?;
            graph_nodes = checked_add_u64(graph_nodes, 1)?;
            for values in thing.fields().values() {
                let count = u64::try_from(values.len()).unwrap_or(u64::MAX);
                attributes = checked_add_u64(attributes, count)?;
                members = checked_add_u64(members, count)?;
            }
            for players in thing.roles().values() {
                let count = u64::try_from(players.len()).unwrap_or(u64::MAX);
                graph_nodes = checked_add_u64(graph_nodes, count)?;
                role_players = checked_add_u64(role_players, count)?;
                members = checked_add_u64(members, count)?;
                for player in players {
                    for values in player.fields().values() {
                        let count = u64::try_from(values.len()).unwrap_or(u64::MAX);
                        attributes = checked_add_u64(attributes, count)?;
                        members = checked_add_u64(members, count)?;
                    }
                    if player.fields().is_empty() {
                        let count = u64::try_from(player.keys().len()).unwrap_or(u64::MAX);
                        attributes = checked_add_u64(attributes, count)?;
                        members = checked_add_u64(members, count)?;
                    }
                }
            }
        }
        for (name, actual, ceiling) in [
            ("graph_nodes", graph_nodes, self.limits.graph_nodes),
            ("attribute_values", attributes, self.limits.attribute_values),
            (
                "collection_members",
                members,
                self.limits.collection_members,
            ),
            ("role_players", role_players, self.limits.role_players),
        ] {
            if actual > ceiling {
                return Err(resource_limit(
                    "projected_batch_output_resource_limit",
                    "Projected batch output exceeds a captured resource ceiling",
                    name,
                    actual,
                    ceiling,
                ));
            }
        }
        self.charge_bytes(bytes, "projected_batch_output_byte_limit")
    }

    fn charge_bytes(
        &mut self,
        amount: u64,
        code_value: &'static str,
    ) -> std::result::Result<(), SdkExecutionDiagnostic> {
        let actual = self.bytes.saturating_add(amount);
        if actual > self.limits.bytes {
            return Err(resource_limit(
                code_value,
                "Projected batch cumulative bytes exceed the captured ceiling",
                "bytes",
                actual,
                self.limits.bytes,
            ));
        }
        self.bytes = actual;
        Ok(())
    }
}

struct HydrationConsumer<'executor, 'projection, 'plan> {
    executor: &'executor ProjectedBatchExecutor<'projection>,
    plan: &'plan BatchPlan<'plan>,
    identity: &'plan DatabaseExecutionIdentity,
    targets: Vec<String>,
    resolved_references: Option<Box<crate::plan_builders::bulk_write::ReferenceSnapshots>>,
    slots: Vec<Option<ProjectedThing>>,
    ordered: crate::BatchWriteResult<ProjectedThing>,
    additional: Option<BTreeMap<usize, Vec<ProjectedThing>>>,
    single_documents: Vec<Value>,
    deferred: Option<SdkExecutionDiagnostic>,
}

impl<'executor, 'projection, 'plan> HydrationConsumer<'executor, 'projection, 'plan> {
    fn new(
        executor: &'executor ProjectedBatchExecutor<'projection>,
        plan: &'plan BatchPlan<'plan>,
        identity: &'plan DatabaseExecutionIdentity,
    ) -> std::result::Result<Self, SdkExecutionDiagnostic> {
        let mut targets = Vec::new();
        targets
            .try_reserve_exact(plan.row_count)
            .map_err(|_| allocation_failure())?;
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(plan.row_count)
            .map_err(|_| allocation_failure())?;
        slots.resize_with(plan.row_count, || None);
        let ordered = crate::BatchWriteResult::try_with_capacity(plan.row_count)
            .map_err(|_| allocation_failure())?;
        Ok(Self {
            executor,
            plan,
            identity,
            targets,
            resolved_references: None,
            slots,
            ordered,
            additional: None,
            single_documents: Vec::new(),
            deferred: None,
        })
    }

    fn accept_document(
        &mut self,
        mut document: Value,
    ) -> std::result::Result<(), SdkExecutionDiagnostic> {
        #[cfg(test)]
        if self.executor.fail_hydration_allocation {
            return Err(allocation_failure());
        }
        let ordinal = document_index(&document, "ordinal", self.plan.row_count)?;
        if self.plan.single_input {
            if self.plan.model.id().kind() == TypeKind::Relation {
                let descriptor = self
                    .executor
                    .installed
                    .relation_descriptor(self.plan.model.id())
                    .map_err(|error| runtime_projection_error(error, self.plan.model.id()))?;
                normalize_scoped_role_labels(self.plan.model, descriptor, &mut document)?;
            }
            self.capture_reference_snapshot(&document, ordinal)?;
            self.single_documents
                .try_reserve(1)
                .map_err(|_| allocation_failure())?;
            self.single_documents.push(document);
            return Ok(());
        }
        let returned_iid = document
            .get("_iid")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                integrity_at_row(
                    "projected_batch_hydration_target_missing",
                    "Provider hydration omitted its mutation identity",
                    self.plan.model.id(),
                    ordinal,
                )
            })?
            .to_owned();
        if self.slots[ordinal]
            .as_ref()
            .is_some_and(|thing| thing.iid().eq_ignore_ascii_case(&returned_iid))
            || self
                .additional
                .as_ref()
                .and_then(|rows| rows.get(&ordinal))
                .is_some_and(|rows| {
                    rows.iter()
                        .any(|thing| thing.iid().eq_ignore_ascii_case(&returned_iid))
                })
        {
            return Err(integrity_at_row(
                "projected_batch_hydration_duplicate",
                "Provider hydration repeated one root within its input row",
                self.plan.model.id(),
                ordinal,
            ));
        }
        let expected = if self.plan.operation == ProjectedBatchOperation::Update {
            self.targets
                .get(ordinal)
                .map(String::as_str)
                .ok_or_else(|| {
                    integrity_at_row(
                        "projected_batch_hydration_target_missing",
                        "Provider hydration arrived before its mutation identity",
                        self.plan.model.id(),
                        ordinal,
                    )
                })?
        } else {
            returned_iid.as_str()
        };
        if self.plan.operation == ProjectedBatchOperation::Update
            && !returned_iid.eq_ignore_ascii_case(expected)
        {
            return Err(integrity_at_row(
                "projected_batch_update_identity_changed",
                "A projected update returned a different exact target IID",
                self.plan.model.id(),
                ordinal,
            ));
        }
        let thing = match self.plan.model.id().kind() {
            TypeKind::Entity => {
                let descriptor = self
                    .executor
                    .installed
                    .entity_descriptor(self.plan.model.id())
                    .map_err(|error| runtime_projection_error(error, self.plan.model.id()))?;
                let row = hydrate_dynamic_entity(descriptor, &document).map_err(|error| {
                    at_type(
                        lower_orm_error(&error, SdkProviderOperation::Read),
                        self.plan.model.id(),
                    )
                })?;
                self.executor
                    .project_entity(row, self.plan.model.id(), expected, self.identity)?
            }
            TypeKind::Relation => {
                let descriptor = self
                    .executor
                    .installed
                    .relation_descriptor(self.plan.model.id())
                    .map_err(|error| runtime_projection_error(error, self.plan.model.id()))?;
                normalize_scoped_role_labels(self.plan.model, descriptor, &mut document)?;
                let row = hydrate_dynamic_relation(descriptor, &document).map_err(|error| {
                    at_type(
                        lower_orm_error(&error, SdkProviderOperation::Read),
                        self.plan.model.id(),
                    )
                })?;
                self.executor.project_relation(
                    row,
                    self.plan.model.id(),
                    expected,
                    self.identity,
                )?
            }
            _ => return Err(SdkExecutionDiagnostic::internal_failure()),
        };
        self.capture_reference_snapshot(&document, ordinal)?;
        if self.slots[ordinal].is_none() {
            self.slots[ordinal] = Some(thing);
        } else {
            let additional = self
                .additional
                .get_or_insert_default()
                .entry(ordinal)
                .or_default();
            additional
                .try_reserve(1)
                .map_err(|_| allocation_failure())?;
            additional.push(thing);
        }
        Ok(())
    }

    fn capture_reference_snapshot(
        &mut self,
        document: &Value,
        ordinal: usize,
    ) -> std::result::Result<(), SdkExecutionDiagnostic> {
        if self.plan.capture_role_snapshots
            && !self
                .resolved_references
                .as_ref()
                .is_some_and(|references| references.contains_key(&ordinal))
        {
            let captured = document_reference_snapshots(
                self.executor.installed,
                self.plan,
                document,
                ordinal,
            )?;
            self.resolved_references
                .get_or_insert_default()
                .insert(ordinal, captured);
        }
        Ok(())
    }

    fn take_deferred(&mut self) -> Option<SdkExecutionDiagnostic> {
        self.deferred.take()
    }

    fn finish(
        mut self,
    ) -> std::result::Result<crate::BatchWriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        if let Some(error) = self.deferred {
            return Err(error);
        }
        if self.plan.single_input {
            let executor = crate::ProjectedCrudExecutor::new(self.executor.installed);
            let type_id = self.plan.model.id();
            let mode = crate::projected_crud::ExecutionMode::default();
            let result = match type_id.kind() {
                TypeKind::Entity => executor.hydrate_entity_documents(
                    self.identity,
                    type_id,
                    self.executor
                        .installed
                        .entity_descriptor(type_id)
                        .map_err(|error| runtime_projection_error(error, type_id))?,
                    &self.single_documents,
                    mode,
                ),
                TypeKind::Relation => executor.hydrate_relation_documents(
                    self.identity,
                    type_id,
                    self.executor
                        .installed
                        .relation_descriptor(type_id)
                        .map_err(|error| runtime_projection_error(error, type_id))?,
                    &self.single_documents,
                    mode,
                ),
                _ => return Err(SdkExecutionDiagnostic::internal_failure()),
            }?;
            if self.plan.operation == ProjectedBatchOperation::Update {
                let expected = self
                    .targets
                    .first()
                    .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                for thing in result.items() {
                    if !thing.iid().eq_ignore_ascii_case(expected) {
                        return Err(integrity_at_row(
                            "projected_batch_update_identity_changed",
                            "A projected update returned a different exact target IID",
                            type_id,
                            0,
                        ));
                    }
                }
            }
            return crate::BatchWriteResult::try_from_input(result.into_items(), 0);
        }
        for (ordinal, value) in self.slots.into_iter().enumerate() {
            if let Some(thing) = value {
                self.ordered
                    .try_push(ordinal, thing)
                    .map_err(|_| allocation_failure())?;
            }
            if let Some(additional) = self
                .additional
                .as_mut()
                .and_then(|rows| rows.remove(&ordinal))
            {
                for thing in additional {
                    self.ordered
                        .try_push(ordinal, thing)
                        .map_err(|_| allocation_failure())?;
                }
            }
        }
        Ok(self.ordered)
    }
}

fn document_reference_snapshots(
    installed: &InstalledRuntimeProjection,
    plan: &BatchPlan<'_>,
    document: &Value,
    ordinal: usize,
) -> std::result::Result<BTreeMap<RoleId, Vec<String>>, SdkExecutionDiagnostic> {
    let mut snapshots = BTreeMap::new();
    for (index, role) in plan.model.complete_read().roles().keys().enumerate() {
        if !plan.role_references.iter().any(|reference| {
            &reference.role == role
                && reference.reference.iid().is_none()
                && is_same_or_subtype(installed, plan.model.id(), reference.reference.type_id())
        }) {
            continue;
        }
        let prefix = format!("reference_{index}_");
        let mut iids = Vec::new();
        for (key, value) in document
            .as_object()
            .ok_or_else(SdkExecutionDiagnostic::internal_failure)?
        {
            if !key.starts_with(&prefix) || value.is_null() {
                continue;
            }
            let iid = value
                .as_str()
                .filter(|iid| type_bridge_contract::id::is_canonical_thing_iid(iid))
                .ok_or_else(|| {
                    integrity_at_row(
                        "provider_identity_mismatch",
                        "Reference snapshot did not carry a canonical IID",
                        plan.model.id(),
                        ordinal,
                    )
                })?;
            iids.push(iid.to_ascii_lowercase());
        }
        iids.sort_unstable();
        snapshots.insert(role.clone(), iids);
    }
    Ok(snapshots)
}

fn validate_written_role_identities(
    installed: &InstalledRuntimeProjection,
    plan: &BatchPlan<'_>,
    thing: &ProjectedThing,
    ordinal: usize,
    snapshots: Option<&BTreeMap<RoleId, Vec<String>>>,
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    let create =
        row_create(plan.rows[ordinal].1).ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
    for (role, read_role) in plan.model.complete_read().roles() {
        if !read_role.multiplicity().collection_mode().is_unordered() {
            continue;
        }
        let requested = create
            .roles()
            .get(role)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let returned = thing
            .roles()
            .get(role)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if !plan.single_input && requested.len() != returned.len() {
            return Err(integrity_at_row(
                "provider_identity_mismatch",
                "Returned role players differ from the write input",
                plan.model.id(),
                ordinal,
            ));
        }
        let resolved = snapshots.and_then(|snapshots| snapshots.get(role));
        if let Some(resolved) = resolved {
            let mut returned_iids = returned
                .iter()
                .map(|player| player.iid().to_ascii_lowercase())
                .collect::<Vec<_>>();
            returned_iids.sort_unstable();
            let matches = if plan.single_input {
                returned_iids.dedup();
                let mut expected_iids = resolved.clone();
                expected_iids.sort_unstable();
                expected_iids.dedup();
                expected_iids == returned_iids
            } else {
                resolved == &returned_iids
            };
            if !matches {
                return Err(integrity_at_row(
                    "provider_identity_mismatch",
                    "Returned role players differ from the invocation reference IIDs",
                    plan.model.id(),
                    ordinal,
                ));
            }
        }
        for expected in requested {
            // An IID remains authoritative when this update changes that
            // same target's keys; those hydrated keys are post-write values.
            let updated_iid = !expected.keys().is_empty()
                && plan.operation == ProjectedBatchOperation::Update
                && is_same_or_subtype(installed, plan.model.id(), expected.type_id())
                && expected.iid().is_some_and(|iid| {
                    plan.rows.iter().any(|(_, row)| {
                        row_iid(row).is_some_and(|target| target.eq_ignore_ascii_case(iid))
                    })
                });
            let found = returned.iter().any(|player| {
                is_same_or_subtype(installed, player.type_id(), expected.type_id())
                    && expected
                        .iid()
                        .is_none_or(|iid| iid.eq_ignore_ascii_case(player.iid()))
                    && (resolved.is_some()
                        || updated_iid
                        || expected.keys().iter().all(|(field, value)| {
                            player.keys().iter().any(|(actual, supplied)| {
                                actual.attribute() == field.attribute()
                                    && supplied.value() == value.value()
                            })
                        }))
            });
            if !found {
                return Err(integrity_at_row(
                    "provider_identity_mismatch",
                    "Returned role player contradicts its requested IID or key",
                    plan.model.id(),
                    ordinal,
                ));
            }
        }
    }
    Ok(())
}

impl AnswerConsumer for HydrationConsumer<'_, '_, '_> {
    fn accept(&mut self, item: AnswerItem) -> crate::Result<AnswerControl> {
        if self.deferred.is_some() {
            return Ok(AnswerControl::Continue);
        }
        let result = match item {
            AnswerItem::Document(document) => self.accept_document(document),
            AnswerItem::Row(_) => Err(integrity_at_type(
                "projected_batch_answer_kind_invalid",
                "Projected batch hydration returned rows instead of documents",
                self.plan.model.id(),
            )),
        };
        if let Err(error) = result {
            self.deferred = Some(error);
        }
        Ok(AnswerControl::Continue)
    }
}

impl ProjectedBatchExecutor<'_> {
    fn project_entity(
        &self,
        row: DynamicEntityRow,
        type_id: &TypeId,
        expected_iid: &str,
        identity: &DatabaseExecutionIdentity,
    ) -> std::result::Result<ProjectedThing, SdkExecutionDiagnostic> {
        let iid = exact_row_identity(row.iid, row.type_name, type_id, expected_iid)?;
        let fields = self.project_fields(type_id, &row.attributes)?;
        ProjectedThing::try_new_for_database(
            self.installed,
            type_id.clone(),
            iid,
            fields,
            Vec::new(),
            identity.clone(),
        )
        .map_err(provider_integrity)
    }

    fn project_relation(
        &self,
        row: DynamicRelationRow,
        type_id: &TypeId,
        expected_iid: &str,
        identity: &DatabaseExecutionIdentity,
    ) -> std::result::Result<ProjectedThing, SdkExecutionDiagnostic> {
        let iid = exact_row_identity(row.iid, row.type_name, type_id, expected_iid)?;
        let fields = self.project_fields(type_id, &row.attributes)?;
        let roles = self.project_roles(type_id, &row.role_players, identity)?;
        ProjectedThing::try_new_for_database(
            self.installed,
            type_id.clone(),
            iid,
            fields,
            roles,
            identity.clone(),
        )
        .map_err(provider_integrity)
    }

    fn project_fields(
        &self,
        type_id: &TypeId,
        attributes: &DynamicAttributeMap,
    ) -> std::result::Result<Vec<(OwnsFactId, Vec<ProjectedAttributeValue>)>, SdkExecutionDiagnostic>
    {
        let model = self.model_integrity(type_id)?;
        let mut by_label = BTreeMap::<&str, Vec<&OwnsFactId>>::new();
        for field in model.complete_read().fields() {
            by_label
                .entry(field.token().attribute().label().as_str())
                .or_default()
                .push(field.token());
        }
        let mut evidence = BTreeMap::<OwnsFactId, Vec<&AttributeValue>>::new();
        for (label, value) in attributes {
            let candidates = by_label
                .get(label.as_str())
                .map(Vec::as_slice)
                .unwrap_or_default();
            let [field] = candidates else {
                return Err(integrity_at_type(
                    if candidates.is_empty() {
                        "unexpected_provider_attribute"
                    } else {
                        "ambiguous_provider_attribute"
                    },
                    if candidates.is_empty() {
                        "The provider row contains an unprojected ownership"
                    } else {
                        "The provider ownership maps to more than one projected field"
                    },
                    type_id,
                ));
            };
            evidence.entry((*field).clone()).or_default().push(value);
        }
        let mut fields = Vec::new();
        fields
            .try_reserve_exact(model.complete_read().fields().len())
            .map_err(|_| allocation_failure())?;
        for read in model.complete_read().fields() {
            let field = read.token().clone();
            let attribute_type = attribute_type(&field);
            let values = evidence.remove(&field).unwrap_or_default();
            let mut projected = Vec::new();
            projected
                .try_reserve_exact(values.len())
                .map_err(|_| allocation_failure())?;
            for value in values {
                projected.push(
                    ProjectedAttributeValue::try_from_hydrated_attribute_value(
                        self.installed,
                        attribute_type.clone(),
                        value,
                    )
                    .map_err(provider_integrity)?,
                );
            }
            fields.push((field, projected));
        }
        Ok(fields)
    }

    fn project_roles(
        &self,
        relation_type: &TypeId,
        players: &[DynamicRolePlayer],
        identity: &DatabaseExecutionIdentity,
    ) -> std::result::Result<Vec<(RoleId, Vec<ProjectedRolePlayer>)>, SdkExecutionDiagnostic> {
        let model = self.model_integrity(relation_type)?;
        let mut by_label = BTreeMap::<&str, Vec<&RoleId>>::new();
        for role in model.complete_read().roles().keys() {
            let token = model
                .query_tokens()
                .roles()
                .get(role)
                .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
            by_label
                .entry(token.role().label().as_str())
                .or_default()
                .push(role);
        }
        let mut evidence = BTreeMap::<RoleId, Vec<&DynamicRolePlayer>>::new();
        for player in players {
            let candidates = by_label
                .get(player.role_name.as_str())
                .map(Vec::as_slice)
                .unwrap_or_default();
            let [role] = candidates else {
                return Err(integrity_at_type(
                    if candidates.is_empty() {
                        "unexpected_provider_role"
                    } else {
                        "ambiguous_provider_role"
                    },
                    if candidates.is_empty() {
                        "The provider row contains an unprojected active role"
                    } else {
                        "The provider role maps to more than one projected role"
                    },
                    relation_type,
                ));
            };
            evidence.entry((*role).clone()).or_default().push(player);
        }
        let mut roles = Vec::new();
        roles
            .try_reserve_exact(model.complete_read().roles().len())
            .map_err(|_| allocation_failure())?;
        for (role, read_role) in model.complete_read().roles() {
            let token = model
                .query_tokens()
                .roles()
                .get(role)
                .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
            let players = evidence.remove(role).unwrap_or_default();
            if token
                .annotations()
                .keys()
                .any(|annotation| annotation.kind() == &AnnotationKindId::Distinct)
                && let Some((first, duplicate)) = first_player_duplicate(&players)
            {
                return Err(integrity_at_role_with_indices(
                    "ordered_distinct_duplicate",
                    "The provider repeated a player in a distinct projected role",
                    relation_type,
                    role,
                    first,
                    duplicate,
                ));
            }
            let mut projected = Vec::new();
            projected
                .try_reserve_exact(players.len())
                .map_err(|_| allocation_failure())?;
            for (index, player) in players.into_iter().enumerate() {
                projected.push(self.project_role_player(
                    relation_type,
                    role,
                    index,
                    read_role,
                    token.accepted_players(),
                    player,
                    identity,
                )?);
            }
            roles.push((role.clone(), projected));
        }
        Ok(roles)
    }

    #[allow(clippy::too_many_arguments)]
    fn project_role_player(
        &self,
        relation_type: &TypeId,
        role: &RoleId,
        index: usize,
        read_role: &ReadRoleProjection,
        token_players: &BTreeSet<TypeId>,
        player: &DynamicRolePlayer,
        identity: &DatabaseExecutionIdentity,
    ) -> std::result::Result<ProjectedRolePlayer, SdkExecutionDiagnostic> {
        let iid = player.player_iid.as_deref().ok_or_else(|| {
            integrity_at_role(
                "hydrated_player_iid_missing",
                "A provider role player omitted its canonical IID",
                relation_type,
                role,
            )
        })?;
        if !is_canonical_thing_iid(iid) {
            return Err(integrity_at_role(
                "hydrated_player_iid_invalid",
                "A provider role player returned a noncanonical IID",
                relation_type,
                role,
            ));
        }
        let label = player.player_type_name.as_deref().ok_or_else(|| {
            integrity_at_role(
                "hydrated_player_type_missing",
                "A provider role player omitted its exact concrete type",
                relation_type,
                role,
            )
        })?;
        let candidates = self
            .installed
            .projection()
            .models()
            .values()
            .filter(|candidate| {
                candidate.id().label().as_str() == label
                    && matches!(candidate.id().kind(), TypeKind::Entity | TypeKind::Relation)
                    && !candidate.declaration().is_abstract()
                    && candidate.declaration().is_constructible()
                    && read_role.players().iter().any(|allowed| {
                        is_same_or_subtype(self.installed, candidate.id(), allowed.id())
                    })
                    && token_players
                        .iter()
                        .any(|allowed| is_same_or_subtype(self.installed, candidate.id(), allowed))
            })
            .map(ModelProjection::id)
            .collect::<Vec<_>>();
        let [player_type] = candidates.as_slice() else {
            return Err(integrity_at_role(
                if candidates.is_empty() {
                    "hydrated_role_player_not_accepted"
                } else {
                    "projected_player_type_ambiguous"
                },
                if candidates.is_empty() {
                    "The provider role player type is outside the projected role domain"
                } else {
                    "The provider role player label does not select one projected type"
                },
                relation_type,
                role,
            ));
        };
        let form = ProjectedRolePlayer::form_for_read_role(self.installed, read_role, player_type)
            .map_err(provider_integrity)?;
        let attributes = self
            .installed
            .role_player_attributes(player_type, &player.attributes)
            .map_err(|error| {
                let diagnostic = at_type(
                    lower_orm_error(&error, SdkProviderOperation::Read),
                    relation_type,
                );
                append_path(
                    diagnostic,
                    [
                        SdkDiagnosticPathSegment::Role(role.clone()),
                        SdkDiagnosticPathSegment::Index(u64::try_from(index).unwrap_or(u64::MAX)),
                        SdkDiagnosticPathSegment::Type((*player_type).clone()),
                    ],
                )
            })?;
        let player_model = self.model_integrity(player_type)?;
        let fields = if form == ProjectedModelForm::Complete {
            self.project_fields(player_type, &attributes)?
        } else {
            Vec::new()
        };
        let keys = if form == ProjectedModelForm::Complete {
            fields
                .iter()
                .flat_map(|(field, values)| {
                    values
                        .iter()
                        .cloned()
                        .map(move |value| (field.clone(), value))
                })
                .filter(|(field, _)| player_model.reference_read().key_fields().contains(field))
                .collect()
        } else {
            self.project_reference_keys(player_type, player_model, &attributes)?
        };
        let reference = ProjectedReference::try_new_for_database(
            self.installed,
            (*player_type).clone(),
            Some(iid.to_ascii_lowercase()),
            keys,
            identity.clone(),
        )
        .map_err(provider_integrity)?;
        match form {
            ProjectedModelForm::Complete => ProjectedRolePlayer::try_new_complete_for_hydration(
                self.installed,
                read_role,
                reference,
                fields,
            ),
            ProjectedModelForm::Reference => ProjectedRolePlayer::try_new_reference_for_hydration(
                self.installed,
                read_role,
                reference,
            ),
        }
        .map_err(provider_integrity)
    }

    fn project_reference_keys(
        &self,
        player_type: &TypeId,
        model: &ModelProjection,
        attributes: &DynamicAttributeMap,
    ) -> std::result::Result<Vec<(OwnsFactId, ProjectedAttributeValue)>, SdkExecutionDiagnostic>
    {
        let mut keys = Vec::new();
        keys.try_reserve_exact(model.reference_read().key_fields().len())
            .map_err(|_| allocation_failure())?;
        for field in model.reference_read().key_fields() {
            let values = attributes
                .iter()
                .filter(|(label, _)| label == field.attribute().label().as_str())
                .map(|(_, value)| value)
                .collect::<Vec<_>>();
            match values.as_slice() {
                [value] => keys.push((
                    field.clone(),
                    ProjectedAttributeValue::try_from_hydrated_attribute_value(
                        self.installed,
                        attribute_type(field),
                        value,
                    )
                    .map_err(provider_integrity)?,
                )),
                [] => {
                    return Err(integrity_at_field(
                        "hydrated_reference_key_mismatch",
                        "The provider role player omitted a projected reference key",
                        player_type,
                        field,
                    ));
                }
                _ => {
                    return Err(integrity_at_field(
                        "duplicate_reference_key",
                        "The provider role player repeated a projected reference key",
                        player_type,
                        field,
                    ));
                }
            }
        }
        Ok(keys)
    }

    fn model_integrity(
        &self,
        type_id: &TypeId,
    ) -> std::result::Result<&ModelProjection, SdkExecutionDiagnostic> {
        self.installed
            .projection()
            .models()
            .get(type_id)
            .ok_or_else(|| {
                integrity_at_type(
                    "runtime_projection_mismatch",
                    "Provider hydration refers to a model outside the installed projection",
                    type_id,
                )
            })
    }
}

fn preflight_statement_complexity(source: &str) -> std::result::Result<(), SdkExecutionDiagnostic> {
    // These are intentionally conservative source-level upper bounds. Every
    // emitted variable occurrence, block and pipeline keyword is counted, so
    // generated delete/distinct/try/reduce stages and nested hydration can
    // never be omitted by a hand-maintained schema formula.
    let variables =
        u64::try_from(source.bytes().filter(|byte| *byte == b'$').count()).unwrap_or(u64::MAX);
    let branches =
        u64::try_from(source.bytes().filter(|byte| *byte == b'{').count()).unwrap_or(u64::MAX);
    let stages = u64::try_from(
        source
            .split(|character: char| !character.is_ascii_alphabetic())
            .filter(|token| {
                matches!(
                    *token,
                    "given"
                        | "match"
                        | "try"
                        | "insert"
                        | "put"
                        | "delete"
                        | "select"
                        | "distinct"
                        | "reduce"
                        | "fetch"
                )
            })
            .count(),
    )
    .unwrap_or(u64::MAX);
    for (dimension, actual) in [
        ("variables", variables),
        ("branches", branches),
        ("stages", stages),
    ] {
        if actual > COMPILER_CARDINALITY_MAX {
            return Err(resource_limit(
                "projected_batch_compilation_complexity_limit",
                "Projected batch schema routing exceeds the provider compiler cardinality",
                dimension,
                actual,
                COMPILER_CARDINALITY_MAX,
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn preflight_transport_bytes(
    prepared: &PreparedProjectedBatchInvocation<'_>,
    model: &ModelProjection,
    rows: &[(usize, &ProjectedBatchRow)],
    fields: &[FieldRoute],
    roles: &[RoleRoute],
    references: &[RoleReference<'_>],
    attach: bool,
) -> std::result::Result<u64, SdkExecutionDiagnostic> {
    let mut total = 0_u64;
    let mutation_rows = preflight_mutation_rows(prepared.batch().operation(), rows, model)?;
    total = checked_add_u64(total, given_rows_bytes(&mutation_rows)?)?;
    if attach {
        let attach_rows = preflight_attach_rows(rows, fields, roles, references)?;
        total = checked_add_u64(total, given_rows_bytes(&attach_rows)?)?;
    }
    Ok(total)
}

fn preflight_mutation_rows(
    operation: ProjectedBatchOperation,
    rows: &[(usize, &ProjectedBatchRow)],
    model: &ModelProjection,
) -> std::result::Result<GivenRowsSpec, SdkExecutionDiagnostic> {
    let key_fields = if operation == ProjectedBatchOperation::Put {
        model.reference_read().key_fields()
    } else {
        &[]
    };
    let mut variables = vec!["ordinal".to_owned()];
    match operation {
        ProjectedBatchOperation::Update | ProjectedBatchOperation::Delete => {
            variables.push("target-iid".to_owned())
        }
        ProjectedBatchOperation::Put => {
            variables.extend((0..key_fields.len()).map(|index| format!("put-key-{index}")));
        }
        ProjectedBatchOperation::Insert => {}
    }
    let mut output = Vec::new();
    output
        .try_reserve_exact(rows.len())
        .map_err(|_| allocation_failure())?;
    for (ordinal, row) in rows {
        let mut values = vec![ordinal_value(*ordinal)?];
        match operation {
            ProjectedBatchOperation::Update | ProjectedBatchOperation::Delete => values.push(
                GivenValue::String(row_iid(row).expect("identity row").to_ascii_lowercase()),
            ),
            ProjectedBatchOperation::Put => {
                let create = row_create(row).expect("put create");
                for field in key_fields {
                    values.push(given_from_canonical(
                        create.fields()[field][0].value().clone(),
                    )?);
                }
            }
            ProjectedBatchOperation::Insert => {}
        }
        output.push(values);
    }
    Ok(GivenRowsSpec {
        variables,
        rows: output,
    })
}

fn preflight_attach_rows(
    rows: &[(usize, &ProjectedBatchRow)],
    fields: &[FieldRoute],
    roles: &[RoleRoute],
    references: &[RoleReference<'_>],
) -> std::result::Result<GivenRowsSpec, SdkExecutionDiagnostic> {
    let mut variables = vec!["ordinal".to_owned(), "target-iid".to_owned()];
    variables.extend(fields.iter().map(|route| route.variable.clone()));
    variables.extend(roles.iter().map(|route| route.variable.clone()));
    let width = variables.len();
    let max_iid = format!("0x{}", "0".repeat(MAX_PROVIDER_IID_BYTES as usize - 2));
    let field_columns = fields
        .iter()
        .enumerate()
        .map(|(index, route)| (route.field.clone(), index + 2))
        .collect::<BTreeMap<_, _>>();
    let mut output = Vec::new();
    let mut reference_index = 0_usize;
    for (ordinal, row) in rows {
        let create = row_create(row).expect("non-delete attach row");
        let mut emitted = false;
        for (field, values) in create
            .fields()
            .iter()
            .filter(|(_, values)| !values.is_empty())
        {
            let column = *field_columns
                .get(field)
                .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
            for value in values {
                let mut event = empty_event(width, *ordinal, Some(&max_iid))?;
                event[column] = given_from_canonical(value.value().clone())?;
                output.push(event);
                emitted = true;
            }
        }
        for players in create.roles().values() {
            for _ in players {
                let prepared_reference = references
                    .get(reference_index)
                    .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                let column = roles
                    .iter()
                    .position(|route| route.key.role == prepared_reference.role)
                    .map(|index| index + 2 + fields.len())
                    .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                let mut event = empty_event(width, *ordinal, Some(&max_iid))?;
                event[column] = GivenValue::String(max_iid.clone());
                output.push(event);
                reference_index += 1;
                emitted = true;
            }
        }
        if !emitted {
            output.push(empty_event(width, *ordinal, Some(&max_iid))?);
        }
    }
    Ok(GivenRowsSpec {
        variables,
        rows: output,
    })
}

fn source_bytes(source: &str) -> std::result::Result<u64, SdkExecutionDiagnostic> {
    u64::try_from(source.len()).map_err(|_| complexity_overflow())
}

pub(crate) fn given_rows_bytes(
    rows: &impl serde::Serialize,
) -> std::result::Result<u64, SdkExecutionDiagnostic> {
    #[derive(Default)]
    struct CountingWriter(u64);

    impl std::io::Write for CountingWriter {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(u64::try_from(buffer.len()).unwrap_or(u64::MAX))
                .ok_or_else(|| std::io::Error::other("encoded GivenRows length overflow"))?;
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut writer = CountingWriter::default();
    serde_json::to_writer(&mut writer, rows)
        .map_err(|_| SdkExecutionDiagnostic::internal_failure())?;
    Ok(writer.0)
}

fn field_typeql_type(
    installed: &InstalledRuntimeProjection,
    field: &OwnsFactId,
) -> std::result::Result<&'static str, SdkExecutionDiagnostic> {
    let attribute = attribute_type(field);
    let tag = installed
        .projection()
        .models()
        .get(&attribute)
        .and_then(|model| model.declaration().value_type())
        .ok_or_else(|| {
            integrity_at_field(
                "projected_attribute_domain_missing",
                "A projected ownership has no canonical scalar domain",
                field.owner(),
                field,
            )
        })?;
    Ok(match tag {
        ValueTypeTag::String => "string",
        ValueTypeTag::Long => "integer",
        ValueTypeTag::Double => "double",
        ValueTypeTag::Boolean => "boolean",
        ValueTypeTag::Date => "date",
        ValueTypeTag::DateTime => "datetime",
        ValueTypeTag::DateTimeTz => "datetime-tz",
        ValueTypeTag::Decimal => "decimal",
        ValueTypeTag::Duration => "duration",
    })
}

fn given_from_canonical(
    value: CanonicalValue,
) -> std::result::Result<GivenValue, SdkExecutionDiagnostic> {
    use type_bridge_contract::temporal::TimeZoneDesignator;
    if let Err(error) = type_bridge_schema::validate_provider_temporal_value(&value) {
        return Err(SdkExecutionDiagnostic::invalid_input(
            code(error.code().as_str()),
            message("Projected temporal scalar is outside the exact provider transport domain"),
        ));
    }
    Ok(match value {
        CanonicalValue::String(value) => GivenValue::String(value.as_str().to_owned()),
        CanonicalValue::Long(value) => GivenValue::Integer(value),
        CanonicalValue::Double(value) => GivenValue::Double(value.get()),
        CanonicalValue::Boolean(value) => GivenValue::Boolean(value),
        CanonicalValue::Date(value) => GivenValue::Date(value.to_string()),
        CanonicalValue::DateTime(value) => GivenValue::Datetime(value.to_string()),
        CanonicalValue::DateTimeTz(value) => GivenValue::DatetimeTzExact {
            local: value.local().to_string(),
            named_zone: match value.zone() {
                TimeZoneDesignator::Named(name) => Some(name.clone()),
                TimeZoneDesignator::Utc | TimeZoneDesignator::OffsetSeconds(_) => None,
            },
            effective_offset_seconds: value.effective_offset_seconds(),
        },
        CanonicalValue::Decimal(value) => GivenValue::Decimal(value.as_str().to_owned()),
        CanonicalValue::Duration(value) => {
            let (negative, months, days, seconds, nanosecond) = value.components();
            debug_assert!(
                !negative,
                "provider temporal preflight rejects negative duration"
            );
            let months = u32::try_from(months).map_err(|_| {
                SdkExecutionDiagnostic::invalid_input(
                    code("provider_duration_out_of_range"),
                    message(
                        "Projected temporal scalar is outside the exact provider transport domain",
                    ),
                )
            })?;
            let days = u32::try_from(days).map_err(|_| {
                SdkExecutionDiagnostic::invalid_input(
                    code("provider_duration_out_of_range"),
                    message(
                        "Projected temporal scalar is outside the exact provider transport domain",
                    ),
                )
            })?;
            let nanos = seconds
                .checked_mul(1_000_000_000)
                .and_then(|seconds| seconds.checked_add(u64::from(nanosecond)))
                .ok_or_else(|| {
                    SdkExecutionDiagnostic::invalid_input(
                        code("provider_duration_out_of_range"),
                        message("Projected temporal scalar is outside the exact provider transport domain"),
                    )
                })?;
            GivenValue::Duration {
                months,
                days,
                nanos,
            }
        }
    })
}

fn validate_reference_origins(
    rows: &[(usize, &ProjectedBatchRow)],
    expected: &DatabaseExecutionIdentity,
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    for (ordinal, row) in rows {
        let Some(create) = row_create(row) else {
            continue;
        };
        for (role, references) in create.roles() {
            for (index, reference) in references.iter().enumerate() {
                if reference
                    .database_identity()
                    .is_some_and(|identity| identity != expected)
                {
                    return Err(append_path(
                        SdkExecutionDiagnostic::invalid_input(
                            code("reference_database_mismatch"),
                            message(
                                "The projected reference belongs to a different database execution identity",
                            ),
                        ),
                        [
                            SdkDiagnosticPathSegment::Argument(name("rows")),
                            SdkDiagnosticPathSegment::Index(
                                u64::try_from(*ordinal).unwrap_or(u64::MAX),
                            ),
                            SdkDiagnosticPathSegment::Role(role.clone()),
                            SdkDiagnosticPathSegment::Index(
                                u64::try_from(index).unwrap_or(u64::MAX),
                            ),
                            SdkDiagnosticPathSegment::Type(reference.type_id().clone()),
                        ],
                    ));
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_provider_write_inputs(
    rows: &[(usize, &ProjectedBatchRow)],
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    for (ordinal, row) in rows {
        let Some(create) = row_create(row) else {
            continue;
        };
        validate_provider_create_input(create, *ordinal)?;
    }
    Ok(())
}

pub(crate) fn validate_provider_create_input(
    create: &ProjectedCreate,
    ordinal: usize,
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    create.validate_native_write()?;
    for (field, values) in create.fields() {
        for value in values {
            if let Err(error) = type_bridge_schema::validate_provider_temporal_value(value.value())
            {
                return Err(append_path(
                    SdkExecutionDiagnostic::invalid_input(
                        code(error.code().as_str()),
                        message(
                            "Projected temporal scalar is outside the exact provider transport domain",
                        ),
                    ),
                    [
                        SdkDiagnosticPathSegment::Argument(name("rows")),
                        SdkDiagnosticPathSegment::Index(u64::try_from(ordinal).unwrap_or(u64::MAX)),
                        SdkDiagnosticPathSegment::Field(field.clone()),
                    ],
                ));
            }
        }
    }
    for (role, references) in create.roles() {
        for (reference_index, reference) in references.iter().enumerate() {
            for (field, value) in reference.keys() {
                if let Err(error) =
                    type_bridge_schema::validate_provider_temporal_value(value.value())
                {
                    return Err(append_path(
                        SdkExecutionDiagnostic::invalid_input(
                            code(error.code().as_str()),
                            message(
                                "Projected temporal scalar is outside the exact provider transport domain",
                            ),
                        ),
                        [
                            SdkDiagnosticPathSegment::Argument(name("rows")),
                            SdkDiagnosticPathSegment::Index(
                                u64::try_from(ordinal).unwrap_or(u64::MAX),
                            ),
                            SdkDiagnosticPathSegment::Role(role.clone()),
                            SdkDiagnosticPathSegment::Index(
                                u64::try_from(reference_index).unwrap_or(u64::MAX),
                            ),
                            SdkDiagnosticPathSegment::Type(reference.type_id().clone()),
                            SdkDiagnosticPathSegment::Field(field.clone()),
                        ],
                    ));
                }
            }
        }
    }
    Ok(())
}

fn row_create(row: &ProjectedBatchRow) -> Option<&ProjectedCreate> {
    match row {
        ProjectedBatchRow::Create(create) => Some(create),
        ProjectedBatchRow::Update { replacement, .. } => Some(replacement),
        ProjectedBatchRow::Delete { .. } => None,
    }
}

fn row_iid(row: &ProjectedBatchRow) -> Option<&str> {
    match row {
        ProjectedBatchRow::Update { iid, .. } | ProjectedBatchRow::Delete { iid } => Some(iid),
        ProjectedBatchRow::Create(_) => None,
    }
}

fn ordinal_value(ordinal: usize) -> std::result::Result<GivenValue, SdkExecutionDiagnostic> {
    i64::try_from(ordinal)
        .map(GivenValue::Integer)
        .map_err(|_| complexity_overflow())
}

fn unwrap_scalar(value: &Value) -> &Value {
    value.get("value").unwrap_or(value)
}

fn document_integer(
    document: &Value,
    key: &str,
) -> std::result::Result<i64, SdkExecutionDiagnostic> {
    document
        .as_object()
        .and_then(|object| object.get(key))
        .map(unwrap_scalar)
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            integrity(
                "projected_batch_ordinal_invalid",
                "Provider evidence omitted a required integer correlation value",
            )
        })
}

fn document_index(
    document: &Value,
    key: &str,
    ceiling: usize,
) -> std::result::Result<usize, SdkExecutionDiagnostic> {
    let value = document_integer(document, key)?;
    let value = usize::try_from(value).map_err(|_| {
        integrity(
            "projected_batch_ordinal_invalid",
            "Provider evidence returned an out-of-range correlation ordinal",
        )
    })?;
    if value >= ceiling {
        return Err(integrity(
            "projected_batch_ordinal_invalid",
            "Provider evidence returned an out-of-range correlation ordinal",
        ));
    }
    Ok(value)
}

fn document_string(
    document: &Value,
    key: &str,
) -> std::result::Result<String, SdkExecutionDiagnostic> {
    document
        .as_object()
        .and_then(|object| object.get(key))
        .map(unwrap_scalar)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| {
            integrity(
                "projected_batch_provider_value_invalid",
                "Provider evidence omitted a required scalar string",
            )
        })
}

fn exact_row_identity(
    iid: Option<String>,
    concrete_type: Option<String>,
    type_id: &TypeId,
    requested_iid: &str,
) -> std::result::Result<String, SdkExecutionDiagnostic> {
    let mut iid = iid.ok_or_else(|| {
        integrity_at_type(
            "hydrated_iid_missing",
            "The provider row omitted its canonical IID",
            type_id,
        )
    })?;
    if !iid.eq_ignore_ascii_case(requested_iid) || !is_canonical_thing_iid(&iid) {
        return Err(integrity_at_type(
            "hydrated_iid_mismatch",
            "The provider row returned a different or noncanonical IID",
            type_id,
        ));
    }
    if concrete_type.as_deref() != Some(type_id.label().as_str()) {
        return Err(integrity_at_type(
            "hydrated_type_mismatch",
            "The provider row returned a different exact concrete type",
            type_id,
        ));
    }
    iid.make_ascii_lowercase();
    Ok(iid)
}

fn normalize_scoped_role_labels(
    model: &ModelProjection,
    descriptor: &crate::_descriptor::RelationDescriptor,
    document: &mut Value,
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    let Some(players) = document
        .as_object_mut()
        .and_then(|object| object.get_mut("role_players"))
        .and_then(Value::as_array_mut)
    else {
        return Ok(());
    };
    for player in players {
        let Some(role) = player
            .as_object_mut()
            .and_then(|object| {
                if object.contains_key("role") {
                    object.get_mut("role")
                } else {
                    object.get_mut("role_name")
                }
            })
            .and_then(role_label_string_mut)
        else {
            continue;
        };
        let Some((scope, local)) = role.rsplit_once(':') else {
            continue;
        };
        let projected_matches = model
            .query_tokens()
            .roles()
            .keys()
            .filter(|candidate| {
                candidate.declaring_relation().as_str() == scope
                    && candidate.label().as_str() == local
            })
            .count();
        let descriptor_matches = descriptor
            .roles
            .iter()
            .filter(|candidate| candidate.role_name == local)
            .count();
        if projected_matches != 1 || descriptor_matches != 1 {
            return Err(integrity_at_type(
                "provider_hydration_failed",
                "Provider relation evidence returned an unknown scoped role",
                model.id(),
            ));
        }
        role.drain(..=scope.len());
    }
    Ok(())
}

fn role_label_string_mut(value: &mut Value) -> Option<&mut String> {
    match value {
        Value::String(value) => Some(value),
        Value::Object(object) => object.get_mut("value").and_then(role_label_string_mut),
        _ => None,
    }
}

fn first_player_duplicate(players: &[&DynamicRolePlayer]) -> Option<(usize, usize)> {
    let mut earliest = None;
    for (index, player) in players.iter().enumerate() {
        let (Some(type_name), Some(iid)) = (
            player.player_type_name.as_deref(),
            player.player_iid.as_deref(),
        ) else {
            continue;
        };
        if let Some(first) = players[..index].iter().position(|candidate| {
            candidate.player_type_name.as_deref() == Some(type_name)
                && candidate
                    .player_iid
                    .as_deref()
                    .is_some_and(|candidate_iid| candidate_iid.eq_ignore_ascii_case(iid))
        }) {
            let candidate = (first.min(index), first.max(index));
            if earliest.is_none_or(|current: (usize, usize)| {
                (candidate.1, candidate.0) < (current.1, current.0)
            }) {
                earliest = Some(candidate);
            }
        }
    }
    earliest
}

fn is_same_or_subtype(
    installed: &InstalledRuntimeProjection,
    candidate: &TypeId,
    ancestor: &TypeId,
) -> bool {
    let mut current = Some(candidate);
    let mut visited = BTreeSet::new();
    while let Some(type_id) = current {
        if type_id == ancestor {
            return true;
        }
        if !visited.insert(type_id) {
            return false;
        }
        current = installed
            .projection()
            .models()
            .get(type_id)
            .and_then(|model| model.declaration().parent());
    }
    false
}

fn attribute_type(field: &OwnsFactId) -> TypeId {
    TypeId::new(TypeKind::Attribute, field.attribute().label().as_str())
        .expect("validated ownership contains a canonical attribute label")
}

fn runtime_projection_error(error: OrmError, type_id: &TypeId) -> SdkExecutionDiagnostic {
    at_type(lower_orm_error(&error, SdkProviderOperation::Read), type_id)
}

fn provider_integrity(diagnostic: SdkExecutionDiagnostic) -> SdkExecutionDiagnostic {
    if matches!(
        diagnostic.category(),
        SdkDiagnosticCategory::Integrity | SdkDiagnosticCategory::ResourceLimit
    ) {
        diagnostic
    } else {
        let mut lowered = SdkExecutionDiagnostic::integrity(
            code(diagnostic.code().as_str()),
            message("Provider evidence violates the installed projected model"),
        );
        lowered = append_path(lowered, diagnostic.path().iter().cloned());
        lowered
    }
}

fn code(value: impl Into<String>) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("executor diagnostic code is canonical")
}

fn message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("executor diagnostic message is canonical")
}

fn name(value: impl Into<String>) -> SdkDiagnosticName {
    SdkDiagnosticName::new(value).expect("executor diagnostic name is canonical")
}

fn append_path(
    mut diagnostic: SdkExecutionDiagnostic,
    path: impl IntoIterator<Item = SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    for segment in path {
        diagnostic = diagnostic
            .try_at(segment)
            .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure());
    }
    diagnostic
}

fn at_type(diagnostic: SdkExecutionDiagnostic, type_id: &TypeId) -> SdkExecutionDiagnostic {
    append_path(
        diagnostic,
        [SdkDiagnosticPathSegment::Type(type_id.clone())],
    )
}

pub(crate) fn invalid_at_type(
    code_value: &'static str,
    message_value: &'static str,
    type_id: &TypeId,
) -> SdkExecutionDiagnostic {
    at_type(
        SdkExecutionDiagnostic::invalid_input(code(code_value), message(message_value)),
        type_id,
    )
}

fn integrity(code_value: &'static str, message_value: &'static str) -> SdkExecutionDiagnostic {
    SdkExecutionDiagnostic::integrity(code(code_value), message(message_value))
}

fn integrity_at_type(
    code_value: &'static str,
    message_value: &'static str,
    type_id: &TypeId,
) -> SdkExecutionDiagnostic {
    at_type(integrity(code_value, message_value), type_id)
}

fn integrity_at_field(
    code_value: &'static str,
    message_value: &'static str,
    type_id: &TypeId,
    field: &OwnsFactId,
) -> SdkExecutionDiagnostic {
    append_path(
        integrity(code_value, message_value),
        [
            SdkDiagnosticPathSegment::Type(type_id.clone()),
            SdkDiagnosticPathSegment::Field(field.clone()),
        ],
    )
}

fn integrity_at_role(
    code_value: &'static str,
    message_value: &'static str,
    type_id: &TypeId,
    role: &RoleId,
) -> SdkExecutionDiagnostic {
    append_path(
        integrity(code_value, message_value),
        [
            SdkDiagnosticPathSegment::Type(type_id.clone()),
            SdkDiagnosticPathSegment::Role(role.clone()),
        ],
    )
}

fn integrity_at_role_with_indices(
    code_value: &'static str,
    message_value: &'static str,
    type_id: &TypeId,
    role: &RoleId,
    first: usize,
    duplicate: usize,
) -> SdkExecutionDiagnostic {
    append_path(
        integrity_at_role(code_value, message_value, type_id, role),
        [SdkDiagnosticPathSegment::Index(
            u64::try_from(duplicate).unwrap_or(u64::MAX),
        )],
    )
    .try_with_detail(
        name("first_conflicting_index"),
        SdkDiagnosticDetailValue::Count(u64::try_from(first).unwrap_or(u64::MAX)),
    )
    .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure())
}

pub(crate) fn integrity_at_row(
    code_value: &'static str,
    message_value: &'static str,
    type_id: &TypeId,
    ordinal: usize,
) -> SdkExecutionDiagnostic {
    append_path(
        integrity(code_value, message_value),
        [
            SdkDiagnosticPathSegment::Argument(name("rows")),
            SdkDiagnosticPathSegment::Index(u64::try_from(ordinal).unwrap_or(u64::MAX)),
            SdkDiagnosticPathSegment::Type(type_id.clone()),
        ],
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn integrity_at_reference(
    code_value: &'static str,
    message_value: &'static str,
    relation_type: &TypeId,
    role: &RoleId,
    owner_ordinal: usize,
    reference_ordinal: usize,
    player_type: &TypeId,
) -> SdkExecutionDiagnostic {
    append_path(
        integrity(code_value, message_value),
        [
            SdkDiagnosticPathSegment::Argument(name("rows")),
            SdkDiagnosticPathSegment::Index(u64::try_from(owner_ordinal).unwrap_or(u64::MAX)),
            SdkDiagnosticPathSegment::Type(relation_type.clone()),
            SdkDiagnosticPathSegment::Role(role.clone()),
            SdkDiagnosticPathSegment::Index(u64::try_from(reference_ordinal).unwrap_or(u64::MAX)),
            SdkDiagnosticPathSegment::Type(player_type.clone()),
        ],
    )
}

pub(crate) fn resource_limit(
    code_value: &'static str,
    message_value: &'static str,
    dimension: &'static str,
    actual: u64,
    maximum: u64,
) -> SdkExecutionDiagnostic {
    let diagnostic = append_path(
        SdkExecutionDiagnostic::resource_limit(code(code_value), message(message_value)),
        [
            SdkDiagnosticPathSegment::Argument(name("limits")),
            SdkDiagnosticPathSegment::Argument(name(dimension)),
        ],
    );
    if dimension == "bytes" {
        diagnostic
            .try_with_detail(
                name("actual_bytes"),
                SdkDiagnosticDetailValue::ByteCount(actual),
            )
            .and_then(|diagnostic| {
                diagnostic.try_with_detail(
                    name("maximum_bytes"),
                    SdkDiagnosticDetailValue::ByteCount(maximum),
                )
            })
            .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure())
    } else {
        diagnostic
            .try_with_detail(name("actual"), SdkDiagnosticDetailValue::Count(actual))
            .and_then(|diagnostic| {
                diagnostic
                    .try_with_detail(name("maximum"), SdkDiagnosticDetailValue::Count(maximum))
            })
            .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure())
    }
}

fn allocation_failure() -> SdkExecutionDiagnostic {
    resource_limit(
        "projected_batch_allocation_failed",
        "Projected batch executor could not reserve bounded owned storage",
        "bytes",
        u64::MAX,
        u64::MAX,
    )
}

fn complexity_overflow() -> SdkExecutionDiagnostic {
    resource_limit(
        "projected_batch_compilation_complexity_limit",
        "Projected batch compiler complexity accounting overflowed",
        "variables",
        u64::MAX,
        COMPILER_CARDINALITY_MAX,
    )
}

fn checked_add_u64(left: u64, right: u64) -> std::result::Result<u64, SdkExecutionDiagnostic> {
    left.checked_add(right).ok_or_else(complexity_overflow)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use serde_json::json;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::AttributeId;
    use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
    use type_bridge_contract::schema::DocumentId;
    use type_bridge_contract::value::CanonicalString;
    use type_bridge_core_lib::version::Version;
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

    use super::*;
    use crate::session::backend::{
        AnswerCancellation, BoxFuture, DriverBackend, QueryResult, TransactionOps,
    };

    #[derive(Default)]
    struct AllocationState {
        answers: VecDeque<Vec<Value>>,
        statements: Vec<String>,
        calls: usize,
        commits: usize,
        rollbacks: usize,
    }

    struct AllocationBackend {
        state: Arc<Mutex<AllocationState>>,
    }

    impl DriverBackend for AllocationBackend {
        fn open_transaction(
            &self,
            _database: &str,
            _tx_type: TxType,
        ) -> BoxFuture<'_, Result<Box<dyn TransactionOps>, OrmError>> {
            Box::pin(async move {
                Ok(Box::new(AllocationTransaction {
                    state: Arc::clone(&self.state),
                }) as Box<dyn TransactionOps>)
            })
        }

        fn is_open(&self) -> bool {
            true
        }

        fn server_version(&self) -> Option<Version> {
            Some(Version::new(3, 12, 1))
        }

        fn supports_given_rows(&self) -> bool {
            true
        }
    }

    struct AllocationTransaction {
        state: Arc<Mutex<AllocationState>>,
    }

    impl TransactionOps for AllocationTransaction {
        fn supports_given_rows(&self) -> bool {
            true
        }

        fn query(&mut self, _typeql: &str) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            Box::pin(async { panic!("batch allocation probe used an unbounded query") })
        }

        fn query_with_rows(
            &mut self,
            typeql: &str,
            _rows: GivenRowsSpec,
        ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            let answer = {
                let mut state = self.state.lock().unwrap();
                state.calls += 1;
                state.statements.push(typeql.to_owned());
                state.answers.pop_front().expect("unexpected provider call")
            };
            let items = answer
                .into_iter()
                .map(|document| crate::native_batch_fixture::item(typeql, document))
                .collect::<Vec<_>>();
            let fetched = typeql.contains("fetch");
            Box::pin(async move {
                Ok(if fetched {
                    QueryResult::Documents(
                        items
                            .into_iter()
                            .map(|item| {
                                if let crate::session::backend::AnswerItem::Document(value) = item {
                                    value
                                } else {
                                    panic!("document")
                                }
                            })
                            .collect(),
                    )
                } else {
                    QueryResult::Rows(
                        items
                            .into_iter()
                            .map(|item| {
                                if let crate::session::backend::AnswerItem::Row(value) = item {
                                    value
                                } else {
                                    panic!("row")
                                }
                            })
                            .collect(),
                    )
                })
            })
        }

        fn commit(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state.lock().unwrap().commits += 1;
            Box::pin(async { Ok(()) })
        }

        fn rollback(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            self.state.lock().unwrap().rollbacks += 1;
            Box::pin(async { Ok(()) })
        }

        fn close(&mut self) -> BoxFuture<'_, Result<(), OrmError>> {
            Box::pin(async { Ok(()) })
        }
    }

    fn allocation_installed() -> InstalledRuntimeProjection {
        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("batch-executor-allocation.yaml").unwrap(),
            r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
relations:
  membership:
    owns:
      identifier: { key: true }
    relates:
      member: { card: { min: 0, max: 4 } }
plays:
  person:
    membership: [member]
"#,
        )])
        .unwrap();
        let resolved = resolve(
            &normalize_documents(&documents).unwrap(),
            &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
        )
        .unwrap();
        InstalledRuntimeProjection::try_new(
            project(
                &resolved,
                BindingTarget::Python,
                &ProjectionConfig::python(),
                &[ProjectionHandler::python_v1()],
                &[],
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn allocation_value(
        installed: &InstalledRuntimeProjection,
        value: &str,
    ) -> ProjectedAttributeValue {
        ProjectedAttributeValue::try_new(
            installed,
            TypeId::new(TypeKind::Attribute, "identifier").unwrap(),
            CanonicalValue::String(CanonicalString::new(value).unwrap()),
        )
        .unwrap()
    }

    fn allocation_create(
        installed: &InstalledRuntimeProjection,
        kind: TypeKind,
    ) -> ProjectedCreate {
        let type_id = TypeId::new(
            kind,
            if kind == TypeKind::Entity {
                "person"
            } else {
                "membership"
            },
        )
        .unwrap();
        let fields = vec![(
            OwnsFactId::new(type_id.clone(), AttributeId::new("identifier").unwrap()).unwrap(),
            vec![allocation_value(installed, "key")],
        )];
        let roles = if kind == TypeKind::Relation {
            vec![(
                RoleId::new("membership", "member").unwrap(),
                vec![
                    ProjectedReference::try_new(
                        installed,
                        TypeId::new(TypeKind::Entity, "person").unwrap(),
                        Some("0x10".to_owned()),
                        vec![],
                    )
                    .unwrap(),
                ],
            )]
        } else {
            vec![]
        };
        ProjectedCreate::try_new(installed, type_id, fields, roles).unwrap()
    }

    fn allocation_answers(kind: TypeKind) -> VecDeque<Vec<Value>> {
        let mut answers = VecDeque::new();
        answers.push_back(vec![json!({
            "ordinal": 0,
            "_iid": if kind == TypeKind::Entity { "0x20" } else { "0x30" },
            "_type": if kind == TypeKind::Entity { "person" } else { "membership" },
            "attributes": {"identifier": "key"},
            "role_players": []
        })]);
        answers
    }

    #[test]
    fn single_write_hydration_keeps_all_unique_roots_in_provider_order() {
        let installed = allocation_installed();
        for kind in [TypeKind::Entity, TypeKind::Relation] {
            for count in 0..=2 {
                let mut input = allocation_create(&installed, kind);
                if kind == TypeKind::Relation {
                    let role = RoleId::new("membership", "member").unwrap();
                    let players = input.roles().get(&role).unwrap();
                    input = ProjectedCreate::try_new(
                        &installed,
                        input.type_id().clone(),
                        input
                            .fields()
                            .iter()
                            .map(|(field, values)| (field.clone(), values.clone()))
                            .collect(),
                        vec![(role, vec![players[0].clone(), players[0].clone()])],
                    )
                    .unwrap();
                }
                let model = input.type_id().clone();
                let control = ProjectedBatchInvocationControl::capture(
                    QueryExecutionResourceLimits::default(),
                    AnswerCancellation::default(),
                );
                let batch = crate::ProjectedCrudExecutor::new(&installed)
                    .single_controlled_write_batch(
                        &model,
                        ProjectedBatchOperation::Insert,
                        ProjectedBatchRow::Create(input),
                        &control,
                    )
                    .unwrap();
                let prepared =
                    PreparedProjectedBatchInvocation::try_new(&installed, &batch, control).unwrap();
                let identity = DatabaseExecutionIdentity::isolated("single-hydration");
                let plan =
                    BatchPlan::compile(&installed, &prepared, &identity, false, None).unwrap();
                assert!(plan.single_input);
                let executor = ProjectedBatchExecutor::new(&installed);
                let mut consumer = HydrationConsumer::new(&executor, &plan, &identity).unwrap();
                consumer.targets.push(String::new());
                let expected = ["0x22", "0x20"];
                for iid in expected.iter().take(count) {
                    let mut document = json!({
                        "ordinal": 0,
                        "_iid": iid,
                        "_type": model.label().as_str(),
                        "attributes": {"identifier": "key"},
                        "_role_0_iid": "0x10",
                        "_role_0_type": "person",
                        "_role_0_attributes": {"identifier": "key"}
                    });
                    if kind == TypeKind::Entity {
                        document
                            .as_object_mut()
                            .unwrap()
                            .retain(|key, _| !key.starts_with("_role_"));
                    }
                    consumer.accept_document(document.clone()).unwrap();
                    consumer.accept_document(document).unwrap();
                }
                let roots = consumer.finish().unwrap();
                assert_eq!(
                    roots
                        .items()
                        .iter()
                        .map(ProjectedThing::iid)
                        .collect::<Vec<_>>(),
                    expected[..count]
                );
                for thing in roots.items() {
                    validate_written_role_identities(&installed, &plan, thing, 0, None).unwrap();
                }
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn batch_writes_keep_zero_and_plural_input_roots_and_commit_once() {
        let installed = allocation_installed();
        for kind in [TypeKind::Entity, TypeKind::Relation] {
            for operation in [
                ProjectedBatchOperation::Insert,
                ProjectedBatchOperation::Put,
            ] {
                for borrowed in [false, true] {
                    let template = allocation_create(&installed, kind);
                    let model = template.type_id().clone();
                    let rows = ["absent", "present"].map(|key| {
                        ProjectedBatchRow::Create(
                            ProjectedCreate::try_new(
                                &installed,
                                model.clone(),
                                vec![(
                                    OwnsFactId::new(
                                        model.clone(),
                                        AttributeId::new("identifier").unwrap(),
                                    )
                                    .unwrap(),
                                    vec![allocation_value(&installed, key)],
                                )],
                                template
                                    .roles()
                                    .iter()
                                    .map(|(role, players)| (role.clone(), players.clone()))
                                    .collect(),
                            )
                            .unwrap(),
                        )
                    });
                    let batch =
                        ProjectedBatch::try_new(&installed, model.clone(), operation, rows.into())
                            .unwrap();
                    let documents = ["0x22", "0x21"].map(|iid| {
                        let mut document = json!({
                            "ordinal":1, "_iid":iid, "_type":model.label().as_str(),
                            "attributes":{"identifier":"present"},
                        });
                        if kind == TypeKind::Relation {
                            document["_role_0_players"] = json!([{
                                "_iid":"0x10", "_type":"person", "attributes":{"identifier":"player"}
                            }]);
                        }
                        document
                    });
                    let state = Arc::new(Mutex::new(AllocationState {
                        answers: VecDeque::from([documents.into()]),
                        ..AllocationState::default()
                    }));
                    let database = Database::with_backend(
                        Box::new(AllocationBackend {
                            state: Arc::clone(&state),
                        }),
                        "batch-zero-plural",
                    );
                    let executor = ProjectedBatchExecutor::new(&installed);
                    let control = ProjectedBatchInvocationControl::capture(
                        Default::default(),
                        Default::default(),
                    );
                    let transaction = if borrowed {
                        Some(database.transaction_context(TxType::Write).await.unwrap())
                    } else {
                        None
                    };
                    let result = match &transaction {
                        Some(transaction) => {
                            executor
                                .execute_in_transaction(transaction, &batch, control)
                                .await
                        }
                        None => executor.execute(&database, &batch, control).await,
                    }
                    .unwrap_or_else(|error| panic!("{kind:?}/{operation:?}/{borrowed}: {error:?}"));
                    let ProjectedBatchResult::Things(result) = result else {
                        panic!("expected roots")
                    };
                    assert_eq!(result.count(), 2);
                    assert_eq!(result.items().len(), result.input_indexes().len());
                    assert_eq!(result.input_indexes(), &[1, 1]);
                    assert_eq!(
                        result
                            .items()
                            .iter()
                            .map(ProjectedThing::iid)
                            .collect::<Vec<_>>(),
                        ["0x22", "0x21"]
                    );
                    if let Some(transaction) = transaction {
                        assert_eq!(state.lock().unwrap().commits, 0);
                        transaction.commit_sdk().await.unwrap();
                    }
                    let state = state.lock().unwrap();
                    assert_eq!(state.calls, 1);
                    assert_eq!((state.commits, state.rollbacks), (1, 0));
                    assert!(state.answers.is_empty());
                }
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn single_write_native_execution_commits_empty_answers_without_diagnosis() {
        let installed = allocation_installed();
        for kind in [TypeKind::Entity, TypeKind::Relation] {
            for operation in [
                ProjectedBatchOperation::Insert,
                ProjectedBatchOperation::Update,
            ] {
                let input = allocation_create(&installed, kind);
                let model = input.type_id().clone();
                let row = if operation == ProjectedBatchOperation::Update {
                    ProjectedBatchRow::Update {
                        iid: "0x20".into(),
                        replacement: input,
                    }
                } else {
                    ProjectedBatchRow::Create(input)
                };
                let control = ProjectedBatchInvocationControl::capture(
                    QueryExecutionResourceLimits::default(),
                    AnswerCancellation::default(),
                );
                let batch = crate::ProjectedCrudExecutor::new(&installed)
                    .single_controlled_write_batch(&model, operation, row, &control)
                    .unwrap();
                let state = Arc::new(Mutex::new(AllocationState {
                    answers: VecDeque::from([vec![]]),
                    ..AllocationState::default()
                }));
                let database = Database::with_backend(
                    Box::new(AllocationBackend {
                        state: Arc::clone(&state),
                    }),
                    "single-zero",
                );
                let result = ProjectedBatchExecutor::new(&installed)
                    .execute(&database, &batch, control)
                    .await
                    .unwrap();
                let ProjectedBatchResult::Things(things) = result else {
                    panic!("expected things")
                };
                assert_eq!(things.count(), 0);
                assert!(things.input_indexes().is_empty());
                let state = state.lock().unwrap();
                assert_eq!(state.calls, 1);
                assert_eq!((state.commits, state.rollbacks), (1, 0));
                assert!(state.answers.is_empty());
            }
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn returning_delete_captures_existing_rows_before_mutation() {
        let installed = allocation_installed();
        for kind in [TypeKind::Entity, TypeKind::Relation] {
            let label = if kind == TypeKind::Entity {
                "person"
            } else {
                "membership"
            };
            let batch = ProjectedBatch::try_new(
                &installed,
                TypeId::new(kind, label).unwrap(),
                ProjectedBatchOperation::Delete,
                ["0x20", "0x21", "0x22"]
                    .into_iter()
                    .map(|iid| ProjectedBatchRow::Delete { iid: iid.into() })
                    .collect(),
            )
            .unwrap();
            let state = Arc::new(Mutex::new(AllocationState {
                answers: VecDeque::from([
                    vec![
                        json!({"ordinal":2,"_iid":"0x22","_type":label,"attributes":{"identifier":"last"},"role_players":[]}),
                        json!({"ordinal":0,"_iid":"0x20","_type":label,"attributes":{"identifier":"first"},"role_players":[]}),
                    ],
                    vec![
                        json!({"ordinal":0}),
                        json!({"ordinal":1}),
                        json!({"ordinal":2}),
                    ],
                ]),
                ..AllocationState::default()
            }));
            let database = Database::with_backend(
                Box::new(AllocationBackend {
                    state: Arc::clone(&state),
                }),
                "returning-delete",
            );
            let result = ProjectedBatchExecutor::new(&installed)
                .returning_deleted()
                .execute(
                    &database,
                    &batch,
                    ProjectedBatchInvocationControl::capture(
                        QueryExecutionResourceLimits::default(),
                        AnswerCancellation::default(),
                    ),
                )
                .await
                .unwrap();
            let ProjectedBatchResult::Things(things) = result else {
                panic!("expected snapshots")
            };
            assert!(things.items().iter().all(ProjectedThing::is_deleted));
            assert_eq!(things.input_indexes(), &[0, 2]);
            assert_eq!(things.count(), 2);
            assert_eq!(
                things.items()[0]
                    .try_to_reference(&installed)
                    .unwrap_err()
                    .code()
                    .as_str(),
                "thing_deleted"
            );
            assert_eq!(
                things
                    .items()
                    .iter()
                    .map(ProjectedThing::iid)
                    .collect::<Vec<_>>(),
                vec!["0x20", "0x22"]
            );
            let state = state.lock().unwrap();
            assert_eq!(state.calls, 2);
            assert!(!state.statements[0].contains("delete"));
            assert!(state.statements[1].contains("delete"));
            assert_eq!(state.commits, 1);
            assert_eq!(state.rollbacks, 0);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn returning_delete_refuses_limits_and_bad_evidence_before_mutation() {
        let installed = allocation_installed();
        let batch = ProjectedBatch::try_new(
            &installed,
            TypeId::new(TypeKind::Entity, "person").unwrap(),
            ProjectedBatchOperation::Delete,
            vec![ProjectedBatchRow::Delete { iid: "0x20".into() }],
        )
        .unwrap();
        for statement_limit in [1, 2] {
            let state = Arc::new(Mutex::new(AllocationState {
                answers: VecDeque::from([vec![json!({
                    "ordinal":0,"_iid":"0x20","_type":"person","attributes":{}
                })]]),
                ..AllocationState::default()
            }));
            let database = Database::with_backend(
                Box::new(AllocationBackend {
                    state: Arc::clone(&state),
                }),
                "refused-delete",
            );
            let limits = QueryExecutionResourceLimits {
                statements: Some(statement_limit),
                ..QueryExecutionResourceLimits::default()
            };
            assert!(
                ProjectedBatchExecutor::new(&installed)
                    .returning_deleted()
                    .execute(
                        &database,
                        &batch,
                        ProjectedBatchInvocationControl::capture(
                            limits,
                            AnswerCancellation::default()
                        ),
                    )
                    .await
                    .is_err()
            );
            let state = state.lock().unwrap();
            assert_eq!(state.calls, usize::from(statement_limit == 2));
            assert!(
                state
                    .statements
                    .iter()
                    .all(|source| !source.contains("delete"))
            );
            assert_eq!(state.commits, 0);
            assert_eq!(state.rollbacks, 1);
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn hydration_allocation_failure_after_mutation_rolls_back_both_kinds() {
        let installed = allocation_installed();
        for kind in [TypeKind::Entity, TypeKind::Relation] {
            let type_id = TypeId::new(
                kind,
                if kind == TypeKind::Entity {
                    "person"
                } else {
                    "membership"
                },
            )
            .unwrap();
            let batch = ProjectedBatch::try_new(
                &installed,
                type_id,
                ProjectedBatchOperation::Insert,
                vec![ProjectedBatchRow::Create(allocation_create(
                    &installed, kind,
                ))],
            )
            .unwrap();
            let state = Arc::new(Mutex::new(AllocationState {
                answers: allocation_answers(kind),
                ..AllocationState::default()
            }));
            let database = Database::with_backend(
                Box::new(AllocationBackend {
                    state: Arc::clone(&state),
                }),
                "allocation-probe",
            );
            let diagnostic = ProjectedBatchExecutor::new(&installed)
                .failing_hydration_allocation_for_test()
                .execute(
                    &database,
                    &batch,
                    ProjectedBatchInvocationControl::capture(
                        QueryExecutionResourceLimits::default(),
                        AnswerCancellation::default(),
                    ),
                )
                .await
                .unwrap_err();
            assert_eq!(
                diagnostic.code().as_str(),
                "projected_batch_allocation_failed"
            );
            let state = state.lock().unwrap();
            assert!(state.answers.is_empty());
            assert_eq!(state.calls, 1);
            assert_eq!(state.rollbacks, 1);
            assert_eq!(state.commits, 0);
        }
    }
}
