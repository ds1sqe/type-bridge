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
        let control = control.constrained_by(target.answer_limits());
        self.preflight(filter, &control, false)?;
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
        self.execute_mutation(target, filter, &plan, &control, |answer| match answer {
            PlanAnswer::Matched(_) => Ok(()),
            _ => Err(SdkExecutionDiagnostic::internal_failure()),
        })
        .await
    }

    async fn execute_mutation<T, F>(
        &self,
        target: QueryTarget<'_>,
        filter: &ProjectedManagerFilter,
        plan: &crate::plan_builders::BuiltPlan,
        control: &ProjectedManagerFilterInvocationControl,
        mapper: F,
    ) -> Result<T, QueryTargetError>
    where
        F: FnOnce(PlanAnswer) -> Result<T, SdkExecutionDiagnostic>,
    {
        control.checkpoint(filter.model())?;
        match target {
            QueryTarget::Proxy(proxy) => {
                let authority = self
                    .installed
                    .plan_authority()
                    .map_err(crate::plan_builders::manager::diagnostic)?;
                let entry = type_bridge_contract::query_remote_v3::StagedPlanV3::new(
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
                            mapper(answers.pop().expect("one checked mutation answer"))
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
                let result = Self::run_mutation(&transaction, filter, plan, control, mapper).await;
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
                Self::run_mutation(transaction, filter, plan, control, mapper).await
            }
        }
    }

    async fn run_mutation<T, F>(
        transaction: &TransactionContext,
        filter: &ProjectedManagerFilter,
        plan: &crate::plan_builders::BuiltPlan,
        control: &ProjectedManagerFilterInvocationControl,
        mapper: F,
    ) -> Result<T, QueryTargetError>
    where
        F: FnOnce(PlanAnswer) -> Result<T, SdkExecutionDiagnostic>,
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
        let mut ledger =
            ExecutionLedger::new(control.limits, filter.resource_measure().bytes() as u64);
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
            let value = mapper(answer)?;
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
