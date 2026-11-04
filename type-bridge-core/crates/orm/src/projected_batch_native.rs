//! Native plan execution retains the common batch lease, event matrix and consumers.

use super::*;
use crate::plan_builders::{
    BuiltPlan,
    batch_native::{self as build, BatchDraft},
};
use crate::query_plan_exec::{self, remote_value};
use crate::session::backend::QueryResult;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::query_plan::QueryPattern;
use type_bridge_contract::query_plan::v3::{DeleteItem, DocumentRole, Stage};
use type_bridge_contract::query_remote::RemoteValue;

fn diagnostic(error: Diagnostic) -> SdkExecutionDiagnostic {
    lower_orm_error(
        &crate::plan_builders::manager::diagnostic(error),
        SdkProviderOperation::Write,
    )
}

impl BatchPlan<'_> {
    pub(super) fn complete_write_plans(
        &self,
    ) -> std::result::Result<crate::plan_builders::bulk_write::WriteProgram, SdkExecutionDiagnostic>
    {
        use crate::plan_builders::bulk_write::{WriteKind, WriteRow, WriteShape};
        use crate::plan_builders::reference::{Reference, ReferenceIdentity};
        let authority = self.authority.as_ref();
        let fields = self
            .rows
            .iter()
            .map(|(_, row)| {
                let create =
                    row_create(row).ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                let mut fields = Vec::new();
                for (field, values) in create.fields() {
                    let declaring = self.declaring_field(field)?;
                    fields.extend(
                        values
                            .iter()
                            .map(|value| (declaring.clone(), value.value().clone())),
                    );
                }
                Ok(fields)
            })
            .collect::<std::result::Result<Vec<_>, SdkExecutionDiagnostic>>()?;
        let routes = self
            .reference_routes
            .iter()
            .map(|route| (route.key.clone(), &route.candidate_types))
            .collect::<BTreeMap<_, _>>();
        let mut references = (0..self.row_count).map(|_| Vec::new()).collect::<Vec<_>>();
        for reference in &self.role_references {
            let route_key = if reference.reference.iid().is_some() {
                ReferenceRouteKey::Iid(
                    reference.role.clone(),
                    reference.reference.type_id().clone(),
                )
            } else {
                ReferenceRouteKey::Key(
                    reference.role.clone(),
                    reference.reference.type_id().clone(),
                    reference
                        .reference
                        .keys()
                        .first_key_value()
                        .ok_or_else(SdkExecutionDiagnostic::internal_failure)?
                        .0
                        .clone(),
                )
            };
            let candidates = routes
                .get(&route_key)
                .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
            let identity = if let Some(iid) = reference.reference.iid() {
                ReferenceIdentity::Iid(iid)
            } else {
                let (field, value) = reference
                    .reference
                    .keys()
                    .first_key_value()
                    .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                ReferenceIdentity::Key {
                    attribute: field.attribute(),
                    value: value.value().clone(),
                }
            };
            references[reference.owner_ordinal].push((
                reference.role.clone(),
                Reference {
                    candidates,
                    identity,
                },
            ));
        }
        let rows = self
            .rows
            .iter()
            .zip(&fields)
            .zip(&references)
            .map(|(((_, row), fields), players)| WriteRow {
                iid: row_iid(row),
                fields,
                players,
            })
            .collect::<Vec<_>>();
        let keys = self
            .model
            .reference_read()
            .key_fields()
            .iter()
            .map(|key| self.declaring_field(key).cloned())
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let cleared_fields = self
            .field_routes
            .iter()
            .map(|route| self.declaring_field(&route.field).cloned())
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let cleared_roles = self
            .model
            .complete_read()
            .roles()
            .iter()
            .filter(|(_, projection)| projection.multiplicity().collection_mode().is_unordered())
            .map(|(role, _)| role.clone())
            .collect::<Vec<_>>();
        let document_roles = self
            .model
            .complete_read()
            .roles()
            .iter()
            .map(|(role, projection)| {
                if projection.multiplicity().collection_mode().is_unordered() {
                    DocumentRole::collected(role.clone())
                } else {
                    DocumentRole::new(role.clone(), false)
                }
            })
            .collect();
        let kind = match self.operation {
            ProjectedBatchOperation::Insert => WriteKind::Insert,
            ProjectedBatchOperation::Put => WriteKind::Put,
            ProjectedBatchOperation::Update => WriteKind::Update,
            ProjectedBatchOperation::Delete => {
                return Err(SdkExecutionDiagnostic::internal_failure());
            }
        };
        crate::plan_builders::bulk_write::write_many(
            authority,
            kind,
            WriteShape {
                references_see_prior_writes: false,
                single_input: self.single_input,
                include_subtypes: false,
                return_documents: true,
                type_id: self.model.id(),
                keys: &keys,
                cleared_fields: &cleared_fields,
                cleared_roles: &cleared_roles,
                document_roles,
            },
            &rows,
        )
        .map_err(diagnostic)
    }

    fn declaring_field(
        &self,
        field: &OwnsFactId,
    ) -> std::result::Result<&OwnsFactId, SdkExecutionDiagnostic> {
        self.model
            .query_tokens()
            .fields()
            .get(field)
            .map(|token| token.declaring_id())
            .ok_or_else(SdkExecutionDiagnostic::internal_failure)
    }

    pub(super) fn native_delete(
        &self,
        rows: GivenRowsSpec,
    ) -> std::result::Result<BuiltPlan, SdkExecutionDiagnostic> {
        // Ordered models retain the native literal-IID deletion plan from 06.0.
        // Compiled field routes already exclude ordered ownership.
        if self.field_routes.len() != self.model.create().fields().len()
            || self
                .model
                .query_tokens()
                .roles()
                .values()
                .any(|role| !role.multiplicity().collection_mode().is_unordered())
        {
            let iids = rows
                .rows
                .iter()
                .map(|row| {
                    let Some(GivenValue::String(iid)) = row.get(1) else {
                        return Err(SdkExecutionDiagnostic::internal_failure());
                    };
                    Ok(iid.clone())
                })
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let registry = self
                .installed
                .manager_registry()
                .map_err(|error| lower_orm_error(&error, SdkProviderOperation::Write))?;
            let manager = crate::plan_builders::manager::ManagerPlans::new(registry)
                .with_query_limits(self.authority.shared_query_limits());
            return if let [iid] = iids.as_slice() {
                manager.delete(self.model.id().label().as_str(), iid, true, false)
            } else {
                manager.delete_many(self.model.id().label().as_str(), &iids, true, false)
            }
            .map_err(|error| lower_orm_error(&error, SdkProviderOperation::Write));
        }
        let columns = [(ValueTypeTag::Long, false), (ValueTypeTag::String, false)];
        let mut draft = BatchDraft::new(&self.authority, rows, &columns).map_err(diagnostic)?;
        let mut patterns = Vec::new();
        let ordinal = draft
            .bind_input(0, "ordinal", &mut patterns)
            .map_err(diagnostic)?;
        let thing = draft.draft.bind("thing").map_err(diagnostic)?;
        patterns.push(build::isa(thing, self.model.id()).into());
        patterns.push(
            QueryPattern::Iid {
                binding: thing,
                iid: draft.inputs[1].clone(),
            }
            .into(),
        );
        draft
            .finish(
                self.authority.as_ref(),
                vec![
                    Stage::Match { patterns },
                    Stage::Delete {
                        items: vec![DeleteItem::Thing { binding: thing }],
                    },
                ],
                true,
                vec![ordinal],
            )
            .map_err(diagnostic)
    }
}

