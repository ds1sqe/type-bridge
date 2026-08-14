//! Atomic binding-neutral execution for homogeneous projected mutation batches.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;

use serde_json::Value;
use type_bridge_contract::id::{RoleId, TypeId, TypeKind, is_canonical_thing_iid};
use type_bridge_contract::projection::{ModelProjection, ProjectedModelForm, ReadRoleProjection};
use type_bridge_contract::schema::{AnnotationKindId, OwnsFactId};
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCategory, SdkDiagnosticCode, SdkDiagnosticDetailValue, SdkDiagnosticMessage,
    SdkDiagnosticName, SdkDiagnosticPathSegment, SdkExecutionDiagnostic, SdkProviderOperation,
};
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};

use crate::_dynamic::{
    DynamicAttributeMap, DynamicEntityRow, DynamicRelationRow, DynamicRolePlayer,
};
use crate::_manager::hydration::{hydrate_dynamic_entity, hydrate_dynamic_relation};
use crate::OrmError;
use crate::execution_diagnostic::lower_orm_error;
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
    AnswerConsumer, AnswerControl, AnswerItem, BoundedAnswerLimits, BoundedAnswerReader,
    BoundedAnswerStats, GivenRowsSpec, GivenValue, QueryV2AnswerLimits,
};
use crate::session::context::TransactionContextMutationLease;
use crate::session::database::DatabaseExecutionIdentity;
use crate::session::{Database, TransactionContext, TxType};
use crate::value::AttributeValue;

const COMPILER_CARDINALITY_MAX: u64 = u16::MAX as u64;
const MAX_PROVIDER_IID_BYTES: u64 = (2 + type_bridge_contract::id::MAX_THING_IID_HEX_DIGITS) as u64;

/// Result of one homogeneous projected mutation batch.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ProjectedBatchResult {
    /// Complete post-mutation projected things in input ordinal order.
    Things(Vec<ProjectedThing>),
    /// Every requested delete was applied as an idempotent present-or-absent operation.
    Deleted,
}

/// Binding-neutral atomic executor for [`ProjectedBatch`].
pub struct ProjectedBatchExecutor<'projection> {
    installed: &'projection InstalledRuntimeProjection,
    #[cfg(test)]
    fail_hydration_allocation: bool,
}

impl<'projection> ProjectedBatchExecutor<'projection> {
    /// Create an executor over one verified installed runtime projection.
    #[must_use]
    pub const fn new(installed: &'projection InstalledRuntimeProjection) -> Self {
        Self {
            installed,
            #[cfg(test)]
            fail_hydration_allocation: false,
        }
    }

    #[cfg(test)]
    fn failing_hydration_allocation_for_test(mut self) -> Self {
        self.fail_hydration_allocation = true;
        self
    }

    /// Execute and commit one batch in an owned write transaction.
    pub async fn execute(
        &self,
        database: &Database,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
    ) -> std::result::Result<ProjectedBatchResult, SdkExecutionDiagnostic> {
        let prepared = PreparedProjectedBatchInvocation::try_new(self.installed, batch, control)?;
        if batch.is_empty() {
            return Ok(empty_result(batch.operation()));
        }
        prepared.check_control()?;
        require_given_capability(database.supports_given_stage())?;
        let identity = database.execution_identity();
        let plan = BatchPlan::compile(self.installed, &prepared, &identity)?;
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
            .execute_prepared_in_transaction(&transaction, &prepared, plan, &identity)
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
                Err(primary)
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
        let prepared = PreparedProjectedBatchInvocation::try_new(self.installed, batch, control)?;
        if batch.is_empty() {
            return Ok(empty_result(batch.operation()));
        }
        if transaction.tx_type() != TxType::Write {
            return Err(invalid_at_type(
                "transaction_type_mismatch",
                "Projected batch execution requires a borrowed write transaction",
                batch.model(),
            ));
        }
        prepared.check_control()?;
        let supported = await_controlled(
            transaction.supports_given_stage(),
            prepared.deadline(),
            prepared.cancellation(),
        )
        .await
        .map_err(|error| controlled_orm(error, SdkProviderOperation::Read, batch.model()))?;
        require_given_capability(supported)?;
        let identity = transaction.execution_identity().clone();
        let plan = BatchPlan::compile(self.installed, &prepared, &identity)?;
        prepared.check_control()?;
        self.execute_prepared_in_transaction(transaction, &prepared, plan, &identity)
            .await
    }

    async fn execute_prepared_in_transaction(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedProjectedBatchInvocation<'_>,
        mut plan: BatchPlan<'_>,
        identity: &DatabaseExecutionIdentity,
    ) -> std::result::Result<ProjectedBatchResult, SdkExecutionDiagnostic> {
        let mut lease = await_controlled(
            transaction.acquire_mutation_lease(),
            prepared.deadline(),
            prepared.cancellation(),
        )
        .await
        .map_err(|error| controlled_orm(error, SdkProviderOperation::Write, plan.model.id()))?;
        let result = self
            .run_with_lease(&mut lease, prepared, &mut plan, identity)
            .await;
        match result {
            Ok(value) => {
                lease.complete_success();
                Ok(value)
            }
            Err(diagnostic) => {
                lease.record_failure(&diagnostic);
                Err(diagnostic)
            }
        }
    }

