//! Resolve a static typed-read plan stack without provider I/O.

use super::plan_adapter::{AdaptedMatchRequest, Layout};
use super::plan_hydration::{
    GraphBuilder, HydrationAuthority, PredicateWitnesses, SlotDraft, Solution,
    charge_selected_solution_semantics, evaluate_pattern_with_witnesses, finish_hydrated_rows,
    insert_plan_documents, try_sort_iids, validate_plan_documents, validate_solution_order,
};
use super::{MatchOperation, RowCardinality, ValidatedMatchRequest};
use crate::QueryExecutionResourceLimits;
use crate::query_plan_exec::PlanAnswer;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::id::TypeId;
use type_bridge_contract::query_model_evidence::ModelOutcome;
use type_bridge_contract::query_model_limits::ModelReplyDecodeLimits;
use type_bridge_contract::query_plan::{ModelOutputSlot, QueryOutput};
use type_bridge_contract::query_remote::RemoteValue;
use type_bridge_contract::query_remote::StagedAnswer;
use type_bridge_contract::value::CanonicalValue;

type Types = BTreeMap<(u16, String), TypeId>;

fn failure(code: &'static str, message: &'static str) -> Diagnostic {
    crate::plan_lowering::failure(DiagnosticCategory::Integrity, code, message)
}

fn shared(error: crate::error::PlanExecutionError) -> Diagnostic {
    match error {
        crate::error::PlanExecutionError::Validation(error) => error,
        crate::error::PlanExecutionError::Provider(_) => failure(
            "query_plan_result_state",
            "pure plan result validation returned a provider failure",
        ),
    }
}