impl ProjectedBatchExecutor<'_> {
    pub(crate) async fn execute_single_on_proxy_mapped<T, F>(
        &self,
        proxy: &crate::ProxyQueryConnection,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
        mapper: F,
    ) -> std::result::Result<T, crate::QueryTargetError>
    where
        F: FnOnce(
            crate::WriteResult<ProjectedThing>,
        ) -> std::result::Result<T, SdkExecutionDiagnostic>,
    {
        if !batch.is_single_write()
            || batch.len() != 1
            || batch.operation() == ProjectedBatchOperation::Delete
        {
            return Err(invalid_at_type(
                "projected_crud_single_row_required",
                "Single-write execution requires one create or update input",
                batch.model(),
            )
            .into());
        }
        self.execute_on_proxy_mapped(proxy, batch, control, None, |result| {
            let ProjectedBatchResult::Things(things) = result else {
                return Err(SdkExecutionDiagnostic::internal_failure());
            };
            mapper(crate::WriteResult::new(things.into_items())).map(Some)
        })
        .await?
        .ok_or_else(|| SdkExecutionDiagnostic::internal_failure().into())
    }

    pub(super) async fn execute_on_proxy_mapped<T, F>(
        &self,
        proxy: &crate::ProxyQueryConnection,
        batch: &ProjectedBatch,
        control: ProjectedBatchInvocationControl,
        empty: T,
        mapper: F,
    ) -> std::result::Result<T, crate::QueryTargetError>
    where
        F: FnOnce(ProjectedBatchResult) -> std::result::Result<T, SdkExecutionDiagnostic>,
    {
        if self.return_deleted && batch.operation() != ProjectedBatchOperation::Delete {
            return Err(invalid_at_type(
                "delete_returning_operation",
                "Deletion snapshots require a delete batch",
                batch.model(),
            )
            .into());
        }
        let prepared = PreparedProjectedBatchInvocation::try_new(
            self.installed,
            batch,
            control.constrained_by(Some(proxy.answer_limits())),
        )?;
        if batch.is_empty() {
            return Ok(empty);
        }
        prepared.check_control()?;
        let identity = proxy.execution_identity();
        let query_limits = proxy
            .manager_write_query_limits(prepared.deadline(), prepared.cancellation())
            .await?;
        let mut plan = BatchPlan::compile(
            self.installed,
            &prepared,
            identity,
            self.return_deleted,
            query_limits.as_ref(),
        )?;
        if batch.operation() == ProjectedBatchOperation::Delete {
            return self
                .execute_proxy_delete(proxy, &prepared, &plan, identity, mapper)
                .await;
        }
        let (mut program, mut ledger) = plan
            .write_program
            .take()
            .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
        let entries = program
            .staged_entries(&plan.authority)
            .map_err(diagnostic)?
            .ok_or_else(|| {
                SdkExecutionDiagnostic::unsupported_capability(
                    SdkDiagnosticCode::new("manager_proxy_dynamic_dependencies")
                        .expect("static code"),
                    SdkDiagnosticMessage::new(
                        "The effective proxy query limits require an unsupported write dependency graph",
                    )
                    .expect("static message"),
                )
            })?;
        let guard_contexts = program.guard_contexts();
        prepared.check_control()?;
        proxy
            .execute_write_stack_mapped(
                entries,
                prepared.limits(),
                prepared.cancellation(),
                prepared.deadline(),
                |answers| {
                    let mut consumer = HydrationConsumer::new(self, &plan, identity)?;
                    for (_, row) in &plan.rows {
                        consumer
                            .targets
                            .push(row_iid(row).unwrap_or_default().to_owned());
                    }
                    for answer in answers {
                        let step = program
                            .next()
                            .map_err(diagnostic)?
                            .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                        if plan.capture_role_snapshots
                            && let Some(snapshots) = program.take_reference_snapshots()
                        {
                            consumer.resolved_references = Some(snapshots);
                        }
                        let records_answers = program.records_answers();
                        program.record_answer(&step, &answer).map_err(diagnostic)?;
                        if let Some(guard) = step
                            .reference_resolution
                            .as_ref()
                            .and_then(|resolution| resolution.guard.as_ref())
                        {
                            let iids = step.reference_iids(&answer).map_err(diagnostic)?;
                            guard.validate(&iids, |role, reference_ordinal| {
                                plan.role_references
                                    .iter()
                                    .filter(|reference| {
                                        reference.owner_ordinal == step.ordinal
                                            && reference.role == *role
                                    })
                                    .nth(reference_ordinal)
                                    .map(|reference| (reference.reference_ordinal, reference.reference.type_id().clone()))
                            })?;
                        }
                        if step.reference_resolution.as_ref().is_some_and(|resolution| resolution.guard.is_none()) {
                            let iids = step.reference_iids(&answer).map_err(diagnostic)?;
                            program.accept_reference_iids(&step, iids).map_err(diagnostic)?;
                        }
                        if step.complete {
                            accept_hydration_answer(step.plan.local(), answer, &mut consumer)?;
                        } else if !records_answers {
                            let iid = step.identity(&answer).map_err(diagnostic)?;
                            program.accept_identity(&step, iid).map_err(diagnostic)?;
                        }
                    }
                    if program.next().map_err(diagnostic)?.is_some() {
                        return Err(SdkExecutionDiagnostic::internal_failure());
                    }
                    let snapshots = consumer.resolved_references.take();
                    let things = consumer.finish()?;
                    ledger.charge_projected(things.items())?;
                    for (thing, &index) in things.items().iter().zip(things.input_indexes()) {
                        validate_written_role_identities(
                            self.installed,
                            &plan,
                            thing,
                            index,
                            snapshots
                                .as_ref()
                                .and_then(|snapshots| snapshots.get(&index)),
                        )?;
                    }
                    prepared.check_control()?;
                    mapper(ProjectedBatchResult::Things(things))
                },
            )
            .await
            .map_err(|error| {
                crate::QueryTargetError::Commit(Box::new(error.map_remote_diagnostic(
                    |diagnostic| {
                        use type_bridge_contract::sdk_diagnostic::{
                            SdkDiagnosticDetailValue, SdkDiagnosticName,
                        };
                        if diagnostic.code().as_str() != "query_stage_result_ref_missing" {
                            return None;
                        }
                        let coordinate = |name| match diagnostic
                            .details()
                            .get(&SdkDiagnosticName::new(name).ok()?)?
                        {
                            SdkDiagnosticDetailValue::Signed(value) => usize::try_from(*value).ok(),
                            SdkDiagnosticDetailValue::Count(value) => usize::try_from(*value).ok(),
                            _ => None,
                        };
                        let context = guard_contexts.get(&(
                            coordinate("source_plan")?,
                            u16::try_from(coordinate("source_column")?).ok()?,
                        ))?;
                        match context {
                            crate::plan_builders::bulk_write::ReferenceContext::Guard(context) => context.validate(&[], |role, reference_ordinal| plan.role_references.iter().filter(|reference| reference.owner_ordinal == context.row() && reference.role == *role).nth(reference_ordinal).map(|reference| (reference.reference_ordinal, reference.reference.type_id().clone()))).err(),
                            crate::plan_builders::bulk_write::ReferenceContext::Snapshot(ordinal) => {
                                let mut references = plan.role_references.iter().collect::<Vec<_>>();
                                references.sort_by_key(|reference| reference.owner_ordinal);
                                let reference = references.get(*ordinal)?;
                                Some(integrity_at_reference("projected_batch_reference_missing", "A projected role-player reference did not resolve exactly once", plan.model.id(), &reference.role, reference.owner_ordinal, reference.reference_ordinal, reference.reference.type_id()))
                            },
                        }
                    },
                )))
            })
    }

    async fn execute_proxy_delete<T, F>(
        &self,
        proxy: &crate::ProxyQueryConnection,
        prepared: &PreparedProjectedBatchInvocation<'_>,
        plan: &BatchPlan<'_>,
        identity: &DatabaseExecutionIdentity,
        mapper: F,
    ) -> Result<T, crate::QueryTargetError>
    where
        F: FnOnce(ProjectedBatchResult) -> Result<T, SdkExecutionDiagnostic>,
    {
        let iids = self.return_deleted.then(|| {
            plan.rows
                .iter()
                .map(|(_, row)| row_iid(row).expect("normalized deletion IID").to_owned())
                .collect::<Vec<_>>()
        });
        let plans = if let Some(iids) = &iids {
            let registry = self.installed.manager_registry().map_err(|error| {
                at_type(
                    lower_orm_error(&error, SdkProviderOperation::Write),
                    plan.model.id(),
                )
            })?;
            let manager = crate::plan_builders::manager::ManagerPlans::new(registry)
                .with_query_limits(plan.authority.shared_query_limits());
            crate::_manager::returning::DeleteSelection::Iids(iids)
                .plans(&manager, plan.model.id().label().as_str(), true)
                .map_err(|error| {
                    at_type(
                        lower_orm_error(&error, SdkProviderOperation::Write),
                        plan.model.id(),
                    )
                })?
                .into_vec()
        } else {
            input_chunks(plan.delete_rows()?, false)?
                .into_iter()
                .map(|rows| plan.native_delete(rows))
                .collect::<Result<Vec<_>, _>>()?
        };
        let statements = plans
            .len()
            .checked_mul(if self.return_deleted { 2 } else { 1 })
            .ok_or_else(complexity_overflow)?;
        let resources = prepared.limits().for_adapter_statements(statements);
        let mut ledger =
            ExecutionLedger::new(resources, prepared.batch().resource_measure().bytes());
        ledger.charge_preflight(
            plan.preflight_bytes,
            u32::try_from(statements).map_err(|_| complexity_overflow())?,
        )?;
        let entries = plans
            .iter()
            .map(|native| {
                type_bridge_contract::query_remote_v3::StagedPlanV3::new(
                    native.local(),
                    native.local().given_rows().unwrap_or_default().to_vec(),
                    plan.authority.declared(),
                )
                .map_err(diagnostic)
            })
            .collect::<Result<Vec<_>, _>>()?;
        prepared.check_control()?;
        proxy
            .execute_write_stack_mapped(
                entries,
                resources,
                prepared.cancellation(),
                prepared.deadline(),
                |answers| {
                    if answers.len() != plans.len() {
                        return Err(SdkExecutionDiagnostic::internal_failure());
                    }
                    let result = if let Some(iids) = &iids {
                        let registry = self
                            .installed
                            .manager_registry()
                            .map_err(|error| lower_orm_error(&error, SdkProviderOperation::Read))?;
                        let descriptor = registry
                            .descriptor_id(plan.model.id().label().as_str())
                            .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                        let rows = plans
                            .iter()
                            .zip(answers)
                            .map(|(plan, answer)| {
                                let crate::query_plan_exec::PlanAnswer::Documents(documents) =
                                    answer
                                else {
                                    return Err(SdkExecutionDiagnostic::internal_failure());
                                };
                                crate::_manager::returning::capture_rows(
                                    &registry,
                                    plan.local(),
                                    &documents,
                                    &descriptor,
                                    resources,
                                )
                                .map_err(|error| {
                                    lower_orm_error(&error, SdkProviderOperation::Read)
                                })
                            })
                            .collect::<Result<Vec<_>, _>>()?;
                        let ordered =
                            crate::_manager::returning::ordered_things(rows.iter().flatten(), iids)
                                .map_err(|error| {
                                    lower_orm_error(&error, SdkProviderOperation::Read)
                                })?;
                        let rows = lower_deleted_rows(&registry, plan.model.id().kind(), &ordered)
                            .map_err(|error| lower_orm_error(&error, SdkProviderOperation::Read))?;
                        let things = self.project_deleted_rows(plan, rows, identity)?;
                        ledger.charge_projected(things.items())?;
                        ProjectedBatchResult::Things(things)
                    } else {
                        let mut identities = DeleteIdentityConsumer::new(plan.row_count)?;
                        for (native, answer) in plans.iter().zip(answers) {
                            accept_native_answer(
                                native.local(),
                                answer,
                                deletion_reply(native.local()),
                                &mut identities,
                            )?;
                        }
                        identities.finish(plan)?;
                        ProjectedBatchResult::Deleted
                    };
                    prepared.check_control()?;
                    mapper(result)
                },
            )
            .await
            .map_err(|error| crate::QueryTargetError::Commit(Box::new(error)))
    }

    fn project_deleted_rows(
        &self,
        plan: &BatchPlan<'_>,
        rows: Vec<(usize, DeletedRow)>,
        identity: &DatabaseExecutionIdentity,
    ) -> Result<crate::BatchWriteResult<ProjectedThing>, SdkExecutionDiagnostic> {
        let mut things = crate::BatchWriteResult::try_with_capacity(rows.len())
            .map_err(|_| allocation_failure())?;
        for (index, row) in rows {
            let mut thing = match row {
                DeletedRow::Entity(row) => {
                    let iid = row
                        .iid
                        .clone()
                        .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                    self.project_entity(row, plan.model.id(), &iid, identity)?
                }
                DeletedRow::Relation(row) => {
                    let iid = row
                        .iid
                        .clone()
                        .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
                    self.project_relation(row, plan.model.id(), &iid, identity)?
                }
            };
            thing.mark_deleted()?;
            things
                .try_push(index, thing)
                .map_err(|_| allocation_failure())?;
        }
        Ok(things)
    }

    pub(super) async fn run_native(
        &self,
        lease: &mut TransactionContextMutationLease,
        prepared: &PreparedProjectedBatchInvocation<'_>,
        plan: &mut BatchPlan<'_>,
        identity: &DatabaseExecutionIdentity,
    ) -> std::result::Result<ProjectedBatchResult, SdkExecutionDiagnostic> {
        if plan.operation == ProjectedBatchOperation::Delete && plan.return_deleted {
            let registry = plan.installed.manager_registry().map_err(|error| {
                at_type(
                    lower_orm_error(&error, SdkProviderOperation::Read),
                    plan.model.id(),
                )
            })?;
            let plans = crate::plan_builders::manager::ManagerPlans::new(registry)
                .with_query_limits(plan.authority.shared_query_limits());
            let iids = plan
                .rows
                .iter()
                .map(|(_, row)| row_iid(row).expect("normalized delete IID").to_owned())
                .collect::<Vec<_>>();
            let input_bytes = prepared.batch().resource_measure().bytes();
            let mut deletion_policy = prepared.limits();
            if input_bytes > deletion_policy.bytes {
                return Err(resource_limit(
                    "projected_batch_transport_byte_limit",
                    "Deletion input exceeds the byte ceiling",
                    "bytes",
                    input_bytes,
                    deletion_policy.bytes,
                ));
            }
            deletion_policy.bytes -= input_bytes;
            let rows = crate::_manager::returning::delete_on_lease(
                lease,
                &plans,
                plan.model.id().label().as_str(),
                crate::_manager::returning::DeleteSelection::Iids(&iids),
                true,
                deletion_policy,
                prepared.cancellation(),
                prepared.deadline(),
                |registry, things, _, remaining| {
                    let rows = lower_deleted_rows(registry, plan.model.id().kind(), things)?;
                    Ok((rows, remaining))
                },
            )
            .await
            .map_err(|error| {
                at_type(
                    lower_orm_error(&error, SdkProviderOperation::Write),
                    plan.model.id(),
                )
            })?;
            let (rows, remaining) = rows;
            let things = self.project_deleted_rows(plan, rows, identity)?;
            let mut ledger = ExecutionLedger::new(remaining, 0);
            ledger.charge_projected(things.items())?;
            prepared.check_control()?;
            return Ok(ProjectedBatchResult::Things(things));
        }
        if plan.operation != ProjectedBatchOperation::Delete {
            let (mut program, mut ledger) = plan
                .write_program
                .take()
                .ok_or_else(SdkExecutionDiagnostic::internal_failure)?;
            let authority = Arc::clone(&plan.authority);
            let mut consumer = HydrationConsumer::new(self, plan, identity)?;
            for (_, row) in &plan.rows {
                consumer
                    .targets
                    .push(row_iid(row).unwrap_or_default().to_owned());
            }
            let mut reference_order = None;
            while let Some(step) = program.next().map_err(diagnostic)? {
                let records_answers = program.records_answers();
                let mut recorded = Vec::new();
                if plan.capture_role_snapshots
                    && let Some(snapshots) = program.take_reference_snapshots()
                {
                    consumer.resolved_references = Some(snapshots);
                }
                if let Some(guard) = step
                    .reference_resolution
                    .as_ref()
                    .and_then(|resolution| resolution.guard.as_ref())
                {
                    let mut returned = Vec::new();
                    let mut capture = |item: AnswerItem| {
                        let AnswerItem::Document(document) = item else {
                            return Err(OrmError::ExecutionDiagnostic(Box::new(
                                SdkExecutionDiagnostic::internal_failure(),
                            )));
                        };
                        let ordinal = document_index(&document, "ordinal", usize::MAX)
                            .map_err(|error| OrmError::ExecutionDiagnostic(Box::new(error)))?;
                        let iid = document_string(&document, "iid")
                            .map_err(|error| OrmError::ExecutionDiagnostic(Box::new(error)))?;
                        step.check_identity(&iid).map_err(evidence)?;
                        returned.push((ordinal, iid));
                        Ok(AnswerControl::Continue)
                    };
                    dispatch(
                        lease,
                        &step.plan,
                        Reply::ReferenceResolution,
                        prepared,
                        &mut capture,
                        &mut ledger,
                        true,
                        &authority,
                        &mut |item: &AnswerItem| {
                            if records_answers {
                                recorded.push(item.clone());
                            }
                        },
                    )
                    .await?;
                    guard.validate(&returned, |role, ordinal| {
                        plan.role_references
                            .iter()
                            .filter(|reference| {
                                reference.owner_ordinal == step.ordinal && reference.role == *role
                            })
                            .nth(ordinal)
                            .map(|reference| {
                                (
                                    reference.reference_ordinal,
                                    reference.reference.type_id().clone(),
                                )
                            })
                    })?;
                    record_static_answer(&mut program, &step, recorded, records_answers)?;
                    continue;
                }
                if let Some(resolution) = &step.reference_resolution {
                    let order = reference_order.get_or_insert_with(|| {
                        let mut references = plan.role_references.iter().collect::<Vec<_>>();
                        references.sort_by_key(|reference| reference.owner_ordinal);
                        Box::new(ReferenceOrder { references })
                    });
                    let mut returned = Vec::with_capacity(resolution.ordinals.len());
                    let mut seen = BTreeSet::new();
                    let mut capture = |item: AnswerItem| {
                        let AnswerItem::Document(document) = item else {
                            return Err(OrmError::ExecutionDiagnostic(Box::new(
                                SdkExecutionDiagnostic::internal_failure(),
                            )));
                        };
                        let ordinal = document_index(&document, "ordinal", order.references.len())
                            .map_err(|error| OrmError::ExecutionDiagnostic(Box::new(error)))?;
                        let reference = order.references[ordinal];
                        if resolution.ordinals.binary_search(&ordinal).is_err() {
                            return Err(OrmError::ExecutionDiagnostic(Box::new(
                                integrity_at_type(
                                    "provider_identity_mismatch",
                                    "Reference lookup returned an unexpected ordinal",
                                    plan.model.id(),
                                ),
                            )));
                        }
                        if !seen.insert(ordinal) {
                            return Err(OrmError::ExecutionDiagnostic(Box::new(
                                integrity_at_reference(
                                    "projected_batch_reference_ambiguous",
                                    "A projected role-player reference resolved more than one exact thing",
                                    plan.model.id(),
                                    &reference.role,
                                    reference.owner_ordinal,
                                    reference.reference_ordinal,
                                    reference.reference.type_id(),
                                ),
                            )));
                        }
                        let iid = document_string(&document, "iid")
                            .map_err(|error| OrmError::ExecutionDiagnostic(Box::new(error)))?;
                        if reference
                            .reference
                            .iid()
                            .is_some_and(|expected| !expected.eq_ignore_ascii_case(&iid))
                        {
                            return Err(OrmError::ExecutionDiagnostic(Box::new(
                                integrity_at_reference(
                                    "provider_identity_mismatch",
                                    "The provider prerequisite returned a different requested IID",
                                    plan.model.id(),
                                    &reference.role,
                                    reference.owner_ordinal,
                                    reference.reference_ordinal,
                                    reference.reference.type_id(),
                                ),
                            )));
                        }
                        returned.push((ordinal, iid));
                        Ok(AnswerControl::Continue)
                    };
                    dispatch(
                        lease,
                        &step.plan,
                        Reply::ReferenceResolution,
                        prepared,
                        &mut capture,
                        &mut ledger,
                        true,
                        &authority,
                        &mut |item: &AnswerItem| {
                            if records_answers {
                                recorded.push(item.clone());
                            }
                        },
                    )
                    .await?;
                    if let Some(missing) = resolution
                        .ordinals
                        .iter()
                        .find(|ordinal| !seen.contains(ordinal))
                    {
                        let reference = order.references[*missing];
                        return Err(integrity_at_reference(
                            "projected_batch_reference_missing",
                            "A projected role-player reference did not resolve exactly once",
                            plan.model.id(),
                            &reference.role,
                            reference.owner_ordinal,
                            reference.reference_ordinal,
                            reference.reference.type_id(),
                        ));
                    }
                    program
                        .accept_reference_iids(&step, returned)
                        .map_err(diagnostic)?;
                    record_static_answer(&mut program, &step, recorded, records_answers)?;
                    continue;
                }
                if !step.complete {
                    let mut returned = None;
                    let mut capture = |item: AnswerItem| {
                        let AnswerItem::Document(document) = item else {
                            return Err(OrmError::ExecutionDiagnostic(Box::new(
                                SdkExecutionDiagnostic::internal_failure(),
                            )));
                        };
                        let ordinal = document_index(&document, "ordinal", plan.row_count)
                            .map_err(|error| OrmError::ExecutionDiagnostic(Box::new(error)))?;
                        let iid =
                            if step.missing_iid_noop && document.get("iid").is_none() {
                                None
                            } else {
                                Some(document_string(&document, "iid").map_err(|error| {
                                    OrmError::ExecutionDiagnostic(Box::new(error))
                                })?)
                            };
                        if ordinal != step.ordinal || (returned.is_some() && !records_answers) {
                            return Err(OrmError::ExecutionDiagnostic(Box::new(integrity_at_row(
                                "provider_identity_mismatch",
                                "A write piece repeated or changed its input ordinal",
                                plan.model.id(),
                                step.ordinal,
                            ))));
                        }
                        if let Some(iid) = &iid {
                            step.check_identity(iid).map_err(evidence)?;
                        }
                        returned = Some(iid);
                        Ok(AnswerControl::Continue)
                    };
                    dispatch(
                        lease,
                        &step.plan,
                        Reply::Mutation,
                        prepared,
                        &mut capture,
                        &mut ledger,
                        false,
                        &authority,
                        &mut |item: &AnswerItem| {
                            if records_answers {
                                recorded.push(item.clone());
                            }
                        },
                    )
                    .await?;
                    if records_answers {
                        record_static_answer(&mut program, &step, recorded, true)?;
                        continue;
                    }
                    let iid = returned.ok_or_else(|| {
                        integrity_at_row(
                            "provider_identity_mismatch",
                            "A write piece omitted its model identity",
                            plan.model.id(),
                            step.ordinal,
                        )
                    })?;
                    record_static_answer(&mut program, &step, recorded, records_answers)?;
                    program.accept_identity(&step, iid).map_err(diagnostic)?;
                    continue;
                }
                dispatch(
                    lease,
                    &step.plan,
                    Reply::Hydration,
                    prepared,
                    &mut consumer,
                    &mut ledger,
                    false,
                    &authority,
                    &mut |item: &AnswerItem| {
                        if records_answers {
                            recorded.push(item.clone());
                        }
                    },
                )
                .await
                .map_err(|later| consumer.take_deferred().unwrap_or(later))?;
                record_static_answer(&mut program, &step, recorded, records_answers)?;
            }
            let reference_snapshots = consumer.resolved_references.take();
            let things = consumer.finish()?;
            ledger.charge_projected(things.items())?;
            for (thing, &ordinal) in things.items().iter().zip(things.input_indexes()) {
                validate_written_role_identities(
                    self.installed,
                    plan,
                    thing,
                    ordinal,
                    reference_snapshots
                        .as_ref()
                        .and_then(|snapshots| snapshots.get(&ordinal)),
                )?;
            }

            prepared.check_control()?;
            return Ok(ProjectedBatchResult::Things(things));
        }
        let authority = Arc::clone(&plan.authority);
        let mut ledger = ExecutionLedger::new(
            prepared.limits(),
            prepared.batch().resource_measure().bytes(),
        );
        let mut preflight = ExecutionLedger::new(
            prepared.limits(),
            prepared.batch().resource_measure().bytes(),
        );
        preflight.charge_preflight(plan.preflight_bytes, plan.statement_count)?;
        let mutations = input_chunks(plan.delete_rows()?, false)?
            .into_iter()
            .map(|rows| plan.native_delete(rows))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let statement_count = mutations.len();
        ledger.limits = prepared.limits().for_adapter_statements(statement_count);
        ledger.charge_preflight(
            0,
            u32::try_from(statement_count).map_err(|_| complexity_overflow())?,
        )?;
        let mut identities = DeleteIdentityConsumer::new(plan.row_count)?;
        for native in mutations {
            dispatch(
                lease,
                &native,
                deletion_reply(native.local()),
                prepared,
                &mut identities,
                &mut ledger,
                false,
                &authority,
                &mut |_: &AnswerItem| {},
            )
            .await
            .map_err(|later| identities.take_deferred().unwrap_or(later))?;
        }
        identities.finish(plan)?;
        prepared.check_control()?;
        Ok(ProjectedBatchResult::Deleted)
    }
}

