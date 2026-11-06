//! One projection-neutral capture, delete and proof path for every returning manager.

use crate::_registry::DescriptorRegistry;
use crate::error::Result;
use crate::match_request::{
    HydratedThing, MatchResult, SlotValue, validated_match_result_from_plan,
};
use crate::plan_builders::manager::{ManagerPlans, invalid};
use crate::query_plan_exec::{self, PlanAnswer};
use crate::session::backend::AnswerCancellation;
use crate::session::context::TransactionContextMutationLease;
use crate::{QueryExecutionDeadline, QueryExecutionResourceLimits};
use std::collections::HashMap;

#[expect(
    clippy::large_enum_variant,
    reason = "single-plan returning deletes avoid an allocation"
)]
pub(crate) enum DeletionPlans {
    One(crate::plan_builders::BuiltPlan),
    Many(Vec<crate::plan_builders::BuiltPlan>),
}
impl DeletionPlans {
    fn from_vec(mut plans: Vec<crate::plan_builders::BuiltPlan>) -> Self {
        if plans.len() == 1 {
            Self::One(plans.pop().expect("one deletion plan"))
        } else {
            Self::Many(plans)
        }
    }
    pub(crate) fn into_vec(self) -> Vec<crate::plan_builders::BuiltPlan> {
        match self {
            Self::One(plan) => vec![plan],
            Self::Many(plans) => plans,
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::One(_) => 1,
            Self::Many(plans) => plans.len(),
        }
    }
}

pub(crate) enum DeleteSelection<'a> {
    Iids(&'a [String]),
    Entities(&'a [(Option<String>, crate::_dynamic::DynamicAttributeMap)]),
    Relations(
        &'a [(
            Option<String>,
            crate::_dynamic::DynamicAttributeMap,
            Vec<crate::_dynamic::DynamicRolePlayerInput>,
        )],
    ),
}