    async fn run_with_lease(
        &self,
        lease: &mut TransactionContextMutationLease,
        prepared: &PreparedProjectedBatchInvocation<'_>,
        plan: &mut BatchPlan<'_>,
        identity: &DatabaseExecutionIdentity,
    ) -> std::result::Result<ProjectedBatchResult, SdkExecutionDiagnostic> {
        let mut ledger = ExecutionLedger::new(
            prepared.limits(),
            prepared.batch().resource_measure().bytes(),
        );
        ledger.charge_preflight(plan.preflight_bytes, plan.statement_count)?;
        prepared.check_control()?;

        let mut resolved = ResolvedPrerequisites::new(plan)?;
        if let Some(statement) = plan.prerequisite.take() {
            let mut consumer = PrerequisiteConsumer::new(plan, &mut resolved);
            let dispatched = dispatch(
                lease,
                false,
                statement,
                prepared,
                &mut consumer,
                plan.prerequisite_item_ceiling()
                    .min(ledger.remaining_items()),
                ledger.remaining_bytes(),
            )
            .await;
            let stats = match dispatched {
                Ok(stats) => stats,
                Err(later) => return Err(consumer.take_deferred().unwrap_or(later)),
            };
            ledger.record_reply(stats)?;
            consumer.finish()?;
        }
        resolved.finish(plan)?;
        prepared.check_control()?;

        let remaining_reply_items = u64::try_from(plan.row_count)
            .unwrap_or(u64::MAX)
            .saturating_mul(if plan.operation == ProjectedBatchOperation::Delete {
                1
            } else {
                2
            });
        ledger.require_remaining_items(remaining_reply_items)?;

        let mutation_rows = plan.mutation_rows(&resolved)?;
        let mut identities = OrdinalIdentityConsumer::new(plan.row_count, plan.operation)?;
        let mut pending_attach = if plan.operation == ProjectedBatchOperation::Delete {
            None
        } else {
            Some(plan.attach_rows(&resolved, None)?)
        };
        let mut hydrated = if plan.operation == ProjectedBatchOperation::Delete {
            None
        } else {
            Some(HydrationConsumer::new(self, plan, identity)?)
        };
        let dispatched = dispatch_rows(
            lease,
            true,
            &plan.mutation_source,
            mutation_rows,
            prepared,
            &mut identities,
            (plan.row_count as u64 + 1).min(ledger.remaining_items()),
            ledger.remaining_bytes(),
        )
        .await;
        let stats = match dispatched {
            Ok(stats) => stats,
            Err(later) => return Err(identities.take_deferred().unwrap_or(later)),
        };
        ledger.record_reply(stats)?;
        identities.finish(plan, &resolved)?;

        if plan.operation == ProjectedBatchOperation::Delete {
            prepared.check_control()?;
            return Ok(ProjectedBatchResult::Deleted);
        }

        let mut attach_rows = pending_attach
            .take()
            .expect("non-delete plan reserved attach rows");
        let hydration_consumer = hydrated
            .as_mut()
            .expect("non-delete plan reserved hydration slots");
        identities.move_iids_into(&mut hydration_consumer.targets)?;
        fill_target_iids(&mut attach_rows, &hydration_consumer.targets)?;
        let attach_source = plan
            .attach_source
            .as_deref()
            .expect("non-delete plan has attach source");
        let dispatched = dispatch_rows(
            lease,
            true,
            attach_source,
            attach_rows,
            prepared,
            hydration_consumer,
            (plan.row_count as u64 + 1).min(ledger.remaining_items()),
            ledger.remaining_bytes(),
        )
        .await;
        let stats = match dispatched {
            Ok(stats) => stats,
            Err(later) => return Err(hydration_consumer.take_deferred().unwrap_or(later)),
        };
        ledger.record_reply(stats)?;
        let things = hydrated
            .take()
            .expect("non-delete plan retained hydration consumer")
            .finish()?;
        ledger.charge_projected(&things)?;
        prepared.check_control()?;
        Ok(ProjectedBatchResult::Things(things))
    }
}

fn empty_result(operation: ProjectedBatchOperation) -> ProjectedBatchResult {
    if operation == ProjectedBatchOperation::Delete {
        ProjectedBatchResult::Deleted
    } else {
        ProjectedBatchResult::Things(Vec::new())
    }
}