fn record_static_answer(
    program: &mut crate::plan_builders::bulk_write::WriteProgram,
    step: &crate::plan_builders::bulk_write::WriteStep,
    recorded: Vec<AnswerItem>,
    required: bool,
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    if !required {
        return Ok(());
    }
    let rows = !matches!(step.plan.local().pipeline().last(), Some(Stage::Fetch(_)));
    let values = recorded
        .into_iter()
        .map(|item| match (rows, item) {
            (true, AnswerItem::Row(value)) | (false, AnswerItem::Document(value)) => Ok(value),
            _ => Err(SdkExecutionDiagnostic::internal_failure()),
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let result = if rows {
        QueryResult::Rows(values)
    } else {
        QueryResult::Documents(values)
    };
    let answer = TransactionContext::resolve_plan(step.plan.local(), result)
        .map_err(|error| lower_orm_error(&error, SdkProviderOperation::Write))?;
    program.record_answer(step, &answer).map_err(diagnostic)
}

fn input_row_upper_bound(row: &[GivenValue]) -> usize {
    row.iter().fold(2_usize, |total, value| {
        let text = match value {
            GivenValue::String(value)
            | GivenValue::Date(value)
            | GivenValue::Datetime(value)
            | GivenValue::DatetimeTz(value)
            | GivenValue::Decimal(value) => value.len(),
            GivenValue::DatetimeTzExact {
                local, named_zone, ..
            } => local
                .len()
                .saturating_add(named_zone.as_ref().map_or(0, String::len)),
            _ => 0,
        };
        total
            .saturating_add(512)
            .saturating_add(text.saturating_mul(6))
    })
}

#[cfg(test)]
fn input_upper_bound(rows: &GivenRowsSpec) -> usize {
    rows.rows.iter().fold(2_usize, |total, row| {
        total.saturating_add(input_row_upper_bound(row))
    })
}

fn input_chunks(
    rows: GivenRowsSpec,
    lookup: bool,
) -> std::result::Result<Vec<GivenRowsSpec>, SdkExecutionDiagnostic> {
    input_chunks_reserved(rows, lookup, 0)
}

fn input_chunks_reserved(
    rows: GivenRowsSpec,
    lookup: bool,
    reserved_bytes_per_row: usize,
) -> std::result::Result<Vec<GivenRowsSpec>, SdkExecutionDiagnostic> {
    let GivenRowsSpec { variables, rows } = rows;
    crate::plan_builders::input::chunks(
        rows,
        reserved_bytes_per_row,
        |row| input_row_upper_bound(row),
        |row| {
            let canonical = type_bridge_contract::query_plan::InputRow::new(
                row.iter()
                    .cloned()
                    .map(build::canonical)
                    .collect::<std::result::Result<_, _>>()
                    .map_err(diagnostic)?,
            );
            usize::try_from(given_rows_bytes(&canonical)?).map_err(|_| complexity_overflow())
        },
        |previous, row| !lookup && previous.first() == row.first(),
    )
    .map(|chunks| {
        chunks
            .into_iter()
            .map(|rows| GivenRowsSpec {
                variables: variables.clone(),
                rows,
            })
            .collect()
    })
}

fn lower_deleted_rows(
    registry: &crate::_registry::DescriptorRegistry,
    kind: TypeKind,
    things: &[(usize, &crate::match_request::HydratedThing)],
) -> crate::Result<Vec<(usize, DeletedRow)>> {
    things
        .iter()
        .map(|(index, thing)| {
            let row = match kind {
                TypeKind::Entity => crate::_manager::dynamic::plan_entity_row(registry, thing)
                    .map(DeletedRow::Entity),
                TypeKind::Relation => crate::_manager::dynamic::plan_relation_row(registry, thing)
                    .map(DeletedRow::Relation),
                _ => Err(crate::plan_builders::manager::invalid(
                    "manager_result_shape",
                    "Deletion requires an entity or relation",
                )),
            }?;
            Ok((*index, row))
        })
        .collect()
}

enum DeletedRow {
    Entity(DynamicEntityRow),
    Relation(DynamicRelationRow),
}

#[derive(Clone, Copy)]
enum Reply {
    ReferenceResolution,
    Mutation,
    MatchedDeletion,
    Hydration,
}

struct ReferenceOrder<'plan, 'schema> {
    references: Vec<&'plan RoleReference<'schema>>,
}

struct Consumer<'a, R> {
    record: &'a mut R,
    inner: &'a mut dyn AnswerConsumer,
    plan: &'a type_bridge_contract::query_plan::v3::QueryPlanV3,
    reply: Reply,
    deferred: Option<SdkExecutionDiagnostic>,
}

fn evidence(error: Diagnostic) -> OrmError {
    crate::match_request::MatchError::new(
        crate::match_request::MatchErrorCategory::ResultDecode,
        error.code().as_str(),
        "Native batch returned invalid typed evidence",
    )
    .into()
}
impl<R: FnMut(&AnswerItem) + Send> Consumer<'_, R> {
    fn decode(&mut self, item: AnswerItem) -> crate::Result<AnswerControl> {
        let document = match self.reply {
            Reply::MatchedDeletion => {
                if !matches!(item, AnswerItem::Row(_)) {
                    return Err(crate::match_request::MatchError::new(
                        crate::match_request::MatchErrorCategory::ResultDecode,
                        "query_plan_batch_answer",
                        "Native matched deletion returned documents",
                    )
                    .into());
                }
                return Ok(AnswerControl::Continue);
            }
            Reply::Hydration => {
                let AnswerItem::Document(document) = item else {
                    return Err(crate::match_request::MatchError::new(
                        crate::match_request::MatchErrorCategory::ResultDecode,
                        "query_plan_batch_answer",
                        "Native batch fetch returned rows",
                    )
                    .into());
                };
                let fields =
                    query_plan_exec::remote_document(&document, self.plan).map_err(evidence)?;
                let mut document = query_plan_exec::document_json(&fields).map_err(evidence)?;
                let fetched = self
                    .plan
                    .pipeline()
                    .iter()
                    .find_map(|stage| {
                        if let Stage::Fetch(document) = stage {
                            Some(document)
                        } else {
                            None
                        }
                    })
                    .expect("fetch shape");
                let mut players = Vec::new();
                for (index, role) in fetched.roles.iter().enumerate() {
                    // Batch fetches leave only ordered roles uncollected.
                    if !role.collect {
                        let object = document.as_object_mut().expect("validated object");
                        let iid = object.remove(&format!("_role_{index}_iid"));
                        object.remove(&format!("_role_{index}_type"));
                        object.remove(&format!("_role_{index}_attributes"));
                        if iid.is_some_and(|value| !value.is_null()) {
                            return Err(crate::match_request::MatchError::new(
                                crate::match_request::MatchErrorCategory::ResultDecode,
                                "query_plan_hydration_ordered_role",
                                "populated ordered roles require provider fetch ordinals",
                            )
                            .into());
                        }
                        continue;
                    }
                    let list = document
                        .as_object_mut()
                        .expect("validated object")
                        .remove(&format!("_role_{index}_players"))
                        .expect("validated role list");
                    let Value::Array(list) = list else {
                        unreachable!("validated list")
                    };
                    for player in list {
                        players.push(serde_json::json!({"role": format!("{}:{}", role.role.declaring_relation(), role.role.label()), "iid": player["_iid"], "type_name": player["_type"], "attributes": player["attributes"]}));
                    }
                }
                if !fetched.roles.is_empty() {
                    document["role_players"] = players.into();
                }
                document
            }
            Reply::Mutation | Reply::ReferenceResolution => {
                let AnswerItem::Row(row) = item else {
                    return Err(crate::match_request::MatchError::new(
                        crate::match_request::MatchErrorCategory::ResultDecode,
                        "query_plan_batch_answer",
                        "Native batch scalar output returned documents",
                    )
                    .into());
                };
                let type_bridge_contract::query_plan::QueryOutput::Rows { columns } =
                    self.plan.output()
                else {
                    unreachable!("row output")
                };
                if matches!(self.reply, Reply::ReferenceResolution) {
                    if columns.len() % 2 != 0 {
                        return Err(OrmError::ExecutionDiagnostic(Box::new(
                            SdkExecutionDiagnostic::internal_failure(),
                        )));
                    }
                    for pair in columns.chunks_exact(2) {
                        let value =
                            |column: type_bridge_contract::migration_assertion::BindingId| {
                                let name = self.plan.bindings()[usize::from(column.get())]
                                    .variable()
                                    .as_str();
                                remote_value(&row, name, self.plan.limits().codec).map_err(evidence)
                            };
                        let RemoteValue::Value {
                            value: type_bridge_contract::value::CanonicalValue::Long(ordinal),
                        } = value(pair[0])?
                        else {
                            return Err(OrmError::ExecutionDiagnostic(Box::new(
                                SdkExecutionDiagnostic::internal_failure(),
                            )));
                        };
                        match value(pair[1])? {
                            RemoteValue::Absent => {}
                            RemoteValue::Thing { iid, type_id } => {
                                if !self
                                    .plan
                                    .binding_types(pair[1])
                                    .is_some_and(|types| types.contains(&type_id))
                                {
                                    return Err(OrmError::ExecutionDiagnostic(Box::new(
                                        integrity_at_type(
                                            "provider_identity_mismatch",
                                            "Reference lookup returned a type outside its declared candidates",
                                            &type_id,
                                        ),
                                    )));
                                }
                                if self.inner.accept(AnswerItem::Document(
                                    serde_json::json!({"ordinal":ordinal,"iid":iid}),
                                ))? == AnswerControl::Stop
                                {
                                    return Ok(AnswerControl::Stop);
                                }
                            }
                            _ => {
                                return Err(OrmError::ExecutionDiagnostic(Box::new(
                                    SdkExecutionDiagnostic::internal_failure(),
                                )));
                            }
                        }
                    }
                    return Ok(AnswerControl::Continue);
                }
                let names = &["ordinal", "iid"];
                let mut document = serde_json::Map::new();
                for (column, key) in columns.iter().zip(names) {
                    let name = self.plan.bindings()[column.get() as usize]
                        .variable()
                        .as_str();
                    let value =
                        remote_value(&row, name, self.plan.limits().codec).map_err(evidence)?;
                    match value {
                        RemoteValue::Thing { iid, type_id } => {
                            if matches!(self.reply, Reply::ReferenceResolution)
                                && !self
                                    .plan
                                    .binding_types(*column)
                                    .is_some_and(|types| types.contains(&type_id))
                            {
                                return Err(OrmError::ExecutionDiagnostic(Box::new(
                                    integrity_at_type(
                                        "provider_identity_mismatch",
                                        "Reference lookup returned a type outside its declared candidates",
                                        &type_id,
                                    ),
                                )));
                            }
                            document.insert("iid".into(), iid.into());
                        }
                        RemoteValue::Absent
                            if matches!(self.reply, Reply::Mutation) && *key == "iid" => {}
                        RemoteValue::Value { value } | RemoteValue::Attribute { value, .. } => {
                            document.insert(
                                (*key).into(),
                                query_plan_exec::scalar_json(&value).map_err(evidence)?,
                            );
                        }
                        _ => {
                            return Err(crate::match_request::MatchError::new(
                                crate::match_request::MatchErrorCategory::ResultDecode,
                                "query_plan_batch_answer",
                                "Native batch scalar output has an invalid concept",
                            )
                            .into());
                        }
                    }
                }
                document.into()
            }
        };
        self.inner.accept(AnswerItem::Document(document))
    }
}

