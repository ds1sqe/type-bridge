//! Controlled single writes retain the common single-plan compiler.

use super::*;
use crate::plan_builders::BuiltPlan;
use crate::projected_batch::PreparedProjectedBatchInvocation;
use crate::projected_batch_executor::{ExecutionLedger, given_rows_bytes};

/// Direct target for one controlled single-input write.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub enum WriteTarget<'a> {
    /// Own and complete one write transaction.
    Database(&'a Database),
    /// Reuse the caller's write transaction.
    Transaction(&'a TransactionContext),
}

pub(crate) enum WriteModel {
    Entity(PreparedEntityCreate),
    Relation(PreparedRelationCreate),
}

pub(crate) struct SingleWrite {
    model: WriteModel,
    pub(crate) plan: BuiltPlan,
}

impl ProjectedCrudExecutor<'_> {
    /// Validate and retain the one input of a controlled write before dispatch.
    #[doc(hidden)]
    pub fn single_controlled_write_batch(
        &self,
        model: &TypeId,
        operation: ProjectedBatchOperation,
        row: ProjectedBatchRow,
        control: &ProjectedBatchInvocationControl,
    ) -> Result<ProjectedBatch, SdkExecutionDiagnostic> {
        let operation = if operation == ProjectedBatchOperation::Put
            && self
                .installed
                .projection()
                .models()
                .get(model)
                .is_some_and(|model| model.reference_read().key_fields().is_empty())
        {
            ProjectedBatchOperation::Insert
        } else {
            operation
        };
        ProjectedBatch::try_single_write(self.installed, model.clone(), operation, row, control)
    }

    /// Execute one single write on its connection-selected target.
    ///
    /// Direct targets materialize and map before commit. Proxy targets receive
    /// an already committed reply before hydration, the final control check,
    /// and mapping; failures there use `CommitError::proxy_projection_failure`
    /// to preserve the remote commit outcome.
    #[doc(hidden)]
    pub async fn execute_single_write_on_target<T, F>(
        &self,
        target: crate::QueryTarget<'_>,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
        mapper: F,
    ) -> Result<T, crate::QueryTargetError>
    where
        F: FnOnce(WriteResult<ProjectedThing>) -> Result<T, SdkExecutionDiagnostic>,
    {
        if !batch.is_single_write() || batch.len() != 1 {
            return Err(single_batch_result_invalid(batch.model()).into());
        }
        match target {
            crate::QueryTarget::Database(database) => self
                .execute_controlled_single_write(
                    WriteTarget::Database(database),
                    batch,
                    control,
                    mapper,
                )
                .await
                .map_err(Into::into),
            crate::QueryTarget::Transaction(transaction) => self
                .execute_controlled_single_write(
                    WriteTarget::Transaction(transaction),
                    batch,
                    control,
                    mapper,
                )
                .await
                .map_err(Into::into),
            crate::QueryTarget::Proxy(proxy) => {
                self.execute_proxy_single_write(proxy, batch, control, mapper)
                    .await
            }
        }
    }

    async fn execute_proxy_single_write<T, F>(
        &self,
        proxy: &crate::ProxyQueryConnection,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
        mapper: F,
    ) -> Result<T, crate::QueryTargetError>
    where
        F: FnOnce(WriteResult<ProjectedThing>) -> Result<T, SdkExecutionDiagnostic>,
    {
        let prepared = PreparedProjectedBatchInvocation::try_new(
            self.installed,
            batch,
            control.constrained_by(Some(proxy.answer_limits())),
        )?;
        prepared.check_control()?;
        let replacement = single_write_replacement(batch)?;
        crate::projected_batch_executor::validate_provider_write_inputs(&[batch
            .row_at(0)
            .expect("one checked single write row")])?;
        let identity = proxy.execution_identity();
        let model = self.prepare_write_model(batch.model(), replacement, identity)?;
        let query_limits = proxy
            .manager_write_query_limits(prepared.deadline(), prepared.cancellation())
            .await?;
        let authority = self
            .installed
            .plan_authority_with_query_limits(query_limits.as_ref())
            .map_err(|error| plan_build_failure(&error, batch.model()))?;
        let Some(single) = self.compile_controlled_write(batch, replacement, model, &authority)?
        else {
            return Box::pin(
                ProjectedBatchExecutor::new(self.installed).execute_single_on_proxy_mapped(
                    proxy,
                    batch,
                    prepared.into_control(),
                    mapper,
                ),
            )
            .await;
        };
        let entry = type_bridge_contract::query_remote_v3::StagedPlanV3::new(
            single.plan.local(),
            single
                .plan
                .local()
                .given_rows()
                .unwrap_or_default()
                .to_vec(),
            authority.declared(),
        )
        .map_err(|error| plan_build_failure(&error, batch.model()))?;
        let mut ledger = ExecutionLedger::new(prepared.limits(), batch.resource_measure().bytes());
        ledger.charge_preflight(0, 1)?;
        proxy
            .execute_write_stack_mapped(
                vec![entry],
                prepared.limits(),
                prepared.cancellation(),
                prepared.deadline(),
                |mut answers| {
                    if answers.len() != 1 {
                        return Err(write_plan_invalid(batch.model()));
                    }
                    let Some(PlanAnswer::Documents(documents)) = answers.pop() else {
                        return Err(write_plan_invalid(batch.model()));
                    };
                    let things = self.hydrate_controlled_documents(
                        identity,
                        batch,
                        &single,
                        &documents,
                        ExecutionMode::default(),
                    )?;
                    ledger.charge_projected(things.items())?;
                    prepared.check_control()?;
                    mapper(things)
                },
            )
            .await
            .map_err(|error| crate::QueryTargetError::Commit(Box::new(error)))
    }

    /// Execute one direct write and map its complete answer before committing.
    ///
    /// Binding compatibility adapters use this boundary to retain their result
    /// shape without moving policy preflight or materialization after commit.
    /// Proxy targets use `execute_single_write_on_target` and map after the
    /// server commits.
    #[doc(hidden)]
    pub async fn execute_controlled_single_write<T, F>(
        &self,
        target: WriteTarget<'_>,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
        mapper: F,
    ) -> Result<T, SdkExecutionDiagnostic>
    where
        F: FnOnce(WriteResult<ProjectedThing>) -> Result<T, SdkExecutionDiagnostic>,
    {
        self.execute_controlled_single_write_with_mode(
            target,
            batch,
            control,
            ExecutionMode::default(),
            mapper,
        )
        .await
    }

    /// Map a borrowed single write while retaining the Rust facade's original
    /// provider cause and compatibility phase under the mutation lease.
    #[doc(hidden)]
    pub async fn execute_single_write_in_transaction_with_compatibility<T, F>(
        &self,
        transaction: &TransactionContext,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
        mapper: F,
    ) -> Result<T, ProjectedCrudCompatibilityFailure>
    where
        F: FnOnce(WriteResult<ProjectedThing>) -> Result<T, SdkExecutionDiagnostic>,
    {
        let capture = CompatibilityCapture::default();
        let result = self
            .execute_controlled_single_write_with_mode(
                WriteTarget::Transaction(transaction),
                batch,
                control,
                ExecutionMode::compatibility(&capture),
                mapper,
            )
            .await;
        compatibility_result(result, &capture)
    }

    async fn execute_controlled_single_write_with_mode<T, F>(
        &self,
        target: WriteTarget<'_>,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
        mode: ExecutionMode<'_>,
        mapper: F,
    ) -> Result<T, SdkExecutionDiagnostic>
    where
        F: FnOnce(WriteResult<ProjectedThing>) -> Result<T, SdkExecutionDiagnostic>,
    {
        if batch.len() != 1 {
            return Err(crate::projected_batch_executor::invalid_at_type(
                "projected_crud_single_row_required",
                "Single-write execution requires exactly one input row",
                batch.model(),
            ));
        }
        let ceiling = match target {
            WriteTarget::Database(database) => database.answer_limits(),
            WriteTarget::Transaction(transaction) => transaction.answer_limits(),
        };
        let prepared = PreparedProjectedBatchInvocation::try_new(
            self.installed,
            batch,
            control.constrained_by(ceiling),
        )?;
        if let WriteTarget::Transaction(transaction) = target
            && transaction.tx_type() != TxType::Write
        {
            return Err(crate::projected_batch_executor::invalid_at_type(
                "transaction_type_mismatch",
                "Projected batch execution requires a borrowed write transaction",
                batch.model(),
            ));
        }
        prepared.check_control()?;
        let identity = match target {
            WriteTarget::Database(database) => database.execution_identity(),
            WriteTarget::Transaction(transaction) => transaction.execution_identity().clone(),
        };
        let given = match target {
            WriteTarget::Database(database) => database.supports_given_stage(),
            WriteTarget::Transaction(transaction) => await_crud_controlled(
                transaction.supports_given_stage(),
                prepared.deadline(),
                prepared.cancellation(),
            )
            .await
            .map_err(|error| {
                mode.controlled_crud_orm(error, SdkProviderOperation::Write, batch.model())
            })?,
        };
        crate::projected_batch_executor::require_native_provider(match target {
            WriteTarget::Database(database) => database.server_version(),
            WriteTarget::Transaction(transaction) => transaction.server_version(),
        })?;
        let query_limits = match target {
            WriteTarget::Database(database) => database.shared_query_limits(),
            WriteTarget::Transaction(transaction) => transaction.shared_query_limits(),
        };
        let Some(single) = self.prepare_controlled_write(batch, &identity, query_limits)? else {
            return Box::pin(self.execute_forced_single_write(target, batch, prepared, mapper))
                .await;
        };
        let query = if given {
            crate::query_plan_exec::staged_query(single.plan.local(), true)
        } else {
            crate::query_plan_exec::staged_literal_query(single.plan.local())
        }
        .map_err(|error| plan_build_failure(&error, batch.model()))?;
        let row_bytes = query
            .rows
            .as_ref()
            .map(|rows| given_rows_bytes(&rows.rows))
            .transpose()?
            .unwrap_or(0);
        let request_bytes = u64::try_from(query.typeql.len())
            .unwrap_or(u64::MAX)
            .saturating_add(row_bytes);
        let mut ledger = ExecutionLedger::new(prepared.limits(), batch.resource_measure().bytes());
        ledger.charge_preflight(request_bytes, 1)?;
        prepared.check_control()?;
        match target {
            WriteTarget::Database(database) => {
                let transaction = await_crud_controlled(
                    database.transaction_context(TxType::Write),
                    prepared.deadline(),
                    prepared.cancellation(),
                )
                .await
                .map_err(|error| {
                    mode.controlled_crud_orm(
                        error,
                        SdkProviderOperation::OpenWriteTransaction,
                        batch.model(),
                    )
                })?;
                let result = self
                    .run_controlled_write(
                        &transaction,
                        &prepared,
                        &single,
                        query,
                        &mut ledger,
                        mapper,
                        mode,
                    )
                    .await;
                match result.and_then(|thing| prepared.check_control().map(|()| thing)) {
                    Ok(thing) => {
                        transaction.commit_sdk().await.map_err(|error| {
                            append_path(
                                error,
                                [SdkDiagnosticPathSegment::Type(batch.model().clone())],
                            )
                        })?;
                        Ok(thing)
                    }
                    Err(primary) => {
                        let _ = transaction.rollback().await;
                        Err(primary)
                    }
                }
            }
            WriteTarget::Transaction(transaction) => {
                self.run_controlled_write(
                    transaction,
                    &prepared,
                    &single,
                    query,
                    &mut ledger,
                    mapper,
                    mode,
                )
                .await
            }
        }
    }

    async fn execute_forced_single_write<T, F>(
        &self,
        target: WriteTarget<'_>,
        batch: &ProjectedBatch,
        prepared: PreparedProjectedBatchInvocation<'_>,
        mapper: F,
    ) -> Result<T, SdkExecutionDiagnostic>
    where
        F: FnOnce(WriteResult<ProjectedThing>) -> Result<T, SdkExecutionDiagnostic>,
    {
        let mapper = |result| {
            single_batch_write_result(result, batch.model())
                .and_then(mapper)
                .map(Some)
        };
        let control = prepared.into_control();
        let executor = ProjectedBatchExecutor::new(self.installed);
        let result = match target {
            WriteTarget::Database(database) => {
                executor
                    .execute_mapped(database, batch, control, None, mapper)
                    .await
            }
            WriteTarget::Transaction(transaction) => {
                executor
                    .execute_in_transaction_mapped(transaction, batch, control, None, mapper)
                    .await
            }
        }?;
        result.ok_or_else(|| single_batch_result_invalid(batch.model()))
    }

    fn prepare_controlled_write(
        &self,
        batch: &ProjectedBatch,
        identity: &DatabaseExecutionIdentity,
        query_limits: Option<&Arc<type_bridge_contract::limits::QueryLimits>>,
    ) -> Result<Option<SingleWrite>, SdkExecutionDiagnostic> {
        let replacement = single_write_replacement(batch)?;
        crate::projected_batch_executor::validate_provider_write_inputs(&[batch
            .row_at(0)
            .expect("one validated write row")])?;
        let authority = self
            .installed
            .plan_authority_with_query_limits(query_limits)
            .map_err(|error| plan_build_failure(&error, batch.model()))?;
        let model = self.prepare_write_model(batch.model(), replacement, identity)?;
        self.compile_controlled_write(batch, replacement, model, &authority)
    }

    pub(crate) fn prepare_write_model(
        &self,
        model: &TypeId,
        replacement: &ProjectedCreate,
        identity: &DatabaseExecutionIdentity,
    ) -> Result<WriteModel, SdkExecutionDiagnostic> {
        match model.kind() {
            TypeKind::Entity => Ok(WriteModel::Entity(self.prepare_entity_create(replacement)?)),
            TypeKind::Relation => {
                let prepared = self.prepare_relation_create(replacement)?;
                require_role_player_database_identity(&prepared, identity)?;
                Ok(WriteModel::Relation(prepared))
            }
            _ => Err(single_batch_result_invalid(model)),
        }
    }

    pub(crate) fn prepare_filtered_update(
        &self,
        filter: &crate::ProjectedManagerFilter,
        model: WriteModel,
        authority: &PlanAuthority,
    ) -> Result<SingleWrite, SdkExecutionDiagnostic> {
        let (write, players, roles) = match &model {
            WriteModel::Entity(prepared) => (&prepared.plan_write, Vec::new(), Vec::new()),
            WriteModel::Relation(prepared) => (
                &prepared.plan_write,
                plan_references(prepared)?,
                self.document_roles(prepared)?,
            ),
        };
        let plan = update(
            authority,
            ValueTransport::Given,
            ThingUpdate {
                type_id: filter.model(),
                selection: UpdateSelection::Filter(filter),
                cleared_fields: &write.cleared_fields,
                fields: &write.fields,
                cleared_roles: &write.cleared_roles,
                players: &players,
                document_roles: roles,
            },
        )
        .map_err(|error| plan_build_failure(&error, filter.model()))?;
        Ok(SingleWrite { model, plan })
    }

    pub(crate) fn hydrate_filtered_update(
        &self,
        single: &SingleWrite,
        identity: &DatabaseExecutionIdentity,
        documents: &[serde_json::Value],
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        self.hydrate_write_documents(identity, single, documents, ExecutionMode::default())
    }

    fn compile_controlled_write(
        &self,
        batch: &ProjectedBatch,
        replacement: &ProjectedCreate,
        model: WriteModel,
        authority: &PlanAuthority,
    ) -> Result<Option<SingleWrite>, SdkExecutionDiagnostic> {
        let (write, players, roles) = match &model {
            WriteModel::Entity(prepared) => (&prepared.plan_write, Vec::new(), Vec::new()),
            WriteModel::Relation(prepared) => (
                &prepared.plan_write,
                plan_references(prepared)?,
                self.document_roles(prepared)?,
            ),
        };
        let relation_put = if batch.operation() == ProjectedBatchOperation::Put
            && matches!(&model, WriteModel::Relation(_))
        {
            self.lower_keyed_put(replacement)?
        } else {
            None
        };
        let plan = if let Some((_, ProjectedBatchRow::Update { iid, .. })) = batch.row_at(0) {
            update(
                authority,
                ValueTransport::Given,
                ThingUpdate {
                    type_id: batch.model(),
                    selection: UpdateSelection::Iid(iid),
                    cleared_fields: &write.cleared_fields,
                    fields: &write.fields,
                    cleared_roles: &write.cleared_roles,
                    players: &players,
                    document_roles: roles,
                },
            )
        } else if batch.operation() == ProjectedBatchOperation::Put
            && let Some(put) = match &model {
                WriteModel::Entity(model) => model.keyed_put.as_ref(),
                WriteModel::Relation(_) => relation_put.as_ref(),
            }
        {
            let put = KeyedPut {
                include_subtypes: false,
                return_document: true,
                type_id: batch.model(),
                keys: &put.keys,
                fields: &put.fields,
                cleared: &put.cleared,
                attached: &put.attached,
            };
            match &model {
                WriteModel::Entity(_) => keyed_entity_put(authority, ValueTransport::Given, &put),
                WriteModel::Relation(_) => keyed_relation_put(
                    authority,
                    ValueTransport::Given,
                    &put,
                    RelationPut {
                        players: &players,
                        cleared_roles: &write.cleared_roles,
                        document_roles: roles,
                    },
                ),
            }
        } else {
            match &model {
                WriteModel::Entity(_) => entity_insert(
                    authority,
                    ValueTransport::Given,
                    batch.model(),
                    &write.created,
                ),
                WriteModel::Relation(prepared) => relation_insert(
                    authority,
                    ValueTransport::Given,
                    batch.model(),
                    &prepared.plan_write.created,
                    &players,
                    roles,
                ),
            }
        };
        let plan = match plan {
            Ok(plan) => plan,
            Err(error) if crate::plan_builders::bulk_write::forces_pieces(&error) => {
                return Ok(None);
            }
            Err(error) => return Err(plan_build_failure(&error, batch.model())),
        };
        Ok(Some(SingleWrite { model, plan }))
    }

    fn hydrate_write_documents(
        &self,
        identity: &DatabaseExecutionIdentity,
        single: &SingleWrite,
        documents: &[serde_json::Value],
        mode: ExecutionMode<'_>,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        let thing = match &single.model {
            WriteModel::Entity(model) => self.hydrate_entity_documents(
                identity,
                &model.type_id,
                &model.descriptor,
                documents,
                mode,
            )?,
            WriteModel::Relation(model) => self.hydrate_relation_documents(
                identity,
                &model.type_id,
                &model.descriptor,
                documents,
                mode,
            )?,
        };
        Ok(thing)
    }

    fn hydrate_controlled_documents(
        &self,
        identity: &DatabaseExecutionIdentity,
        batch: &ProjectedBatch,
        single: &SingleWrite,
        documents: &[serde_json::Value],
        mode: ExecutionMode<'_>,
    ) -> Result<WriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        let thing = self.hydrate_write_documents(identity, single, documents, mode)?;
        if let Some((_, ProjectedBatchRow::Update { iid, .. })) = batch.row_at(0) {
            for item in thing.items() {
                require_update_iid(item.iid(), iid, batch.model())?;
            }
        }
        Ok(thing)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "Compatibility capture follows the controlled mutation lease"
    )]
    async fn run_controlled_write<T, F>(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedProjectedBatchInvocation<'_>,
        single: &SingleWrite,
        query: crate::session::staged::StagedQuery,
        ledger: &mut ExecutionLedger,
        mapper: F,
        mode: ExecutionMode<'_>,
    ) -> Result<T, SdkExecutionDiagnostic>
    where
        F: FnOnce(WriteResult<ProjectedThing>) -> Result<T, SdkExecutionDiagnostic>,
    {
        let type_id = prepared.batch().model();
        let mut lease = await_crud_controlled(
            transaction.acquire_mutation_lease(),
            prepared.deadline(),
            prepared.cancellation(),
        )
        .await
        .map_err(|error| mode.controlled_crud_orm(error, SdkProviderOperation::Write, type_id))?;
        lease
            .check_plan_provider(single.plan.local())
            .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Write, type_id))?;
        let result = async {
            prepared.check_control()?;
            let mut policy = prepared.limits();
            policy.items = ledger.remaining_items();
            policy.bytes = ledger.remaining_bytes();
            let limits = crate::query_plan_exec::plan_answer_limits(
                policy,
                prepared.cancellation(),
                prepared.deadline(),
            )
            .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Write, type_id))?;
            let (answer, stats) = await_crud_controlled(
                lease.dispatch_plan_counted(query, limits, Some(single.plan.local())),
                prepared.deadline(),
                prepared.cancellation(),
            )
            .await
            .map_err(|error| {
                mode.controlled_crud_orm(error, SdkProviderOperation::Write, type_id)
            })?;
            ledger.record_reply(stats)?;
            prepared.check_control()?;
            let answer = TransactionContext::resolve_plan(single.plan.local(), answer)
                .map_err(|error| mode.orm_at_type(error, SdkProviderOperation::Write, type_id))?;
            let PlanAnswer::Documents(documents) = answer else {
                return Err(write_plan_invalid(type_id));
            };

            let thing = self.hydrate_controlled_documents(
                transaction.execution_identity(),
                prepared.batch(),
                single,
                &documents,
                mode,
            )?;
            ledger.charge_projected(thing.items())?;
            prepared.check_control()?;
            let mapped = mapper(thing)?;
            prepared.check_control()?;
            Ok(mapped)
        }
        .await;
        match result {
            Ok(thing) => {
                lease.complete_success();
                Ok(thing)
            }
            Err(diagnostic) => {
                lease.record_failure(&diagnostic);
                Err(diagnostic)
            }
        }
    }
}