fn rows(
    stack: &AdaptedMatchRequest,
    answers: &[PlanAnswer],
    index: usize,
    types: &mut Types,
    witnesses: &mut PredicateWitnesses,
) -> Result<Vec<Solution>, Diagnostic> {
    let plan = stack.plans.get(index).ok_or_else(|| {
        failure(
            "query_plan_result_stack",
            "read answer has no corresponding plan",
        )
    })?;
    let answer = answers
        .get(index)
        .ok_or_else(|| {
            failure(
                "query_plan_result_stack",
                "read plan has no corresponding answer",
            )
        })?
        .to_staged_v3(plan)?;
    answer.validate_for(plan)?;
    let (StagedAnswer::Rows { rows }, QueryOutput::Rows { columns }) = (answer, plan.output())
    else {
        return Err(failure(
            "query_plan_result_rows",
            "selection did not return thing rows",
        ));
    };
    let mut solutions = rows
        .into_iter()
        .map(|row| {
            let mut bindings = BTreeMap::new();
            for (value, binding) in row.iter().zip(columns) {
                if !stack
                    .hydration
                    .bindings()
                    .iter()
                    .any(|expected| expected.binding() == *binding)
                {
                    continue;
                }
                let RemoteValue::Thing { iid, type_id } = value else {
                    return Err(failure(
                        "query_plan_result_rows",
                        "selected binding is not a present thing",
                    ));
                };
                let expected = stack
                    .hydration
                    .bindings()
                    .iter()
                    .find(|expected| expected.binding() == *binding)
                    .ok_or_else(|| {
                        failure(
                            "query_plan_result_rows",
                            "selection references an unknown match binding",
                        )
                    })?;
                if !expected.concrete_descriptors().contains(type_id) {
                    return Err(failure(
                        "query_plan_result_type",
                        "selected thing is outside its declared type domain",
                    ));
                }
                if types
                    .iter()
                    .any(|((_, existing), previous)| existing == iid && previous != type_id)
                {
                    return Err(failure(
                        "query_plan_result_type",
                        "one IID carries conflicting concrete types",
                    ));
                }
                if let Some(previous) = types.insert((binding.get(), iid.clone()), type_id.clone())
                    && previous != *type_id
                {
                    return Err(failure(
                        "query_plan_result_type",
                        "one selected IID carries conflicting concrete types",
                    ));
                }
                bindings.insert(binding.get(), iid.clone());
            }
            if let Layout::Fetch {
                witnesses: fields, ..
            } = &stack.layout
            {
                for (field, column) in fields {
                    let Some(position) = columns.iter().position(|binding| binding == column)
                    else {
                        continue;
                    };
                    let iid = bindings.get(&field.binding().get()).ok_or_else(|| {
                        failure(
                            "query_plan_result_rows",
                            "predicate witness omits its thing",
                        )
                    })?;
                    let values = witnesses
                        .entry((
                            field.binding().get(),
                            iid.clone(),
                            field.attribute().clone(),
                        ))
                        .or_default();
                    match row.get(position) {
                        Some(
                            RemoteValue::Value { value } | RemoteValue::Attribute { value, .. },
                        ) => {
                            let value = value.clone().into();
                            values.push(value);
                        }
                        Some(RemoteValue::Absent) => {}
                        _ => {
                            return Err(failure(
                                "query_v2_model_predicate_evidence",
                                "predicate witness is not a scalar or explicit absence",
                            ));
                        }
                    }
                }
            }
            Ok(Solution { bindings })
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    for values in witnesses.values_mut() {
        values.sort();
        values.dedup();
    }
    if matches!(&stack.layout, Layout::Fetch {mapping:Some(mapping),..} if *mapping == index) {
        solutions.sort_by(|left, right| left.bindings.cmp(&right.bindings));
        solutions.dedup_by(|left, right| left.bindings == right.bindings);
    }
    Ok(solutions)
}

fn count(
    stack: &AdaptedMatchRequest,
    answers: &[PlanAnswer],
    index: usize,
) -> Result<u64, Diagnostic> {
    let answer = answers
        .get(index)
        .ok_or_else(|| failure("query_plan_result_stack", "count answer is missing"))?
        .to_staged_v3(&stack.plans[index])?;
    let StagedAnswer::Rows { rows } = answer else {
        return Err(failure(
            "query_plan_result_count",
            "count plan did not return a scalar row",
        ));
    };
    match rows.as_slice() {
        [row] => match row.as_slice() {
            [
                RemoteValue::Value {
                    value: CanonicalValue::Long(value),
                },
            ] => u64::try_from(*value)
                .map_err(|_| failure("query_plan_result_count", "count result is negative")),
            _ => Err(failure(
                "query_plan_result_count",
                "count result is not one integer",
            )),
        },
        _ => Err(failure(
            "query_plan_result_count",
            "count result is not one row",
        )),
    }
}

fn documents(answers: &[PlanAnswer], index: usize) -> Result<&[Value], Diagnostic> {
    match answers.get(index) {
        Some(PlanAnswer::Documents(documents)) => Ok(documents),
        _ => Err(failure(
            "query_plan_result_documents",
            "hydration plan did not return documents",
        )),
    }
}

pub(crate) fn resolve(
    stack: &AdaptedMatchRequest,
    request: &ValidatedMatchRequest,
    answers: &[PlanAnswer],
    limits: QueryExecutionResourceLimits,
) -> Result<ModelOutcome, Diagnostic> {
    if stack.plans.len() != answers.len() {
        return Err(failure(
            "query_plan_result_stack",
            "read answer count differs from the static plan stack",
        ));
    }
    match &stack.layout {
        Layout::Count { root, answer } => {
            return Ok(ModelOutcome::DistinctCount {
                root: type_bridge_contract::migration_assertion::BindingId::new(root.get())?,
                value: count(stack, answers, *answer)?,
            });
        }
        Layout::Exists { root, answer } => {
            let Some(PlanAnswer::Exists(value)) = answers.get(*answer) else {
                return Err(failure(
                    "query_plan_result_exists",
                    "existence plan returned the wrong answer kind",
                ));
            };
            return Ok(ModelOutcome::DistinctExists {
                root: type_bridge_contract::migration_assertion::BindingId::new(root.get())?,
                value: *value,
            });
        }
        Layout::WitnessReduction { .. } => {
            return Err(failure(
                "query_plan_result_reduction",
                "witness reduction requires the shared model validator",
            ));
        }
        Layout::Reduction { .. } => return resolve_reduction(stack, answers, limits),
        Layout::Fetch { .. } => {}
    }
    let Layout::Fetch {
        selection,
        mapping,
        hydration,
        total,
        output,
        order,
        ..
    } = &stack.layout
    else {
        unreachable!()
    };
    let reply_limits = ModelReplyDecodeLimits {
        max_bytes: limits.bytes,
        max_items: limits.items,
        max_graph_nodes: limits.graph_nodes,
        max_attribute_values: limits.attribute_values,
        max_collection_members: limits.collection_members,
        max_role_players: limits.role_players,
    };
    let authority = HydrationAuthority::new(&stack.hydration).map_err(shared)?;
    let mut graph = GraphBuilder::new(reply_limits);
    let mut types = Types::new();
    let mut witnesses = PredicateWitnesses::new();
    let hydrated = hydration
        .iter()
        .map(|(_, binding, _)| binding.get())
        .collect::<BTreeSet<_>>();
    let (selected, solutions) = if let Some(mapping) = mapping {
        let mut selected = selection
            .map(|index| rows(stack, answers, index, &mut types, &mut witnesses))
            .transpose()?
            .unwrap_or_default();
        let solutions = rows(stack, answers, *mapping, &mut types, &mut witnesses)?;
        let page_root = if selection.is_none() {
            match &request.request().operation {
                MatchOperation::PageBy { root, .. } => Some(root.get()),
                MatchOperation::FetchRows { .. } if order.identity_tiebreakers().len() == 1 => {
                    Some(order.identity_tiebreakers()[0].get())
                }
                MatchOperation::FetchRows { .. } => None,
                _ => {
                    return Err(failure(
                        "query_plan_result_stack",
                        "mapping has no independent selection baseline",
                    ));
                }
            }
        } else {
            None
        };
        if let Some(selection) = selection {
            let QueryOutput::Rows { columns } = stack.plans[*selection].output() else {
                return Err(failure(
                    "query_plan_result_rows",
                    "selection output is not rows",
                ));
            };
            let key = |solution: &Solution| {
                columns
                    .iter()
                    .map(|binding| {
                        solution
                            .bindings
                            .get(&binding.get())
                            .cloned()
                            .ok_or_else(|| {
                                failure(
                                    "query_plan_result_window",
                                    "window mapping omits a selected binding",
                                )
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()
            };
            let expected = selected
                .iter()
                .map(key)
                .collect::<Result<BTreeSet<_>, _>>()?;
            let actual = solutions
                .iter()
                .map(key)
                .collect::<Result<BTreeSet<_>, _>>()?;
            if expected.len() != selected.len() || actual != expected {
                return Err(failure(
                    "query_plan_result_window",
                    "mapping tuples differ from the distinct selected window",
                ));
            }
        }
        let mut root_partitions = Vec::new();
        for (answer, binding, concrete) in hydration {
            let documents = documents(answers, *answer)?;
            validate_plan_documents(&stack.plans[*answer], documents, limits).map_err(shared)?;
            if documents.iter().any(|document| {
                !concrete.iter().any(|type_id| {
                    document.get("_type").and_then(Value::as_str) == Some(type_id.label().as_str())
                })
            }) {
                return Err(failure(
                    "query_plan_result_type",
                    "concrete hydration partition returned another type",
                ));
            }
            let actual = insert_plan_documents(
                &mut graph,
                &authority,
                binding.get(),
                &stack.plans[*answer],
                documents,
            )
            .map_err(shared)?;
            let actual_set = actual.iter().cloned().collect::<BTreeSet<_>>();
            let expected = types
                .iter()
                .filter_map(|((ordinal, iid), type_id)| {
                    (*ordinal == binding.get() && concrete.contains(type_id)).then_some(iid.clone())
                })
                .collect::<BTreeSet<_>>();
            if actual_set != expected {
                return Err(failure(
                    "query_plan_result_window",
                    "hydration identities differ from the selected window",
                ));
            }
            if page_root == Some(binding.get()) {
                root_partitions.push(actual);
            }
        }
        if let Some(root) = page_root {
            let mut roots = Vec::new();
            for partition in root_partitions {
                let witnesses = partition
                    .iter()
                    .map(|iid| Solution {
                        bindings: BTreeMap::from([(root, iid.clone())]),
                    })
                    .collect::<Vec<_>>();
                validate_solution_order(&witnesses, order, &graph).map_err(shared)?;
                roots.extend(partition);
            }
            let unique = roots.iter().cloned().collect::<BTreeSet<_>>();
            if unique.len() != roots.len() {
                return Err(failure(
                    "query_plan_result_window",
                    "page hydration partitions overlap",
                ));
            }
            try_sort_iids(&mut roots, order, &graph).map_err(shared)?;
            selected = roots
                .into_iter()
                .map(|iid| Solution {
                    bindings: BTreeMap::from([(root, iid)]),
                })
                .collect();
        } else if selection.is_none() {
            let selected_bindings = order
                .identity_tiebreakers()
                .iter()
                .map(|binding| binding.get())
                .collect::<BTreeSet<_>>();
            let mut seen = BTreeSet::new();
            for solution in &solutions {
                let bindings = solution
                    .bindings
                    .iter()
                    .filter(|(binding, _)| selected_bindings.contains(binding))
                    .map(|(binding, iid)| (*binding, iid.clone()))
                    .collect::<BTreeMap<_, _>>();
                if bindings.len() != selected_bindings.len() {
                    return Err(failure(
                        "query_plan_result_window",
                        "mapping omits a selected tuple identity",
                    ));
                }
                if seen.insert(bindings.clone()) {
                    selected.push(Solution { bindings });
                }
            }
            super::plan_hydration::try_sort_solutions(&mut selected, order, &graph)
                .map_err(shared)?;
        }
        (selected, solutions)
    } else {
        let Some((answer, binding, _)) = hydration.first() else {
            return Err(failure(
                "query_plan_result_stack",
                "single fetch has no document plan",
            ));
        };
        let documents = documents(answers, *answer)?;
        validate_plan_documents(&stack.plans[*answer], documents, limits).map_err(shared)?;
        let iids = insert_plan_documents(
            &mut graph,
            &authority,
            binding.get(),
            &stack.plans[*answer],
            documents,
        )
        .map_err(shared)?;
        let solutions = iids
            .into_iter()
            .map(|iid| Solution {
                bindings: BTreeMap::from([(binding.get(), iid)]),
            })
            .collect::<Vec<_>>();
        (solutions.clone(), solutions)
    };
    for ((binding, iid, attribute), values) in &witnesses {
        let concrete = types.get(&(*binding, iid.clone())).ok_or_else(|| {
            failure(
                "query_v2_model_predicate_evidence",
                "predicate witness has no correlated identity",
            )
        })?;
        let field = stack
            .hydration
            .descriptors()
            .iter()
            .find(|descriptor| descriptor.descriptor() == concrete)
            .and_then(|descriptor| {
                descriptor
                    .fields()
                    .iter()
                    .find(|field| field.attribute() == attribute)
            })
            .ok_or_else(|| {
                failure(
                    "query_v2_model_predicate_evidence",
                    "predicate witness has no schema field",
                )
            })?;
        if values.is_empty() && field.cardinality().min() > 0 {
            return Err(failure(
                "query_v2_model_predicate_evidence",
                "required predicate operand is absent",
            ));
        }
        if let Some(node) = graph.nodes.get(iid) {
            let actual = node
                .attributes
                .iter()
                .find(|(id, _)| id == attribute)
                .map(|(_, values)| values.as_slice())
                .unwrap_or_default();
            if values.is_empty() != actual.is_empty()
                || values.iter().any(|value| !actual.contains(value))
            {
                return Err(failure(
                    "query_v2_model_predicate_evidence",
                    "predicate witness contradicts the selected model",
                ));
            }
        }
    }
    for solution in &solutions {
        if solution.bindings.len() != stack.hydration.bindings().len() {
            return Err(failure(
                "query_plan_result_rows",
                "mapping omits a matched binding",
            ));
        }
        if let Some(predicate) = &stack.predicate
            && !evaluate_pattern_with_witnesses(
                predicate,
                solution,
                &graph,
                Some((&witnesses, &hydrated)),
            )
            .map_err(shared)?
        {
            return Err(failure(
                "query_v2_model_predicate_evidence",
                "hydrated solution does not satisfy the validated match predicate",
            ));
        }
    }
    charge_selected_solution_semantics(&solutions, &graph, reply_limits, Some(&hydrated))
        .map_err(shared)?;
    let selected_ids = selected
        .first()
        .map(|solution| solution.bindings.keys().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    let key = |solution: &Solution| {
        selected_ids
            .iter()
            .map(|binding| {
                solution.bindings.get(binding).cloned().ok_or_else(|| {
                    failure(
                        "query_plan_result_window",
                        "mapping lost a selected binding",
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()
    };
    let mut grouped = BTreeMap::<Vec<String>, Vec<&Solution>>::new();
    for solution in &solutions {
        grouped.entry(key(solution)?).or_default().push(solution);
    }
    let representatives = selected
        .iter()
        .map(|selected| {
            grouped
                .get(&key(selected)?)
                .and_then(|solutions| solutions.first().copied())
                .cloned()
                .ok_or_else(|| {
                    failure(
                        "query_plan_result_window",
                        "selected tuple has no complete mapping witness",
                    )
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !selected.is_empty() {
        validate_solution_order(&representatives, order, &graph).map_err(shared)?;
    }
    let mut drafts = Vec::with_capacity(selected.len());
    let mut collection_members = 0_u64;
    for selected in &selected {
        let group = grouped.get(&key(selected)?).ok_or_else(|| {
            failure(
                "query_plan_result_window",
                "selected tuple has no mapping witness",
            )
        })?;
        let mut slots = Vec::new();
        for slot in output.slots() {
            match slot {
                ModelOutputSlot::One { binding, declared } => {
                    let iid = group[0].bindings[&binding.get()].clone();
                    if group
                        .iter()
                        .any(|solution| solution.bindings.get(&binding.get()) != Some(&iid))
                    {
                        return Err(failure(
                            "query_plan_result_window",
                            "singular slot has conflicting mapping witnesses",
                        ));
                    }
                    slots.push(SlotDraft::Singular {
                        declared: declared.clone(),
                        iid,
                    });
                }
                ModelOutputSlot::Collect {
                    binding,
                    declared,
                    distinct,
                    order,
                } => {
                    let mut iids = group
                        .iter()
                        .map(|solution| solution.bindings[&binding.get()].clone())
                        .collect::<Vec<_>>();
                    if *distinct {
                        let mut seen = BTreeSet::new();
                        iids.retain(|iid| seen.insert(iid.clone()));
                    }
                    try_sort_iids(&mut iids, order, &graph).map_err(shared)?;
                    collection_members = collection_members.saturating_add(iids.len() as u64);
                    if collection_members > limits.collection_members {
                        return Err(crate::plan_lowering::failure(
                            DiagnosticCategory::ResourceLimit,
                            "query_v2_model_collection_limit",
                            "collected slots exceed the collection budget",
                        ));
                    }
                    slots.push(SlotDraft::Collection {
                        declared: declared.clone(),
                        iids,
                    });
                }
            }
        }
        drafts.push(slots);
    }
    if let MatchOperation::FetchRows {
        cardinality: RowCardinality::Complete,
        window,
        ..
    } = &request.request().operation
        && selected.len() as u64 > window.limit
    {
        return Err(crate::plan_lowering::failure(
            DiagnosticCategory::ResourceLimit,
            "query_v2_model_complete_rows_exceeded",
            "more distinct rows matched than the complete fetch accepts",
        ));
    }
    let (graph, rows) = finish_hydrated_rows(graph, drafts).map_err(shared)?;
    match &request.request().operation {
        MatchOperation::FetchRows { .. } => Ok(ModelOutcome::HydratedRows { graph, rows }),
        MatchOperation::PageBy { root, window, .. } => Ok(ModelOutcome::HydratedPage {
            graph,
            entries: rows,
            root: type_bridge_contract::migration_assertion::BindingId::new(root.get())?,
            limit: window.limit,
            offset: window.offset,
            total: total
                .map(|index| count(stack, answers, index))
                .transpose()?,
        }),
        _ => Err(failure(
            "query_plan_result_operation",
            "hydrated rows answer a different match operation",
        )),
    }
}

fn scalar_rows(
    stack: &AdaptedMatchRequest,
    answers: &[PlanAnswer],
    index: usize,
) -> Result<Vec<Vec<RemoteValue>>, Diagnostic> {
    let plan = stack
        .plans
        .get(index)
        .ok_or_else(|| failure("query_plan_result_stack", "reduction plan is missing"))?;
    let answer = answers
        .get(index)
        .ok_or_else(|| failure("query_plan_result_stack", "reduction answer is missing"))?
        .to_staged_v3(plan)?;
    answer.validate_for(plan)?;
    match answer {
        StagedAnswer::Rows { rows } => Ok(rows
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|value| match value {
                        RemoteValue::Attribute { value, .. } => RemoteValue::Value { value },
                        value => value,
                    })
                    .collect()
            })
            .collect()),
        _ => Err(failure(
            "query_plan_result_reduction",
            "reduction returned the wrong answer kind",
        )),
    }
}

fn reduction_key(values: &[RemoteValue]) -> Result<String, Diagnostic> {
    serde_json::to_string(values).map_err(|_| {
        failure(
            "query_plan_result_reduction",
            "reduction key cannot be encoded",
        )
    })
}

fn reduced_cell(
    value: Option<&RemoteValue>,
    size: u64,
    term: &type_bridge_contract::query_plan::ModelReductionTerm,
) -> Result<type_bridge_contract::query_model_evidence::ModelReducedValue, Diagnostic> {
    use type_bridge_contract::query_model_evidence::ModelReducedValue as Cell;
    use type_bridge_contract::query_plan::ModelReductionKind as Kind;
    use type_bridge_contract::value::ValueTypeTag;
    if term.reduction() == Kind::Count {
        return Ok(Cell::Count { value: size });
    }
    let absent = size == 0 || (term.reduction() == Kind::Std && size < 2);
    let double = matches!(term.reduction(), Kind::Mean | Kind::Median | Kind::Std)
        || term
            .input()
            .is_some_and(|field| field.value_type() == ValueTypeTag::Double);
    if absent {
        return Ok(if double {
            Cell::DoubleBits {
                value: (term.reduction() == Kind::Sum).then_some(0.0_f64.to_bits()),
            }
        } else {
            Cell::Long {
                value: (term.reduction() == Kind::Sum).then_some(0),
            }
        });
    }
    match value {
        Some(RemoteValue::Value {
            value: CanonicalValue::Long(value),
        }) if !double => Ok(Cell::Long {
            value: Some(*value),
        }),
        Some(RemoteValue::Value {
            value: CanonicalValue::Double(value),
        }) if double => Ok(Cell::DoubleBits {
            value: Some(value.get().to_bits()),
        }),
        _ => Err(failure(
            "query_plan_result_reduction",
            "reduction cell has the wrong numeric domain",
        )),
    }
}

fn exact_long_total(
    low: Option<&RemoteValue>,
    high: Option<&RemoteValue>,
    count: u64,
) -> Result<i128, Diagnostic> {
    use super::plan_adapter::{LONG_SUM_BASE, LONG_SUM_MAX_INPUTS};
    if count > LONG_SUM_MAX_INPUTS {
        return Err(crate::plan_lowering::failure(
            DiagnosticCategory::ResourceLimit,
            "reduction_input_bound",
            "integer sum exceeds its exact native input bound",
        )
        .with_detail("limit", "reduction_input_bound")
        .with_detail("maximum", LONG_SUM_MAX_INPUTS as i64)
        .with_detail("source", type_bridge_contract::limits::LIMIT_SOURCE_DEFAULT));
    }
    let integer = |value: Option<&RemoteValue>| match value {
        Some(RemoteValue::Value {
            value: CanonicalValue::Long(value),
        }) => Ok(*value),
        _ => Err(failure(
            "query_plan_result_reduction",
            "integer sum partial is not a long",
        )),
    };
    Ok(i128::from(integer(high)?) * i128::from(LONG_SUM_BASE) + i128::from(integer(low)?))
}

fn checked_long_sum(
    low: Option<&RemoteValue>,
    high: Option<&RemoteValue>,
    count: u64,
) -> Result<type_bridge_contract::query_model_evidence::ModelReducedValue, Diagnostic> {
    let value = i64::try_from(exact_long_total(low, high, count)?).map_err(|_| {
        crate::plan_lowering::failure(
            DiagnosticCategory::ResourceLimit,
            "reduction_overflow",
            "integer sum left the canonical long domain",
        )
    })?;
    Ok(type_bridge_contract::query_model_evidence::ModelReducedValue::Long { value: Some(value) })
}

fn resolve_reduction(
    stack: &AdaptedMatchRequest,
    answers: &[PlanAnswer],
    limits: QueryExecutionResourceLimits,
) -> Result<ModelOutcome, Diagnostic> {
    use type_bridge_contract::query_model_evidence::{ModelReductionGroup, ModelReductionRow};
    use type_bridge_contract::query_plan::{ModelReductionKey, ModelValue};
    let Layout::Reduction {
        root,
        group,
        reducers,
        anchor,
        terms,
        hydration,
    } = &stack.layout
    else {
        unreachable!()
    };
    let width = match group {
        None => 0,
        Some(ModelReductionKey::Fields { fields }) => fields.len(),
        Some(_) => 1,
    };
    let mut groups = BTreeMap::new();
    for row in scalar_rows(stack, answers, *anchor)? {
        let Some(RemoteValue::Value {
            value: CanonicalValue::Long(count),
        }) = row.get(width)
        else {
            return Err(failure(
                "query_plan_result_reduction",
                "reduction anchor has no count",
            ));
        };
        let count = u64::try_from(*count)
            .map_err(|_| failure("query_plan_result_reduction", "reduction count is negative"))?;
        if groups
            .insert(
                reduction_key(&row[..width])?,
                (row[..width].to_vec(), count, Vec::new()),
            )
            .is_some()
        {
            return Err(failure(
                "query_plan_result_reduction",
                "reduction group is duplicated",
            ));
        }
    }
    let mut inputs = BTreeMap::new();
    for index in terms
        .iter()
        .flatten()
        .map(|source| source.plan)
        .collect::<BTreeSet<_>>()
    {
        let mut seen = BTreeSet::new();
        let rows = scalar_rows(stack, answers, index)?
            .into_iter()
            .map(|row| {
                let key = reduction_key(&row[..width])?;
                if !seen.insert(key.clone()) {
                    return Err(failure(
                        "query_plan_result_reduction",
                        "reduction group is duplicated",
                    ));
                }
                Ok((key, row))
            })
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        inputs.insert(index, rows);
    }
    for (term, source) in reducers.iter().zip(terms) {
        for (_, count, values) in groups.values_mut() {
            values.push(if source.is_none() {
                type_bridge_contract::query_model_evidence::ModelReducedValue::Count {
                    value: *count,
                }
            } else {
                reduced_cell(None, 0, term)?
            });
        }
        let Some(source) = source else {
            continue;
        };
        let mut observed = BTreeSet::new();
        for (key, row) in &inputs[&source.plan] {
            let Some((_, count, values)) = groups.get_mut(key) else {
                return Err(failure(
                    "query_plan_result_window",
                    "reduction returned an unexpected group",
                ));
            };
            let Some(RemoteValue::Value {
                value: CanonicalValue::Long(size),
            }) = row.get(source.count)
            else {
                return Err(failure(
                    "query_plan_result_reduction",
                    "reduction input count is missing",
                ));
            };
            let size = u64::try_from(*size).map_err(|_| {
                failure(
                    "query_plan_result_reduction",
                    "reduction input count is negative",
                )
            })?;
            if size > *count {
                return Err(failure(
                    "query_plan_result_window",
                    "reduction input count exceeds the root count",
                ));
            }
            *values.last_mut().ok_or_else(|| {
                failure("query_plan_result_reduction", "reduction cell is missing")
            })? = if let Some(high) = source.sum_high {
                if term.reduction() == type_bridge_contract::query_plan::ModelReductionKind::Mean {
                    let total = exact_long_total(row.get(source.value), row.get(high), size)?;
                    type_bridge_contract::query_model_evidence::ModelReducedValue::DoubleBits {
                        value: (size > 0).then(|| (total as f64 / size as f64).to_bits()),
                    }
                } else {
                    checked_long_sum(row.get(source.value), row.get(high), size)?
                }
            } else {
                reduced_cell(row.get(source.value), size, term)?
            };
            observed.insert(key);
        }
        if !source.optional
            && groups
                .iter()
                .any(|(key, (_, count, _))| *count > 0 && !observed.contains(key))
        {
            return Err(failure(
                "query_plan_result_window",
                "reduction omitted a mandatory input group",
            ));
        }
    }
    let authority = HydrationAuthority::new(&stack.hydration).map_err(shared)?;
    let reply_limits = ModelReplyDecodeLimits {
        max_bytes: limits.bytes,
        max_items: limits.items,
        max_graph_nodes: limits.graph_nodes,
        max_attribute_values: limits.attribute_values,
        max_collection_members: limits.collection_members,
        max_role_players: limits.role_players,
    };
    let mut graph = GraphBuilder::new(reply_limits);
    let mut expected = BTreeSet::new();
    if matches!(group, Some(ModelReductionKey::Binding { .. })) {
        for (keys, _, _) in groups.values() {
            let [RemoteValue::Thing { iid, .. }] = keys.as_slice() else {
                return Err(failure(
                    "query_plan_result_reduction",
                    "group key is not a thing",
                ));
            };
            expected.insert(iid.clone());
        }
    }
    let mut observed = BTreeSet::new();
    for (index, binding, _) in hydration {
        let plan = &stack.plans[*index];
        let docs = documents(answers, *index)?;
        validate_plan_documents(plan, docs, limits).map_err(shared)?;
        observed.extend(
            insert_plan_documents(&mut graph, &authority, binding.get(), plan, docs)
                .map_err(shared)?,
        );
    }
    if expected != observed {
        return Err(failure(
            "query_plan_result_window",
            "reduction group hydration differs from selected groups",
        ));
    }
    let finished = graph.finish(&expected).map_err(shared)?;
    let mut rows = Vec::new();
    for (keys, _, values) in groups.into_values() {
        let key = match group {
            None => None,
            Some(ModelReductionKey::Binding { binding }) => {
                let [RemoteValue::Thing { iid, .. }] = keys.as_slice() else {
                    return Err(failure(
                        "query_plan_result_reduction",
                        "group key is not a thing",
                    ));
                };
                Some(ModelReductionGroup::thing(
                    finished
                        .reference(
                            authority
                                .binding(binding.get())
                                .map_err(shared)?
                                .declared_descriptor(),
                            iid,
                        )
                        .map_err(shared)?,
                ))
            }
            Some(_) => {
                let keys = keys
                    .into_iter()
                    .map(|value| match value {
                        RemoteValue::Value { value } => Ok(ModelValue::from(value)),
                        _ => Err(failure(
                            "query_plan_result_reduction",
                            "group key is not a scalar",
                        )),
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Some(if matches!(group, Some(ModelReductionKey::Field { .. })) {
                    ModelReductionGroup::field(keys.into_iter().next().unwrap())
                } else {
                    ModelReductionGroup::fields(keys)
                })
            }
        };
        rows.push(ModelReductionRow::new(key, values));
    }
    rows.sort_by(|left, right| match (left.group(), right.group()) {
        (
            Some(ModelReductionGroup::Thing { value: left }),
            Some(ModelReductionGroup::Thing { value: right }),
        ) => left.node().cmp(&right.node()),
        (
            Some(ModelReductionGroup::Field { value: left }),
            Some(ModelReductionGroup::Field { value: right }),
        ) => left
            .semantic_cmp_same_domain(right)
            .unwrap_or(std::cmp::Ordering::Equal),
        (
            Some(ModelReductionGroup::Fields { values: left }),
            Some(ModelReductionGroup::Fields { values: right }),
        ) => left
            .iter()
            .zip(right)
            .map(|(left, right)| {
                left.semantic_cmp_same_domain(right)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .find(|order| !order.is_eq())
            .unwrap_or(std::cmp::Ordering::Equal),
        _ => std::cmp::Ordering::Equal,
    });
    Ok(ModelOutcome::ModelReduction {
        graph: finished.graph,
        root: *root,
        group: group.clone(),
        reducers: reducers.clone(),
        rows,
    })
}

#[cfg(test)]
mod sum_tests {
    use super::super::plan_adapter::LONG_SUM_MAX_INPUTS;
    use super::*;
    fn long(value: i64) -> RemoteValue {
        RemoteValue::Value {
            value: CanonicalValue::Long(value),
        }
    }

    #[test]
    fn native_long_sum_halves_preserve_signs_extremes_and_final_domain() {
        for (low, high, count, expected) in [
            (4294967295, 2147483647, 1, i64::MAX),
            (0, -2147483648, 1, i64::MIN),
            (-4294967295, -2147483647, 1, i64::MIN + 1),
            (1, -2147483648, 1, i64::MIN + 1),
            (-1, 0, 1, -1),
            (4294967295, -1, 1, -1),
            (4294967295, -1, 2, -1),
            (4294967296, -1, 3, 0),
            (0, 0, 0, 0),
            (-2147483648, 0, LONG_SUM_MAX_INPUTS, -2147483648),
        ] {
            let value = checked_long_sum(Some(&long(low)), Some(&long(high)), count).unwrap();
            assert_eq!(
                value,
                type_bridge_contract::query_model_evidence::ModelReducedValue::Long {
                    value: Some(expected)
                }
            );
        }
        for (low, high) in [(8589934590, 4294967294), (0, -4294967296)] {
            let error = checked_long_sum(Some(&long(low)), Some(&long(high)), 2).unwrap_err();
            assert_eq!(error.code().as_str(), "reduction_overflow");
            assert_eq!(error.category(), DiagnosticCategory::ResourceLimit);
        }
    }

    #[test]
    fn native_long_mean_uses_exact_total_even_when_the_long_sum_would_overflow() {
        for (low, high, expected) in [
            (8589934590, 4294967294, i64::MAX as f64),
            (0, -4294967296, i64::MIN as f64),
            (4294967295, -1, -0.5),
        ] {
            let total = exact_long_total(Some(&long(low)), Some(&long(high)), 2).unwrap();
            assert_eq!((total as f64 / 2.0).to_bits(), expected.to_bits());
        }
    }

    #[test]
    fn native_long_sum_checks_the_same_reduce_count_before_its_partial_cells() {
        let error = checked_long_sum(None, None, LONG_SUM_MAX_INPUTS + 1).unwrap_err();
        assert_eq!(error.code().as_str(), "reduction_input_bound");
        assert_eq!(
            error.details().get("maximum"),
            Some(&type_bridge_contract::diagnostic::DiagnosticDetailValue::Long(2147483648))
        );
        assert_eq!(
            error.details().get("limit"),
            Some(
                &type_bridge_contract::diagnostic::DiagnosticDetailValue::Text(
                    "reduction_input_bound".into()
                )
            )
        );
        assert_eq!(
            checked_long_sum(None, Some(&long(0)), 1)
                .unwrap_err()
                .code()
                .as_str(),
            "query_plan_result_reduction"
        );
    }
}