impl<R: FnMut(&AnswerItem) + Send> AnswerConsumer for Consumer<'_, R> {
    fn accept(&mut self, item: AnswerItem) -> crate::Result<AnswerControl> {
        if self.deferred.is_some() {
            return Ok(AnswerControl::Continue);
        }
        (self.record)(&item);
        match self.decode(item) {
            Ok(control) => Ok(control),
            Err(error) => {
                self.deferred = Some(lower_orm_error(&error, SdkProviderOperation::Read));
                Ok(AnswerControl::Continue)
            }
        }
    }
}

fn accept_hydration_answer(
    plan: &type_bridge_contract::query_plan::v3::QueryPlanV3,
    answer: crate::query_plan_exec::PlanAnswer,
    inner: &mut dyn AnswerConsumer,
) -> Result<(), SdkExecutionDiagnostic> {
    accept_native_answer(plan, answer, Reply::Hydration, inner)
}

fn accept_native_answer(
    plan: &type_bridge_contract::query_plan::v3::QueryPlanV3,
    answer: crate::query_plan_exec::PlanAnswer,
    reply: Reply,
    inner: &mut dyn AnswerConsumer,
) -> Result<(), SdkExecutionDiagnostic> {
    let mut consumer = Consumer {
        record: &mut |_: &AnswerItem| {},
        inner,
        plan,
        reply,
        deferred: None,
    };
    match (reply, answer) {
        (Reply::MatchedDeletion, crate::query_plan_exec::PlanAnswer::Matched(_))
            if matched_delete(plan) => {}
        (Reply::Hydration, crate::query_plan_exec::PlanAnswer::Documents(documents)) => {
            for document in documents {
                consumer
                    .decode(AnswerItem::Document(document))
                    .map_err(|error| lower_orm_error(&error, SdkProviderOperation::Read))?;
            }
        }
        (
            Reply::Mutation | Reply::ReferenceResolution,
            crate::query_plan_exec::PlanAnswer::Rows(rows),
        ) => {
            for row in rows {
                consumer
                    .decode(AnswerItem::Row(row))
                    .map_err(|error| lower_orm_error(&error, SdkProviderOperation::Write))?;
            }
        }
        _ => return Err(SdkExecutionDiagnostic::internal_failure()),
    }
    Ok(())
}