impl ProjectedCrudExecutor<'_> {
    pub(super) async fn execute_controlled_delete(
        &self,
        target: WriteTarget<'_>,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
    ) -> Result<(), SdkExecutionDiagnostic> {
        let ceiling = match target {
            WriteTarget::Database(database) => database.answer_limits(),
            WriteTarget::Transaction(transaction) => transaction.answer_limits(),
        };
        let prepared = PreparedProjectedBatchInvocation::try_new(
            self.installed,
            batch,
            control.constrained_by(ceiling),
        )?;
        if let WriteTarget::Transaction(transaction) = target
            && transaction.tx_type() != TxType::Write
        {
            return Err(crate::projected_batch_executor::invalid_at_type(
                "transaction_type_mismatch",
                "Projected batch execution requires a borrowed write transaction",
                batch.model(),
            ));
        }
        crate::projected_batch_executor::require_native_provider(match target {
            WriteTarget::Database(database) => database.server_version(),
            WriteTarget::Transaction(transaction) => transaction.server_version(),
        })?;
        prepared.check_control()?;
        let Some((_, ProjectedBatchRow::Delete { iid })) = batch.row_at(0) else {
            return Err(single_batch_result_invalid(batch.model()));
        };
        let plans = ManagerPlans::new(
            self.installed
                .manager_registry()
                .map_err(|error| orm_at_type(error, SdkProviderOperation::Write, batch.model()))?,
        );
        let plan = plans
            .delete(batch.model().label().as_str(), iid, true, false)
            .map_err(|error| orm_at_type(error, SdkProviderOperation::Write, batch.model()))?;
        let query = crate::query_plan_exec::staged_query(plan.local(), true)
            .map_err(|error| plan_build_failure(&error, batch.model()))?;
        let mut ledger = ExecutionLedger::new(prepared.limits(), batch.resource_measure().bytes());
        ledger.charge_preflight(u64::try_from(query.typeql.len()).unwrap_or(u64::MAX), 1)?;
        prepared.check_control()?;
        match target {
            WriteTarget::Database(database) => {
                let transaction = await_crud_controlled(
                    database.transaction_context(TxType::Write),
                    prepared.deadline(),
                    prepared.cancellation(),
                )
                .await
                .map_err(|error| {
                    controlled_crud_orm(
                        error,
                        SdkProviderOperation::OpenWriteTransaction,
                        batch.model(),
                    )
                })?;
                let result = self
                    .run_controlled_delete(&transaction, &prepared, &plan, query, &mut ledger)
                    .await;
                match result.and_then(|()| prepared.check_control()) {
                    Ok(()) => transaction.commit_sdk().await.map_err(|error| {
                        append_path(
                            error,
                            [SdkDiagnosticPathSegment::Type(batch.model().clone())],
                        )
                    }),
                    Err(primary) => {
                        let _ = transaction.rollback().await;
                        Err(primary)
                    }
                }
            }
            WriteTarget::Transaction(transaction) => {
                self.run_controlled_delete(transaction, &prepared, &plan, query, &mut ledger)
                    .await
            }
        }
    }

    async fn run_controlled_delete(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedProjectedBatchInvocation<'_>,
        plan: &BuiltPlan,
        query: crate::session::staged::StagedQuery,
        ledger: &mut ExecutionLedger,
    ) -> Result<(), SdkExecutionDiagnostic> {
        let type_id = prepared.batch().model();
        let mut lease = await_crud_controlled(
            transaction.acquire_mutation_lease(),
            prepared.deadline(),
            prepared.cancellation(),
        )
        .await
        .map_err(|error| controlled_crud_orm(error, SdkProviderOperation::Write, type_id))?;
        lease
            .check_plan_provider(plan.local())
            .map_err(|error| orm_at_type(error, SdkProviderOperation::Write, type_id))?;
        let result = async {
            prepared.check_control()?;
            let mut policy = prepared.limits();
            policy.items = ledger.remaining_items();
            policy.bytes = ledger.remaining_bytes();
            let limits = crate::query_plan_exec::plan_answer_limits(
                policy,
                prepared.cancellation(),
                prepared.deadline(),
            )
            .map_err(|error| orm_at_type(error, SdkProviderOperation::Write, type_id))?;
            let (answer, stats) = await_crud_controlled(
                lease.dispatch_plan_counted(query, limits, Some(plan.local())),
                prepared.deadline(),
                prepared.cancellation(),
            )
            .await
            .map_err(|error| controlled_crud_orm(error, SdkProviderOperation::Write, type_id))?;
            ledger.record_reply(stats)?;
            let answer = TransactionContext::resolve_plan(plan.local(), answer)
                .map_err(|error| orm_at_type(error, SdkProviderOperation::Write, type_id))?;
            if !matches!(answer, PlanAnswer::Matched(0 | 1)) {
                return Err(single_batch_result_invalid(type_id));
            }
            prepared.check_control()
        }
        .await;
        match result {
            Ok(()) => {
                lease.complete_success();
                Ok(())
            }
            Err(error) => {
                lease.record_failure(&error);
                Err(error)
            }
        }
    }
}