fn require_given_capability(supported: bool) -> std::result::Result<(), SdkExecutionDiagnostic> {
    if supported {
        Ok(())
    } else {
        Err(SdkExecutionDiagnostic::unsupported_capability(
            code("projected_batch_transport_unsupported"),
            message(
                "Projected batch execution requires TypeDB 3.12 and band-9 scalar GivenRows transport",
            ),
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
        _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline.instant())) => Err(ControlledAwaitError::Interrupted(SdkExecutionDiagnostic::data_operation_deadline_exceeded())),
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

#[derive(Clone)]
struct ProviderStatement {
    source: String,
    rows: GivenRowsSpec,
}

struct FieldRoute {
    field: OwnsFactId,
    variable: String,
    typeql_type: &'static str,
}

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
struct RoleRouteKey {
    role: RoleId,
    player_type: TypeId,
}

struct RoleRoute {
    key: RoleRouteKey,
    provider_role: String,
    variable: String,
    player_variable: String,
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
    key_typeql_type: Option<&'static str>,
}

struct RoleReference<'a> {
    owner_ordinal: usize,
    reference_ordinal: usize,
    role: RoleId,
    reference: &'a ProjectedReference,
}

struct BatchPlan<'a> {
    installed: &'a InstalledRuntimeProjection,
    model: &'a ModelProjection,
    operation: ProjectedBatchOperation,
    rows: Vec<(usize, &'a ProjectedBatchRow)>,
    field_routes: Vec<FieldRoute>,
    role_routes: Vec<RoleRoute>,
    role_references: Vec<RoleReference<'a>>,
    prerequisite: Option<ProviderStatement>,
    mutation_source: String,
    attach_source: Option<String>,
    row_count: usize,
    statement_count: u32,
    preflight_bytes: u64,
}

impl<'a> BatchPlan<'a> {
    fn compile(
        installed: &'a InstalledRuntimeProjection,
        prepared: &PreparedProjectedBatchInvocation<'a>,
        identity: &DatabaseExecutionIdentity,
    ) -> std::result::Result<Self, SdkExecutionDiagnostic> {
        let batch = prepared.batch();
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
        validate_provider_scalar_inputs(&rows)?;

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
        let prerequisite = compile_prerequisite(
            installed,
            model,
            batch.operation(),
            &rows,
            &reference_routes,
            &role_references,
        )?;
        let mutation_source = compile_mutation_source(installed, model, batch.operation())?;
        let attach_source = if batch.operation() == ProjectedBatchOperation::Delete {
            None
        } else {
            Some(compile_attach_source(
                installed,
                model,
                &field_routes,
                &role_routes,
            )?)
        };
        let statement_count = batch.resource_measure().statements();
        let actual_statement_count =
            u32::from(prerequisite.is_some()) + 1 + u32::from(attach_source.is_some());
        if actual_statement_count != statement_count {
            return Err(SdkExecutionDiagnostic::internal_failure());
        }
        preflight_complexity(
            prerequisite.as_ref(),
            &mutation_source,
            attach_source.as_deref(),
        )?;
        let preflight_bytes = preflight_transport_bytes(
            prepared,
            model,
            &rows,
            &field_routes,
            &role_routes,
            &role_references,
            prerequisite.as_ref(),
            &mutation_source,
            attach_source.as_deref(),
        )?;
        Ok(Self {
            installed,
            model,
            operation: batch.operation(),
            rows,
            field_routes,
            role_routes,
            role_references,
            prerequisite,
            mutation_source,
            attach_source,
            row_count: batch.len(),
            statement_count,
            preflight_bytes,
        })
    }

    fn prerequisite_item_ceiling(&self) -> u64 {
        u64::try_from(self.row_count + self.role_references.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1)
    }

    fn mutation_rows(
        &self,
        _resolved: &ResolvedPrerequisites,
    ) -> std::result::Result<GivenRowsSpec, SdkExecutionDiagnostic> {
        let mut variables = vec!["ordinal".to_owned()];
        let mut result_rows = Vec::new();
        result_rows
            .try_reserve_exact(self.row_count)
            .map_err(|_| allocation_failure())?;
        match self.operation {
            ProjectedBatchOperation::Insert => {
                for (ordinal, _) in &self.rows {
                    result_rows.push(vec![ordinal_value(*ordinal)?]);
                }
            }
            ProjectedBatchOperation::Update | ProjectedBatchOperation::Delete => {
                variables.push("target-iid".to_owned());
                for (ordinal, row) in &self.rows {
                    result_rows.push(vec![
                        ordinal_value(*ordinal)?,
                        GivenValue::String(
                            row_iid(row)
                                .expect("update/delete normalized row has IID")
                                .to_ascii_lowercase(),
                        ),
                    ]);
                }
            }
            ProjectedBatchOperation::Put => {
                for index in 0..self.model.reference_read().key_fields().len() {
                    variables.push(format!("put-key-{index}"));
                }
                for (ordinal, row) in &self.rows {
                    let create = row_create(row).expect("put row is create");
                    let mut values = Vec::new();
                    values
                        .try_reserve_exact(variables.len())
                        .map_err(|_| allocation_failure())?;
                    values.push(ordinal_value(*ordinal)?);
                    for field in self.model.reference_read().key_fields() {
                        values.push(given_from_canonical(
                            create.fields()[field][0].value().clone(),
                        )?);
                    }
                    result_rows.push(values);
                }
            }
        }
        Ok(GivenRowsSpec {
            variables,
            rows: result_rows,
        })
    }

    fn attach_rows(
        &self,
        resolved: &ResolvedPrerequisites,
        target_iids: Option<&[String]>,
    ) -> std::result::Result<GivenRowsSpec, SdkExecutionDiagnostic> {
        let mut variables = vec!["ordinal".to_owned(), "target-iid".to_owned()];
        variables.extend(self.field_routes.iter().map(|route| route.variable.clone()));
        variables.extend(self.role_routes.iter().map(|route| route.variable.clone()));
        let width = variables.len();
        let event_count = self
            .rows
            .iter()
            .map(|(_, row)| {
                row_create(row)
                    .map_or(0, |create| {
                        create.fields().values().map(Vec::len).sum::<usize>()
                            + create.roles().values().map(Vec::len).sum::<usize>()
                    })
                    .max(1)
            })
            .try_fold(0_usize, usize::checked_add)
            .ok_or_else(complexity_overflow)?;
        let mut rows = Vec::new();
        rows.try_reserve_exact(event_count)
            .map_err(|_| allocation_failure())?;
        let field_index = self
            .field_routes
            .iter()
            .enumerate()
            .map(|(index, route)| (route.field.clone(), index + 2))
            .collect::<BTreeMap<_, _>>();
        let role_index = self
            .role_routes
            .iter()
            .enumerate()
            .map(|(index, route)| (route.key.clone(), index + 2 + self.field_routes.len()))
            .collect::<BTreeMap<_, _>>();

        let mut flat_reference = 0_usize;
        for (ordinal, row) in &self.rows {
            let create = row_create(row).expect("non-delete row has create");
            let mut emitted = false;
            for (field, values) in create.fields() {
                let column = *field_index
                    .get(field)
                    .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                for value in values {
                    let mut event = empty_event(
                        width,
                        *ordinal,
                        target_iids.map(|iids| iids[*ordinal].as_str()),
                    )?;
                    event[column] = given_from_canonical(value.value().clone())?;
                    rows.push(event);
                    emitted = true;
                }
            }
            for (role, references) in create.roles() {
                for reference in references {
                    let prepared_reference = self
                        .role_references
                        .get(flat_reference)
                        .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                    debug_assert_eq!(prepared_reference.reference, reference);
                    let resolved_reference = resolved.reference(flat_reference)?;
                    let route = RoleRouteKey {
                        role: role.clone(),
                        player_type: resolved_reference.concrete_type.clone(),
                    };
                    let column = *role_index
                        .get(&route)
                        .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                    let mut event = empty_event(
                        width,
                        *ordinal,
                        target_iids.map(|iids| iids[*ordinal].as_str()),
                    )?;
                    event[column] = GivenValue::String(resolved_reference.iid.clone());
                    rows.push(event);
                    flat_reference += 1;
                    emitted = true;
                }
            }
            if !emitted {
                rows.push(empty_event(
                    width,
                    *ordinal,
                    target_iids.map(|iids| iids[*ordinal].as_str()),
                )?);
            }
        }
        if flat_reference != self.role_references.len() {
            return Err(SdkExecutionDiagnostic::internal_failure());
        }
        Ok(GivenRowsSpec { variables, rows })
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

fn fill_target_iids(
    rows: &mut GivenRowsSpec,
    target_iids: &[String],
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    for row in &mut rows.rows {
        let Some(GivenValue::Integer(ordinal)) = row.first() else {
            return Err(SdkExecutionDiagnostic::internal_failure());
        };
        let ordinal =
            usize::try_from(*ordinal).map_err(|_| SdkExecutionDiagnostic::internal_failure())?;
        let iid = target_iids
            .get(ordinal)
            .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
        let target = row
            .get_mut(1)
            .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
        let GivenValue::String(target) = target else {
            return Err(SdkExecutionDiagnostic::internal_failure());
        };
        target.clear();
        target.push_str(iid);
    }
    Ok(())
}

fn compile_field_routes(
    installed: &InstalledRuntimeProjection,
    model: &ModelProjection,
) -> std::result::Result<Vec<FieldRoute>, SdkExecutionDiagnostic> {
    let mut routes = Vec::new();
    routes
        .try_reserve_exact(model.create().fields().len())
        .map_err(|_| allocation_failure())?;
    for (index, field) in model.create().fields().iter().enumerate() {
        routes.push(FieldRoute {
            field: field.token().clone(),
            variable: format!("field-{index}"),
            typeql_type: field_typeql_type(installed, field.token())?,
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
        let provider_role = provider_role_name(installed, model, &key.role)?;
        routes.push(RoleRoute {
            key,
            provider_role,
            variable: format!("player-iid-{index}"),
            player_variable: format!("player-{index}"),
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

fn provider_role_name(
    installed: &InstalledRuntimeProjection,
    model: &ModelProjection,
    role: &RoleId,
) -> std::result::Result<String, SdkExecutionDiagnostic> {
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
    Ok(found.role_name.clone())
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
        let (key_variable, key_typeql_type) = match &key {
            ReferenceRouteKey::Iid(_, _) => (None, None),
            ReferenceRouteKey::Key(_, _, field) => (
                Some(format!("reference-key-{index}")),
                Some(field_typeql_type(installed, field)?),
            ),
        };
        routes.push(ReferenceRoute {
            key,
            candidate_types: candidate_types.into_iter().collect(),
            key_variable,
            key_typeql_type,
        });
    }
    Ok(routes)
}

fn compile_prerequisite(
    installed: &InstalledRuntimeProjection,
    model: &ModelProjection,
    operation: ProjectedBatchOperation,
    rows: &[(usize, &ProjectedBatchRow)],
    routes: &[ReferenceRoute],
    references: &[RoleReference<'_>],
) -> std::result::Result<Option<ProviderStatement>, SdkExecutionDiagnostic> {
    let include_put = operation == ProjectedBatchOperation::Put;
    if !include_put && references.is_empty() {
        return Ok(None);
    }

    let put_keys = model.reference_read().key_fields();
    let mut variables = vec![
        "kind".to_owned(),
        "ordinal".to_owned(),
        "reference-ordinal".to_owned(),
        "route".to_owned(),
        "wanted-iid".to_owned(),
    ];
    variables.extend((0..put_keys.len()).map(|index| format!("put-key-{index}")));
    variables.extend(routes.iter().filter_map(|route| route.key_variable.clone()));
    let width = variables.len();
    let mut given_rows = Vec::new();
    given_rows
        .try_reserve_exact(rows.len() + references.len())
        .map_err(|_| allocation_failure())?;

    if include_put {
        for (ordinal, row) in rows {
            let mut values = base_prerequisite_row(width, 0, *ordinal, -1, -1)?;
            let create = row_create(row).expect("put rows have create input");
            for (index, field) in put_keys.iter().enumerate() {
                values[5 + index] =
                    given_from_canonical(create.fields()[field][0].value().clone())?;
            }
            given_rows.push(values);
        }
    }

    let route_index = routes
        .iter()
        .enumerate()
        .map(|(index, route)| (route.key.clone(), index))
        .collect::<BTreeMap<_, _>>();
    let mut key_column_by_route = BTreeMap::new();
    let mut next_key_column = 5 + put_keys.len();
    for (index, route) in routes.iter().enumerate() {
        if route.key_variable.is_some() {
            key_column_by_route.insert(index, next_key_column);
            next_key_column += 1;
        }
    }
    for prepared in references {
        let route_key = match prepared.reference.iid() {
            Some(_) => {
                ReferenceRouteKey::Iid(prepared.role.clone(), prepared.reference.type_id().clone())
            }
            None => ReferenceRouteKey::Key(
                prepared.role.clone(),
                prepared.reference.type_id().clone(),
                prepared
                    .reference
                    .keys()
                    .first_key_value()
                    .expect("validated key reference has one key")
                    .0
                    .clone(),
            ),
        };
        let index = *route_index
            .get(&route_key)
            .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
        let mut values = base_prerequisite_row(
            width,
            1,
            prepared.owner_ordinal,
            i64::try_from(prepared.reference_ordinal).map_err(|_| complexity_overflow())?,
            i64::try_from(index).map_err(|_| complexity_overflow())?,
        )?;
        if let Some(iid) = prepared.reference.iid() {
            values[4] = GivenValue::String(iid.to_ascii_lowercase());
        } else {
            let (_, key) = prepared
                .reference
                .keys()
                .first_key_value()
                .expect("validated key reference has one key");
            values[*key_column_by_route
                .get(&index)
                .ok_or_else(SdkExecutionDiagnostic::internal_failure)?] =
                given_from_canonical(key.value().clone())?;
        }
        given_rows.push(values);
    }

    let mut source = String::new();
    source.push_str("given $kind: integer, $ordinal: integer, $reference-ordinal: integer, $route: integer, $wanted-iid: string?");
    for (index, field) in put_keys.iter().enumerate() {
        source.push_str(&format!(
            ", $put-key-{index}: {}?",
            field_typeql_type(installed, field)?
        ));
    }
    for route in routes {
        if let (Some(variable), Some(domain)) = (&route.key_variable, route.key_typeql_type) {
            source.push_str(&format!(", ${variable}: {domain}?"));
        }
    }
    source.push_str(";\nmatch\n");
    let mut branches = Vec::new();
    if include_put {
        let mut branch = format!(
            "{{\n  $kind == 0;\n  $thing isa! {}",
            model.id().label().as_str()
        );
        for (index, field) in put_keys.iter().enumerate() {
            branch.push_str(&format!(
                ", has {} == $put-key-{index}",
                field.attribute().label().as_str()
            ));
        }
        branch.push_str(";\n}");
        branches.push(branch);
    }
    for (index, route) in routes.iter().enumerate() {
        let exact_type_route = match route.candidate_types.as_slice() {
            [candidate] => format!("$thing isa! {};", candidate.label().as_str()),
            candidates => {
                candidates
                    .iter()
                    .map(|candidate| format!("{{ $thing isa! {}; }}", candidate.label().as_str()))
                    .collect::<Vec<_>>()
                    .join(" or ")
                    + ";"
            }
        };
        let identity = match &route.key {
            ReferenceRouteKey::Iid(_, _) => {
                "let $actual-iid = iid($thing);\n  $actual-iid == $wanted-iid;".to_owned()
            }
            ReferenceRouteKey::Key(_, _, field) => format!(
                "$thing has {} == ${};",
                field.attribute().label().as_str(),
                route
                    .key_variable
                    .as_deref()
                    .expect("key route has variable")
            ),
        };
        branches.push(format!(
            "{{\n  $kind == 1;\n  $route == {index};\n  {exact_type_route}\n  {identity}\n}}",
        ));
    }
    source.push_str(&branches.join(" or "));
    source.push_str(
        ";\n$thing isa! $thing-type;\nfetch {\n  \"kind\": $kind,\n  \"ordinal\": $ordinal,\n  \"reference_ordinal\": $reference-ordinal,\n  \"iid\": iid($thing),\n  \"type\": label($thing-type)\n};",
    );
    Ok(Some(ProviderStatement {
        source,
        rows: GivenRowsSpec {
            variables,
            rows: given_rows,
        },
    }))
}

fn base_prerequisite_row(
    width: usize,
    kind: i64,
    ordinal: usize,
    reference_ordinal: i64,
    route: i64,
) -> std::result::Result<Vec<GivenValue>, SdkExecutionDiagnostic> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(width)
        .map_err(|_| allocation_failure())?;
    values.extend([
        GivenValue::Integer(kind),
        ordinal_value(ordinal)?,
        GivenValue::Integer(reference_ordinal),
        GivenValue::Integer(route),
        GivenValue::Empty,
    ]);
    values.resize(width, GivenValue::Empty);
    Ok(values)
}

fn compile_mutation_source(
    installed: &InstalledRuntimeProjection,
    model: &ModelProjection,
    operation: ProjectedBatchOperation,
) -> std::result::Result<String, SdkExecutionDiagnostic> {
    let label = model.id().label().as_str();
    if operation == ProjectedBatchOperation::Delete {
        return Ok(format!(
            "given $ordinal: integer, $target-iid: string;\nmatch\ntry {{\n  $thing isa! {label};\n  let $actual-iid = iid($thing);\n  $actual-iid == $target-iid;\n}};\ndelete try {{ $thing; }};\nselect $ordinal;\nfetch {{ \"ordinal\": $ordinal }};"
        ));
    }

    let mut source = String::new();
    match operation {
        ProjectedBatchOperation::Insert => {
            source.push_str(&format!(
                "given $ordinal: integer;\ninsert\n$thing isa {label};"
            ));
        }
        ProjectedBatchOperation::Update => {
            source.push_str(&format!(
                "given $ordinal: integer, $target-iid: string;\nmatch\n$thing isa! {label};\nlet $actual-iid = iid($thing);\n$actual-iid == $target-iid;"
            ));
            append_clear_stages(installed, model, operation, &mut source)?;
        }
        ProjectedBatchOperation::Put => {
            source.push_str("given $ordinal: integer");
            for (index, field) in model.reference_read().key_fields().iter().enumerate() {
                source.push_str(&format!(
                    ", $put-key-{index}: {}",
                    field_typeql_type(installed, field)?
                ));
            }
            source.push_str(&format!(";\nput\n$thing isa {label}"));
            for (index, field) in model.reference_read().key_fields().iter().enumerate() {
                source.push_str(&format!(
                    ", has {} == $put-key-{index}",
                    field.attribute().label().as_str()
                ));
            }
            source.push_str(&format!(
                ";\nmatch\n$thing isa! {label};\nselect $ordinal, $thing;\ndistinct;"
            ));
            append_clear_stages(installed, model, operation, &mut source)?;
        }
        ProjectedBatchOperation::Delete => unreachable!(),
    }
    source.push_str("\nfetch {\n  \"ordinal\": $ordinal,\n  \"iid\": iid($thing)\n};");
    Ok(source)
}

fn append_clear_stages(
    installed: &InstalledRuntimeProjection,
    model: &ModelProjection,
    operation: ProjectedBatchOperation,
    source: &mut String,
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    let attributes = match model.id().kind() {
        TypeKind::Entity => {
            &installed
                .entity_descriptor(model.id())
                .map_err(|error| runtime_projection_error(error, model.id()))?
                .owned_attributes
        }
        TypeKind::Relation => {
            &installed
                .relation_descriptor(model.id())
                .map_err(|error| runtime_projection_error(error, model.id()))?
                .owned_attributes
        }
        _ => return Err(SdkExecutionDiagnostic::internal_failure()),
    };
    for (index, attribute) in attributes
        .iter()
        .filter(|attribute| operation != ProjectedBatchOperation::Put || !attribute.is_key())
        .enumerate()
    {
        source.push_str(&format!(
            "\nmatch\ntry {{\n  $thing has {} $old-attribute-{index};\n  $old-attribute-{index} isa! {};\n}};\ndelete try {{ has $old-attribute-{index} of $thing; }};\nselect $ordinal, $thing;\ndistinct;",
            attribute.attr_name, attribute.attr_name
        ));
    }
    if model.id().kind() == TypeKind::Relation {
        let descriptor = installed
            .relation_descriptor(model.id())
            .map_err(|error| runtime_projection_error(error, model.id()))?;
        for (index, role) in descriptor.roles.iter().enumerate() {
            // This exact relation binding is intentionally repeated before
            // every links stage. It prevents the TypeDB 3.12.1 mixed-kind
            // `links` executor panic from ever receiving an untyped relation
            // position.
            source.push_str(&format!(
                "\nmatch\n$thing isa! {};\nmatch\ntry {{ $thing links ({}: $old-player-{index}); }};\ndelete try {{ links ({}: $old-player-{index}) of $thing; }};\nselect $ordinal, $thing;\ndistinct;",
                model.id().label().as_str(), role.role_name, role.role_name
            ));
        }
    }
    Ok(())
}

fn compile_attach_source(
    _installed: &InstalledRuntimeProjection,
    model: &ModelProjection,
    fields: &[FieldRoute],
    roles: &[RoleRoute],
) -> std::result::Result<String, SdkExecutionDiagnostic> {
    let label = model.id().label().as_str();
    let mut source = "given $ordinal: integer, $target-iid: string".to_owned();
    for field in fields {
        source.push_str(&format!(", ${}: {}?", field.variable, field.typeql_type));
    }
    for role in roles {
        source.push_str(&format!(", ${}: string?", role.variable));
    }
    source.push_str(&format!(
        ";\nmatch\n$thing isa! {label};\nlet $actual-iid = iid($thing);\n$actual-iid == $target-iid;\n$thing isa! $thing-type;"
    ));
    for (index, role) in roles.iter().enumerate() {
        source.push_str(&format!(
            "\ntry {{\n  ${} isa! {};\n  let $player-actual-iid-{index} = iid(${});\n  $player-actual-iid-{index} == ${};\n}};",
            role.player_variable,
            role.key.player_type.label().as_str(),
            role.player_variable,
            role.variable
        ));
    }
    if !fields.is_empty() || !roles.is_empty() {
        source.push_str("\ninsert");
        for field in fields {
            source.push_str(&format!(
                "\ntry {{ $thing has {} == ${}; }};",
                field.field.attribute().label().as_str(),
                field.variable
            ));
        }
        for role in roles {
            if model.id().kind() != TypeKind::Relation {
                return Err(SdkExecutionDiagnostic::internal_failure());
            }
            // `$thing` is exact-bound to the concrete relation above; only
            // the player position varies across exact entity/relation routes.
            source.push_str(&format!(
                "\ntry {{ $thing links ({}: ${}); }};",
                role.provider_role, role.player_variable
            ));
        }
    }
    source.push_str("\nselect $ordinal, $thing, $thing-type;\ndistinct;");
    if model.id().kind() == TypeKind::Relation {
        source.push_str("\nreduce $event-count = count groupby $ordinal, $thing, $thing-type;");
    }
    source.push_str(
        "\nfetch {\n  \"ordinal\": $ordinal,\n  \"_iid\": iid($thing),\n  \"_type\": label($thing-type),\n  \"attributes\": { $thing.* }",
    );
    if model.id().kind() == TypeKind::Relation {
        source.push_str(
            ",\n  \"role_players\": [\n    match\n    $thing links ($role: $player);\n    $player isa! $player-type;\n    fetch {\n      \"role\": label($role),\n      \"iid\": iid($player),\n      \"type_name\": label($player-type),\n      \"attributes\": { $player.* }\n    };\n  ]",
        );
    }
    source.push_str("\n};");
    Ok(source)
}

struct ResolvedReference {
    concrete_type: TypeId,
    iid: String,
}

struct ResolvedPrerequisites {
    put_hits: Vec<Option<String>>,
    put_seen: Vec<bool>,
    references: Vec<Option<ResolvedReference>>,
}

impl ResolvedPrerequisites {
    fn new(plan: &BatchPlan<'_>) -> std::result::Result<Self, SdkExecutionDiagnostic> {
        let mut put_hits = Vec::new();
        let mut put_seen = Vec::new();
        if plan.operation == ProjectedBatchOperation::Put {
            put_hits
                .try_reserve_exact(plan.row_count)
                .map_err(|_| allocation_failure())?;
            put_hits.resize(plan.row_count, None);
            put_seen
                .try_reserve_exact(plan.row_count)
                .map_err(|_| allocation_failure())?;
            put_seen.resize(plan.row_count, false);
        }
        let mut references = Vec::new();
        references
            .try_reserve_exact(plan.role_references.len())
            .map_err(|_| allocation_failure())?;
        references.resize_with(plan.role_references.len(), || None);
        Ok(Self {
            put_hits,
            put_seen,
            references,
        })
    }

    fn finish(&self, plan: &BatchPlan<'_>) -> std::result::Result<(), SdkExecutionDiagnostic> {
        for (index, resolved) in self.references.iter().enumerate() {
            if resolved.is_none() {
                let prepared = &plan.role_references[index];
                return Err(integrity_at_reference(
                    "projected_batch_reference_missing",
                    "A projected role-player reference did not resolve exactly once",
                    plan.model.id(),
                    &prepared.role,
                    prepared.owner_ordinal,
                    prepared.reference_ordinal,
                    prepared.reference.type_id(),
                ));
            }
        }
        Ok(())
    }

    fn reference(
        &self,
        index: usize,
    ) -> std::result::Result<&ResolvedReference, SdkExecutionDiagnostic> {
        self.references
            .get(index)
            .and_then(Option::as_ref)
            .ok_or_else(SdkExecutionDiagnostic::internal_failure)
    }
}

struct PrerequisiteConsumer<'plan, 'result> {
    plan: &'plan BatchPlan<'plan>,
    resolved: &'result mut ResolvedPrerequisites,
    deferred: Option<SdkExecutionDiagnostic>,
}

impl<'plan, 'result> PrerequisiteConsumer<'plan, 'result> {
    fn new(plan: &'plan BatchPlan<'plan>, resolved: &'result mut ResolvedPrerequisites) -> Self {
        Self {
            plan,
            resolved,
            deferred: None,
        }
    }

    fn finish(self) -> std::result::Result<(), SdkExecutionDiagnostic> {
        self.deferred.map_or(Ok(()), Err)
    }

    fn take_deferred(&mut self) -> Option<SdkExecutionDiagnostic> {
        self.deferred.take()
    }

    fn accept_document(
        &mut self,
        document: Value,
    ) -> std::result::Result<(), SdkExecutionDiagnostic> {
        let kind = document_integer(&document, "kind")?;
        let ordinal = document_index(&document, "ordinal", self.plan.row_count)?;
        let mut iid = document_string(&document, "iid")?;
        let concrete_label = document_string(&document, "type")?;
        if !is_canonical_thing_iid(&iid) {
            return Err(integrity_at_type(
                "provider_identity_mismatch",
                "The provider prerequisite returned a noncanonical IID",
                self.plan.model.id(),
            ));
        }
        iid.make_ascii_lowercase();
        match kind {
            0 if self.plan.operation == ProjectedBatchOperation::Put => {
                if concrete_label != self.plan.model.id().label().as_str() {
                    return Err(integrity_at_type(
                        "provider_identity_mismatch",
                        "The provider put prerequisite returned a different exact type",
                        self.plan.model.id(),
                    ));
                }
                if self.resolved.put_seen[ordinal] {
                    return Err(integrity_at_row(
                        "projected_batch_put_identity_ambiguous",
                        "A projected put key resolved more than one existing exact thing",
                        self.plan.model.id(),
                        ordinal,
                    ));
                }
                self.resolved.put_seen[ordinal] = true;
                self.resolved.put_hits[ordinal] = Some(iid);
                Ok(())
            }
            1 => {
                let reference_ordinal = document_index(
                    &document,
                    "reference_ordinal",
                    self.plan.role_references.len(),
                )?;
                let prepared = &self.plan.role_references[reference_ordinal];
                if prepared.owner_ordinal != ordinal {
                    return Err(integrity_at_reference(
                        "projected_batch_reference_ordinal_mismatch",
                        "Provider prerequisite evidence was correlated to a different input row",
                        self.plan.model.id(),
                        &prepared.role,
                        prepared.owner_ordinal,
                        reference_ordinal,
                        prepared.reference.type_id(),
                    ));
                }
                let concrete_type = resolve_concrete_type(self.plan, prepared, &concrete_label)?;
                if let Some(expected_iid) = prepared.reference.iid()
                    && !iid.eq_ignore_ascii_case(expected_iid)
                {
                    return Err(integrity_at_reference(
                        "provider_identity_mismatch",
                        "The provider prerequisite returned a different requested IID",
                        self.plan.model.id(),
                        &prepared.role,
                        ordinal,
                        reference_ordinal,
                        prepared.reference.type_id(),
                    ));
                }
                if self.resolved.references[reference_ordinal].is_some() {
                    return Err(integrity_at_reference(
                        "projected_batch_reference_ambiguous",
                        "A projected role-player reference resolved more than one exact thing",
                        self.plan.model.id(),
                        &prepared.role,
                        ordinal,
                        reference_ordinal,
                        prepared.reference.type_id(),
                    ));
                }
                self.resolved.references[reference_ordinal] =
                    Some(ResolvedReference { concrete_type, iid });
                Ok(())
            }
            _ => Err(integrity_at_type(
                "projected_batch_prerequisite_kind_invalid",
                "Provider prerequisite evidence carried an unknown lookup kind",
                self.plan.model.id(),
            )),
        }
    }
}

impl AnswerConsumer for PrerequisiteConsumer<'_, '_> {
    fn accept(&mut self, item: AnswerItem) -> crate::Result<AnswerControl> {
        if self.deferred.is_some() {
            return Ok(AnswerControl::Continue);
        }
        let result = match item {
            AnswerItem::Document(document) => self.accept_document(document),
            AnswerItem::Row(_) => Err(integrity_at_type(
                "projected_batch_answer_kind_invalid",
                "A projected batch prerequisite returned rows instead of documents",
                self.plan.model.id(),
            )),
        };
        if let Err(error) = result {
            self.deferred = Some(error);
        }
        Ok(AnswerControl::Continue)
    }
}

fn resolve_concrete_type(
    plan: &BatchPlan<'_>,
    prepared: &RoleReference<'_>,
    label: &str,
) -> std::result::Result<TypeId, SdkExecutionDiagnostic> {
    let candidates = plan
        .role_routes
        .iter()
        .filter(|route| {
            route.key.role == prepared.role
                && route.key.player_type.label().as_str() == label
                && is_same_or_subtype(
                    plan.installed,
                    &route.key.player_type,
                    prepared.reference.type_id(),
                )
        })
        .map(|route| route.key.player_type.clone())
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [candidate] => Ok(candidate.clone()),
        _ => Err(integrity_at_reference(
            if candidates.is_empty() {
                "projected_batch_reference_type_invalid"
            } else {
                "projected_batch_reference_type_ambiguous"
            },
            if candidates.is_empty() {
                "The provider resolved a role-player type outside the projected domain"
            } else {
                "The provider role-player label maps to more than one projected concrete type"
            },
            plan.model.id(),
            &prepared.role,
            prepared.owner_ordinal,
            prepared.reference_ordinal,
            prepared.reference.type_id(),
        )),
    }
}

struct OrdinalIdentityConsumer {
    slots: Vec<Option<String>>,
    operation: ProjectedBatchOperation,
    deferred: Option<SdkExecutionDiagnostic>,
}

impl OrdinalIdentityConsumer {
    fn new(
        row_count: usize,
        operation: ProjectedBatchOperation,
    ) -> std::result::Result<Self, SdkExecutionDiagnostic> {
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(row_count)
            .map_err(|_| allocation_failure())?;
        slots.resize(row_count, None);
        Ok(Self {
            slots,
            operation,
            deferred: None,
        })
    }

    fn finish(
        &self,
        plan: &BatchPlan<'_>,
        resolved: &ResolvedPrerequisites,
    ) -> std::result::Result<(), SdkExecutionDiagnostic> {
        if let Some(error) = &self.deferred {
            return Err(error.clone());
        }
        for (ordinal, value) in self.slots.iter().enumerate() {
            if value.is_none() {
                return Err(integrity_at_row(
                    "projected_batch_mutation_result_missing",
                    "A projected batch mutation omitted one input ordinal",
                    plan.model.id(),
                    ordinal,
                ));
            }
            if plan.operation == ProjectedBatchOperation::Put
                && let Some(expected) = &resolved.put_hits[ordinal]
                && !value
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case(expected))
            {
                return Err(integrity_at_row(
                    "projected_batch_put_identity_changed",
                    "A projected put hit returned a different exact IID",
                    plan.model.id(),
                    ordinal,
                ));
            }
            if plan.operation == ProjectedBatchOperation::Update {
                let expected = row_iid(plan.rows[ordinal].1)
                    .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                if !value
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case(expected))
                {
                    return Err(integrity_at_row(
                        "projected_batch_update_identity_changed",
                        "A projected update returned a different exact target IID",
                        plan.model.id(),
                        ordinal,
                    ));
                }
            }
            if plan.operation == ProjectedBatchOperation::Delete
                && let Some(returned) = value.as_deref()
                && !returned.is_empty()
            {
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
        if plan.operation != ProjectedBatchOperation::Delete {
            for (ordinal, iid) in self.slots.iter().enumerate() {
                let iid = iid
                    .as_deref()
                    .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                if let Some(first) = self.slots[..ordinal].iter().position(|candidate| {
                    candidate
                        .as_deref()
                        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(iid))
                }) {
                    return Err(integrity_at_row_with_first(
                        "projected_batch_mutation_identity_duplicate",
                        "Provider mutation returned the same IID for two input ordinals",
                        plan.model.id(),
                        ordinal,
                        first,
                    ));
                }
            }
        }
        Ok(())
    }

    fn take_deferred(&mut self) -> Option<SdkExecutionDiagnostic> {
        self.deferred.take()
    }

    fn move_iids_into(
        self,
        output: &mut Vec<String>,
    ) -> std::result::Result<(), SdkExecutionDiagnostic> {
        if !output.is_empty() || output.capacity() < self.slots.len() {
            return Err(SdkExecutionDiagnostic::internal_failure());
        }
        for value in self.slots {
            output.push(value.ok_or_else(SdkExecutionDiagnostic::internal_failure)?);
        }
        Ok(())
    }
}

impl AnswerConsumer for OrdinalIdentityConsumer {
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
                None if self.operation == ProjectedBatchOperation::Delete => String::new(),
                None => {
                    return Err(integrity(
                        "projected_batch_mutation_identity_missing",
                        "A projected batch mutation omitted its canonical IID",
                    ));
                }
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
            if self.operation != ProjectedBatchOperation::Delete && !is_canonical_thing_iid(&value)
            {
                return Err(integrity(
                    "provider_identity_mismatch",
                    "A projected batch mutation returned a noncanonical IID",
                ));
            }
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

async fn dispatch(
    lease: &mut TransactionContextMutationLease,
    mutation: bool,
    statement: ProviderStatement,
    prepared: &PreparedProjectedBatchInvocation<'_>,
    consumer: &mut dyn AnswerConsumer,
    max_items: u64,
    max_bytes: u64,
) -> std::result::Result<BoundedAnswerStats, SdkExecutionDiagnostic> {
    let ProviderStatement { source, rows } = statement;
    dispatch_rows_ref(
        lease, mutation, &source, rows, prepared, consumer, max_items, max_bytes,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_rows(
    lease: &mut TransactionContextMutationLease,
    mutation: bool,
    source: &str,
    rows: GivenRowsSpec,
    prepared: &PreparedProjectedBatchInvocation<'_>,
    consumer: &mut dyn AnswerConsumer,
    max_items: u64,
    max_bytes: u64,
) -> std::result::Result<BoundedAnswerStats, SdkExecutionDiagnostic> {
    dispatch_rows_ref(
        lease, mutation, source, rows, prepared, consumer, max_items, max_bytes,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_rows_ref(
    lease: &mut TransactionContextMutationLease,
    mutation: bool,
    source: &str,
    rows: GivenRowsSpec,
    prepared: &PreparedProjectedBatchInvocation<'_>,
    consumer: &mut dyn AnswerConsumer,
    max_items: u64,
    max_bytes: u64,
) -> std::result::Result<BoundedAnswerStats, SdkExecutionDiagnostic> {
    prepared.check_control()?;
    let limits = QueryV2AnswerLimits {
        answer: BoundedAnswerLimits {
            max_items: max_items.min(prepared.limits().items),
            max_bytes,
            deadline: Some(prepared.deadline().instant()),
            cancellation: prepared.cancellation().clone(),
        },
        max_collection_members: prepared.limits().collection_members,
    };
    let operation = if mutation {
        SdkProviderOperation::Write
    } else {
        SdkProviderOperation::Read
    };
    let mut observed = LocallyBoundedConsumer::new(consumer, limits.answer.clone());
    let future = async {
        if mutation {
            lease
                .mutate_v2_with_rows_bounded(source, rows, limits, &mut observed)
                .await
        } else {
            lease
                .query_v2_with_rows_bounded(source, rows, limits, &mut observed)
                .await
        }
    };
    let mut provider = await_controlled(future, prepared.deadline(), prepared.cancellation())
        .await
        .map_err(|error| controlled_orm(error, operation, prepared.batch().model()))?;
    let local = observed.stats();
    provider.processed_items = provider.processed_items.max(local.processed_items);
    provider.response_bytes = provider.response_bytes.max(local.response_bytes);
    provider.stopped_early |= local.stopped_early;
    Ok(provider)
}

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

struct ExecutionLedger {
    limits: QueryExecutionResourceLimits,
    bytes: u64,
    items: u64,
    statements: u32,
}

impl ExecutionLedger {
    const fn new(limits: QueryExecutionResourceLimits, input_bytes: u64) -> Self {
        Self {
            limits,
            bytes: input_bytes,
            items: 0,
            statements: 0,
        }
    }

    fn charge_preflight(
        &mut self,
        request_bytes: u64,
        statements: u32,
    ) -> std::result::Result<(), SdkExecutionDiagnostic> {
        if statements > self.limits.statements {
            return Err(resource_limit(
                "projected_batch_statement_limit",
                "Projected batch execution exceeds the provider statement ceiling",
                "statements",
                u64::from(statements),
                u64::from(self.limits.statements),
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

    fn record_reply(
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

    fn remaining_bytes(&self) -> u64 {
        self.limits.bytes.saturating_sub(self.bytes)
    }

    fn remaining_items(&self) -> u64 {
        self.limits.items.saturating_sub(self.items)
    }

    fn require_remaining_items(
        &self,
        required: u64,
    ) -> std::result::Result<(), SdkExecutionDiagnostic> {
        let remaining = self.remaining_items();
        if required > remaining {
            return Err(resource_limit(
                "projected_batch_reply_item_limit",
                "Projected batch requires more correlated provider answers than remain",
                "items",
                self.items.saturating_add(required),
                self.limits.items,
            ));
        }
        Ok(())
    }

    fn charge_projected(
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
    slots: Vec<Option<ProjectedThing>>,
    ordered: Vec<ProjectedThing>,
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
        let mut ordered = Vec::new();
        ordered
            .try_reserve_exact(plan.row_count)
            .map_err(|_| allocation_failure())?;
        Ok(Self {
            executor,
            plan,
            identity,
            targets,
            slots,
            ordered,
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
        if self.slots[ordinal].is_some() {
            return Err(integrity_at_row(
                "projected_batch_hydration_duplicate",
                "Provider hydration repeated one projected batch ordinal",
                self.plan.model.id(),
                ordinal,
            ));
        }
        let expected = self.targets.get(ordinal).ok_or_else(|| {
            integrity_at_row(
                "projected_batch_hydration_target_missing",
                "Provider hydration arrived before its mutation identity",
                self.plan.model.id(),
                ordinal,
            )
        })?;
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
        self.slots[ordinal] = Some(thing);
        Ok(())
    }

    fn take_deferred(&mut self) -> Option<SdkExecutionDiagnostic> {
        self.deferred.take()
    }

    fn finish(mut self) -> std::result::Result<Vec<ProjectedThing>, SdkExecutionDiagnostic> {
        if let Some(error) = self.deferred {
            return Err(error);
        }
        for (ordinal, value) in self.slots.into_iter().enumerate() {
            self.ordered.push(value.ok_or_else(|| {
                integrity_at_row(
                    "projected_batch_hydration_missing",
                    "Provider hydration omitted one projected batch ordinal",
                    self.plan.model.id(),
                    ordinal,
                )
            })?);
        }
        Ok(self.ordered)
    }
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

fn preflight_complexity(
    prerequisite: Option<&ProviderStatement>,
    mutation_source: &str,
    attach_source: Option<&str>,
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    if let Some(statement) = prerequisite {
        preflight_statement_complexity(&statement.source)?;
    }
    preflight_statement_complexity(mutation_source)?;
    if let Some(source) = attach_source {
        preflight_statement_complexity(source)?;
    }
    Ok(())
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
    prerequisite: Option<&ProviderStatement>,
    mutation_source: &str,
    attach_source: Option<&str>,
) -> std::result::Result<u64, SdkExecutionDiagnostic> {
    let mut total = 0_u64;
    if let Some(statement) = prerequisite {
        total = checked_add_u64(total, source_bytes(&statement.source)?)?;
        total = checked_add_u64(total, given_rows_bytes(&statement.rows)?)?;
    }
    let mutation_rows = preflight_mutation_rows(prepared.batch().operation(), rows, model)?;
    total = checked_add_u64(total, source_bytes(mutation_source)?)?;
    total = checked_add_u64(total, given_rows_bytes(&mutation_rows)?)?;
    if let Some(source) = attach_source {
        let attach_rows = preflight_attach_rows(rows, fields, roles, references)?;
        total = checked_add_u64(total, source_bytes(source)?)?;
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
        for (field, values) in create.fields() {
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

fn given_rows_bytes(rows: &GivenRowsSpec) -> std::result::Result<u64, SdkExecutionDiagnostic> {
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

fn validate_provider_scalar_inputs(
    rows: &[(usize, &ProjectedBatchRow)],
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    for (ordinal, row) in rows {
        let Some(create) = row_create(row) else {
            continue;
        };
        for (field, values) in create.fields() {
            for value in values {
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
                                u64::try_from(*ordinal).unwrap_or(u64::MAX),
                            ),
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
                                    u64::try_from(*ordinal).unwrap_or(u64::MAX),
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

fn invalid_at_type(
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

fn integrity_at_row(
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

fn integrity_at_row_with_first(
    code_value: &'static str,
    message_value: &'static str,
    type_id: &TypeId,
    ordinal: usize,
    first: usize,
) -> SdkExecutionDiagnostic {
    integrity_at_row(code_value, message_value, type_id, ordinal)
        .try_with_detail(
            name("first_conflicting_index"),
            SdkDiagnosticDetailValue::Count(u64::try_from(first).unwrap_or(u64::MAX)),
        )
        .unwrap_or_else(|_| SdkExecutionDiagnostic::internal_failure())
}

#[allow(clippy::too_many_arguments)]
fn integrity_at_reference(
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

fn resource_limit(
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
    use std::sync::{Arc, Mutex};

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
            _typeql: &str,
            _rows: GivenRowsSpec,
        ) -> BoxFuture<'_, Result<QueryResult, OrmError>> {
            let answer = {
                let mut state = self.state.lock().unwrap();
                state.calls += 1;
                state.answers.pop_front().expect("unexpected provider call")
            };
            Box::pin(async move { Ok(QueryResult::Documents(answer)) })
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
        if kind == TypeKind::Relation {
            answers.push_back(vec![json!({
                "kind": 1,
                "ordinal": 0,
                "reference_ordinal": 0,
                "iid": "0x10",
                "type": "person"
            })]);
        }
        answers.push_back(vec![json!({
            "ordinal": 0,
            "iid": if kind == TypeKind::Entity { "0x20" } else { "0x30" }
        })]);
        answers.push_back(vec![json!({
            "ordinal": 0,
            "_iid": if kind == TypeKind::Entity { "0x20" } else { "0x30" },
            "_type": if kind == TypeKind::Entity { "person" } else { "membership" },
            "attributes": {"identifier": "key"},
            "role_players": []
        })]);
        answers
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
            assert_eq!(state.calls, if kind == TypeKind::Entity { 2 } else { 3 });
            assert_eq!(state.rollbacks, 1);
            assert_eq!(state.commits, 0);
        }
    }
}