fn deletion_reply(plan: &type_bridge_contract::query_plan::v3::QueryPlanV3) -> Reply {
    if matched_delete(plan) {
        Reply::MatchedDeletion
    } else {
        Reply::Mutation
    }
}

fn matched_delete(plan: &type_bridge_contract::query_plan::v3::QueryPlanV3) -> bool {
    matches!(
        plan.operation(),
        type_bridge_contract::query_plan::v3::Operation::Write {
            returns: type_bridge_contract::query_plan::v3::WriteReturns::Matched,
        }
    )
}

#[allow(clippy::too_many_arguments)]
async fn dispatch<R: FnMut(&AnswerItem) + Send>(
    lease: &mut TransactionContextMutationLease,
    plan: &BuiltPlan,
    reply: Reply,
    prepared: &PreparedProjectedBatchInvocation<'_>,
    consumer: &mut dyn AnswerConsumer,
    ledger: &mut ExecutionLedger,
    lookup: bool,
    authority: &crate::plan_builders::authority::PlanAuthority,
    record: &mut R,
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    let mut query = lease
        .prepare_plan(plan.local())
        .map_err(|error| lower_orm_error(&error, SdkProviderOperation::Write))?;
    preflight_statement_complexity(&query.typeql)?;
    let Some(rows) = query.rows.take() else {
        ledger.charge_bytes(
            source_bytes(&query.typeql)?,
            "projected_batch_transport_byte_limit",
        )?;
        return dispatch_query(
            lease,
            plan.local(),
            query,
            reply,
            prepared,
            consumer,
            ledger,
            record,
        )
        .await;
    };
    ledger.charge_bytes(
        checked_add_u64(source_bytes(&query.typeql)?, given_rows_bytes(&rows)?)?,
        "projected_batch_transport_byte_limit",
    )?;
    let chunks = if lookup {
        split_lookups(&query.typeql, rows, SplitLimits::CANONICAL)?
    } else {
        split_stage(&query.typeql, rows, SplitLimits::CANONICAL)?
    };
    if chunks.len() == 1 {
        query.rows = chunks.into_iter().next();
        return dispatch_query(
            lease,
            plan.local(),
            query,
            reply,
            prepared,
            consumer,
            ledger,
            record,
        )
        .await;
    }
    for chunk in chunks {
        let rows = chunk
            .rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(build::canonical)
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map(type_bridge_contract::query_plan::InputRow::new)
            })
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(diagnostic)?;
        let local = plan
            .local()
            .with_given_rows(rows, authority.declared())
            .map_err(diagnostic)?;
        let query = lease
            .prepare_plan(&local)
            .map_err(|error| lower_orm_error(&error, SdkProviderOperation::Write))?;
        dispatch_query(
            lease, &local, query, reply, prepared, consumer, ledger, record,
        )
        .await?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_query<R: FnMut(&AnswerItem) + Send>(
    lease: &mut TransactionContextMutationLease,
    plan: &type_bridge_contract::query_plan::v3::QueryPlanV3,
    query: crate::session::staged::StagedQuery,
    reply: Reply,
    prepared: &PreparedProjectedBatchInvocation<'_>,
    consumer: &mut dyn AnswerConsumer,
    ledger: &mut ExecutionLedger,
    record: &mut R,
) -> std::result::Result<(), SdkExecutionDiagnostic> {
    prepared.check_control()?;
    let limits = AnswerLimits {
        answer: BoundedAnswerLimits {
            max_items: ledger.remaining_items(),
            max_bytes: ledger.remaining_bytes(),
            deadline: prepared.deadline().instant(),
            cancellation: prepared.cancellation().clone(),
        },
        max_collection_members: prepared.limits().collection_members,
        ..Default::default()
    };
    let mut consumer = Consumer {
        record,
        inner: consumer,
        plan,
        reply,
        deferred: None,
    };
    let mut observed = LocallyBoundedConsumer::new(&mut consumer, limits.answer.clone());
    let future = lease.dispatch_plan_bounded(query, limits, &mut observed);
    let dispatched = await_controlled(future, prepared.deadline(), prepared.cancellation()).await;
    let measured = observed.stats();
    drop(observed);
    if let Some(first) = consumer.deferred {
        return Err(first);
    }
    let mut stats = dispatched.map_err(|error| {
        controlled_orm(
            error,
            if plan.writes() {
                SdkProviderOperation::Write
            } else {
                SdkProviderOperation::Read
            },
            prepared.batch().model(),
        )
    })?;
    stats.processed_items = stats.processed_items.max(measured.processed_items);
    stats.response_bytes = stats.response_bytes.max(measured.response_bytes);
    stats.stopped_early |= measured.stopped_early;
    ledger.record_reply(stats)?;
    Ok(())
}