fn single_write_replacement(
    batch: &ProjectedBatch,
) -> Result<&ProjectedCreate, SdkExecutionDiagnostic> {
    match batch.row_at(0) {
        Some((_, ProjectedBatchRow::Update { replacement, .. })) => Ok(replacement),
        Some((_, ProjectedBatchRow::Create(create)))
            if matches!(
                batch.operation(),
                ProjectedBatchOperation::Insert | ProjectedBatchOperation::Put,
            ) =>
        {
            Ok(create)
        }
        _ => Err(single_batch_result_invalid(batch.model())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
    use type_bridge_contract::schema::DocumentId;
    use type_bridge_contract::value::CanonicalString;
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

    fn wide_single_projection() -> InstalledRuntimeProjection {
        wide_single_projection_with_ordered(false)
    }

    fn wide_single_projection_with_ordered(ordered: bool) -> InstalledRuntimeProjection {
        wide_single_projection_for_target(ordered, BindingTarget::Python)
    }

    fn wide_single_projection_for_target(
        ordered: bool,
        target: BindingTarget,
    ) -> InstalledRuntimeProjection {
        let (config, handler) = match target {
            BindingTarget::Python => (ProjectionConfig::python(), ProjectionHandler::python_v2()),
            BindingTarget::TypeScript => (
                ProjectionConfig::typescript(),
                ProjectionHandler::typescript_v2(),
            ),
            BindingTarget::Rust => (ProjectionConfig::rust(), ProjectionHandler::rust_v2()),
            _ => unreachable!(),
        };
        let source = "format: typebridge.schema/v2\nattributes:\n  item-key: { value: string }\n  item-tag: { value: string }\nentities:\n  wide-item:\n    owns:\n      item-key: { key: true }\n      item-tag: { card: { min: 0 } }\nrelations:\n  wide-edge:\n    owns:\n      item-key: { key: true }\n      item-tag: { card: { min: 0 } }\n    relates:\n      member: { card: { min: 0, max: 4 } }\nplays:\n  wide-item:\n    wide-edge: [member]\n  wide-edge:\n    wide-edge: [member]\n";
        let source = if ordered {
            source.replace(
                "item-tag: { card: { min: 0 } }",
                "item-tag: { card: { min: 0 }, ordered: true }",
            )
        } else {
            source.to_owned()
        };
        let declared = normalize_documents(
            &SchemaDocumentSet::parse([(
                DocumentId::new("single-wide.yaml").unwrap(),
                source.as_str(),
            )])
            .unwrap(),
        )
        .unwrap();
        let resolved = resolve(
            &declared,
            &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
        )
        .unwrap();
        InstalledRuntimeProjection::try_new(
            project(&resolved, target, &config, &[handler], &[]).unwrap(),
        )
        .unwrap()
    }

    struct SingleProxyTransport {
        schema: type_bridge_contract::schema::DeclaredSchema,
        advertisement: type_bridge_contract::query_remote::RemoteCapabilities,
        signer: crate::remote_signing::RemoteReplySigningKey,
        model: TypeId,
        roots: usize,
        tags: usize,
        input_indexes: std::sync::Mutex<Option<Vec<usize>>>,
        mutation_columns: std::sync::atomic::AtomicUsize,
        missing_guard_ordinal: std::sync::atomic::AtomicUsize,
        dispatched_writes: std::sync::atomic::AtomicUsize,
        seen: std::sync::Mutex<Vec<type_bridge_contract::query_remote_v3::StagedRequestV3>>,
    }
    impl crate::session::staged_proxy::StagedTransport for SingleProxyTransport {
        fn staged_capabilities(
            &self,
        ) -> crate::session::backend::BoxFuture<'_, Result<Vec<u8>, crate::QueryTargetError>>
        {
            panic!("the fixture pins its staged advertisement")
        }
        fn exchange_staged<'a>(
            &'a self,
            bytes: &'a [u8],
        ) -> crate::session::backend::BoxFuture<'a, Result<Vec<u8>, crate::QueryTargetError>>
        {
            use type_bridge_contract::query_remote::RemoteValue;
            use type_bridge_contract::query_remote_v3::{
                StagedAnswerV3, StagedOutcomeV3, StagedReplyV3, StagedRequestV3,
            };
            Box::pin(async move {
                let request = StagedRequestV3::decode_with_limits(
                    bytes,
                    &self.schema,
                    &self.advertisement,
                    self.advertisement.query_limits(),
                )
                .unwrap();
                let self_player = request.plans().iter().any(|entry| {
                    !entry.plan().writes()
                        && match entry.plan().output() {
                            type_bridge_contract::query_plan::QueryOutput::Rows { columns }
                                if columns.len() >= 2 =>
                            {
                                entry
                                    .plan()
                                    .binding_types(columns[1])
                                    .is_some_and(|types| types.contains(&self.model))
                            }
                            _ => false,
                        }
                });
                let input_indexes = self.input_indexes.lock().unwrap().clone();
                let mut answers = Vec::new();
                for entry in request.plans() {
                    let plan = match entry.bind_results(&answers, &self.schema) {
                        Ok(plan) => plan,
                        Err(error) => {
                            let reply = StagedReplyV3::new(
                                &request,
                                StagedOutcomeV3::Failed {
                                    diagnostic:
                                        crate::execution_diagnostic::lower_remote_query_diagnostic(
                                            error,
                                        ),
                                },
                                vec![],
                            )
                            .unwrap()
                            .encode_signed(&request, &self.advertisement, &self.signer)
                            .unwrap();
                            self.seen.lock().unwrap().push(request);
                            return Ok(reply);
                        }
                    };
                    if plan.writes() {
                        self.dispatched_writes
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    if let type_bridge_contract::query_plan::QueryOutput::Rows { columns } =
                        plan.output()
                        && columns.first().is_some_and(|column| {
                            plan.bindings()[usize::from(column.get())]
                                .variable()
                                .as_str()
                                .starts_with("reference_ordinal")
                        })
                    {
                        let mut cells = Vec::new();
                        for pair in columns.chunks_exact(2) {
                            let ordinal = plan.pipeline().iter().find_map(|stage| {
                                let type_bridge_contract::query_plan::v3::Stage::Match { patterns } = stage else { return None; };
                                patterns.iter().find_map(|pattern| match pattern {
                                    type_bridge_contract::query_plan::v3::Pattern::Let { binding, expr: type_bridge_contract::query_plan::v3::Expr::Operand(type_bridge_contract::query_plan::QueryOperand::Literal { value: CanonicalValue::Long(ordinal) }) } if *binding == pair[0] => Some(*ordinal),
                                    _ => None,
                                })
                            }).unwrap();
                            cells.push(RemoteValue::Value {
                                value: CanonicalValue::Long(ordinal),
                            });
                            cells.push(
                                if usize::try_from(ordinal).unwrap()
                                    == self
                                        .missing_guard_ordinal
                                        .load(std::sync::atomic::Ordering::Relaxed)
                                {
                                    RemoteValue::Absent
                                } else {
                                    RemoteValue::Thing {
                                        iid: if self_player {
                                            "0x20".into()
                                        } else {
                                            format!("0x{:02x}", 0x10 + ordinal)
                                        },
                                        type_id: plan
                                            .binding_types(pair[1])
                                            .unwrap()
                                            .iter()
                                            .next()
                                            .unwrap()
                                            .clone(),
                                    }
                                },
                            );
                        }
                        answers.push(StagedAnswerV3::Rows { rows: vec![cells] });
                        continue;
                    }
                    let answer = if let Some(fetch) =
                        plan.pipeline().iter().find_map(|stage| match stage {
                            type_bridge_contract::query_plan::v3::Stage::Fetch(fetch) => {
                                Some(fetch)
                            }
                            _ => None,
                        }) {
                        let mut documents = Vec::new();
                        for (position, iid) in
                            ["0x20", "0x22", "0x24"].iter().take(self.roots).enumerate()
                        {
                            let input_index = input_indexes
                                .as_ref()
                                .map_or(0, |indexes| indexes[position]);
                            let mut document = serde_json::json!({
                                "_iid": iid, "_type": self.model.label().as_str(),
                                "attributes": {"item-key":"key", "item-tag": (0..self.tags).map(|index| format!("tag-{index}")).collect::<Vec<_>>()}
                            });
                            for scalar in &fetch.scalars {
                                document[scalar.key.as_str()] = serde_json::json!(input_index);
                            }
                            for (index, role) in fetch.roles.iter().enumerate() {
                                let player = if self_player {
                                    serde_json::json!({"_iid":"0x20", "_type":self.model.label().as_str(), "attributes":{"item-key":"key"}})
                                } else {
                                    serde_json::json!({"_iid":"0x10", "_type":"wide-item", "attributes":{"item-key":"player"}})
                                };
                                if role.collect {
                                    document[format!("_role_{index}_players")] =
                                        serde_json::json!([player]);
                                } else {
                                    document[format!("_role_{index}_iid")] = player["_iid"].clone();
                                    document[format!("_role_{index}_type")] =
                                        player["_type"].clone();
                                    document[format!("_role_{index}_attributes")] =
                                        player["attributes"].clone();
                                }
                            }
                            documents.push(
                                crate::query_plan_exec::remote_document(&document, &plan).unwrap(),
                            );
                        }
                        StagedAnswerV3::Documents { documents }
                    } else if matches!(
                        plan.operation(),
                        type_bridge_contract::query_plan::v3::Operation::Write {
                            returns: type_bridge_contract::query_plan::v3::WriteReturns::Matched,
                        }
                    ) {
                        StagedAnswerV3::Matched {
                            value: self.roots as u64,
                        }
                    } else {
                        let type_bridge_contract::query_plan::QueryOutput::Rows { columns } =
                            plan.output()
                        else {
                            panic!("identity output")
                        };
                        assert_eq!(
                            columns.len(),
                            self.mutation_columns
                                .load(std::sync::atomic::Ordering::Relaxed)
                        );
                        StagedAnswerV3::Rows {
                            rows: ["0x20", "0x22", "0x24"]
                                .iter()
                                .take(self.roots)
                                .enumerate()
                                .map(|(position, iid)| {
                                    let index = input_indexes
                                        .as_ref()
                                        .map_or(0, |indexes| indexes[position]);
                                    let mut row = vec![RemoteValue::Value {
                                        value: CanonicalValue::Long(index as i64),
                                    }];
                                    if columns.len() == 2 {
                                        row.push(RemoteValue::Thing {
                                            iid: (*iid).into(),
                                            type_id: self.model.clone(),
                                        });
                                    }
                                    row
                                })
                                .collect(),
                        }
                    };
                    answers.push(answer);
                }
                let reply = StagedReplyV3::new(&request, StagedOutcomeV3::Committed, answers)
                    .unwrap()
                    .encode_signed(&request, &self.advertisement, &self.signer)
                    .unwrap();
                self.seen.lock().unwrap().push(request);
                Ok(reply)
            })
        }
    }
    fn single_proxy_fixture(
        model: TypeId,
        roots: usize,
        tags: usize,
    ) -> (
        InstalledRuntimeProjection,
        crate::ProxyQueryConnection,
        Arc<SingleProxyTransport>,
    ) {
        single_proxy_fixture_with_limits(
            model,
            roots,
            tags,
            type_bridge_contract::limits::QueryLimits::V3,
        )
    }

    fn single_proxy_fixture_with_limits(
        model: TypeId,
        roots: usize,
        tags: usize,
        server_limits: type_bridge_contract::limits::QueryLimits,
    ) -> (
        InstalledRuntimeProjection,
        crate::ProxyQueryConnection,
        Arc<SingleProxyTransport>,
    ) {
        single_proxy_fixture_with_projection(
            model,
            roots,
            tags,
            server_limits,
            wide_single_projection(),
        )
    }

    fn single_proxy_fixture_with_projection(
        model: TypeId,
        roots: usize,
        tags: usize,
        server_limits: type_bridge_contract::limits::QueryLimits,
        installed: InstalledRuntimeProjection,
    ) -> (
        InstalledRuntimeProjection,
        crate::ProxyQueryConnection,
        Arc<SingleProxyTransport>,
    ) {
        use type_bridge_contract::query_remote::{RemoteCapabilities, RemoteExecutorBinding};
        let schema = installed.plan_authority().unwrap().declared().clone();
        let profile = SemanticProfileId::new("typedb-3.13.6/v1").unwrap();
        let context = type_bridge_schema::ManagedDeltaContext::new(
            type_bridge_contract::managed_scope::ManagedScopeId::new("single-proxy").unwrap(),
            profile.clone(),
            type_bridge_schema::schema_authority_capability_vocabulary(),
        );
        let verified = type_bridge_schema::build_schema_authority(
            &schema,
            schema.required_capabilities(),
            &context,
        )
        .unwrap();
        let installed = installed.with_schema_authority(&verified);
        let authority = crate::plan_authority::QueryAuthority::from_declared_bytes(
            &type_bridge_contract::schema::encode_declared_schema(&schema).unwrap(),
            "single-proxy",
            profile.as_str(),
        )
        .unwrap();
        let signer = crate::remote_signing::RemoteReplySigningKey::from_secret_bytes([0x52; 32]);
        let mut caps = type_bridge_contract::query_plan::v3::query_plan_v3_capability_vocabulary();
        for name in [
            type_bridge_contract::query_remote_v3::CAP_QUERY_REMOTE_STAGED_V3,
            type_bridge_contract::query_remote_v3::CAP_QUERY_REMOTE_WRITE_V3,
            type_bridge_contract::query_remote_v3::CAP_QUERY_REMOTE_ANSWER_DIMENSIONS,
            type_bridge_contract::query_remote_v3::CAP_QUERY_STAGE_RESULT_REF,
        ] {
            caps.insert(type_bridge_contract::capability::CapabilityId::new(name).unwrap());
        }
        let advertisement = RemoteCapabilities::new(
            caps,
            RemoteExecutorBinding::new("single-proxy-executor", "epoch-single-proxy").unwrap(),
            signer.public_key(),
        )
        .with_query_limits(server_limits)
        .unwrap();
        let proxy = crate::ProxyQueryConnection::with_exchange(
            advertisement.encode().unwrap(),
            Arc::new(authority),
            Default::default(),
        )
        .unwrap();
        let transport = Arc::new(SingleProxyTransport {
            schema,
            advertisement,
            signer,
            model,
            roots,
            tags,
            input_indexes: std::sync::Mutex::new(None),
            mutation_columns: std::sync::atomic::AtomicUsize::new(2),
            missing_guard_ordinal: std::sync::atomic::AtomicUsize::new(usize::MAX),
            dispatched_writes: std::sync::atomic::AtomicUsize::new(0),
            seen: std::sync::Mutex::new(vec![]),
        });
        (
            installed,
            proxy.with_staged_transport(transport.clone()),
            transport,
        )
    }

    fn proxy_single_input(
        installed: &InstalledRuntimeProjection,
        model: &TypeId,
        tags: usize,
    ) -> ProjectedCreate {
        proxy_batch_input(installed, model, tags, "key")
    }

    fn proxy_batch_input(
        installed: &InstalledRuntimeProjection,
        model: &TypeId,
        tags: usize,
        key: &str,
    ) -> ProjectedCreate {
        let kind = model.kind();
        let scalar = |attribute: &str, value: String| {
            ProjectedAttributeValue::try_new(
                installed,
                TypeId::new(TypeKind::Attribute, attribute).unwrap(),
                CanonicalValue::String(CanonicalString::new(value).unwrap()),
            )
            .unwrap()
        };
        ProjectedCreate::try_new(
            installed,
            model.clone(),
            vec![
                (
                    OwnsFactId::new(model.clone(), AttributeId::new("item-key").unwrap()).unwrap(),
                    vec![scalar("item-key", key.to_owned())],
                ),
                (
                    OwnsFactId::new(model.clone(), AttributeId::new("item-tag").unwrap()).unwrap(),
                    (0..tags)
                        .map(|index| scalar("item-tag", format!("tag-{index}")))
                        .collect(),
                ),
            ],
            if kind == TypeKind::Relation {
                vec![(
                    RoleId::new("wide-edge", "member").unwrap(),
                    vec![
                        ProjectedReference::try_new(
                            installed,
                            TypeId::new(TypeKind::Entity, "wide-item").unwrap(),
                            Some("0x10".into()),
                            vec![],
                        )
                        .unwrap(),
                    ],
                )]
            } else {
                vec![]
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn proxy_wide_updates_freeze_self_key_references_in_one_exchange() {
        for missing in [false, true] {
            let model = TypeId::new(TypeKind::Relation, "wide-edge").unwrap();
            let role = RoleId::new("wide-edge", "member").unwrap();
            let key =
                OwnsFactId::new(model.clone(), AttributeId::new("item-key").unwrap()).unwrap();
            let (installed, proxy, transport) = single_proxy_fixture(model.clone(), 1, 300);
            if missing {
                transport
                    .missing_guard_ordinal
                    .store(0, std::sync::atomic::Ordering::Relaxed);
            }
            let base = proxy_single_input(&installed, &model, 300);
            let reference = ProjectedReference::try_new(
                &installed,
                model.clone(),
                None,
                vec![(
                    key.clone(),
                    ProjectedAttributeValue::try_new(
                        &installed,
                        TypeId::new(TypeKind::Attribute, "item-key").unwrap(),
                        CanonicalValue::String(CanonicalString::new("old-key").unwrap()),
                    )
                    .unwrap(),
                )],
            )
            .unwrap();
            let create = ProjectedCreate::try_new(
                &installed,
                model.clone(),
                base.fields()
                    .iter()
                    .map(|(field, values)| (field.clone(), values.clone()))
                    .collect(),
                vec![(role.clone(), vec![reference])],
            )
            .unwrap();
            let control =
                ProjectedBatchInvocationControl::capture(Default::default(), Default::default());
            let batch = ProjectedBatch::try_new_for_invocation(
                &installed,
                model.clone(),
                ProjectedBatchOperation::Update,
                vec![ProjectedBatchRow::Update {
                    iid: "0x20".into(),
                    replacement: create,
                }],
                &control,
            )
            .unwrap();
            let result = ProjectedBatchExecutor::new(&installed)
                .execute_on_target_mapped(
                    crate::QueryTarget::Proxy(&proxy),
                    &batch,
                    control,
                    None,
                    |result| Ok(Some(result)),
                )
                .await;
            if missing {
                let crate::QueryTargetError::Commit(error) = result.unwrap_err() else {
                    panic!("signed refusal");
                };
                let expected = crate::projected_batch_executor::integrity_at_reference(
                    "projected_batch_reference_missing",
                    "A projected role-player reference did not resolve exactly once",
                    &model,
                    &role,
                    0,
                    0,
                    &model,
                );
                assert_eq!(error.diagnostic().code(), expected.code());
                assert_eq!(error.diagnostic().path(), expected.path());
                assert_eq!(
                    error.error.commit_failure_certainty(),
                    Some(crate::CommitFailureCertainty::DefinitelyAborted)
                );
                assert_eq!(
                    transport
                        .dispatched_writes
                        .load(std::sync::atomic::Ordering::Relaxed),
                    0
                );
            } else {
                let Some(ProjectedBatchResult::Things(things)) = result.unwrap() else {
                    panic!("written root");
                };
                assert_eq!(things.count(), 1);
                assert_eq!(things.input_indexes(), [0]);
                assert_eq!(things.items()[0].iid(), "0x20");
                let player = &things.items()[0].roles()[&role][0];
                assert_eq!(player.iid(), "0x20");
                assert_eq!(
                    player.keys()[&key].value(),
                    &CanonicalValue::String(CanonicalString::new("key").unwrap())
                );
            }
            let seen = transport.seen.lock().unwrap();
            assert_eq!(seen.len(), 1);
            assert_eq!(seen[0].plans().len(), 3);
            assert!(!seen[0].plans()[0].plan().writes());
            assert!(
                seen[0].plans()[1..]
                    .iter()
                    .flat_map(|entry| entry.result_refs())
                    .all(|reference| reference.source_plan == 0)
            );
        }
    }

    #[tokio::test]
    async fn proxy_update_snapshot_frames_keep_one_exchange_and_original_missing_coordinates() {
        for missing in [false, true] {
            let model = TypeId::new(TypeKind::Relation, "wide-edge").unwrap();
            let role = RoleId::new("wide-edge", "member").unwrap();
            let key =
                OwnsFactId::new(model.clone(), AttributeId::new("item-key").unwrap()).unwrap();
            let mut limits = type_bridge_contract::limits::QueryLimits::V3;
            limits.structural.selected_slots = 4;
            let (installed, proxy, transport) =
                single_proxy_fixture_with_limits(model.clone(), 1, 300, limits);
            if missing {
                transport
                    .missing_guard_ordinal
                    .store(3, std::sync::atomic::Ordering::Relaxed);
            }
            let base = proxy_single_input(&installed, &model, 300);
            let reference = ProjectedReference::try_new(
                &installed,
                model.clone(),
                None,
                vec![(
                    key.clone(),
                    ProjectedAttributeValue::try_new(
                        &installed,
                        TypeId::new(TypeKind::Attribute, "item-key").unwrap(),
                        CanonicalValue::String(CanonicalString::new("old-key").unwrap()),
                    )
                    .unwrap(),
                )],
            )
            .unwrap();
            let create = ProjectedCreate::try_new(
                &installed,
                model.clone(),
                base.fields()
                    .iter()
                    .map(|(field, values)| (field.clone(), values.clone()))
                    .collect(),
                vec![(role.clone(), vec![reference; 4])],
            )
            .unwrap();
            let control =
                ProjectedBatchInvocationControl::capture(Default::default(), Default::default());
            let executor = ProjectedCrudExecutor::new(&installed);
            let batch = executor
                .single_controlled_write_batch(
                    &model,
                    ProjectedBatchOperation::Update,
                    ProjectedBatchRow::Update {
                        iid: "0x20".into(),
                        replacement: create,
                    },
                    &control,
                )
                .unwrap();
            let result = executor
                .execute_single_write_on_target(
                    crate::QueryTarget::Proxy(&proxy),
                    &batch,
                    control,
                    Ok,
                )
                .await;
            if missing {
                let crate::QueryTargetError::Commit(error) = result.unwrap_err() else {
                    panic!("signed refusal");
                };
                let expected = crate::projected_batch_executor::integrity_at_reference(
                    "projected_batch_reference_missing",
                    "A projected role-player reference did not resolve exactly once",
                    &model,
                    &role,
                    0,
                    3,
                    &model,
                );
                assert_eq!(error.diagnostic().code(), expected.code());
                assert_eq!(error.diagnostic().path(), expected.path());
                assert_eq!(
                    error.error.commit_failure_certainty(),
                    Some(crate::CommitFailureCertainty::DefinitelyAborted)
                );
                assert_eq!(
                    transport
                        .dispatched_writes
                        .load(std::sync::atomic::Ordering::Relaxed),
                    0
                );
            } else {
                let things = result.unwrap();
                assert_eq!(things.count(), 1);
                assert_eq!(things.items()[0].iid(), "0x20");
                let player = &things.items()[0].roles()[&role][0];
                assert_eq!(player.iid(), "0x20");
                assert_eq!(
                    player.keys()[&key].value(),
                    &CanonicalValue::String(CanonicalString::new("key").unwrap())
                );
            }
            let seen = transport.seen.lock().unwrap();
            assert_eq!(seen.len(), 1);
            let plans = seen[0].plans();
            let lookups = plans
                .iter()
                .take_while(|entry| !entry.plan().writes())
                .count();
            assert_eq!(lookups, 3);
            assert!(plans[lookups..].iter().all(|entry| {
                entry
                    .result_refs()
                    .iter()
                    .map(|reference| reference.source_plan)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    <= 1
            }));
        }
    }

    #[tokio::test]
    async fn proxy_insert_and_keyed_put_prove_invocation_start_references_before_writes() {
        for operation in [
            ProjectedBatchOperation::Insert,
            ProjectedBatchOperation::Put,
        ] {
            let model = TypeId::new(TypeKind::Relation, "wide-edge").unwrap();
            let role = RoleId::new("wide-edge", "member").unwrap();
            let key =
                OwnsFactId::new(model.clone(), AttributeId::new("item-key").unwrap()).unwrap();
            let (installed, proxy, transport) = single_proxy_fixture(model.clone(), 1, 300);
            transport
                .missing_guard_ordinal
                .store(1, std::sync::atomic::Ordering::Relaxed);
            let base = proxy_single_input(&installed, &model, 300);
            let reference = ProjectedReference::try_new(
                &installed,
                model.clone(),
                None,
                vec![(
                    key,
                    ProjectedAttributeValue::try_new(
                        &installed,
                        TypeId::new(TypeKind::Attribute, "item-key").unwrap(),
                        CanonicalValue::String(CanonicalString::new("prior-key").unwrap()),
                    )
                    .unwrap(),
                )],
            )
            .unwrap();
            let create = ProjectedCreate::try_new(
                &installed,
                model.clone(),
                base.fields()
                    .iter()
                    .map(|(field, values)| (field.clone(), values.clone()))
                    .collect(),
                vec![(role.clone(), vec![reference])],
            )
            .unwrap();
            let second_base = proxy_batch_input(&installed, &model, 300, "second-key");
            let second = ProjectedCreate::try_new(
                &installed,
                model.clone(),
                second_base
                    .fields()
                    .iter()
                    .map(|(field, values)| (field.clone(), values.clone()))
                    .collect(),
                create
                    .roles()
                    .iter()
                    .map(|(role, players)| (role.clone(), players.clone()))
                    .collect(),
            )
            .unwrap();
            let batch = ProjectedBatch::try_new(
                &installed,
                model.clone(),
                operation,
                vec![
                    ProjectedBatchRow::Create(create),
                    ProjectedBatchRow::Create(second),
                ],
            )
            .unwrap();
            let result = ProjectedBatchExecutor::new(&installed)
                .execute_on_target_mapped(
                    crate::QueryTarget::Proxy(&proxy),
                    &batch,
                    ProjectedBatchInvocationControl::capture(
                        Default::default(),
                        Default::default(),
                    ),
                    None,
                    |result| Ok(Some(result)),
                )
                .await;
            let crate::QueryTargetError::Commit(error) = result.unwrap_err() else {
                panic!("signed refusal")
            };
            let expected = crate::projected_batch_executor::integrity_at_reference(
                "projected_batch_reference_missing",
                "A projected role-player reference did not resolve exactly once",
                &model,
                &role,
                1,
                1,
                &model,
            );
            assert_eq!(error.diagnostic().code(), expected.code());
            assert_eq!(error.diagnostic().path(), expected.path());
            assert_eq!(
                error.error.commit_failure_certainty(),
                Some(crate::CommitFailureCertainty::DefinitelyAborted)
            );
            assert_eq!(
                transport
                    .dispatched_writes
                    .load(std::sync::atomic::Ordering::Relaxed),
                0
            );
            let seen = transport.seen.lock().unwrap();
            assert_eq!(seen.len(), 1);
            assert_eq!(seen[0].plans().len(), 5);
            assert!(!seen[0].plans()[0].plan().writes());
            assert_eq!(seen[0].plans()[2].result_refs()[0].source_plan, 1);
        }
    }

    #[tokio::test]
    async fn proxy_guard_after_an_earlier_row_reports_the_batch_reference_ordinal() {
        let model = TypeId::new(TypeKind::Relation, "wide-edge").unwrap();
        let player = TypeId::new(TypeKind::Entity, "wide-item").unwrap();
        let role = RoleId::new("wide-edge", "member").unwrap();
        let (installed, proxy, transport) = single_proxy_fixture(model.clone(), 1, 300);
        transport
            .missing_guard_ordinal
            .store(1, std::sync::atomic::Ordering::Relaxed);
        let first = proxy_batch_input(&installed, &model, 1, "first");
        let base = proxy_batch_input(&installed, &model, 300, "second");
        let mut players = base.roles()[&role].clone();
        players.push(
            ProjectedReference::try_new(&installed, player.clone(), Some("0x11".into()), vec![])
                .unwrap(),
        );
        let second = ProjectedCreate::try_new(
            &installed,
            model.clone(),
            base.fields()
                .iter()
                .map(|(field, values)| (field.clone(), values.clone()))
                .collect(),
            vec![(role.clone(), players)],
        )
        .unwrap();
        let batch = ProjectedBatch::try_new(
            &installed,
            model.clone(),
            ProjectedBatchOperation::Insert,
            vec![
                ProjectedBatchRow::Create(first),
                ProjectedBatchRow::Create(second),
            ],
        )
        .unwrap();
        let result = ProjectedBatchExecutor::new(&installed)
            .execute_on_target_mapped(
                crate::QueryTarget::Proxy(&proxy),
                &batch,
                ProjectedBatchInvocationControl::capture(Default::default(), Default::default()),
                None,
                |result| Ok(Some(result)),
            )
            .await;
        let crate::QueryTargetError::Commit(error) = result.unwrap_err() else {
            panic!("signed refusal")
        };
        let expected = crate::projected_batch_executor::integrity_at_reference(
            "projected_batch_reference_missing",
            "A projected role-player reference did not resolve exactly once",
            &model,
            &role,
            1,
            2,
            &player,
        );
        assert_eq!(error.diagnostic().code(), expected.code());
        assert_eq!(error.diagnostic().path(), expected.path());
        assert_eq!(
            error.error.commit_failure_certainty(),
            Some(crate::CommitFailureCertainty::DefinitelyAborted)
        );
        assert_eq!(
            transport
                .dispatched_writes
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        let seen = transport.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].plans().len(), 4);
        assert!(seen[0].plans()[0].plan().writes());
        assert!(!seen[0].plans()[1].plan().writes());
    }

    #[tokio::test]
    async fn proxy_wide_guard_maps_signed_missing_cell_before_any_write() {
        let model = TypeId::new(TypeKind::Relation, "wide-edge").unwrap();
        let player = TypeId::new(TypeKind::Entity, "wide-item").unwrap();
        let role = RoleId::new("wide-edge", "member").unwrap();
        let (installed, proxy, transport) = single_proxy_fixture(model.clone(), 1, 300);
        transport
            .missing_guard_ordinal
            .store(2, std::sync::atomic::Ordering::Relaxed);
        let base = proxy_single_input(&installed, &model, 300);
        let create = ProjectedCreate::try_new(
            &installed,
            model.clone(),
            base.fields()
                .iter()
                .map(|(field, values)| (field.clone(), values.clone()))
                .collect(),
            vec![(
                role.clone(),
                (0..3)
                    .map(|index| {
                        ProjectedReference::try_new(
                            &installed,
                            player.clone(),
                            Some(format!("0x{:02x}", 0x10 + index)),
                            vec![],
                        )
                        .unwrap()
                    })
                    .collect(),
            )],
        )
        .unwrap();
        let control =
            ProjectedBatchInvocationControl::capture(Default::default(), Default::default());
        let batch = ProjectedBatch::try_new_for_invocation(
            &installed,
            model.clone(),
            ProjectedBatchOperation::Insert,
            vec![ProjectedBatchRow::Create(create)],
            &control,
        )
        .unwrap();
        let error = ProjectedBatchExecutor::new(&installed)
            .execute_on_target_mapped(
                crate::QueryTarget::Proxy(&proxy),
                &batch,
                control,
                (),
                |_| panic!("a refused graph cannot publish roots"),
            )
            .await
            .unwrap_err();
        let crate::QueryTargetError::Commit(error) = error else {
            panic!("classified signed refusal");
        };
        let expected = crate::projected_batch_executor::integrity_at_reference(
            "projected_batch_reference_missing",
            "A projected role-player reference did not resolve exactly once",
            &model,
            &role,
            0,
            2,
            &player,
        );
        let actual = error.diagnostic();
        assert_eq!(actual.code(), expected.code());
        assert_eq!(actual.category(), expected.category());
        assert_eq!(actual.path(), expected.path());
        assert_eq!(
            error.error.commit_failure_certainty(),
            Some(crate::CommitFailureCertainty::DefinitelyAborted)
        );
        assert!(std::error::Error::source(error.as_ref()).is_some());
        assert_eq!(
            transport
                .dispatched_writes
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        let seen = transport.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].plans().len() > 1);
        assert_eq!(seen[0].plans()[1].result_refs().len(), 2);
        use type_bridge_contract::sdk_diagnostic::{SdkDiagnosticDetailValue, SdkDiagnosticName};
        assert_eq!(
            actual
                .details()
                .get(&SdkDiagnosticName::new("source_column").unwrap()),
            Some(&SdkDiagnosticDetailValue::Signed(3))
        );
    }

    #[tokio::test]
    async fn batch_proxy_writes_keep_sparse_plural_indexes_and_one_exchange() {
        for kind in [TypeKind::Entity, TypeKind::Relation] {
            for operation in [
                ProjectedBatchOperation::Insert,
                ProjectedBatchOperation::Put,
                ProjectedBatchOperation::Update,
            ] {
                let model = TypeId::new(
                    kind,
                    if kind == TypeKind::Entity {
                        "wide-item"
                    } else {
                        "wide-edge"
                    },
                )
                .unwrap();
                let update = operation == ProjectedBatchOperation::Update;
                let indexes = if update { vec![0, 2] } else { vec![1, 1, 2] };
                let (installed, proxy, transport) =
                    single_proxy_fixture(model.clone(), indexes.len(), 1);
                *transport.input_indexes.lock().unwrap() = Some(indexes.clone());
                let control = ProjectedBatchInvocationControl::capture(
                    Default::default(),
                    Default::default(),
                );
                let rows = (0..3)
                    .map(|index| {
                        let input =
                            proxy_batch_input(&installed, &model, 1, &format!("key-{index}"));
                        if update {
                            ProjectedBatchRow::Update {
                                iid: ["0x20", "0x21", "0x22"][index].into(),
                                replacement: input,
                            }
                        } else {
                            ProjectedBatchRow::Create(input)
                        }
                    })
                    .collect();
                let batch = ProjectedBatch::try_new_for_invocation(
                    &installed, model, operation, rows, &control,
                )
                .unwrap();
                let mapped = std::sync::atomic::AtomicUsize::new(0);
                let direct = crate::projected_batch_executor::inline_batch_plan_bytes_for_test(
                    &installed,
                    &batch,
                    proxy.execution_identity(),
                    false,
                )
                .unwrap();
                let result = ProjectedBatchExecutor::new(&installed)
                    .execute_on_target_mapped(
                        crate::QueryTarget::Proxy(&proxy),
                        &batch,
                        control,
                        None,
                        |result| {
                            mapped.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            Ok(Some(result))
                        },
                    )
                    .await
                    .unwrap();
                let Some(ProjectedBatchResult::Things(things)) = result else {
                    panic!("expected mapped batch");
                };
                assert_eq!(things.count(), indexes.len());
                assert_eq!(things.input_indexes(), indexes);
                assert_eq!(mapped.load(std::sync::atomic::Ordering::Relaxed), 1);
                let seen = transport.seen.lock().unwrap();
                assert_eq!(seen.len(), 1);
                assert_eq!(seen[0].plans().len(), 1);
                assert_eq!(
                    seen[0].plans()[0]
                        .bound_plan(&transport.schema)
                        .unwrap()
                        .canonical_bytes()
                        .unwrap(),
                    direct
                );
            }
        }
    }

    #[tokio::test]
    async fn batch_proxy_empty_inputs_are_validated_without_exchange_or_mapper() {
        let model = TypeId::new(TypeKind::Entity, "wide-item").unwrap();
        let (installed, proxy, transport) = single_proxy_fixture(model.clone(), 0, 0);
        for operation in [
            ProjectedBatchOperation::Insert,
            ProjectedBatchOperation::Put,
            ProjectedBatchOperation::Update,
            ProjectedBatchOperation::Delete,
        ] {
            let control =
                ProjectedBatchInvocationControl::capture(Default::default(), Default::default());
            let batch = ProjectedBatch::try_new_for_invocation(
                &installed,
                model.clone(),
                operation,
                vec![],
                &control,
            )
            .unwrap();
            let value = ProjectedBatchExecutor::new(&installed)
                .execute_on_target_mapped(
                    crate::QueryTarget::Proxy(&proxy),
                    &batch,
                    control,
                    7,
                    |_| panic!("empty mapper invoked"),
                )
                .await
                .unwrap();
            assert_eq!(value, 7);
            assert!(transport.seen.lock().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn field_filter_update_hydrates_every_match_in_one_staged_write() {
        use crate::{
            ProjectedManagerComparison, ProjectedManagerFilter, ProjectedManagerFilterExecutor,
            ProjectedManagerFilterInvocationControl,
        };
        use type_bridge_contract::projection::ProjectedTokenIdentity;
        for (kind, label) in [
            (TypeKind::Entity, "wide-item"),
            (TypeKind::Relation, "wide-edge"),
        ] {
            for roots in [0, 1, 3] {
                let model = TypeId::new(kind, label).unwrap();
                let (installed, proxy, transport) = single_proxy_fixture(model.clone(), roots, 2);
                let replacement = proxy_single_input(&installed, &model, 2);
                let field =
                    OwnsFactId::new(model.clone(), AttributeId::new("item-tag").unwrap()).unwrap();
                let token = ProjectedTokenIdentity::Field {
                    owner: model.clone(),
                    field,
                };
                let value = ProjectedAttributeValue::try_new(
                    &installed,
                    TypeId::new(TypeKind::Attribute, "item-tag").unwrap(),
                    CanonicalValue::String(CanonicalString::new("before").unwrap()),
                )
                .unwrap();
                let filter = ProjectedManagerFilter::try_new(&installed, model)
                    .unwrap()
                    .try_and(&installed, &token, ProjectedManagerComparison::Eq, &value)
                    .unwrap();
                let result = ProjectedManagerFilterExecutor::new(&installed)
                    .update_on_target(
                        crate::QueryTarget::Proxy(&proxy),
                        &filter,
                        &replacement,
                        ProjectedManagerFilterInvocationControl::capture(
                            Default::default(),
                            Default::default(),
                        ),
                    )
                    .await
                    .unwrap();
                assert_eq!(result.count(), roots);
                let seen = transport.seen.lock().unwrap();
                assert_eq!(seen.len(), 1);
                assert_eq!(seen[0].plans().len(), 1);
                assert_eq!(
                    transport
                        .dispatched_writes
                        .load(std::sync::atomic::Ordering::Relaxed),
                    1
                );
            }
        }
    }

    #[tokio::test]
    async fn field_filter_delete_uses_one_staged_write_for_every_match() {
        use crate::{
            ProjectedManagerComparison, ProjectedManagerFilter, ProjectedManagerFilterExecutor,
            ProjectedManagerFilterInvocationControl,
        };
        use type_bridge_contract::projection::ProjectedTokenIdentity;
        for (kind, label) in [
            (TypeKind::Entity, "wide-item"),
            (TypeKind::Relation, "wide-edge"),
        ] {
            let model = TypeId::new(kind, label).unwrap();
            let (installed, proxy, transport) = single_proxy_fixture(model.clone(), 3, 0);
            let field =
                OwnsFactId::new(model.clone(), AttributeId::new("item-tag").unwrap()).unwrap();
            let token = ProjectedTokenIdentity::Field {
                owner: model.clone(),
                field,
            };
            let value = ProjectedAttributeValue::try_new(
                &installed,
                TypeId::new(TypeKind::Attribute, "item-tag").unwrap(),
                CanonicalValue::String(
                    type_bridge_contract::value::CanonicalString::new("tag-0").unwrap(),
                ),
            )
            .unwrap();
            let filter = ProjectedManagerFilter::try_new(&installed, model.clone())
                .unwrap()
                .try_and(&installed, &token, ProjectedManagerComparison::Eq, &value)
                .unwrap();
            ProjectedManagerFilterExecutor::new(&installed)
                .delete_on_target(
                    crate::QueryTarget::Proxy(&proxy),
                    &filter,
                    ProjectedManagerFilterInvocationControl::capture(
                        Default::default(),
                        Default::default(),
                    ),
                )
                .await
                .unwrap();
            let seen = transport.seen.lock().unwrap();
            assert_eq!(seen.len(), 1);
            assert_eq!(seen[0].plans().len(), 1);
            let expressions = [crate::_dynamic::DynamicExpr::Compare {
                attr_name: "item-tag".into(),
                operator: crate::_dynamic::DynamicComparisonOp::Eq,
                value: AttributeValue::String("tag-0".into()),
            }];
            let expected = ManagerPlans::new(installed.manager_registry().unwrap())
                .delete_matching(
                    label,
                    crate::plan_builders::manager::ManagerSelection::Filter(&expressions),
                    true,
                    false,
                )
                .unwrap();
            assert_eq!(
                seen[0].plans()[0].plan().canonical_bytes().unwrap(),
                expected.local().canonical_bytes().unwrap()
            );
            assert_eq!(
                transport
                    .dispatched_writes
                    .load(std::sync::atomic::Ordering::Relaxed),
                1
            );
        }
    }

    #[tokio::test]
    async fn ordered_proxy_single_and_many_deletes_preserve_call_plan_and_one_exchange() {
        for target in [
            BindingTarget::Python,
            BindingTarget::TypeScript,
            BindingTarget::Rust,
        ] {
            for kind in [TypeKind::Entity, TypeKind::Relation] {
                for (single, count) in [(true, 1), (false, 1), (false, 2)] {
                    for roots in [0, 1] {
                        let model = TypeId::new(
                            kind,
                            if kind == TypeKind::Entity {
                                "wide-item"
                            } else {
                                "wide-edge"
                            },
                        )
                        .unwrap();
                        let (installed, proxy, transport) = single_proxy_fixture_with_projection(
                            model.clone(),
                            roots,
                            0,
                            type_bridge_contract::limits::QueryLimits::V3,
                            wide_single_projection_for_target(true, target),
                        );
                        transport
                            .mutation_columns
                            .store(1, std::sync::atomic::Ordering::Relaxed);
                        let control = ProjectedBatchInvocationControl::capture(
                            Default::default(),
                            Default::default(),
                        );
                        let batch = if single {
                            ProjectedBatch::try_single_delete(
                                &installed,
                                model,
                                "0x20".into(),
                                &control,
                            )
                        } else {
                            ProjectedBatch::try_new_for_invocation(
                                &installed,
                                model,
                                ProjectedBatchOperation::Delete,
                                ["0x20", "0x21", "0x22"]
                                    .into_iter()
                                    .take(count)
                                    .map(|iid| ProjectedBatchRow::Delete { iid: iid.into() })
                                    .collect(),
                                &control,
                            )
                        }
                        .unwrap();
                        let direct =
                            crate::projected_batch_executor::inline_batch_plan_bytes_for_test(
                                &installed,
                                &batch,
                                proxy.execution_identity(),
                                false,
                            )
                            .unwrap();
                        let output = ProjectedBatchExecutor::new(&installed)
                            .execute_on_target_mapped(
                                crate::QueryTarget::Proxy(&proxy),
                                &batch,
                                control,
                                None,
                                |result| Ok(Some(result)),
                            )
                            .await
                            .unwrap();
                        assert!(matches!(output, Some(ProjectedBatchResult::Deleted)));
                        let seen = transport.seen.lock().unwrap();
                        assert_eq!(seen.len(), 1);
                        assert_eq!(seen[0].plans().len(), 1);
                        assert_eq!(seen[0].plans()[0].plan().canonical_bytes().unwrap(), direct);
                        assert_eq!(
                            transport
                                .dispatched_writes
                                .load(std::sync::atomic::Ordering::Relaxed),
                            1
                        );
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn batch_proxy_deletes_and_snapshots_share_one_plan_and_original_iid_indexes() {
        for kind in [TypeKind::Entity, TypeKind::Relation] {
            for returning in [false, true] {
                for roots in [0, 2] {
                    let model = TypeId::new(
                        kind,
                        if kind == TypeKind::Entity {
                            "wide-item"
                        } else {
                            "wide-edge"
                        },
                    )
                    .unwrap();
                    let (installed, proxy, transport) =
                        single_proxy_fixture(model.clone(), roots, 1);
                    *transport.input_indexes.lock().unwrap() = Some(vec![0, 2]);
                    transport
                        .mutation_columns
                        .store(1, std::sync::atomic::Ordering::Relaxed);
                    let control = ProjectedBatchInvocationControl::capture(
                        Default::default(),
                        Default::default(),
                    );
                    let batch = ProjectedBatch::try_new_for_invocation(
                        &installed,
                        model,
                        ProjectedBatchOperation::Delete,
                        ["0x20", "0x21", "0x22"]
                            .into_iter()
                            .map(|iid| ProjectedBatchRow::Delete { iid: iid.into() })
                            .collect(),
                        &control,
                    )
                    .unwrap();
                    let executor = ProjectedBatchExecutor::new(&installed);
                    let executor = if returning {
                        executor.returning_deleted()
                    } else {
                        executor
                    };
                    let direct = crate::projected_batch_executor::inline_batch_plan_bytes_for_test(
                        &installed,
                        &batch,
                        proxy.execution_identity(),
                        returning,
                    )
                    .unwrap();
                    let output = executor
                        .execute_on_target_mapped(
                            crate::QueryTarget::Proxy(&proxy),
                            &batch,
                            control,
                            None,
                            |result| Ok(Some(result)),
                        )
                        .await
                        .unwrap();
                    if returning {
                        let Some(ProjectedBatchResult::Things(things)) = output else {
                            panic!("expected snapshots");
                        };
                        assert_eq!(things.count(), roots);
                        assert_eq!(
                            things.input_indexes(),
                            if roots == 0 { &[][..] } else { &[0, 2][..] }
                        );
                        assert!(things.items().iter().all(ProjectedThing::is_deleted));
                    } else {
                        assert!(matches!(output, Some(ProjectedBatchResult::Deleted)));
                    }
                    let seen = transport.seen.lock().unwrap();
                    assert_eq!(seen.len(), 1);
                    assert_eq!(seen[0].plans().len(), 1);
                    assert_eq!(
                        seen[0].plans()[0]
                            .bound_plan(&transport.schema)
                            .unwrap()
                            .canonical_bytes()
                            .unwrap(),
                        direct
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn batch_proxy_mapping_failure_preserves_commit_outcome_and_original_diagnostic() {
        let model = TypeId::new(TypeKind::Entity, "wide-item").unwrap();
        let (installed, proxy, transport) = single_proxy_fixture(model.clone(), 1, 1);
        let control =
            ProjectedBatchInvocationControl::capture(Default::default(), Default::default());
        let batch = ProjectedBatch::try_new_for_invocation(
            &installed,
            model.clone(),
            ProjectedBatchOperation::Insert,
            vec![ProjectedBatchRow::Create(proxy_single_input(
                &installed, &model, 1,
            ))],
            &control,
        )
        .unwrap();
        let expected = ProjectedBatchExecutor::binding_materialization_failure(0);
        let error = ProjectedBatchExecutor::new(&installed)
            .execute_on_target_mapped(
                crate::QueryTarget::Proxy(&proxy),
                &batch,
                control,
                (),
                |_| Err(expected.clone()),
            )
            .await
            .unwrap_err();
        let crate::QueryTargetError::Commit(error) = error else {
            panic!("expected staged failure");
        };
        assert_eq!(error.diagnostic().code().as_str(), "commit_outcome_unknown");
        let source = std::error::Error::source(error.as_ref()).unwrap();
        let original = std::error::Error::source(source)
            .unwrap()
            .downcast_ref::<SdkExecutionDiagnostic>()
            .unwrap();
        assert_eq!(*original, expected);
        assert_eq!(transport.seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn single_proxy_writes_share_inline_bytes_and_materialize_all_roots_or_zero() {
        for kind in [TypeKind::Entity, TypeKind::Relation] {
            for tags in [1, 300] {
                for operation in [
                    ProjectedBatchOperation::Insert,
                    ProjectedBatchOperation::Put,
                    ProjectedBatchOperation::Update,
                ] {
                    for roots in 0..=if tags == 300 || operation == ProjectedBatchOperation::Update
                    {
                        1
                    } else {
                        2
                    } {
                        let model = TypeId::new(
                            kind,
                            if kind == TypeKind::Entity {
                                "wide-item"
                            } else {
                                "wide-edge"
                            },
                        )
                        .unwrap();
                        let (installed, proxy, transport) =
                            single_proxy_fixture(model.clone(), roots, tags);
                        let input = proxy_single_input(&installed, &model, tags);
                        let control = ProjectedBatchInvocationControl::capture(
                            Default::default(),
                            AnswerCancellation::default(),
                        );
                        let executor = ProjectedCrudExecutor::new(&installed);
                        let row = if operation == ProjectedBatchOperation::Update {
                            ProjectedBatchRow::Update {
                                iid: "0x20".into(),
                                replacement: input,
                            }
                        } else {
                            ProjectedBatchRow::Create(input)
                        };
                        let batch = executor
                            .single_controlled_write_batch(&model, operation, row, &control)
                            .unwrap();
                        let inline = executor
                            .prepare_controlled_write(&batch, proxy.execution_identity(), None)
                            .unwrap()
                            .map(|single| single.plan.local().canonical_bytes().unwrap());
                        assert_eq!(inline.is_some(), tags == 1);
                        let result = executor.execute_single_write_on_target(crate::QueryTarget::Proxy(&proxy), &batch, control, Ok).await
                            .unwrap_or_else(|error| panic!("kind={kind:?} tags={tags} operation={operation:?} roots={roots}: {error:?}"));
                        assert_eq!(result.count(), roots);
                        assert_eq!(result.item().is_some(), roots == 1);
                        let tag =
                            OwnsFactId::new(model.clone(), AttributeId::new("item-tag").unwrap())
                                .unwrap();
                        assert!(
                            result
                                .items()
                                .iter()
                                .all(|thing| thing.fields()[&tag].len() == tags)
                        );
                        let seen = transport.seen.lock().unwrap();
                        assert_eq!(seen.len(), 1);
                        if let Some(bytes) = inline {
                            assert_eq!(seen[0].plans().len(), 1);
                            assert_eq!(
                                seen[0].plans()[0]
                                    .bound_plan(&transport.schema)
                                    .unwrap()
                                    .canonical_bytes()
                                    .unwrap(),
                                bytes
                            );
                        } else {
                            assert!(seen[0].plans().len() > 1);
                            assert!(
                                seen[0]
                                    .plans()
                                    .iter()
                                    .any(|entry| !entry.result_refs().is_empty())
                            );
                        }
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn single_proxy_write_negotiates_query_limits_before_selecting_pieces() {
        let mut raised = type_bridge_contract::limits::QueryLimits::V3;
        raised.structural.bindings = 1024;
        raised.structural.boolean_terms = 512;
        for client_raised in [false, true] {
            for server_raised in [false, true] {
                let model = TypeId::new(TypeKind::Entity, "wide-item").unwrap();
                let (installed, proxy, transport) = single_proxy_fixture_with_limits(
                    model.clone(),
                    1,
                    300,
                    if server_raised {
                        raised
                    } else {
                        type_bridge_contract::limits::QueryLimits::V3
                    },
                );
                let proxy = proxy
                    .with_query_limits(if client_raised {
                        raised
                    } else {
                        type_bridge_contract::limits::QueryLimits::V3
                    })
                    .unwrap();
                let input = proxy_single_input(&installed, &model, 300);
                let executor = ProjectedCrudExecutor::new(&installed);
                let control = ProjectedBatchInvocationControl::capture(
                    Default::default(),
                    AnswerCancellation::default(),
                );
                let batch = executor
                    .single_controlled_write_batch(
                        &model,
                        ProjectedBatchOperation::Insert,
                        ProjectedBatchRow::Create(input),
                        &control,
                    )
                    .unwrap();
                let result = executor
                    .execute_single_write_on_target(
                        crate::QueryTarget::Proxy(&proxy),
                        &batch,
                        control,
                        Ok,
                    )
                    .await
                    .unwrap();
                assert_eq!(result.count(), 1);
                let seen = transport.seen.lock().unwrap();
                assert_eq!(seen.len(), 1);
                assert_eq!(seen[0].plans().len() == 1, client_raised && server_raised);
                assert_eq!(
                    proxy.query_limits(),
                    if client_raised && server_raised {
                        raised
                    } else {
                        type_bridge_contract::limits::QueryLimits::V3
                    }
                );
            }
        }
    }

    #[tokio::test]
    async fn single_proxy_update_keeps_the_exact_iid_fence_after_remote_commit() {
        let model = TypeId::new(TypeKind::Entity, "wide-item").unwrap();
        let (installed, proxy, transport) = single_proxy_fixture(model.clone(), 2, 1);
        let input = proxy_single_input(&installed, &model, 1);
        let control = ProjectedBatchInvocationControl::capture(
            Default::default(),
            AnswerCancellation::default(),
        );
        let executor = ProjectedCrudExecutor::new(&installed);
        let batch = executor
            .single_controlled_write_batch(
                &model,
                ProjectedBatchOperation::Update,
                ProjectedBatchRow::Update {
                    iid: "0x20".into(),
                    replacement: input,
                },
                &control,
            )
            .unwrap();
        let error = executor
            .execute_single_write_on_target(crate::QueryTarget::Proxy(&proxy), &batch, control, Ok)
            .await
            .unwrap_err();
        let crate::QueryTargetError::Commit(error) = error else {
            panic!("atomic proxy failure")
        };
        assert_eq!(error.diagnostic().code().as_str(), "commit_outcome_unknown");
        let source = std::error::Error::source(error.as_ref()).unwrap();
        let diagnostic = std::error::Error::source(source)
            .unwrap()
            .downcast_ref::<SdkExecutionDiagnostic>()
            .unwrap();
        assert_eq!(
            diagnostic.code().as_str(),
            "projected_batch_update_identity_changed"
        );
        assert_eq!(transport.seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn single_proxy_write_refuses_foreign_player_origins_before_dispatch() {
        let model = TypeId::new(TypeKind::Relation, "wide-edge").unwrap();
        let (installed, proxy, transport) = single_proxy_fixture(model.clone(), 1, 1);
        let input = proxy_single_input(&installed, &model, 1);
        let foreign = ProjectedReference::try_new_for_database(
            &installed,
            TypeId::new(TypeKind::Entity, "wide-item").unwrap(),
            Some("0x10".into()),
            vec![],
            DatabaseExecutionIdentity::isolated("another-proxy"),
        )
        .unwrap();
        let input = ProjectedCreate::try_new(
            &installed,
            model.clone(),
            input
                .fields()
                .iter()
                .map(|(field, values)| (field.clone(), values.clone()))
                .collect(),
            vec![(RoleId::new("wide-edge", "member").unwrap(), vec![foreign])],
        )
        .unwrap();
        let control = ProjectedBatchInvocationControl::capture(
            Default::default(),
            AnswerCancellation::default(),
        );
        let executor = ProjectedCrudExecutor::new(&installed);
        let batch = executor
            .single_controlled_write_batch(
                &model,
                ProjectedBatchOperation::Insert,
                ProjectedBatchRow::Create(input),
                &control,
            )
            .unwrap();
        let error = executor
            .execute_single_write_on_target(crate::QueryTarget::Proxy(&proxy), &batch, control, Ok)
            .await
            .unwrap_err();
        assert_eq!(
            error
                .into_diagnostic(SdkProviderOperation::Write)
                .code()
                .as_str(),
            "reference_database_mismatch"
        );
        assert!(transport.seen.lock().unwrap().is_empty());
    }

    #[test]
    fn canonical_single_cuts_keep_the_captured_control_and_leave_small_plans_inline() {
        let installed = wide_single_projection();
        let executor = ProjectedCrudExecutor::new(&installed);
        let model = TypeId::new(TypeKind::Entity, "wide-item").unwrap();
        let key = AttributeId::new("item-key").unwrap();
        let tag = AttributeId::new("item-tag").unwrap();
        let scalar = |attribute: &AttributeId, value: String| {
            ProjectedAttributeValue::try_new(
                &installed,
                TypeId::new(TypeKind::Attribute, attribute.label().as_str()).unwrap(),
                CanonicalValue::String(CanonicalString::new(value).unwrap()),
            )
            .unwrap()
        };
        for (count, bytes, cut) in [(1, 1, false), (300, 1, true), (5, 900_000, true)] {
            let input = ProjectedCreate::try_new(
                &installed,
                model.clone(),
                vec![
                    (
                        OwnsFactId::new(model.clone(), key.clone()).unwrap(),
                        vec![scalar(&key, "key".into())],
                    ),
                    (
                        OwnsFactId::new(model.clone(), tag.clone()).unwrap(),
                        (0..count)
                            .map(|index| scalar(&tag, format!("{}-{index}", "x".repeat(bytes))))
                            .collect(),
                    ),
                ],
                vec![],
            )
            .unwrap();
            let cancellation = AnswerCancellation::default();
            let control = ProjectedBatchInvocationControl::capture(
                QueryExecutionResourceLimits::default(),
                cancellation.clone(),
            );
            let deadline = control.deadline().instant();
            let batch = executor
                .single_controlled_write_batch(
                    &model,
                    ProjectedBatchOperation::Put,
                    ProjectedBatchRow::Create(input),
                    &control,
                )
                .unwrap();
            let prepared = executor
                .prepare_controlled_write(
                    &batch,
                    &DatabaseExecutionIdentity::isolated("single-wide"),
                    None,
                )
                .unwrap();
            assert_eq!(prepared.is_none(), cut, "count={count} bytes={bytes}");
            if count == 300 {
                let mut limits = type_bridge_contract::limits::QueryLimits::V3;
                limits.structural.bindings = 1024;
                limits.structural.boolean_terms = 512;
                let limits = Arc::new(limits);
                let raised = executor
                    .prepare_controlled_write(
                        &batch,
                        &DatabaseExecutionIdentity::isolated("single-wide"),
                        Some(&limits),
                    )
                    .unwrap()
                    .expect("raised query dimensions keep the full write in one plan");
                assert_eq!(raised.plan.local().limits(), *limits);
            }
            let carried = PreparedProjectedBatchInvocation::try_new(&installed, &batch, control)
                .unwrap()
                .into_control();
            assert_eq!(carried.deadline().instant(), deadline);
            cancellation.cancel();
            assert_eq!(
                carried.checkpoint().unwrap_err().code().as_str(),
                "provider_cancelled"
            );
        }
    }

    #[test]
    fn controlled_single_entity_and_relation_writes_pin_the_uncontrolled_plan_bytes() {
        let source = "format: typebridge.schema/v2\nattributes:\n  score: { value: integer }\nentities:\n  counter:\n    owns:\n      score: { card: 1 }\nrelations:\n  event:\n    relates:\n      member: { card: 1 }\n    owns:\n      score: { card: 1 }\nplays:\n  counter:\n    event: [member]\n";
        let declared = normalize_documents(
            &SchemaDocumentSet::parse([(DocumentId::new("single-update.yaml").unwrap(), source)])
                .unwrap(),
        )
        .unwrap();
        let profile = SemanticProfileId::new("typedb-3.13.6/v1").unwrap();
        let resolved = resolve(&declared, &profile).unwrap();
        let projection = project(
            &resolved,
            BindingTarget::Python,
            &ProjectionConfig::python(),
            &[ProjectionHandler::python_v2()],
            &[],
        )
        .unwrap();
        let installed = InstalledRuntimeProjection::try_new(projection).unwrap();
        let executor = ProjectedCrudExecutor::new(&installed);
        for (kind, label) in [(TypeKind::Entity, "counter"), (TypeKind::Relation, "event")] {
            let model = TypeId::new(kind, label).unwrap();
            let field = OwnsFactId::new(model.clone(), AttributeId::new("score").unwrap()).unwrap();
            let value = ProjectedAttributeValue::try_new(
                &installed,
                TypeId::new(TypeKind::Attribute, "score").unwrap(),
                CanonicalValue::Long(11),
            )
            .unwrap();
            let input = ProjectedCreate::try_new(
                &installed,
                model.clone(),
                vec![(field, vec![value])],
                if kind == TypeKind::Relation {
                    vec![(
                        RoleId::new("event", "member").unwrap(),
                        vec![
                            ProjectedReference::try_new(
                                &installed,
                                TypeId::new(TypeKind::Entity, "counter").unwrap(),
                                Some("0x01".into()),
                                vec![],
                            )
                            .unwrap(),
                        ],
                    )]
                } else {
                    vec![]
                },
            )
            .unwrap();
            let control = ProjectedBatchInvocationControl::capture(
                QueryExecutionResourceLimits::default(),
                AnswerCancellation::default(),
            );
            let batch = executor
                .single_controlled_batch(
                    &model,
                    ProjectedBatchOperation::Update,
                    ProjectedBatchRow::Update {
                        iid: "0x10".into(),
                        replacement: input.clone(),
                    },
                    &control,
                )
                .unwrap();
            let actual = executor
                .prepare_controlled_write(
                    &batch,
                    &DatabaseExecutionIdentity::isolated("single-update"),
                    None,
                )
                .unwrap()
                .expect("ordinary input stays one plan");
            let write = executor.lower_plan_write(&input).unwrap();
            let authority = installed.plan_authority().unwrap();
            let expected = update(
                &authority,
                ValueTransport::Given,
                ThingUpdate {
                    type_id: &model,
                    selection: UpdateSelection::Iid("0x10"),
                    cleared_fields: &write.cleared_fields,
                    fields: &write.fields,
                    cleared_roles: &write.cleared_roles,
                    players: &match &actual.model {
                        WriteModel::Entity(_) => vec![],
                        WriteModel::Relation(prepared) => plan_references(prepared).unwrap(),
                    },
                    document_roles: match &actual.model {
                        WriteModel::Entity(_) => vec![],
                        WriteModel::Relation(prepared) => {
                            executor.document_roles(prepared).unwrap()
                        }
                    },
                },
            )
            .unwrap();
            let iid_filter = crate::ProjectedManagerFilter::try_new(&installed, model.clone())
                .unwrap()
                .try_and_iids(
                    &installed,
                    &[type_bridge_contract::id::Iid::new("0x10").unwrap()],
                )
                .unwrap();
            let filtered = executor
                .prepare_filtered_update(
                    &iid_filter,
                    executor
                        .prepare_write_model(
                            &model,
                            &input,
                            &DatabaseExecutionIdentity::isolated("single-update"),
                        )
                        .unwrap(),
                    &authority,
                )
                .unwrap();
            assert_eq!(
                filtered.plan.local().canonical_bytes().unwrap(),
                actual.plan.local().canonical_bytes().unwrap()
            );
            let bytes = actual.plan.local().canonical_bytes().unwrap();
            assert_eq!(bytes, expected.local().canonical_bytes().unwrap());
            assert_eq!(
                format!("{:x}", Sha256::digest(&bytes)),
                if kind == TypeKind::Entity {
                    "2a37300e6b848335825eb98ae80a38d7d2c3ae536335ede95e4d95e83d2b8ab3"
                } else {
                    "02af9a5869968f713d26d1af2b98d8ceee7c3d70e551163c64d68b2001f92f84"
                },
            );
            let insert_batch = executor
                .single_controlled_write_batch(
                    &model,
                    ProjectedBatchOperation::Insert,
                    ProjectedBatchRow::Create(input),
                    &control,
                )
                .unwrap();
            assert_eq!(insert_batch.resource_measure().statements(), 1);
            let inserted = executor
                .prepare_controlled_write(
                    &insert_batch,
                    &DatabaseExecutionIdentity::isolated("single-insert"),
                    None,
                )
                .unwrap()
                .expect("ordinary input stays one plan");
            let expected_insert = match &inserted.model {
                WriteModel::Entity(prepared) => entity_insert(
                    &authority,
                    ValueTransport::Given,
                    &model,
                    &prepared.plan_write.created,
                ),
                WriteModel::Relation(prepared) => relation_insert(
                    &authority,
                    ValueTransport::Given,
                    &model,
                    &prepared.plan_write.created,
                    &plan_references(prepared).unwrap(),
                    executor.document_roles(prepared).unwrap(),
                ),
            }
            .unwrap();
            assert_eq!(
                inserted.plan.local().canonical_bytes().unwrap(),
                expected_insert.local().canonical_bytes().unwrap()
            );
        }
    }
}
