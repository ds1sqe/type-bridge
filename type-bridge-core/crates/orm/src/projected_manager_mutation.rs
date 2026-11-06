//! Filter mutations retain the transaction lease through answer validation.

use super::*;
use crate::_dynamic::{DynamicComparisonOp, DynamicExpr};
use crate::execution_diagnostic::lower_orm_error;
use crate::plan_builders::manager::{ManagerPlans, ManagerSelection};
use crate::projected_batch_executor::ExecutionLedger;
use crate::query_plan_exec::PlanAnswer;
use crate::session::staged::bounded;
use crate::{QueryTarget, QueryTargetError, TxType};

impl ProjectedManagerFilter {
    fn require_exact_mutation(&self) -> Result<(), SdkExecutionDiagnostic> {
        if self.match_mode != MatchMode::Exact {
            return Err(invalid_input(
                "manager_filter_write_requires_exact_model",
                "Filtered mutations require an exact model scope",
                type_path(self.model()),
            ));
        }
        Ok(())
    }

    pub(crate) fn append_write_predicates(
        &self,
        draft: &mut crate::plan_builders::PlanDraft,
        root: type_bridge_contract::migration_assertion::BindingId,
        patterns: &mut Vec<type_bridge_contract::query_plan::Pattern>,
    ) -> Result<(), type_bridge_contract::diagnostic::Diagnostic> {
        use type_bridge_contract::migration_assertion::ValueComparator;
        use type_bridge_contract::query_plan::Pattern;
        use type_bridge_contract::query_plan::{QueryOperand, QueryPattern};
        for predicate in self.predicates.iter() {
            match predicate {
                ProjectedManagerPredicate::Iid(iid) => patterns.push(Pattern::Iid {
                    binding: root,
                    iid: iid.as_str().to_owned(),
                }),
                ProjectedManagerPredicate::Iids(iids) => {
                    if let [iid] = iids.as_ref() {
                        patterns.push(Pattern::Iid {
                            binding: root,
                            iid: iid.as_str().to_owned(),
                        });
                    } else {
                        patterns.push(QueryPattern::Or { branches: iids.iter().map(|iid| Ok(vec![QueryPattern::Iid {
                            binding: root,
                            iid: QueryOperand::Literal { value: CanonicalValue::String(type_bridge_contract::value::CanonicalString::new(iid.as_str())?) },
                        }])).collect::<Result<_, type_bridge_contract::diagnostic::Diagnostic>>()? }.into());
                    }
                }
                ProjectedManagerPredicate::Field(field) => {
                    let attribute = draft.bind("field")?;
                    patterns.push(
                        QueryPattern::Has {
                            owner: root,
                            attribute,
                            attribute_id: field.field.attribute().clone(),
                        }
                        .into(),
                    );
                    patterns.push(
                        QueryPattern::Value {
                            comparator: match field.comparison {
                                ProjectedManagerComparison::Eq => ValueComparator::Equal,
                                ProjectedManagerComparison::Ne => ValueComparator::NotEqual,
                                ProjectedManagerComparison::Lt => ValueComparator::Less,
                                ProjectedManagerComparison::Lte => ValueComparator::LessOrEqual,
                                ProjectedManagerComparison::Gt => ValueComparator::Greater,
                                ProjectedManagerComparison::Gte => ValueComparator::GreaterOrEqual,
                            },
                            left: QueryOperand::Binding { binding: attribute },
                            right: draft.value(field.value.value().clone())?,
                        }
                        .into(),
                    );
                }
            }
        }
        Ok(())
    }

    fn mutation_expressions(&self) -> Vec<DynamicExpr> {
        self.predicates
            .iter()
            .map(|predicate| match predicate {
                ProjectedManagerPredicate::Field(field) => DynamicExpr::Compare {
                    attr_name: field.field.attribute().label().as_str().to_owned(),
                    operator: match field.comparison {
                        ProjectedManagerComparison::Eq => DynamicComparisonOp::Eq,
                        ProjectedManagerComparison::Ne => DynamicComparisonOp::Neq,
                        ProjectedManagerComparison::Lt => DynamicComparisonOp::Lt,
                        ProjectedManagerComparison::Lte => DynamicComparisonOp::Lte,
                        ProjectedManagerComparison::Gt => DynamicComparisonOp::Gt,
                        ProjectedManagerComparison::Gte => DynamicComparisonOp::Gte,
                    },
                    value: field.value.to_attribute_value(),
                },
                ProjectedManagerPredicate::Iid(iid) => DynamicExpr::Iid {
                    iid: iid.as_str().to_owned(),
                },
                ProjectedManagerPredicate::Iids(iids) => {
                    let iid = |iid: &Iid| DynamicExpr::Iid {
                        iid: iid.as_str().to_owned(),
                    };
                    if let [value] = iids.as_ref() {
                        iid(value)
                    } else {
                        DynamicExpr::Or {
                            exprs: iids.iter().map(iid).collect(),
                        }
                    }
                }
            })
            .collect()
    }
}