#[cfg(test)]
mod native_input_tests {
    use super::*;
    use type_bridge_contract::limits::{MAX_CANONICAL_STRING_BYTES, MAX_INPUT_BYTES};

    #[test]
    fn ordered_deletes_preserve_frozen_plans_and_matched_answers_in_all_projections() {
        use type_bridge_contract::fingerprint::SemanticProfileId;
        use type_bridge_contract::projection::{
            BindingTarget, CSymbolPrefix, ProjectionConfig, ProjectionHandler,
        };
        use type_bridge_contract::schema::DocumentId;
        use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
        let source = include_str!("../../../../tests/contracts/sdk_conformance/models/schema.yaml");
        let declared = normalize_documents(
            &SchemaDocumentSet::parse([(DocumentId::new("models.yaml").unwrap(), source)]).unwrap(),
        )
        .unwrap();
        for profile in ["typedb-3.12.1/v1", "typedb-3.13.6/v1"] {
            let resolved = resolve(&declared, &SemanticProfileId::new(profile).unwrap()).unwrap();
            let authority = type_bridge_schema::build_schema_authority(
                &declared,
                declared.required_capabilities(),
                &type_bridge_schema::ManagedDeltaContext::new(
                    type_bridge_contract::managed_scope::ManagedScopeId::new(
                        "schema-codegen-acceptance",
                    )
                    .unwrap(),
                    SemanticProfileId::new(profile).unwrap(),
                    type_bridge_schema::schema_authority_capability_vocabulary(),
                ),
            )
            .unwrap();
            for (target, config, handler) in [
                (
                    BindingTarget::Python,
                    ProjectionConfig::python(),
                    ProjectionHandler::python_v2(),
                ),
                (
                    BindingTarget::TypeScript,
                    ProjectionConfig::typescript(),
                    ProjectionHandler::typescript_v2(),
                ),
                (
                    BindingTarget::Rust,
                    ProjectionConfig::rust(),
                    ProjectionHandler::rust_v2(),
                ),
                (
                    BindingTarget::C,
                    ProjectionConfig::c(CSymbolPrefix::new("ordered_delete").unwrap()),
                    ProjectionHandler::c_v2(),
                ),
            ] {
                let installed = InstalledRuntimeProjection::try_new(
                    project(&resolved, target, &config, &[handler], &[]).unwrap(),
                )
                .unwrap()
                .with_schema_authority(&authority);
                for (kind, label, scenario, record) in [
                    (TypeKind::Entity, "person", "entity_delete", "data-ada"),
                    (
                        TypeKind::Relation,
                        "network-link",
                        "role_delete",
                        "link-forward",
                    ),
                ] {
                    for count in [1, 2] {
                        let batch = ProjectedBatch::try_new(
                            &installed,
                            TypeId::new(kind, label).unwrap(),
                            ProjectedBatchOperation::Delete,
                            ["0x10", "0x11"]
                                .into_iter()
                                .take(count)
                                .map(|iid| ProjectedBatchRow::Delete { iid: iid.into() })
                                .collect(),
                        )
                        .unwrap();
                        let prepared = PreparedProjectedBatchInvocation::try_new(
                            &installed,
                            &batch,
                            ProjectedBatchInvocationControl::capture(
                                Default::default(),
                                Default::default(),
                            ),
                        )
                        .unwrap();
                        let identity = DatabaseExecutionIdentity::isolated("ordered-delete");
                        let plan =
                            BatchPlan::compile(&installed, &prepared, &identity, false, None)
                                .unwrap();
                        let native = plan.native_delete(plan.delete_rows().unwrap()).unwrap();
                        assert!(matched_delete(native.local()));
                        if count == 1 {
                            assert!(native.local().given_rows().is_none());
                            let mut actual: Value =
                                serde_json::from_slice(&native.local().canonical_bytes().unwrap())
                                    .unwrap();
                            // The twin normalizer only renames this one binding and recorded IID.
                            actual["bindings"][0]["variable"] = serde_json::json!("b0");
                            actual["pipeline"][0]["patterns"][1]["iid"] =
                                serde_json::json!(format!("@record/{record}"));
                            let path = format!(
                                "{}/../../../tests/contracts/sdk_conformance/models/plans/{}/ordered_distinct_collections.{scenario}.json",
                                env!("CARGO_MANIFEST_DIR"),
                                profile.replace('/', "_")
                            );
                            let expected: Value =
                                serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
                            assert_eq!(
                                serde_json::json!([{ "plan": actual, "rows": [] }]),
                                expected
                            );
                        } else {
                            // The native many-delete uses IID inputs without batch ordinals.
                            assert_eq!(native.local().given_rows().unwrap().len(), 2);
                            assert_eq!(native.local().inputs().len(), 1);
                            assert_eq!(native.local().pipeline().len(), 2);
                        }
                        let mut identities = DeleteIdentityConsumer::new(count).unwrap();
                        for matched in [0, count as u64] {
                            accept_native_answer(
                                native.local(),
                                query_plan_exec::PlanAnswer::Matched(matched),
                                Reply::MatchedDeletion,
                                &mut identities,
                            )
                            .unwrap();
                        }
                        identities.finish(&plan).unwrap();
                        for wrong in [
                            query_plan_exec::PlanAnswer::Rows(vec![]),
                            query_plan_exec::PlanAnswer::Documents(vec![]),
                        ] {
                            assert!(
                                accept_native_answer(
                                    native.local(),
                                    wrong,
                                    Reply::MatchedDeletion,
                                    &mut identities
                                )
                                .is_err()
                            );
                        }
                        let mut consumer = Consumer {
                            record: &mut |_: &AnswerItem| {},
                            inner: &mut identities,
                            plan: native.local(),
                            reply: Reply::MatchedDeletion,
                            deferred: None,
                        };
                        assert_eq!(
                            consumer
                                .decode(AnswerItem::Row(serde_json::json!({})))
                                .unwrap(),
                            AnswerControl::Continue
                        );
                        assert!(
                            consumer
                                .decode(AnswerItem::Document(serde_json::json!({})))
                                .is_err()
                        );
                    }
                }
            }
        }
    }