impl DeleteSelection<'_> {
    pub(crate) fn plans(
        &self,
        plans: &ManagerPlans,
        name: &str,
        exact: bool,
    ) -> Result<DeletionPlans> {
        match self {
            Self::Iids(iids) if iids.len() <= type_bridge_contract::limits::MAX_INPUT_ROWS => plans
                .delete_many(name, iids, exact, true)
                .map(DeletionPlans::One),
            Self::Iids(iids) => iids
                .chunks(type_bridge_contract::limits::MAX_INPUT_ROWS)
                .map(|iids| plans.delete_many(name, iids, exact, true))
                .collect::<Result<Vec<_>>>()
                .map(DeletionPlans::Many),
            Self::Entities(items) => plans
                .write_many(
                    name,
                    crate::plan_builders::bulk_write::WriteKind::DeleteReturning,
                    &items
                        .iter()
                        .map(|(iid, fields)| (iid.as_deref(), fields, &[][..]))
                        .collect::<Vec<_>>(),
                )?
                .into_static()
                .map(DeletionPlans::from_vec)
                .map_err(crate::plan_builders::manager::diagnostic),
            Self::Relations(items) => plans
                .write_many(
                    name,
                    crate::plan_builders::bulk_write::WriteKind::DeleteReturning,
                    &items
                        .iter()
                        .map(|(iid, fields, players)| {
                            (
                                iid.as_deref(),
                                fields,
                                if iid.is_some() {
                                    &[][..]
                                } else {
                                    players.as_slice()
                                },
                            )
                        })
                        .collect::<Vec<_>>(),
                )?
                .into_static()
                .map(DeletionPlans::from_vec)
                .map_err(crate::plan_builders::manager::diagnostic),
        }
    }
    fn ordered_iids<'a>(
        &'a self,
        documents: &[serde_json::Value],
    ) -> Result<std::borrow::Cow<'a, [String]>> {
        let count = match self {
            Self::Iids(iids) => return Ok(std::borrow::Cow::Borrowed(iids)),
            Self::Entities(items) => items.len(),
            Self::Relations(items) => items.len(),
        };
        let mut ordered = vec![None; count];
        for document in documents {
            let ordinal = document
                .get("ordinal")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| {
                    invalid(
                        "manager_result_shape",
                        "Deletion capture requires its input ordinal",
                    )
                })?;
            let iid = document
                .get("_iid")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    invalid(
                        "manager_result_shape",
                        "Deletion capture requires a model identity",
                    )
                })?;
            let target = ordered.get_mut(ordinal).ok_or_else(|| {
                invalid(
                    "manager_result_shape",
                    "Deletion capture returned an unknown ordinal",
                )
            })?;
            if target.replace(iid.to_owned()).is_some() {
                return Err(invalid(
                    "manager_result_shape",
                    "Deletion selector matched multiple models",
                ));
            }
        }
        let ordered = ordered.into_iter().flatten().collect::<Vec<_>>();
        super::dynamic::validate_delete_iids(&ordered)?;
        Ok(std::borrow::Cow::Owned(ordered))
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn delete_on_lease<T>(
    lease: &mut TransactionContextMutationLease,
    plans: &ManagerPlans,
    name: &str,
    selection: DeleteSelection<'_>,
    exact: bool,
    policy: QueryExecutionResourceLimits,
    cancellation: &AnswerCancellation,
    deadline: QueryExecutionDeadline,
    materialize: impl FnOnce(
        &DescriptorRegistry,
        &[(usize, &HydratedThing)],
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
    let deletion_plans = selection.plans(plans, name, exact)?;
    let statement_count = deletion_plans.len().saturating_mul(2);
    let policy = policy.for_adapter_statements(statement_count);
    if statement_count > policy.statement_limit() as usize {
        return Err(crate::match_request::MatchError::new(
            crate::match_request::MatchErrorCategory::ResourceLimit,
            "projected_batch_statement_limit",
            "Returning deletion exceeds the provider statement ceiling",
        )
        .into());
    }
    let deletion = match deletion_plans {
        DeletionPlans::One(deletion) => deletion,
        DeletionPlans::Many(deletion_plans) => {
            // Keep forced-chunk frames out of ordinary manager futures.
            return Box::pin(delete_chunks_on_lease(
                lease,
                plans,
                &descriptor,
                deletion_plans,
                selection,
                policy,
                cancellation,
                deadline,
                materialize,
            ))
            .await;
        }
    };
    let mut remaining = policy;
    let mut query = lease.prepare_plan(deletion.local())?;
    charge_request(&query, &mut remaining)?;
    let (source, _) = query.snapshot.take().expect("returning deletion snapshot");
    let mut capture = query.clone();
    capture.typeql = source;
    capture.writes = false;
    capture.redacted_typeql = query.redacted_snapshot.take();
    let limits = query_plan_exec::plan_answer_limits(remaining, cancellation, deadline)?;
    let (answer, stats) = lease
        .dispatch_plan_counted(capture, limits, Some(deletion.local()))
        .await?;
    remaining.bytes = remaining.bytes.saturating_sub(stats.response_bytes);
    remaining.items = remaining.items.saturating_sub(stats.processed_items);
    let PlanAnswer::Documents(documents) = query_plan_exec::resolve_plan(deletion.local(), answer)
        .map_err(query_plan_exec::plan_diagnostic_error)?
    else {
        return Err(invalid(
            "manager_result_shape",
            "Deletion capture requires documents",
        ));
    };
    let rows = capture_rows(
        &plans.registry,
        deletion.local(),
        &documents,
        &descriptor,
        policy,
    )?;
    drop(deletion);
    let iids = selection.ordered_iids(&documents)?;
    let ordered = ordered_things(&rows, &iids)?;
    drop(iids);
    let limits = query_plan_exec::plan_answer_limits(remaining, cancellation, deadline)?;
    let (_, stats) = lease.dispatch_plan_counted(query, limits, None).await?;
    remaining.bytes = remaining.bytes.saturating_sub(stats.response_bytes);
    remaining.items = remaining.items.saturating_sub(stats.processed_items);
    materialize(&plans.registry, &ordered, documents, remaining)
}

#[allow(clippy::too_many_arguments)]
async fn delete_chunks_on_lease<T>(
    lease: &mut TransactionContextMutationLease,
    plans: &ManagerPlans,
    descriptor: &crate::match_request::DescriptorId,
    deletion_plans: Vec<crate::plan_builders::BuiltPlan>,
    selection: DeleteSelection<'_>,
    policy: QueryExecutionResourceLimits,
    cancellation: &AnswerCancellation,
    deadline: QueryExecutionDeadline,
    materialize: impl FnOnce(
        &DescriptorRegistry,
        &[(usize, &HydratedThing)],
        Vec<serde_json::Value>,
        QueryExecutionResourceLimits,
    ) -> Result<T>,
) -> Result<T> {
    let mut remaining = policy;
    let mut chunks = Vec::with_capacity(deletion_plans.len());
    for deletion in deletion_plans {
        let mut deletion_query = lease.prepare_plan(deletion.local())?;
        charge_request(&deletion_query, &mut remaining)?;
        let (source, _) = deletion_query
            .snapshot
            .take()
            .expect("returning deletion snapshot");
        let mut capture_query = deletion_query.clone();
        capture_query.typeql = source;
        capture_query.writes = false;
        capture_query.redacted_typeql = deletion_query.redacted_snapshot.take();
        chunks.push((deletion, capture_query, deletion_query));
    }
    let mut semantic = crate::answer_meter::PlanMeter::new(&query_plan_exec::plan_answer_limits(
        policy,
        cancellation,
        deadline,
    )?);
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
    let rows = frames
        .iter()
        .map(|(capture, documents)| {
            capture_rows(
                &plans.registry,
                capture.local(),
                documents,
                descriptor,
                policy,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let documents = frames
        .into_iter()
        .flat_map(|(_, documents)| documents)
        .collect::<Vec<_>>();
    let iids = selection.ordered_iids(&documents)?;
    let ordered = ordered_things(rows.iter().flatten(), &iids)?;
    for query in deletions {
        let limits = query_plan_exec::plan_answer_limits(remaining, cancellation, deadline)?;
        let (_, stats) = lease.dispatch_plan_counted(query, limits, None).await?;
        remaining.bytes = remaining.bytes.saturating_sub(stats.response_bytes);
        remaining.items = remaining.items.saturating_sub(stats.processed_items);
    }
    materialize(&plans.registry, &ordered, documents, remaining)
}

pub(crate) fn capture_rows(
    registry: &DescriptorRegistry,
    plan: &type_bridge_contract::query_plan::QueryPlan,
    documents: &[serde_json::Value],
    descriptor: &crate::match_request::DescriptorId,
    policy: QueryExecutionResourceLimits,
) -> Result<Vec<crate::match_request::MatchRow>> {
    let (request, result) =
        validated_match_result_from_plan(registry, plan, documents, descriptor, policy)?;
    result.for_request(&request)?;
    let MatchResult::Rows { rows } = result.into_result() else {
        return Err(invalid(
            "manager_result_shape",
            "Deletion capture requires model rows",
        ));
    };
    Ok(rows)
}

pub(crate) fn ordered_things<'row>(
    rows: impl IntoIterator<Item = &'row crate::match_request::MatchRow>,
    iids: &[String],
) -> Result<Vec<(usize, &'row HydratedThing)>> {
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
    for (index, iid) in iids.iter().enumerate() {
        if let Some(thing) = by_iid.remove(&iid.to_ascii_lowercase()) {
            ordered.push((index, thing));
        }
    }
    if !by_iid.is_empty() {
        return Err(invalid(
            "manager_result_shape",
            "Deletion capture returned an unrequested identity",
        ));
    }
    Ok(ordered)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_selector_aliases_refuse_repeated_targets_before_deletion() {
        let selectors = vec![(None, vec![]), (None, vec![])];
        let selection = DeleteSelection::Entities(&selectors);
        let documents = vec![
            serde_json::json!({"ordinal":0,"_iid":"0x01"}),
            serde_json::json!({"ordinal":1,"_iid":"0x01"}),
        ];
        let crate::OrmError::Match(error) = selection.ordered_iids(&documents).unwrap_err() else {
            panic!("resolved aliases require the shared duplicate-target refusal")
        };
        assert_eq!(error.code().as_str(), "duplicate_batch_target");
    }
}
