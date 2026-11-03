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

enum WriteModel {
    Entity(PreparedEntityCreate),
    Relation(PreparedRelationCreate),
}

struct SingleWrite {
    model: WriteModel,
    plan: BuiltPlan,
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
        ProjectedBatch::try_single_write(self.installed, model.clone(), operation, row, control)
    }

    /// Execute one write and map its complete answer before committing.
    ///
    /// Binding compatibility adapters use this boundary to retain their result
    /// shape without moving policy preflight or materialization after commit.
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
                controlled_crud_orm(error, SdkProviderOperation::Write, batch.model())
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
                    controlled_crud_orm(
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
        let replacement = match batch.row_at(0) {
            Some((_, ProjectedBatchRow::Update { replacement, .. })) => replacement,
            Some((_, ProjectedBatchRow::Create(create)))
                if matches!(
                    batch.operation(),
                    ProjectedBatchOperation::Insert | ProjectedBatchOperation::Put
                ) =>
            {
                create
            }
            _ => return Err(single_batch_result_invalid(batch.model())),
        };
        crate::projected_batch_executor::validate_provider_write_inputs(&[batch
            .row_at(0)
            .expect("one validated write row")])?;
        let authority = self
            .installed
            .plan_authority_with_query_limits(query_limits)
            .map_err(|error| plan_build_failure(&error, batch.model()))?;
        let model = match batch.model().kind() {
            TypeKind::Entity => WriteModel::Entity(self.prepare_entity_create(replacement)?),
            TypeKind::Relation => {
                let prepared = self.prepare_relation_create(replacement)?;
                require_role_player_database_identity(&prepared, identity)?;
                WriteModel::Relation(prepared)
            }
            _ => return Err(single_batch_result_invalid(batch.model())),
        };
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
            update_by_iid(
                &authority,
                ValueTransport::Given,
                ThingUpdate {
                    type_id: batch.model(),
                    iid,
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
                WriteModel::Entity(_) => keyed_entity_put(&authority, ValueTransport::Given, &put),
                WriteModel::Relation(_) => keyed_relation_put(
                    &authority,
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
                    &authority,
                    ValueTransport::Given,
                    batch.model(),
                    &write.created,
                ),
                WriteModel::Relation(prepared) => relation_insert(
                    &authority,
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

    async fn run_controlled_write<T, F>(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedProjectedBatchInvocation<'_>,
        single: &SingleWrite,
        query: crate::session::staged::StagedQuery,
        ledger: &mut ExecutionLedger,
        mapper: F,
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
        .map_err(|error| controlled_crud_orm(error, SdkProviderOperation::Write, type_id))?;
        lease
            .check_plan_provider(single.plan.local())
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
                lease.dispatch_plan_counted(query, limits, Some(single.plan.local())),
                prepared.deadline(),
                prepared.cancellation(),
            )
            .await
            .map_err(|error| controlled_crud_orm(error, SdkProviderOperation::Write, type_id))?;
            ledger.record_reply(stats)?;
            prepared.check_control()?;
            let answer = TransactionContext::resolve_plan(single.plan.local(), answer)
                .map_err(|error| orm_at_type(error, SdkProviderOperation::Write, type_id))?;
            let PlanAnswer::Documents(documents) = answer else {
                return Err(write_plan_invalid(type_id));
            };

            let thing = match &single.model {
                WriteModel::Entity(model) => self.hydrate_entity_documents(
                    transaction.execution_identity(),
                    &model.type_id,
                    &model.descriptor,
                    &documents,
                    ExecutionMode::default(),
                )?,
                WriteModel::Relation(model) => self.hydrate_relation_documents(
                    transaction.execution_identity(),
                    &model.type_id,
                    &model.descriptor,
                    &documents,
                    ExecutionMode::default(),
                )?,
            };
            if let Some((_, ProjectedBatchRow::Update { iid, .. })) = prepared.batch().row_at(0) {
                for item in thing.items() {
                    require_update_iid(item.iid(), iid, type_id)?;
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
    use type_bridge_contract::schema::DocumentId;
    use type_bridge_contract::value::CanonicalString;
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

    #[test]
    fn canonical_single_cuts_keep_the_captured_control_and_leave_small_plans_inline() {
        let source = "format: typebridge.schema/v2\nattributes:\n  item-key: { value: string }\n  item-tag: { value: string }\nentities:\n  wide-item:\n    owns:\n      item-key: { key: true }\n      item-tag: { card: { min: 0 } }\n";
        let declared = normalize_documents(
            &SchemaDocumentSet::parse([(DocumentId::new("single-wide.yaml").unwrap(), source)])
                .unwrap(),
        )
        .unwrap();
        let resolved = resolve(
            &declared,
            &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
        )
        .unwrap();
        let installed = InstalledRuntimeProjection::try_new(
            project(
                &resolved,
                BindingTarget::Python,
                &ProjectionConfig::python(),
                &[ProjectionHandler::python_v2()],
                &[],
            )
            .unwrap(),
        )
        .unwrap();
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
            let expected = update_by_iid(
                &authority,
                ValueTransport::Given,
                ThingUpdate {
                    type_id: &model,
                    iid: "0x10",
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