    fn rows(values: Vec<(i64, String)>) -> GivenRowsSpec {
        GivenRowsSpec {
            variables: vec!["ordinal".into(), "field".into()],
            rows: values
                .into_iter()
                .map(|(ordinal, value)| {
                    vec![GivenValue::Integer(ordinal), GivenValue::String(value)]
                })
                .collect(),
        }
    }
    fn exact(rows: &GivenRowsSpec) -> usize {
        let canonical = rows
            .rows
            .iter()
            .map(|row| {
                type_bridge_contract::query_plan::InputRow::new(
                    row.iter()
                        .cloned()
                        .map(build::canonical)
                        .collect::<std::result::Result<_, _>>()
                        .unwrap(),
                )
            })
            .collect::<Vec<_>>();
        given_rows_bytes(&canonical).unwrap() as usize
    }

    #[test]
    fn streamed_static_source_keeps_its_actual_provider_type() {
        use crate::plan_builders::authority::PlanAuthority;
        use crate::plan_builders::bulk_write::{WriteKind, WriteRow, WriteShape, write_many};
        use type_bridge_contract::fingerprint::SemanticProfileId;
        use type_bridge_contract::id::AttributeId;
        use type_bridge_contract::schema_fingerprint::ManagedSemanticSchemaFingerprint;
        use type_bridge_contract::value::CanonicalValue;

        let account = TypeId::new(TypeKind::Entity, "account").unwrap();
        let balance =
            OwnsFactId::new(account.clone(), AttributeId::new("balance").unwrap()).unwrap();
        let fields = (0..300)
            .map(|value| (balance.clone(), CanonicalValue::Long(value)))
            .collect::<Vec<_>>();
        let authority = PlanAuthority::new(
            crate::query_plan_exec::race::schema(),
            ManagedSemanticSchemaFingerprint::compute(
                SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
                b"streamed-static-source",
            )
            .unwrap(),
        );
        for (single_input, roots) in [(false, 1), (true, 1), (true, 2)] {
            for actual_type in ["account", "wrong-provider-type"] {
                let mut program = write_many(
                    &authority,
                    WriteKind::Insert,
                    WriteShape {
                        type_id: &account,
                        keys: &[],
                        cleared_fields: &[],
                        cleared_roles: &[],
                        document_roles: vec![],
                        include_subtypes: false,
                        return_documents: false,
                        references_see_prior_writes: false,
                        single_input,
                    },
                    &[WriteRow {
                        iid: None,
                        fields: &fields,
                        players: &[],
                    }],
                )
                .unwrap()
                .prepare_static()
                .unwrap();
                assert!(program.records_answers());
                let step = program.next().unwrap().unwrap();
                let type_bridge_contract::query_plan::QueryOutput::Rows { columns } =
                    step.plan.local().output()
                else {
                    panic!("allocation answers ordinal and root")
                };
                let mut row = serde_json::Map::new();
                row.insert(
                    step.plan.local().bindings()[usize::from(columns[0].get())]
                        .variable()
                        .as_str()
                        .into(),
                    serde_json::json!({"kind":"value", "value":CanonicalValue::Long(0)}),
                );
                row.insert(step.plan.local().bindings()[usize::from(columns[1].get())].variable().as_str().into(), serde_json::json!({"kind":"thing", "iid":"0x10", "type_id":{"kind":"entity", "label":actual_type}}));
                let mut answers = vec![AnswerItem::Row(row.clone().into())];
                if roots == 2 {
                    row.insert(step.plan.local().bindings()[usize::from(columns[1].get())].variable().as_str().into(), serde_json::json!({"kind":"thing", "iid":"0x20", "type_id":{"kind":"entity", "label":actual_type}}));
                    answers.push(AnswerItem::Row(row.into()));
                }
                record_static_answer(&mut program, &step, answers, true).unwrap();
                program.accept_identity(&step, Some("0x10".into())).unwrap();
                let next = program.next();
                if roots == 2 {
                    assert_eq!(
                        next.err().unwrap().code().as_str(),
                        "query_stage_result_ref_rows"
                    );
                } else if actual_type == "account" {
                    let next = next.unwrap().unwrap();
                    next.check_identity("0x10").unwrap();
                    assert!(next.check_identity("0xff").is_err());
                } else {
                    assert_eq!(
                        next.err().unwrap().code().as_str(),
                        "query_stage_result_ref_answer"
                    );
                }
            }
        }
        let mut targets = write_many(
            &authority,
            WriteKind::Attach,
            WriteShape {
                type_id: &account,
                keys: &[],
                cleared_fields: &[],
                cleared_roles: &[],
                document_roles: vec![],
                include_subtypes: false,
                return_documents: false,
                references_see_prior_writes: false,
                single_input: false,
            },
            &["0x10", "0x20"].map(|iid| WriteRow {
                iid: Some(iid),
                fields: &[],
                players: &[],
            }),
        )
        .unwrap();
        let step = targets.next().unwrap().unwrap();
        step.check_identity("0x10").unwrap();
        step.check_identity("0x20").unwrap();
        assert_eq!(
            step.check_identity("0xff").unwrap_err().code().as_str(),
            "provider_identity_mismatch"
        );
    }