impl ProjectedManagerFilterExecutor<'_> {
    /// Delete every exact model matched by this filter in one transaction.
    /// Borrowed targets retain transaction ownership; owned targets commit once.
    #[doc(hidden)]
    pub async fn delete_on_target(
        &self,
        target: QueryTarget<'_>,
        filter: &ProjectedManagerFilter,
        control: ProjectedManagerFilterInvocationControl,
    ) -> Result<(), QueryTargetError> {
        self.delete_on_target_with_pre_execute(target, filter, control, || async { Ok(()) })
            .await
    }

    /// Run a binding callback after input preparation and before executing a delete.
    /// The callback value is returned only after the mutation succeeds.
    #[doc(hidden)]
    pub async fn delete_on_target_with_pre_execute<T, F>(
        &self,
        target: QueryTarget<'_>,
        filter: &ProjectedManagerFilter,
        control: ProjectedManagerFilterInvocationControl,
        before_execute: impl FnOnce() -> F,
    ) -> Result<T, QueryTargetError>
    where
        F: std::future::Future<Output = Result<T, QueryTargetError>>,
    {
        let control = control.constrained_by(target.answer_limits());
        self.preflight(filter, &control, false)?;
        filter.require_exact_mutation()?;
        if let Some(iids) = filter
            .exact_iids()
            .or_else(|| filter.matches_none.then_some(&[]))
        {
            let input_control = crate::ProjectedBatchInvocationControl::from_parts(
                control.limits,
                control.deadline,
                control.cancellation.clone(),
            );
            let batch = if matches!(
                filter.predicates.as_ref(),
                [ProjectedManagerPredicate::Iid(_)]
            ) {
                crate::ProjectedBatch::try_single_delete(
                    self.installed,
                    filter.model().clone(),
                    iids[0].as_str().to_owned(),
                    &input_control,
                )?
            } else {
                crate::ProjectedBatch::try_new_for_invocation(
                    self.installed,
                    filter.model().clone(),
                    crate::ProjectedBatchOperation::Delete,
                    iids.iter()
                        .map(|iid| crate::ProjectedBatchRow::Delete {
                            iid: iid.as_str().to_owned(),
                        })
                        .collect(),
                    &input_control,
                )?
            };
            let state = before_execute().await?;
            crate::ProjectedBatchExecutor::new(self.installed)
                .execute_on_target_mapped(
                    target,
                    &batch,
                    input_control,
                    (),
                    |result| match result {
                        crate::ProjectedBatchResult::Deleted => Ok(()),
                        _ => Err(SdkExecutionDiagnostic::internal_failure()),
                    },
                )
                .await?;
            return Ok(state);
        }
        let state = before_execute().await?;
        let query_limits = target
            .manager_query_limits(control.deadline, &control.cancellation)
            .await?;
        let registry = self.installed.manager_registry()?;
        let plans = ManagerPlans::new(registry).with_query_limits(query_limits.as_ref());
        let expressions = filter.mutation_expressions();
        let plan = plans.delete_matching(
            filter.model().label().as_str(),
            ManagerSelection::Filter(&expressions),
            true,
            false,
        )?;
        self.execute_mutation(
            target,
            filter,
            &plan,
            &control,
            filter.resource_measure().bytes() as u64,
            |answer, _ledger| match answer {
                PlanAnswer::Matched(_) => Ok(()),
                _ => Err(SdkExecutionDiagnostic::internal_failure()),
            },
        )
        .await?;
        Ok(state)
    }

    /// Replace every exact model matched by this filter and return its new snapshot.
    /// Direct targets hydrate before commit or releasing the borrowed mutation lease.
    #[doc(hidden)]
    pub async fn update_on_target(
        &self,
        target: QueryTarget<'_>,
        filter: &ProjectedManagerFilter,
        replacement: crate::ProjectedCreate,
        control: ProjectedManagerFilterInvocationControl,
    ) -> Result<crate::WriteResult<crate::ProjectedThing>, QueryTargetError> {
        self.update_on_target_mapped(target, filter, replacement, control, Ok)
            .await
    }

    /// Materialize binding results before direct commit or releasing the write lease.
    /// A mapper failure after proxy commit retains the remote commit outcome.
    #[doc(hidden)]
    pub async fn update_on_target_mapped<T, F>(
        &self,
        target: QueryTarget<'_>,
        filter: &ProjectedManagerFilter,
        replacement: crate::ProjectedCreate,
        control: ProjectedManagerFilterInvocationControl,
        mapper: F,
    ) -> Result<T, QueryTargetError>
    where
        F: FnOnce(crate::WriteResult<crate::ProjectedThing>) -> Result<T, SdkExecutionDiagnostic>,
    {
        self.update_on_target_mapped_with_pre_execute(
            target,
            filter,
            replacement,
            control,
            || async { Ok(()) },
            mapper,
        )
        .await
        .map(|(output, ())| output)
    }

    /// Prepare and validate an update before invoking binding hooks.
    #[doc(hidden)]
    pub async fn update_on_target_mapped_with_pre_execute<T, S, F, H>(
        &self,
        target: QueryTarget<'_>,
        filter: &ProjectedManagerFilter,
        replacement: crate::ProjectedCreate,
        control: ProjectedManagerFilterInvocationControl,
        before_execute: impl FnOnce() -> H,
        mapper: F,
    ) -> Result<(T, S), QueryTargetError>
    where
        F: FnOnce(crate::WriteResult<crate::ProjectedThing>) -> Result<T, SdkExecutionDiagnostic>,
        H: std::future::Future<Output = Result<S, QueryTargetError>>,
    {
        let control = control.constrained_by(target.answer_limits());
        self.preflight(filter, &control, false)?;
        filter.require_exact_mutation()?;
        filter.require_boolean_predicates()?;
        let input_control = crate::ProjectedBatchInvocationControl::from_parts(
            control.limits,
            control.deadline,
            control.cancellation.clone(),
        );
        if let [ProjectedManagerPredicate::Iid(iid)] = filter.predicates.as_ref() {
            let executor = crate::ProjectedCrudExecutor::new(self.installed);
            let batch = executor.single_controlled_write_batch(
                filter.model(),
                crate::ProjectedBatchOperation::Update,
                crate::ProjectedBatchRow::Update {
                    iid: iid.as_str().to_owned(),
                    replacement,
                },
                &input_control,
            )?;
            let state = before_execute().await?;
            return executor
                .execute_single_write_on_target(target, &batch, input_control, mapper)
                .await
                .map(|output| (output, state));
        }
        crate::projected_batch::validate_filtered_replacement(
            self.installed,
            filter.model(),
            &replacement,
            &input_control,
        )?;
        let input_bytes = (filter.resource_measure().bytes() as u64)
            .saturating_add(replacement.resource_measure().bytes() as u64);
        let mut ledger = ExecutionLedger::new(control.limits, input_bytes);
        ledger.charge_preflight(0, 1)?;
        let identity = match target {
            QueryTarget::Database(database) => database.execution_identity(),
            QueryTarget::Transaction(transaction) => transaction.execution_identity().clone(),
            QueryTarget::Proxy(proxy) => proxy.execution_identity().clone(),
        };
        crate::projected_batch_executor::validate_provider_create_input(&replacement, 0)?;
        let executor = crate::ProjectedCrudExecutor::new(self.installed);
        let model = executor.prepare_write_model(filter.model(), &replacement, &identity)?;
        if filter.matches_none {
            let state = before_execute().await?;
            return mapper(crate::WriteResult::new(Vec::new()))
                .map(|output| (output, state))
                .map_err(Into::into);
        }
        let query_limits = target
            .manager_query_limits(control.deadline, &control.cancellation)
            .await?;
        let authority = self
            .installed
            .plan_authority_with_query_limits(query_limits.as_ref())
            .map_err(crate::plan_builders::manager::diagnostic)?;
        let prepared = executor.prepare_filtered_update(filter, model, &authority)?;
        let state = before_execute().await?;
        self.execute_mutation(
            target,
            filter,
            &prepared.plan,
            &control,
            input_bytes,
            |answer, ledger| {
                let PlanAnswer::Documents(documents) = answer else {
                    return Err(SdkExecutionDiagnostic::internal_failure());
                };
                let result = executor.hydrate_filtered_update(&prepared, &identity, &documents)?;
                for predicate in filter.predicates.iter() {
                    let iids = match predicate {
                        ProjectedManagerPredicate::Iid(iid) => std::slice::from_ref(iid),
                        ProjectedManagerPredicate::Iids(iids) => iids,
                        _ => continue,
                    };
                    {
                        for item in result.items() {
                            if !iids
                                .iter()
                                .any(|iid| item.iid().eq_ignore_ascii_case(iid.as_str()))
                            {
                                return Err(crate::projected_batch_executor::integrity_at_row(
                                    "projected_batch_update_identity_changed",
                                    "A projected update returned a different exact target IID",
                                    filter.model(),
                                    0,
                                ));
                            }
                        }
                    }
                }
                ledger.charge_projected(result.items())?;
                mapper(result)
            },
        )
        .await
        .map(|output| (output, state))
    }

    async fn execute_mutation<T, F>(
        &self,
        target: QueryTarget<'_>,
        filter: &ProjectedManagerFilter,
        plan: &crate::plan_builders::BuiltPlan,
        control: &ProjectedManagerFilterInvocationControl,
        input_bytes: u64,
        mapper: F,
    ) -> Result<T, QueryTargetError>
    where
        F: FnOnce(PlanAnswer, &mut ExecutionLedger) -> Result<T, SdkExecutionDiagnostic>,
    {
        control.checkpoint(filter.model())?;
        match target {
            QueryTarget::Proxy(proxy) => {
                let mut ledger = ExecutionLedger::new(control.limits, input_bytes);
                ledger.charge_preflight(0, 1)?;
                let authority = self
                    .installed
                    .plan_authority()
                    .map_err(crate::plan_builders::manager::diagnostic)?;
                let entry = type_bridge_contract::query_remote::StagedPlan::new(
                    plan.local(),
                    plan.local().given_rows().unwrap_or_default().to_vec(),
                    authority.declared(),
                )
                .map_err(crate::plan_builders::manager::diagnostic)?;
                proxy
                    .execute_write_stack_mapped(
                        vec![entry],
                        control.limits,
                        &control.cancellation,
                        control.deadline,
                        |mut answers| {
                            control.checkpoint(filter.model())?;
                            if answers.len() != 1 {
                                return Err(SdkExecutionDiagnostic::internal_failure());
                            }
                            mapper(
                                answers.pop().expect("one checked mutation answer"),
                                &mut ledger,
                            )
                        },
                    )
                    .await
                    .map_err(|error| QueryTargetError::Commit(Box::new(error)))
            }
            QueryTarget::Database(database) => {
                let limits = crate::query_plan_exec::plan_answer_limits(
                    control.limits,
                    &control.cancellation,
                    control.deadline,
                )?;
                let transaction =
                    bounded(&limits.answer, database.transaction_context(TxType::Write)).await?;
                let result =
                    Self::run_mutation(&transaction, filter, plan, control, input_bytes, mapper)
                        .await;
                match result {
                    Ok(value) => {
                        transaction.commit_sdk().await?;
                        Ok(value)
                    }
                    Err(error) => {
                        let _ = transaction.rollback().await;
                        Err(error)
                    }
                }
            }
            QueryTarget::Transaction(transaction) => {
                Self::run_mutation(transaction, filter, plan, control, input_bytes, mapper).await
            }
        }
    }

    async fn run_mutation<T, F>(
        transaction: &TransactionContext,
        filter: &ProjectedManagerFilter,
        plan: &crate::plan_builders::BuiltPlan,
        control: &ProjectedManagerFilterInvocationControl,
        input_bytes: u64,
        mapper: F,
    ) -> Result<T, QueryTargetError>
    where
        F: FnOnce(PlanAnswer, &mut ExecutionLedger) -> Result<T, SdkExecutionDiagnostic>,
    {
        if transaction.tx_type() != TxType::Write {
            return Err(invalid_input(
                "transaction_type_mismatch",
                "Filtered mutations require a borrowed write transaction",
                type_path(filter.model()),
            )
            .into());
        }
        let limits = crate::query_plan_exec::plan_answer_limits(
            control.limits,
            &control.cancellation,
            control.deadline,
        )?;
        let mut lease = bounded(&limits.answer, transaction.acquire_mutation_lease()).await?;
        let query = lease.prepare_plan(plan.local())?;
        let mut ledger = ExecutionLedger::new(control.limits, input_bytes);
        ledger.charge_preflight(query.typeql.len() as u64, 1)?;
        let mut policy = control.limits;
        policy.bytes = ledger.remaining_bytes();
        let limits = crate::query_plan_exec::plan_answer_limits(
            policy,
            &control.cancellation,
            control.deadline,
        )?;
        let result = async {
            control.checkpoint(filter.model())?;
            let (answer, stats) = lease
                .dispatch_plan_counted(query, limits, Some(plan.local()))
                .await
                .map_err(|error| lower_orm_error(&error, SdkProviderOperation::Write))?;
            ledger.record_reply(stats)?;
            let answer = TransactionContext::resolve_plan(plan.local(), answer)
                .map_err(|error| lower_orm_error(&error, SdkProviderOperation::Write))?;
            let value = mapper(answer, &mut ledger)?;
            control.checkpoint(filter.model())?;
            Ok::<_, SdkExecutionDiagnostic>(value)
        }
        .await;
        match result {
            Ok(value) => {
                lease.complete_success();
                Ok(value)
            }
            Err(error) => {
                lease.record_failure(&error);
                Err(error.into())
            }
        }
    }
}
