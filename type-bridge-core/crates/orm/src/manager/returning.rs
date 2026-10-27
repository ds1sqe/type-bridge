//! One projection-neutral capture, delete and proof path for every returning manager.

use crate::_registry::DescriptorRegistry;
use crate::error::Result;
use crate::match_request::{
    HydratedThing, MatchResult, SlotValue, validated_match_result_from_plan,
};
use crate::plan_builders::manager::{ManagerPlans, ManagerRead, invalid};
use crate::query_plan_exec::{self, PlanAnswer};
use crate::session::backend::AnswerCancellation;
use crate::session::context::TransactionContextMutationLease;
use crate::{QueryExecutionDeadline, QueryExecutionResourceLimits};
use std::collections::HashMap;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn delete_on_lease<T>(
    lease: &mut TransactionContextMutationLease,
    plans: &ManagerPlans,
    name: &str,
    iids: &[String],
    exact: bool,
    policy: QueryExecutionResourceLimits,
    cancellation: &AnswerCancellation,
    deadline: QueryExecutionDeadline,
    materialize: impl FnOnce(
        &DescriptorRegistry,
        &[&HydratedThing],
        Vec<serde_json::Value>,
        QueryExecutionResourceLimits,
    ) -> Result<T>,
) -> Result<T> {
    let descriptor = plans.registry.descriptor_id(name).ok_or_else(|| {
        invalid(
            "manager_type_unknown",
            "Deletion descriptor is absent from the registry",
        )
    })?;
    let type_id = plans.type_id(name)?;
    let authority = plans
        .registry
        .installed_schema_authority()
        .expect("authority checked");
    let exact = exact || authority.resolved().types()[&type_id].subtypes().is_empty();
    let caller_policy = policy;
    let policy = policy.for_adapter_statements(2);
    if policy.statement_limit() < 2 {
        return Err(crate::match_request::MatchError::new(
            crate::match_request::MatchErrorCategory::ResourceLimit,
            "projected_batch_statement_limit",
            "Returning deletion requires capture and delete statements",
        )
        .into());
    }
    let mut remaining = policy;
    let polymorphic_roles =
        !exact && type_id.kind() == type_bridge_contract::id::TypeKind::Relation;
    if polymorphic_roles {
        let mut captures = Vec::new();
        let mut statement_count = 0_usize;
        for iids in iids.chunks(type_bridge_contract::limits::MAX_INPUT_ROWS) {
            let ManagerRead::Fetch(stack) = plans.read_iids(name, iids)? else {
                unreachable!("model capture stack");
            };
            let mut queries = Vec::with_capacity(stack.plans().len());
            for plan in stack.plans() {
                let query = lease.prepare_plan(plan)?;
                charge_request(&query, &mut remaining)?;
                queries.push(query);
            }
            let deletion = plans.delete_many(name, iids, false, false)?;
            let query = lease.prepare_plan(deletion.local())?;
            charge_request(&query, &mut remaining)?;
            statement_count += stack.plans().len() + 1;
            captures.push((stack, queries, query));
        }
        let capture_policy = caller_policy.for_adapter_statements(statement_count);
        if statement_count > capture_policy.statement_limit() as usize {
            return Err(crate::match_request::MatchError::new(
                crate::match_request::MatchErrorCategory::ResourceLimit,
                "projected_batch_statement_limit",
                "Returning deletion exceeds the provider statement ceiling",
            )
            .into());
        }
        let mut frames = Vec::with_capacity(captures.len());
        let mut deletions = Vec::with_capacity(captures.len());
        let mut semantic = crate::answer_meter::PlanMeter::new(
            &query_plan_exec::plan_answer_limits(policy, cancellation, deadline)?,
        );
        let multiple_chunks = captures.len() > 1;
        for (stack, queries, deletion) in captures {
            let capture_limits = QueryExecutionResourceLimits {
                bytes: remaining.bytes,
                items: remaining.items,
                ..capture_policy
            };
            let (answers, stats) = lease
                .execute_plans(
                    stack.plans(),
                    capture_limits,
                    cancellation,
                    deadline,
                    Some(queries),
                )
                .await?;
            if multiple_chunks {
                for (answer, plan) in answers.iter().zip(stack.plans()) {
                    semantic.accept(
                        &answer
                            .to_staged_v3(plan)
                            .map_err(query_plan_exec::plan_diagnostic_error)?,
                    )?;
                }
            }
            remaining.bytes = remaining.bytes.saturating_sub(stats.response_bytes);
            remaining.items = remaining.items.saturating_sub(stats.processed_items);
            frames.push((stack, answers));
            deletions.push(deletion);
        }
        let proofs = frames
            .iter()
            .map(|(stack, answers)| stack.resolve(&plans.registry, answers, capture_policy))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut rows = Vec::with_capacity(proofs.len());
        for proof in &proofs {
            let MatchResult::Rows { rows: chunk } = proof.result().for_request(proof.request())?
            else {
                return Err(invalid(
                    "manager_result_shape",
                    "Deletion capture requires model rows",
                ));
            };
            rows.push(chunk.as_slice());
        }
        for query in deletions {
            let limits = query_plan_exec::plan_answer_limits(remaining, cancellation, deadline)?;
            let (_, stats) = lease.dispatch_plan_counted(query, limits, None).await?;
            remaining.bytes = remaining.bytes.saturating_sub(stats.response_bytes);
            remaining.items = remaining.items.saturating_sub(stats.processed_items);
        }
        return materialize_ordered(
            &plans.registry,
            rows.into_iter().flatten(),
            iids,
            Vec::new(),
            remaining,
            materialize,
        );
    }
    if iids.len() > type_bridge_contract::limits::MAX_INPUT_ROWS {
        let statements = iids
            .len()
            .div_ceil(type_bridge_contract::limits::MAX_INPUT_ROWS)
            * 2;
        if statements
            > caller_policy
                .for_adapter_statements(statements)
                .statement_limit() as usize
        {
            return Err(crate::match_request::MatchError::new(
                crate::match_request::MatchErrorCategory::ResourceLimit,
                "projected_batch_statement_limit",
                "Returning deletion exceeds the provider statement ceiling",
            )
            .into());
        }
        let mut chunks = Vec::new();
        for iids in iids.chunks(type_bridge_contract::limits::MAX_INPUT_ROWS) {
            let capture = plans.capture_many(name, iids, exact)?;
            let deletion = plans.delete_many(name, iids, exact, false)?;
            let capture_query = lease.prepare_plan(capture.local())?;
            let deletion_query = lease.prepare_plan(deletion.local())?;
            charge_request(&capture_query, &mut remaining)?;
            charge_request(&deletion_query, &mut remaining)?;
            chunks.push((capture, capture_query, deletion_query));
        }
        let mut semantic = crate::answer_meter::PlanMeter::new(
            &query_plan_exec::plan_answer_limits(policy, cancellation, deadline)?,
        );
        let mut frames = Vec::with_capacity(chunks.len());
        let mut deletions = Vec::with_capacity(chunks.len());
        for (capture, query, deletion) in chunks {
            let limits = query_plan_exec::plan_answer_limits(remaining, cancellation, deadline)?;
            let (answer, stats) = lease
                .dispatch_plan_counted(query, limits, Some(capture.local()))
                .await?;
            remaining.bytes = remaining.bytes.saturating_sub(stats.response_bytes);
            remaining.items = remaining.items.saturating_sub(stats.processed_items);
            let answer = query_plan_exec::resolve_plan(capture.local(), answer)
                .map_err(query_plan_exec::plan_diagnostic_error)?;
            semantic.accept(
                &answer
                    .to_staged_v3(capture.local())
                    .map_err(query_plan_exec::plan_diagnostic_error)?,
            )?;
            let PlanAnswer::Documents(documents) = answer else {
                return Err(invalid(
                    "manager_result_shape",
                    "Deletion capture requires documents",
                ));
            };
            frames.push((capture, documents));
            deletions.push(deletion);
        }
        let proofs = frames
            .iter()
            .map(|(capture, documents)| {
                validated_match_result_from_plan(
                    &plans.registry,
                    capture.local(),
                    documents,
                    &descriptor,
                    policy,
                )
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut rows = Vec::with_capacity(proofs.len());
        for (request, proof) in &proofs {
            let MatchResult::Rows { rows: chunk } = proof.for_request(request)? else {
                return Err(invalid(
                    "manager_result_shape",
                    "Deletion capture requires model rows",
                ));
            };
            rows.push(chunk.as_slice());
        }
        for query in deletions {
            let limits = query_plan_exec::plan_answer_limits(remaining, cancellation, deadline)?;
            let (_, stats) = lease.dispatch_plan_counted(query, limits, None).await?;
            remaining.bytes = remaining.bytes.saturating_sub(stats.response_bytes);
            remaining.items = remaining.items.saturating_sub(stats.processed_items);
        }
        let documents = frames
            .into_iter()
            .flat_map(|(_, documents)| documents)
            .collect();
        return materialize_ordered(
            &plans.registry,
            rows.into_iter().flatten(),
            iids,
            documents,
            remaining,
            materialize,
        );
    }
    let deletion = plans.delete_many(name, iids, exact, true)?;
    let query = lease.prepare_plan(deletion.local())?;
    charge_request(&query, &mut remaining)?;
    let limits = query_plan_exec::plan_answer_limits(remaining, cancellation, deadline)?;
    let (answer, stats) = lease
        .dispatch_plan_counted(query, limits, Some(deletion.local()))
        .await?;
    let PlanAnswer::Documents(documents) = query_plan_exec::resolve_plan(deletion.local(), answer)
        .map_err(query_plan_exec::plan_diagnostic_error)?
    else {
        return Err(invalid(
            "manager_result_shape",
            "Deletion capture requires documents",
        ));
    };
    let (request, result) = validated_match_result_from_plan(
        &plans.registry,
        deletion.local(),
        &documents,
        &descriptor,
        policy,
    )?;
    let MatchResult::Rows { rows } = result.for_request(&request)? else {
        return Err(invalid(
            "manager_result_shape",
            "Deletion capture requires model rows",
        ));
    };
    materialize_ordered(
        &plans.registry,
        rows,
        iids,
        documents,
        QueryExecutionResourceLimits {
            bytes: remaining.bytes.saturating_sub(stats.response_bytes),
            items: remaining.items.saturating_sub(stats.processed_items),
            ..remaining
        },
        materialize,
    )
}

fn materialize_ordered<'row, T>(
    registry: &DescriptorRegistry,
    rows: impl IntoIterator<Item = &'row crate::match_request::MatchRow>,
    iids: &[String],
    documents: Vec<serde_json::Value>,
    remaining: QueryExecutionResourceLimits,
    materialize: impl FnOnce(
        &DescriptorRegistry,
        &[&HydratedThing],
        Vec<serde_json::Value>,
        QueryExecutionResourceLimits,
    ) -> Result<T>,
) -> Result<T> {
    let rows = rows.into_iter();
    let mut by_iid = HashMap::with_capacity(rows.size_hint().0);
    for row in rows {
        let [SlotValue::One(thing)] = row.slots() else {
            return Err(invalid(
                "manager_result_shape",
                "Deletion capture requires one model slot",
            ));
        };
        if by_iid
            .insert(thing.concept_id().as_str().to_ascii_lowercase(), thing)
            .is_some()
        {
            return Err(invalid(
                "manager_result_shape",
                "Deletion capture repeated one identity",
            ));
        }
    }
    let mut ordered = Vec::with_capacity(by_iid.len());
    for iid in iids {
        if let Some(thing) = by_iid.remove(&iid.to_ascii_lowercase()) {
            ordered.push(thing);
        }
    }
    if !by_iid.is_empty() {
        return Err(invalid(
            "manager_result_shape",
            "Deletion capture returned an unrequested identity",
        ));
    }
    materialize(registry, &ordered, documents, remaining)
}

fn charge_request(
    query: &crate::session::staged::StagedQuery,
    remaining: &mut QueryExecutionResourceLimits,
) -> Result<()> {
    let statements = if query.snapshot.is_some() { 2 } else { 1 };
    let rows = query
        .rows
        .as_ref()
        .map(crate::projected_batch_executor::given_rows_bytes)
        .transpose()
        .map_err(|_| {
            invalid(
                "manager_query_input",
                "Deletion input size cannot be accounted",
            )
        })?
        .unwrap_or(0);
    let bytes = (query.typeql.len() as u64)
        .saturating_add(
            query
                .snapshot
                .as_ref()
                .map_or(0, |(source, _)| source.len() as u64),
        )
        .saturating_add(rows.saturating_mul(statements));
    if bytes > remaining.bytes {
        return Err(crate::match_request::MatchError::new(
            crate::match_request::MatchErrorCategory::ResourceLimit,
            "projected_batch_transport_byte_limit",
            "Deletion requests exceed the captured byte ceiling",
        )
        .with_detail("actual", bytes)
        .with_detail("maximum", remaining.bytes)
        .into());
    }
    remaining.bytes -= bytes;
    Ok(())
}

/// Retain the shared classified, redacted commit boundary for native managers.
pub(crate) async fn commit(transaction: &crate::TransactionContext) -> Result<()> {
    transaction
        .commit_sdk()
        .await
        .map_err(|diagnostic| crate::OrmError::ExecutionDiagnostic(Box::new(diagnostic)))
}