    #[test]
    fn canonical_input_boundary_uses_a_conservative_fast_guard_and_exact_slow_path() {
        let just_under = rows(
            (0..4)
                .map(|ordinal| (ordinal, "x".repeat(MAX_CANONICAL_STRING_BYTES - 128)))
                .collect(),
        );
        let size = exact(&just_under);
        assert!(
            size < MAX_INPUT_BYTES && size > MAX_INPUT_BYTES - 1024,
            "{size}"
        );
        assert!(input_upper_bound(&just_under) > MAX_INPUT_BYTES);
        assert_eq!(input_chunks(just_under, false).unwrap().len(), 1);

        let escaped = rows(vec![(0, "\0".repeat((MAX_INPUT_BYTES - 1030) / 6))]);
        let size = exact(&escaped);
        assert!(
            size > MAX_INPUT_BYTES - 2048 && size < MAX_INPUT_BYTES,
            "{size}"
        );
        assert!(input_upper_bound(&escaped) >= size);
        assert!(input_upper_bound(&escaped) <= MAX_INPUT_BYTES);
        assert_eq!(input_chunks(escaped, false).unwrap().len(), 1);
    }

    #[test]
    fn attach_input_reserves_target_iids_on_the_exact_size_path() {
        let empty = rows(
            (0..4)
                .map(|ordinal| (ordinal, "x".repeat(MAX_CANONICAL_STRING_BYTES - 128)))
                .collect(),
        );
        let mut attach = GivenRowsSpec {
            variables: vec!["ordinal".into(), "target".into(), "field".into()],
            rows: empty
                .rows
                .into_iter()
                .map(|mut row| {
                    row.insert(1, GivenValue::String(String::new()));
                    row
                })
                .collect(),
        };
        assert!(exact(&attach) < MAX_INPUT_BYTES);
        assert!(exact(&attach) + 4 * MAX_PROVIDER_IID_BYTES as usize > MAX_INPUT_BYTES);
        let chunks =
            input_chunks_reserved(attach.clone(), false, MAX_PROVIDER_IID_BYTES as usize).unwrap();
        assert_eq!(chunks.len(), 2);
        let iid = format!("0x{}", "a".repeat(MAX_PROVIDER_IID_BYTES as usize - 2));
        for mut chunk in chunks {
            for row in &mut chunk.rows {
                row[1] = GivenValue::String(iid.clone());
            }
            assert!(exact(&chunk) <= MAX_INPUT_BYTES);
        }
        // A complete ordinal remains atomic even when its later identities grow.
        for row in &mut attach.rows {
            row[0] = GivenValue::Integer(0);
        }
        assert_eq!(
            input_chunks_reserved(attach, false, MAX_PROVIDER_IID_BYTES as usize)
                .unwrap_err()
                .code()
                .as_str(),
            "projected_batch_input_unit_byte_limit"
        );
    }

    #[test]
    fn canonical_input_split_preserves_whole_ordinals_and_refuses_an_oversized_unit() {
        let text = "x".repeat(MAX_CANONICAL_STRING_BYTES - 128);
        let split = input_chunks(
            rows((0..5).map(|ordinal| (ordinal, text.clone())).collect()),
            false,
        )
        .unwrap();
        assert_eq!(
            split.iter().map(|rows| rows.rows.len()).collect::<Vec<_>>(),
            [4, 1]
        );
        assert!(split.iter().all(|rows| exact(rows) <= MAX_INPUT_BYTES));
        let error =
            input_chunks(rows((0..5).map(|_| (0, text.clone())).collect()), false).unwrap_err();
        assert_eq!(
            error.code().as_str(),
            "projected_batch_input_unit_byte_limit"
        );
    }
}
