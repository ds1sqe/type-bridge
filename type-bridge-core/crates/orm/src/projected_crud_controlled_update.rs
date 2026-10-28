//! Controlled single replacements retain the common single-plan compiler.

use super::*;
use crate::plan_builders::BuiltPlan;
use crate::projected_batch::PreparedProjectedBatchInvocation;
use crate::projected_batch_executor::{ExecutionLedger, given_rows_bytes};

#[derive(Clone, Copy)]
pub(super) enum UpdateTarget<'a> {
    Database(&'a Database),
    Transaction(&'a TransactionContext),
}

enum UpdateModel {
    Entity(PreparedEntityCreate),
    Relation(PreparedRelationCreate),
}

struct SingleUpdate {
    model: UpdateModel,
    plan: BuiltPlan,
}

impl ProjectedCrudExecutor<'_> {
    pub(super) fn single_controlled_update_batch(
        &self,
        model: &TypeId,
        row: ProjectedBatchRow,
        control: &ProjectedBatchInvocationControl,
    ) -> Result<(ProjectedBatch, bool), SdkExecutionDiagnostic> {
        let tokens = self.model_integrity(model)?.query_tokens();
        let native = tokens
            .fields()
            .values()
            .all(|field| field.multiplicity().collection_mode() == CollectionMode::Unordered)
            && tokens
                .roles()
                .values()
                .all(|role| role.multiplicity().collection_mode() == CollectionMode::Unordered);
        let batch = if native {
            ProjectedBatch::try_single_update(self.installed, model.clone(), row, control)?
        } else {
            self.single_controlled_batch(model, ProjectedBatchOperation::Update, row, control)?
        };
        Ok((batch, native))
    }

    pub(super) async fn execute_controlled_single_update(
        &self,
        target: UpdateTarget<'_>,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
        native: bool,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
        if !native {
            let executor = ProjectedBatchExecutor::new(self.installed);
            let mapper = |result| single_batch_thing(result, batch.model()).map(Some);
            return match target {
                UpdateTarget::Database(database) => {
                    executor
                        .execute_mapped(database, batch, control, None, mapper)
                        .await
                }
                UpdateTarget::Transaction(transaction) => {
                    executor
                        .execute_in_transaction_mapped(transaction, batch, control, None, mapper)
                        .await
                }
            }?
            .ok_or_else(|| single_batch_result_invalid(batch.model()));
        }
        let ceiling = match target {
            UpdateTarget::Database(database) => database.answer_limits(),
            UpdateTarget::Transaction(transaction) => transaction.answer_limits(),
        };
        let prepared = PreparedProjectedBatchInvocation::try_new(
            self.installed,
            batch,
            control.constrained_by(ceiling),
        )?;
        if let UpdateTarget::Transaction(transaction) = target
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
            UpdateTarget::Database(database) => database.execution_identity(),
            UpdateTarget::Transaction(transaction) => transaction.execution_identity().clone(),
        };
        let given = match target {
            UpdateTarget::Database(database) => database.supports_given_stage(),
            UpdateTarget::Transaction(transaction) => await_crud_controlled(
                transaction.supports_given_stage(),
                prepared.deadline(),
                prepared.cancellation(),
            )
            .await
            .map_err(|error| {
                controlled_crud_orm(error, SdkProviderOperation::Write, batch.model())
            })?,
        };
        crate::projected_batch_executor::require_given_capability(given)?;
        let single = self.prepare_controlled_update(batch, &identity)?;
        let query = crate::query_plan_exec::staged_query(single.plan.local(), true)
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
            UpdateTarget::Database(database) => {
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
                    .run_controlled_update(&transaction, &prepared, &single, query, &mut ledger)
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
            UpdateTarget::Transaction(transaction) => {
                self.run_controlled_update(transaction, &prepared, &single, query, &mut ledger)
                    .await
            }
        }
    }

    fn prepare_controlled_update(
        &self,
        batch: &ProjectedBatch,
        identity: &DatabaseExecutionIdentity,
    ) -> Result<SingleUpdate, SdkExecutionDiagnostic> {
        let Some((_, ProjectedBatchRow::Update { iid, replacement })) = batch.row_at(0) else {
            return Err(single_batch_result_invalid(batch.model()));
        };
        crate::projected_batch_executor::validate_provider_scalar_inputs(&[batch
            .row_at(0)
            .expect("one validated update row")])?;
        let authority = self
            .installed
            .plan_authority()
            .map_err(|error| plan_build_failure(&error, batch.model()))?;
        let model = match batch.model().kind() {
            TypeKind::Entity => UpdateModel::Entity(self.prepare_entity_create(replacement)?),
            TypeKind::Relation => {
                let prepared = self.prepare_relation_create(replacement)?;
                require_role_player_database_identity(&prepared, identity)?;
                if direct_entity_role_player_inputs(&prepared).is_none() {
                    reject_duplicate_references(&prepared)?;
                }
                UpdateModel::Relation(prepared)
            }
            _ => return Err(single_batch_result_invalid(batch.model())),
        };
        let (write, players, roles) = match &model {
            UpdateModel::Entity(prepared) => (prepared.plan_write.as_ref(), Vec::new(), Vec::new()),
            UpdateModel::Relation(prepared) => (
                prepared.plan_write.as_ref(),
                plan_references(prepared)?,
                self.document_roles(prepared)?,
            ),
        };
        let write = write.ok_or_else(|| single_batch_result_invalid(batch.model()))?;
        let plan = update_by_iid(
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
        .map_err(|error| plan_build_failure(&error, batch.model()))?;
        Ok(SingleUpdate { model, plan })
    }

    async fn run_controlled_update(
        &self,
        transaction: &TransactionContext,
        prepared: &PreparedProjectedBatchInvocation<'_>,
        single: &SingleUpdate,
        query: crate::session::staged::StagedQuery,
        ledger: &mut ExecutionLedger,
    ) -> Result<ProjectedThing, SdkExecutionDiagnostic> {
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
        let mut reference_failure_without_mutation = false;
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
            if documents.is_empty() {
                if matches!(single.model, UpdateModel::Relation(_))
                    && !stats.stopped_early
                    && prepared.limits().statement_limit() > 1
                    && let Err(diagnostic) = ProjectedBatchExecutor::new(self.installed)
                        .diagnose_missing_single_update(
                            &mut lease,
                            prepared,
                            ledger,
                            transaction.execution_identity(),
                        )
                        .await
                {
                    // A complete empty answer from this conditional plan proves no writes;
                    // retain the old pre-mutation reference-failure transaction state.
                    reference_failure_without_mutation =
                        diagnostic.code().as_str() == "projected_batch_reference_missing";
                    return Err(diagnostic);
                }
                return Err(crate::projected_batch_executor::integrity_at_row(
                    "projected_batch_mutation_result_missing",
                    "A projected batch mutation omitted one input ordinal",
                    type_id,
                    0,
                ));
            }
            let thing = match &single.model {
                UpdateModel::Entity(model) => self.hydrate_entity_documents(
                    transaction,
                    model,
                    &documents,
                    ExecutionMode::default(),
                )?,
                UpdateModel::Relation(model) => {
                    let iid = single_document_iid(&documents, type_id)?;
                    if direct_entity_role_player_inputs(model).is_none() {
                        reject_resolved_duplicates(model, &documents)?;
                    }
                    let rows =
                        coalesce_dynamic_relation_by_iid(&model.descriptor, &documents, &iid)
                            .map_err(|error| {
                                orm_at_type(error, SdkProviderOperation::Read, type_id)
                            })?;
                    self.hydrate_relation(
                        require_mutation_relation_row(rows, type_id)?,
                        type_id,
                        &iid,
                        transaction.execution_identity(),
                        ExecutionMode::default(),
                    )?
                }
            };
            let Some((_, ProjectedBatchRow::Update { iid, .. })) = prepared.batch().row_at(0)
            else {
                return Err(single_batch_result_invalid(type_id));
            };
            require_update_iid(thing.iid(), iid, type_id)?;
            ledger.charge_projected(std::slice::from_ref(&thing))?;
            prepared.check_control()?;
            Ok(thing)
        }
        .await;
        match result {
            Ok(thing) => {
                lease.complete_success();
                Ok(thing)
            }
            Err(diagnostic) => {
                if reference_failure_without_mutation {
                    lease.complete_success();
                } else {
                    lease.record_failure(&diagnostic);
                }
                Err(diagnostic)
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
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

    #[test]
    fn controlled_single_entity_and_relation_updates_pin_the_uncontrolled_plan_bytes() {
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
                .prepare_controlled_update(
                    &batch,
                    &DatabaseExecutionIdentity::isolated("single-update"),
                )
                .unwrap();
            let write = executor.lower_plan_write(&input).unwrap().unwrap();
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
                        UpdateModel::Entity(_) => vec![],
                        UpdateModel::Relation(prepared) => plan_references(prepared).unwrap(),
                    },
                    document_roles: match &actual.model {
                        UpdateModel::Entity(_) => vec![],
                        UpdateModel::Relation(prepared) => {
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
        }
    }
}
