//! The one result-validation path from a V2 outcome to a released result.
//!
//! This module performs no I/O and owns no provider. Every live query, direct
//! or remote, ends in [`validated_match_result_from_v2`], which maps one
//! contract-validated [`RemoteOutcomeV2`] onto the exact
//! [`ValidatedMatchRequest`] it answers and atomically constructs
//! [`ValidatedMatchResult`]. No partial rows escape on failure.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::query_plan::{
    CompatibilityValueV2, QueryFieldV2, QueryReductionGroupV2, QueryReductionKindV2,
    QueryReductionTermV2, ReleasedValueKindV2,
};
use type_bridge_contract::query_remote_v2::{
    HydrationGraphV2, HydrationNodeKindV2, HydrationReferenceV2, HydrationSlotV2, RemoteOutcomeV2,
    RemoteReducedValueV2, RemoteReductionGroupV2, RemoteReductionRowV2,
};
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration,
};
use type_bridge_contract::value::CanonicalValue;
use type_bridge_core_lib::decimal::parse_decimal;
use unicase::UniCase;

use crate::_descriptor::{OwnedAttributeDescriptor, TypeDescriptorRef};
use crate::_registry::DescriptorRegistry;
use crate::value::AttributeValue;

use super::error::{MatchError, MatchErrorCategory, MatchErrorPathSegment};
use super::ids::{BindingId, BoundFieldId, DescriptorId, RoleId};
use super::model::{
    ComparisonOp, FetchShape, FetchSlot, MatchExpr, MatchMode, MatchOperation, ReduceTerm,
    Reduction, RowCardinality, ThingKind, Window,
};
use super::result::{
    ConceptId, HydratedAttribute, HydratedRole, HydratedRolePlayer, HydratedThing, MatchResult,
    MatchRow, ReducedValue, ReductionRow, SlotValue, ValidatedMatchResult,
};
use super::validation::ValidatedMatchRequest;

/// Hard ceiling for scalar group-key values across one reduction result.
const MAX_REDUCTION_GROUP_VALUES: usize = 4_000_000;
/// Hard in-memory byte ceiling for one reduction result's group keys.
const MAX_REDUCTION_GROUP_BYTES: usize = 64 * 1024 * 1024;

/// Construction authority held exclusively by canonical result validation.
///
/// The type is visible to the proof wrapper, but its field is private so
/// executors and bindings cannot fabricate validated result evidence.
pub(super) struct ValidatedResultSeal(());

/// Validate typed reduction rows against the validated reduction operation.
///
/// The caller has already proven that the provider echoed the exact root,
/// group, and reducer contract; this checks each row's group evidence, group
/// key domain and uniqueness, and reducer value domains.
fn validate_reduction_rows(
    registry: &DescriptorRegistry,
    validated: &ValidatedMatchRequest,
    rows: Vec<ReductionRow>,
) -> Result<MatchResult, MatchError> {
    let mut budget = EvidenceBudget::default();
    Ok(match &validated.request().operation {
        MatchOperation::ReduceBy {
            root,
            group,
            reducers,
        } => {
            match group {
                None => {
                    if rows.len() != 1 || rows[0].has_group_evidence() {
                        return Err(result_error(
                            "reduction_ungrouped_shape",
                            "ungrouped reductions require exactly one keyless row",
                        ));
                    }
                }
                Some(_) => {
                    for row in &rows {
                        if row.group().is_none() || row.field_group().is_some() {
                            return Err(result_error(
                                "reduction_group_key_missing",
                                "grouped reduction rows require group evidence",
                            ));
                        }
                    }
                }
            }
            validate_reduction_values(&rows, reducers)?;
            MatchResult::Reduction {
                root: *root,
                group: *group,
                rows,
            }
        }
        MatchOperation::ReduceByField {
            root,
            group,
            reducers,
        } => {
            let field_value_type = reduction_group_field_value_type(registry, validated, group)?;
            let mut witnessed = BTreeSet::new();
            for row in &rows {
                if row.group().is_some() {
                    return Err(result_error(
                        "field_reduction_group_kind",
                        "field-grouped reduction rows cannot carry thing group evidence",
                    ));
                }
                let value = row.field_group().ok_or_else(|| {
                    result_error(
                        "field_reduction_group_key_missing",
                        "field-grouped reduction rows require scalar group evidence",
                    )
                })?;
                budget.charge_attribute_values(std::slice::from_ref(value))?;
                if field_value_type != value.value_type_name() || !safe_value(value) {
                    return Err(result_error(
                        "field_reduction_group_value_type",
                        "field-grouped reduction value does not fit its projected field",
                    )
                    .at(MatchErrorPathSegment::Field(group.field.clone())));
                }
                let canonical = canonicalize_provider_attribute_value(value.clone())?;
                if canonical != *value {
                    return Err(result_error(
                        "field_reduction_group_value_noncanonical",
                        "field-grouped reduction value is not canonical",
                    )
                    .at(MatchErrorPathSegment::Field(group.field.clone())));
                }
                if !witnessed.insert(reduction_group_value_key(value)?) {
                    return Err(result_error(
                        "field_reduction_group_duplicate",
                        "field-grouped reduction returned one value more than once",
                    )
                    .at(MatchErrorPathSegment::Field(group.field.clone())));
                }
            }
            validate_reduction_values(&rows, reducers)?;
            MatchResult::FieldReduction {
                root: *root,
                group: group.clone(),
                rows,
            }
        }
        MatchOperation::ReduceByFields {
            root,
            groups,
            reducers,
        } => {
            let field_value_types = groups
                .iter()
                .map(|group| reduction_group_field_value_type(registry, validated, group))
                .collect::<Result<Vec<_>, _>>()?;
            let mut witnessed = BTreeSet::new();
            for row in &rows {
                if row.group().is_some() || row.field_group().is_some() {
                    return Err(result_error(
                        "field_tuple_reduction_group_kind",
                        "tuple-field-grouped reduction rows cannot carry singular group evidence",
                    ));
                }
                let values = row.field_groups().ok_or_else(|| {
                    result_error(
                        "field_tuple_reduction_group_key_missing",
                        "tuple-field-grouped reduction rows require tuple group evidence",
                    )
                })?;
                if values.len() != groups.len() {
                    return Err(result_error(
                        "field_tuple_reduction_group_arity",
                        "tuple-field-grouped reduction row has the wrong key arity",
                    ));
                }
                budget.charge_attribute_values(values)?;
                let mut key = Vec::with_capacity(values.len());
                for ((value, value_type), group) in
                    values.iter().zip(&field_value_types).zip(groups)
                {
                    if value_type != value.value_type_name() || !safe_value(value) {
                        return Err(result_error(
                            "field_tuple_reduction_group_value_type",
                            "tuple-field-grouped reduction value does not fit its projected field",
                        )
                        .at(MatchErrorPathSegment::Field(group.field.clone())));
                    }
                    let canonical = canonicalize_provider_attribute_value(value.clone())?;
                    if canonical != *value {
                        return Err(result_error(
                            "field_tuple_reduction_group_value_noncanonical",
                            "tuple-field-grouped reduction value is not canonical",
                        )
                        .at(MatchErrorPathSegment::Field(group.field.clone())));
                    }
                    key.push(reduction_group_value_key(value)?);
                }
                if !witnessed.insert(key) {
                    return Err(result_error(
                        "field_tuple_reduction_group_duplicate",
                        "tuple-field-grouped reduction returned one value tuple more than once",
                    ));
                }
            }
            validate_reduction_values(&rows, reducers)?;
            MatchResult::FieldTupleReduction {
                root: *root,
                groups: groups.clone(),
                rows,
            }
        }
        _ => {
            return Err(result_error(
                "result_operation_mismatch",
                "provider result variant does not match the validated operation",
            ));
        }
    })
}

fn validate_reduction_values(
    rows: &[super::result::ReductionRow],
    reducers: &[super::model::ReduceTerm],
) -> Result<(), MatchError> {
    for row in rows {
        if row.values().len() != reducers.len() {
            return Err(result_error(
                "reduction_arity_mismatch",
                "reduction rows must carry one value per requested reducer",
            ));
        }
        for (value, term) in row.values().iter().zip(reducers) {
            let variant_valid = matches!(
                (term.reduction, value),
                (Reduction::Count, ReducedValue::Count(_))
                    | (
                        Reduction::Sum | Reduction::Min | Reduction::Max,
                        ReducedValue::Long(_) | ReducedValue::Double(_),
                    )
                    | (
                        Reduction::Mean | Reduction::Median | Reduction::Std,
                        ReducedValue::Double(_),
                    )
            );
            if !variant_valid {
                return Err(result_error(
                    "reduction_value_domain",
                    "reduction value variant does not fit its reducer",
                ));
            }
        }
    }
    Ok(())
}

fn reduction_group_field_value_type(
    registry: &DescriptorRegistry,
    validated: &ValidatedMatchRequest,
    group: &BoundFieldId,
) -> Result<String, MatchError> {
    let binding = validated
        .request()
        .plan
        .bindings
        .iter()
        .find(|binding| binding.id == group.binding)
        .ok_or_else(|| {
            result_error("field_reduction_binding_missing", "group binding is absent")
        })?;
    let type_name = registry
        .descriptor_type_name(&binding.descriptor)
        .ok_or_else(|| {
            result_error(
                "field_reduction_descriptor_missing",
                "group descriptor is absent",
            )
        })?;
    let descriptor = registry.get(&type_name).ok_or_else(|| {
        result_error(
            "field_reduction_descriptor_missing",
            "group descriptor is absent",
        )
    })?;
    descriptor_attributes(&descriptor)
        .iter()
        .find(|field| field.field_name == group.field.name)
        .map(|field| field.value_type.as_str().to_owned())
        .ok_or_else(|| {
            result_error(
                "field_reduction_field_missing",
                "group field is absent from its validated descriptor",
            )
            .at(MatchErrorPathSegment::Field(group.field.clone()))
        })
}

fn reduction_group_value_key(value: &AttributeValue) -> Result<String, MatchError> {
    Ok(match value {
        AttributeValue::String(value) => format!("string:{value:?}"),
        AttributeValue::Long(value) => format!("long:{value}"),
        AttributeValue::Double(value) if value.is_finite() => {
            let bits = if *value == 0.0 { 0 } else { value.to_bits() };
            format!("double:{bits:016x}")
        }
        AttributeValue::Double(_) => {
            return Err(result_error(
                "field_reduction_group_value_type",
                "field-grouped reduction received a non-finite double",
            ));
        }
        AttributeValue::Boolean(value) => format!("boolean:{value}"),
        AttributeValue::Date(value) => format!("date:{value}"),
        AttributeValue::DateTime(value) => format!("datetime:{value}"),
        AttributeValue::DateTimeTZ(value) => format!("datetime-tz:{value}"),
        AttributeValue::Decimal(value) => format!("decimal:{value}"),
        AttributeValue::Duration(value) => format!("duration:{value}"),
    })
}

/// Convert one fully contract-validated V2 compatibility outcome back into
/// the released result object without reconstructing omitted hidden bindings.
///
/// The minimal V2 hydration graph intentionally carries only selected output
/// and shallow role-player closure. The V2 validator has already proven the
/// complete model projection; this function checks the released operation
/// contract (root, window, total, cardinality, reduction echo and values) and
/// performs the lossless released representation mapping.
pub(crate) fn validated_match_result_from_v2(
    registry: &DescriptorRegistry,
    validated: &ValidatedMatchRequest,
    outcome: RemoteOutcomeV2,
) -> Result<ValidatedMatchResult, MatchError> {
    validated.recheck_schema(registry)?;
    let result = match (&validated.request().operation, outcome) {
        (
            MatchOperation::FetchRows {
                output,
                window,
                cardinality,
                ..
            },
            RemoteOutcomeV2::HydratedRows { graph, rows },
        ) => {
            let rows = rows
                .iter()
                .map(|row| released_row_for_output(registry, &graph, row.slots(), output))
                .collect::<Result<Vec<_>, _>>()?;
            if *cardinality == RowCardinality::ExactlyOne
                && let Some(error) = exactly_one_cardinality_error(rows.len())
            {
                return Err(error);
            }
            if u64::try_from(rows.len()).unwrap_or(u64::MAX) > window.limit {
                return Err(resource_error(
                    "row_window_exceeded",
                    "provider returned more distinct rows than the validated window permits",
                ));
            }
            MatchResult::Rows { rows }
        }
        (
            MatchOperation::PageBy {
                root: expected_root,
                output,
                window,
                include_total,
                ..
            },
            RemoteOutcomeV2::HydratedPage {
                entries,
                graph,
                limit,
                offset,
                root,
                total,
            },
        ) => {
            let actual_root = BindingId::new(root.get());
            if actual_root != *expected_root {
                return Err(result_error(
                    "page_root_mismatch",
                    "provider page root does not match the validated operation",
                )
                .at(MatchErrorPathSegment::Binding(actual_root)));
            }
            if (Window { offset, limit }) != *window {
                return Err(result_error(
                    "page_window_mismatch",
                    "provider page window does not match the validated operation",
                ));
            }
            match (*include_total, total) {
                (true, None) => {
                    return Err(result_error(
                        "missing_page_total",
                        "provider omitted a requested same-snapshot page total",
                    ));
                }
                (false, Some(_)) => {
                    return Err(result_error(
                        "unexpected_page_total",
                        "provider returned a page total that was not requested",
                    ));
                }
                _ => {}
            }
            let entries = entries
                .iter()
                .map(|row| released_row_for_output(registry, &graph, row.slots(), output))
                .collect::<Result<Vec<_>, _>>()?;
            if u64::try_from(entries.len()).unwrap_or(u64::MAX) > window.limit {
                return Err(resource_error(
                    "page_window_exceeded",
                    "root selection returned more distinct roots than the validated page window permits",
                ));
            }
            if let Some(total) = total {
                let expected = window.limit.min(total.saturating_sub(window.offset));
                let actual = u64::try_from(entries.len()).unwrap_or(u64::MAX);
                if actual != expected {
                    return Err(result_error(
                        "page_total_length_mismatch",
                        "provider page length is inconsistent with its same-snapshot total and window",
                    ));
                }
            }
            MatchResult::Page {
                root: *expected_root,
                entries,
                window: *window,
                total,
            }
        }
        (
            MatchOperation::CountBy { root: expected },
            RemoteOutcomeV2::DistinctCount { root, value },
        ) => {
            let actual = BindingId::new(root.get());
            require_root(*expected, actual, "count_root_mismatch")?;
            MatchResult::Count {
                root: *expected,
                value,
            }
        }
        (
            MatchOperation::ExistsBy { root: expected },
            RemoteOutcomeV2::DistinctExists { root, value },
        ) => {
            let actual = BindingId::new(root.get());
            require_root(*expected, actual, "exists_root_mismatch")?;
            MatchResult::Exists {
                root: *expected,
                value,
            }
        }
        (
            operation @ (MatchOperation::ReduceBy { .. }
            | MatchOperation::ReduceByField { .. }
            | MatchOperation::ReduceByFields { .. }),
            RemoteOutcomeV2::ModelReduction {
                graph,
                root,
                group,
                reducers,
                rows,
            },
        ) => released_reduction_result(
            registry, validated, operation, &graph, root, group, reducers, rows,
        )?,
        _ => {
            return Err(result_error(
                "result_operation_mismatch",
                "adapted V2 result variant does not match the validated released operation",
            ));
        }
    };
    Ok(ValidatedMatchResult::new(
        ValidatedResultSeal(()),
        validated.request_token(),
        validated.shape_id().clone(),
        result,
    ))
}

#[allow(clippy::too_many_arguments)]
fn released_reduction_result(
    registry: &DescriptorRegistry,
    validated: &ValidatedMatchRequest,
    operation: &MatchOperation,
    graph: &HydrationGraphV2,
    root: type_bridge_contract::migration_assertion::BindingId,
    group: Option<QueryReductionGroupV2>,
    reducers: Vec<QueryReductionTermV2>,
    rows: Vec<RemoteReductionRowV2>,
) -> Result<MatchResult, MatchError> {
    let actual_root = BindingId::new(root.get());
    let echoed_reducers = reducers
        .iter()
        .map(|term| released_reducer(registry, term))
        .collect::<Result<Vec<_>, _>>()?;
    match operation {
        MatchOperation::ReduceBy {
            root,
            group: expected_group,
            reducers,
        } => {
            require_root(*root, actual_root, "reduction_root_mismatch")?;
            let actual_group = match group {
                None => None,
                Some(QueryReductionGroupV2::Binding { binding }) => {
                    Some(BindingId::new(binding.get()))
                }
                Some(
                    QueryReductionGroupV2::Field { .. } | QueryReductionGroupV2::Fields { .. },
                ) => {
                    return Err(result_error(
                        "reduction_group_mismatch",
                        "remote reduction echoed the wrong group contract kind",
                    ));
                }
            };
            if actual_group != *expected_group || &echoed_reducers != reducers {
                return Err(result_error(
                    "reduction_contract_mismatch",
                    "remote reduction did not echo the exact validated group and reducer contract",
                ));
            }
        }
        MatchOperation::ReduceByField {
            root,
            group: expected_group,
            reducers,
        } => {
            require_root(*root, actual_root, "field_reduction_root_mismatch")?;
            let Some(QueryReductionGroupV2::Field { field }) = group else {
                return Err(result_error(
                    "field_reduction_group_mismatch",
                    "remote reduction echoed the wrong field group contract kind",
                ));
            };
            let actual_group = released_query_field(registry, &field)?;
            if &actual_group != expected_group || &echoed_reducers != reducers {
                return Err(result_error(
                    "field_reduction_contract_mismatch",
                    "remote reduction did not echo the exact validated field group and reducer contract",
                ));
            }
        }
        MatchOperation::ReduceByFields {
            root,
            groups: expected_groups,
            reducers,
        } => {
            require_root(*root, actual_root, "field_tuple_reduction_root_mismatch")?;
            let Some(QueryReductionGroupV2::Fields { fields }) = group else {
                return Err(result_error(
                    "field_tuple_reduction_groups_mismatch",
                    "remote reduction echoed the wrong tuple-field group contract kind",
                ));
            };
            let actual_groups = fields
                .iter()
                .map(|field| released_query_field(registry, field))
                .collect::<Result<Vec<_>, _>>()?;
            if &actual_groups != expected_groups || &echoed_reducers != reducers {
                return Err(result_error(
                    "field_tuple_reduction_contract_mismatch",
                    "remote reduction did not echo the exact validated tuple group and reducer contract",
                ));
            }
        }
        _ => {
            return Err(result_error(
                "result_operation_mismatch",
                "remote reduction does not match a validated reduction operation",
            ));
        }
    }
    validate_reduction_rows(
        registry,
        validated,
        released_reduction_rows(registry, graph, rows)?,
    )
}

fn released_reducer(
    registry: &DescriptorRegistry,
    term: &QueryReductionTermV2,
) -> Result<ReduceTerm, MatchError> {
    let reduction = match term.reduction() {
        QueryReductionKindV2::Count => Reduction::Count,
        QueryReductionKindV2::Sum => Reduction::Sum,
        QueryReductionKindV2::Min => Reduction::Min,
        QueryReductionKindV2::Max => Reduction::Max,
        QueryReductionKindV2::Mean => Reduction::Mean,
        QueryReductionKindV2::Median => Reduction::Median,
        QueryReductionKindV2::Std => Reduction::Std,
    };
    Ok(ReduceTerm {
        reduction,
        input: term
            .input()
            .map(|field| released_query_field(registry, field))
            .transpose()?,
    })
}

fn released_query_field(
    registry: &DescriptorRegistry,
    field: &QueryFieldV2,
) -> Result<BoundFieldId, MatchError> {
    let owner = released_descriptor(registry, field.descriptor())?;
    let field_id = registry
        .field_id(&owner, field.attribute().label().as_str())
        .ok_or_else(|| {
            result_error(
                "reduction_contract_field_unknown",
                "remote reduction echoed a field outside the current registry",
            )
        })?;
    Ok(BoundFieldId::new(
        BindingId::new(field.binding().get()),
        field_id,
    ))
}

fn released_reduction_rows(
    registry: &DescriptorRegistry,
    graph: &HydrationGraphV2,
    rows: Vec<RemoteReductionRowV2>,
) -> Result<Vec<ReductionRow>, MatchError> {
    rows.into_iter()
        .map(|row| {
            let values = row
                .values()
                .iter()
                .map(released_reduced_value)
                .collect::<Result<Vec<_>, _>>()?;
            match row.group() {
                None => Ok(ReductionRow::new(None, values)),
                Some(RemoteReductionGroupV2::Thing { value }) => Ok(ReductionRow::new(
                    Some(released_thing(registry, graph, value)?),
                    values,
                )),
                Some(RemoteReductionGroupV2::Field { value }) => Ok(ReductionRow::new_field(
                    released_attribute_value(value)?,
                    values,
                )),
                Some(RemoteReductionGroupV2::Fields { values: group }) => {
                    Ok(ReductionRow::new_fields(
                        group
                            .iter()
                            .map(released_attribute_value)
                            .collect::<Result<Vec<_>, _>>()?,
                        values,
                    ))
                }
            }
        })
        .collect()
}

fn released_reduced_value(value: &RemoteReducedValueV2) -> Result<ReducedValue, MatchError> {
    Ok(match value {
        RemoteReducedValueV2::Count { value } => ReducedValue::Count(*value),
        RemoteReducedValueV2::Long { value } => ReducedValue::Long(*value),
        RemoteReducedValueV2::DoubleBits { value } => {
            let value = value.map(f64::from_bits);
            if value.is_some_and(|value| !value.is_finite()) {
                return Err(result_error(
                    "reduction_value_nonfinite",
                    "remote reduction double bits are outside the finite domain",
                ));
            }
            ReducedValue::Double(value)
        }
    })
}

fn released_row(
    registry: &DescriptorRegistry,
    graph: &HydrationGraphV2,
    slots: &[HydrationSlotV2],
) -> Result<MatchRow, MatchError> {
    let slots = slots
        .iter()
        .map(|slot| match slot {
            HydrationSlotV2::Singular { value } => {
                released_thing(registry, graph, value).map(SlotValue::One)
            }
            HydrationSlotV2::Collection { values } => values
                .iter()
                .map(|value| released_thing(registry, graph, value))
                .collect::<Result<Vec<_>, _>>()
                .map(SlotValue::Many),
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(MatchRow::new(slots))
}

fn released_row_for_output(
    registry: &DescriptorRegistry,
    graph: &HydrationGraphV2,
    slots: &[HydrationSlotV2],
    output: &FetchShape,
) -> Result<MatchRow, MatchError> {
    let expected = output_slots(output).collect::<Vec<_>>();
    if slots.len() != expected.len()
        || slots.iter().zip(expected).any(|(actual, expected)| {
            matches!(
                (actual, expected),
                (HydrationSlotV2::Singular { .. }, FetchSlot::Collect { .. })
                    | (HydrationSlotV2::Collection { .. }, FetchSlot::One { .. })
            )
        })
    {
        return Err(result_error(
            "result_operation_mismatch",
            "adapted V2 row shape does not match the validated released output",
        ));
    }
    released_row(registry, graph, slots)
}

fn released_thing(
    registry: &DescriptorRegistry,
    graph: &HydrationGraphV2,
    reference: &HydrationReferenceV2,
) -> Result<HydratedThing, MatchError> {
    let node = graph_node(graph, reference)?;
    let declared = released_descriptor(registry, reference.declared())?;
    let concrete = released_descriptor(registry, node.concrete())?;
    let attributes = released_attributes(registry, &concrete, node.attributes())?;
    let roles = node
        .roles()
        .iter()
        .map(|role| {
            let owner = released_descriptor(
                registry,
                &TypeId::new(
                    TypeKind::Relation,
                    role.role().declaring_relation().as_str().to_owned(),
                )
                .map_err(|_| {
                    result_error(
                        "malformed_hydrated_descriptor",
                        "hydration role owner is not a valid relation descriptor",
                    )
                })?,
            )?;
            let players = role
                .players()
                .iter()
                .map(|player| released_role_player(registry, graph, player))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(HydratedRole::new(
                RoleId::new(owner, role.role().label().as_str()),
                players,
            ))
        })
        .collect::<Result<Vec<_>, MatchError>>()?;
    Ok(HydratedThing::new(
        ConceptId::new(node.iid()),
        declared,
        concrete,
        released_kind(node.kind()),
        attributes,
        roles,
    ))
}

fn released_role_player(
    registry: &DescriptorRegistry,
    graph: &HydrationGraphV2,
    reference: &HydrationReferenceV2,
) -> Result<HydratedRolePlayer, MatchError> {
    let node = graph_node(graph, reference)?;
    let declared = released_descriptor(registry, reference.declared())?;
    let concrete = released_descriptor(registry, node.concrete())?;
    let attributes = released_attributes(registry, &concrete, node.attributes())?;
    Ok(HydratedRolePlayer::new(
        ConceptId::new(node.iid()),
        declared,
        concrete,
        released_kind(node.kind()),
        attributes,
    ))
}

fn graph_node<'graph>(
    graph: &'graph HydrationGraphV2,
    reference: &HydrationReferenceV2,
) -> Result<&'graph type_bridge_contract::query_remote_v2::HydrationNodeV2, MatchError> {
    usize::try_from(reference.node().get())
        .ok()
        .and_then(|index| graph.nodes().get(index))
        .ok_or_else(|| {
            result_error(
                "unknown_hydrated_descriptor",
                "hydration graph reference has no dense node",
            )
        })
}

fn released_descriptor(
    registry: &DescriptorRegistry,
    descriptor: &TypeId,
) -> Result<DescriptorId, MatchError> {
    let released = registry
        .descriptor_id(descriptor.label().as_str())
        .ok_or_else(|| {
            result_error(
                "unknown_hydrated_descriptor",
                "hydrated descriptor is not registered in the current schema",
            )
            .with_detail(
                "descriptor",
                format!(
                    "{}:{}",
                    match descriptor.kind() {
                        TypeKind::Entity => "entity",
                        TypeKind::Relation => "relation",
                        TypeKind::Attribute => "attribute",
                        TypeKind::Struct => "struct",
                    },
                    descriptor.label()
                ),
            )
        })?;
    let expected = match descriptor.kind() {
        TypeKind::Entity => "entity:",
        TypeKind::Relation => "relation:",
        TypeKind::Attribute | TypeKind::Struct => {
            return Err(result_error(
                "hydrated_descriptor_kind_mismatch",
                "hydrated model descriptor is not an entity or relation",
            ));
        }
    };
    if released.as_str().starts_with(expected) {
        Ok(released)
    } else {
        Err(result_error(
            "hydrated_descriptor_kind_mismatch",
            "hydrated descriptor kind does not match the registered descriptor",
        ))
    }
}

fn released_attributes(
    registry: &DescriptorRegistry,
    concrete: &DescriptorId,
    attributes: &[type_bridge_contract::query_remote_v2::HydrationAttributeEvidenceV2],
) -> Result<Vec<HydratedAttribute>, MatchError> {
    attributes
        .iter()
        .map(|attribute| {
            let field = registry
                .field_id(concrete, attribute.attribute().label().as_str())
                .ok_or_else(|| {
                    result_error(
                        "unknown_hydrated_attribute",
                        "hydrated attribute is not registered on its concrete descriptor",
                    )
                })?;
            let values = attribute
                .values()
                .iter()
                .map(released_attribute_value)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(HydratedAttribute::new(field, values))
        })
        .collect()
}

pub(crate) fn released_attribute_value(
    value: &CompatibilityValueV2,
) -> Result<AttributeValue, MatchError> {
    let value = if let Some(value) = value.canonical_value() {
        match value {
            CanonicalValue::String(value) => AttributeValue::String(value.as_str().to_owned()),
            CanonicalValue::Long(value) => AttributeValue::Long(*value),
            CanonicalValue::Double(value) => AttributeValue::Double(value.get()),
            CanonicalValue::Boolean(value) => AttributeValue::Boolean(*value),
            CanonicalValue::Date(value) => AttributeValue::Date(value.to_string()),
            CanonicalValue::DateTime(value) => AttributeValue::DateTime(value.to_string()),
            CanonicalValue::DateTimeTz(value) => AttributeValue::DateTimeTZ(value.to_string()),
            CanonicalValue::Decimal(value) => AttributeValue::Decimal(value.to_string()),
            CanonicalValue::Duration(value) => AttributeValue::Duration(value.to_string()),
        }
    } else {
        let text = value.released_text().ok_or_else(|| {
            result_error(
                "hydrated_attribute_value_type",
                "hydrated compatibility value has no released representation",
            )
        })?;
        match value.released_kind() {
            Some(ReleasedValueKindV2::String) => AttributeValue::String(text),
            Some(ReleasedValueKindV2::DateTime) => AttributeValue::DateTime(text),
            Some(ReleasedValueKindV2::DateTimeTz) => AttributeValue::DateTimeTZ(text),
            Some(ReleasedValueKindV2::Duration) => AttributeValue::Duration(text),
            Some(ReleasedValueKindV2::Decimal) => AttributeValue::Decimal(text),
            None => {
                return Err(result_error(
                    "hydrated_attribute_value_type",
                    "hydrated compatibility value has no scalar domain",
                ));
            }
        }
    };
    canonicalize_provider_attribute_value(value)
}

pub(crate) fn canonicalize_provider_attribute_value(
    value: AttributeValue,
) -> Result<AttributeValue, MatchError> {
    let malformed = || {
        result_error(
            "hydrated_attribute_value_type",
            "hydrated attribute value is outside its canonical scalar domain",
        )
    };
    match value {
        AttributeValue::Date(value) => value
            .parse::<CanonicalDate>()
            .map(|value| AttributeValue::Date(value.to_string()))
            .map_err(|_| malformed()),
        AttributeValue::DateTime(value) => normalize_provider_fraction(value)
            .parse::<CanonicalDateTime>()
            .map(|value| AttributeValue::DateTime(value.to_string()))
            .map_err(|_| malformed()),
        AttributeValue::DateTimeTZ(value) => normalize_provider_datetime_tz(value)
            .parse::<CanonicalDateTimeTz>()
            .map(|value| AttributeValue::DateTimeTZ(value.to_string()))
            .map_err(|_| malformed()),
        AttributeValue::Decimal(value) => parse_decimal(&value)
            .map(|value| AttributeValue::Decimal(value.canonical_string()))
            .ok_or_else(malformed),
        AttributeValue::Duration(value) => {
            let value = normalize_provider_fraction(value);
            match value.parse::<CanonicalDuration>() {
                Ok(value) => Ok(AttributeValue::Duration(value.to_string())),
                Err(_) => CompatibilityValueV2::released_duration(value.clone())
                    .map(|_| AttributeValue::Duration(value))
                    .map_err(|_| malformed()),
            }
        }
        value => Ok(value),
    }
}

fn normalize_provider_datetime_tz(value: String) -> String {
    let mut normalized = normalize_provider_fraction(value);
    for zero_offset in ["+00:00:00", "-00:00:00", "+00:00", "-00:00"] {
        if normalized.ends_with(zero_offset) {
            normalized.truncate(normalized.len() - zero_offset.len());
            normalized.push('Z');
            break;
        }
    }
    normalized
}

fn normalize_provider_fraction(value: String) -> String {
    let Some(dot) = value.find('.') else {
        return value;
    };
    let fraction_end = value[dot + 1..]
        .find(|character: char| !character.is_ascii_digit())
        .map_or(value.len(), |offset| dot + 1 + offset);
    let trimmed_end = value[dot + 1..fraction_end].trim_end_matches('0').len() + dot + 1;
    if trimmed_end == fraction_end {
        return value;
    }
    let mut normalized = String::with_capacity(value.len());
    normalized.push_str(
        &value[..if trimmed_end == dot + 1 {
            dot
        } else {
            trimmed_end
        }],
    );
    normalized.push_str(&value[fraction_end..]);
    normalized
}

const fn released_kind(kind: HydrationNodeKindV2) -> ThingKind {
    match kind {
        HydrationNodeKindV2::Entity => ThingKind::Entity,
        HydrationNodeKindV2::Relation => ThingKind::Relation,
    }
}

fn require_root(
    expected: BindingId,
    actual: BindingId,
    code: &'static str,
) -> Result<(), MatchError> {
    if expected == actual {
        Ok(())
    } else {
        Err(result_error(
            code,
            "provider aggregate root does not match the validated operation",
        )
        .at(MatchErrorPathSegment::Binding(actual)))
    }
}

/// Return the released exactly-one diagnostic for a proven distinct tuple count.
///
/// The executor uses the same constructor when a provider-bounded identity scan
/// proves cardinality before full graph hydration. Keeping one constructor
/// prevents the optimized path from drifting in code, message, path, or detail.
pub(crate) fn exactly_one_cardinality_error(actual: usize) -> Option<MatchError> {
    let code = match actual {
        0 => "no_result",
        1 => return None,
        _ => "not_unique",
    };
    Some(
        MatchError::new(
            MatchErrorCategory::Cardinality,
            code,
            "exactly-one fetch did not produce exactly one distinct selected tuple",
        )
        .at(MatchErrorPathSegment::Result)
        .with_detail("actual", actual as u64),
    )
}

/// Check the roots of a generated-manager result against its filter.
///
/// A manager filter is a conjunction of literal comparisons on fields of one
/// exact root. The compiler binds one variable per [`BoundFieldId`], so every
/// comparison on a multivalue field must hold for one shared member. Row
/// validation accepts each comparison independently; this is the stricter
/// rule a manager result has to meet.
pub(crate) fn validate_manager_roots(
    validated: &ValidatedMatchRequest,
    root: BindingId,
    things: &[&HydratedThing],
) -> Result<(), MatchError> {
    let request = validated.request();
    let [binding] = request.plan.bindings.as_slice() else {
        return Err(result_error(
            "manager_filter_query_shape_invalid",
            "generated-manager evidence requires exactly one validated binding",
        )
        .at(MatchErrorPathSegment::Plan));
    };
    if binding.id != root || binding.match_mode != MatchMode::Exact {
        return Err(result_error(
            "manager_filter_query_shape_invalid",
            "generated-manager evidence requires one exact root",
        )
        .at(MatchErrorPathSegment::Plan));
    }
    let mut grouped = BTreeMap::<BoundFieldId, Vec<(ComparisonOp, &AttributeValue)>>::new();
    if let Some(predicate) = request.plan.predicate.as_ref()
        && !collect_manager_field_predicates(predicate, root, &mut grouped)
    {
        return Err(result_error(
            "manager_filter_query_shape_invalid",
            "generated-manager evidence admits only a conjunction of root field literals",
        )
        .at(MatchErrorPathSegment::Predicate));
    }

    for (index, thing) in things.iter().enumerate() {
        for (field, predicates) in &grouped {
            let values = thing
                .attributes()
                .iter()
                .find(|attribute| attribute.field() == &field.field)
                .map(HydratedAttribute::values)
                .unwrap_or_default();
            let witnessed = values.iter().any(|candidate| {
                predicates.iter().all(|(operator, literal)| {
                    compare_values(*operator, candidate, literal).unwrap_or(false)
                })
            });
            if !witnessed {
                return Err(result_error(
                    "predicate_evidence_mismatch",
                    "provider hydration does not contain one field member satisfying every authored comparison",
                )
                .at(MatchErrorPathSegment::ProviderEvidence)
                .at(MatchErrorPathSegment::Index(index))
                .at(MatchErrorPathSegment::Field(field.field.clone())));
            }
        }
    }
    Ok(())
}

fn collect_manager_field_predicates<'a>(
    expression: &'a MatchExpr,
    root: BindingId,
    grouped: &mut BTreeMap<BoundFieldId, Vec<(ComparisonOp, &'a AttributeValue)>>,
) -> bool {
    match expression {
        MatchExpr::FieldValue {
            field,
            operator,
            value,
        } if field.binding == root
            && matches!(
                operator,
                ComparisonOp::Equal
                    | ComparisonOp::NotEqual
                    | ComparisonOp::LessThan
                    | ComparisonOp::LessThanOrEqual
                    | ComparisonOp::GreaterThan
                    | ComparisonOp::GreaterThanOrEqual
            ) =>
        {
            grouped
                .entry(field.clone())
                .or_default()
                .push((*operator, value));
            true
        }
        MatchExpr::And { expressions } => expressions
            .iter()
            .all(|expression| collect_manager_field_predicates(expression, root, grouped)),
        MatchExpr::FieldValue { .. }
        | MatchExpr::FieldComparison { .. }
        | MatchExpr::ScalarComparison { .. }
        | MatchExpr::FieldPresence { .. }
        | MatchExpr::BindingIid { .. }
        | MatchExpr::RoleEdge { .. }
        | MatchExpr::Reachable { .. }
        | MatchExpr::Or { .. }
        | MatchExpr::Not { .. } => false,
    }
}

fn descriptor_attributes(descriptor: &TypeDescriptorRef) -> &[OwnedAttributeDescriptor] {
    match descriptor {
        TypeDescriptorRef::Entity(descriptor) => &descriptor.owned_attributes,
        TypeDescriptorRef::Relation(descriptor) => &descriptor.owned_attributes,
    }
}

fn compare_values(
    operator: ComparisonOp,
    left: &AttributeValue,
    right: &AttributeValue,
) -> Result<bool, MatchError> {
    match operator {
        ComparisonOp::Equal | ComparisonOp::NotEqual => {
            let equal = semantic_equal(left, right).ok_or_else(|| {
                result_error(
                    "predicate_value_type_mismatch",
                    "provider predicate values do not have the same validated type",
                )
            })?;
            Ok(if operator == ComparisonOp::Equal {
                equal
            } else {
                !equal
            })
        }
        ComparisonOp::Contains
        | ComparisonOp::StartsWith
        | ComparisonOp::EndsWith
        | ComparisonOp::Regex => {
            let (AttributeValue::String(left), AttributeValue::String(right)) = (left, right)
            else {
                return Err(result_error(
                    "predicate_value_type_mismatch",
                    "provider predicate values do not support the validated string operator",
                ));
            };
            match operator {
                ComparisonOp::Contains => Ok(unicode_case_fold_contains(left, right)),
                ComparisonOp::StartsWith => Ok(left.starts_with(right)),
                ComparisonOp::EndsWith => Ok(left.ends_with(right)),
                ComparisonOp::Regex => {
                    Regex::new(right)
                        .map(|regex| regex.is_match(left))
                        .map_err(|_| {
                            MatchError::new(
                                MatchErrorCategory::InvalidPlan,
                                "invalid_regex_pattern",
                                "validated request contains an invalid regular expression",
                            )
                            .at(MatchErrorPathSegment::Predicate)
                        })
                }
                _ => unreachable!(),
            }
        }
        ComparisonOp::LessThan
        | ComparisonOp::LessThanOrEqual
        | ComparisonOp::GreaterThan
        | ComparisonOp::GreaterThanOrEqual => {
            let ordering = value_order(left, right).ok_or_else(|| {
                result_error(
                    "predicate_value_type_mismatch",
                    "provider predicate values are not order-compatible",
                )
            })?;
            Ok(match operator {
                ComparisonOp::LessThan => ordering == Ordering::Less,
                ComparisonOp::LessThanOrEqual => ordering != Ordering::Greater,
                ComparisonOp::GreaterThan => ordering == Ordering::Greater,
                ComparisonOp::GreaterThanOrEqual => ordering != Ordering::Less,
                _ => unreachable!(),
            })
        }
    }
}

fn unicode_case_fold_contains(left: &str, right: &str) -> bool {
    UniCase::new(left)
        .to_folded_case()
        .contains(&UniCase::new(right).to_folded_case())
}

pub(crate) fn value_order(left: &AttributeValue, right: &AttributeValue) -> Option<Ordering> {
    match (left, right) {
        (AttributeValue::String(left), AttributeValue::String(right)) => Some(left.cmp(right)),
        (AttributeValue::Long(left), AttributeValue::Long(right)) => Some(left.cmp(right)),
        (AttributeValue::Double(left), AttributeValue::Double(right)) => left.partial_cmp(right),
        (AttributeValue::Boolean(left), AttributeValue::Boolean(right)) => Some(left.cmp(right)),
        (AttributeValue::Date(left), AttributeValue::Date(right)) => {
            Some(parse_date(left)?.cmp(&parse_date(right)?))
        }
        (AttributeValue::DateTime(left), AttributeValue::DateTime(right)) => {
            Some(parse_datetime(left, false)?.cmp(&parse_datetime(right, false)?))
        }
        (AttributeValue::DateTimeTZ(left), AttributeValue::DateTimeTZ(right)) => {
            Some(parse_datetime(left, true)?.cmp(&parse_datetime(right, true)?))
        }
        (AttributeValue::Decimal(left), AttributeValue::Decimal(right)) => {
            compare_decimal(left, right)
        }
        (AttributeValue::Duration(left), AttributeValue::Duration(right)) => {
            Some(parse_duration(left)?.cmp(&parse_duration(right)?))
        }
        _ => None,
    }
}

fn semantic_equal(left: &AttributeValue, right: &AttributeValue) -> Option<bool> {
    value_order(left, right).map(|ordering| ordering == Ordering::Equal)
}

fn output_slots(output: &FetchShape) -> impl Iterator<Item = &FetchSlot> {
    let slots: Vec<_> = match output {
        FetchShape::Positional { slots } => slots.iter().collect(),
        FetchShape::Named { slots } => slots.iter().map(|named| &named.slot).collect(),
    };
    slots.into_iter()
}

fn safe_value(value: &AttributeValue) -> bool {
    match value {
        AttributeValue::Double(value) => value.is_finite(),
        AttributeValue::Date(value) => parse_date(value).is_some(),
        AttributeValue::DateTime(value) => parse_datetime(value, false).is_some(),
        AttributeValue::DateTimeTZ(value) => parse_datetime(value, true).is_some(),
        AttributeValue::Decimal(value) => parse_decimal(value).is_some(),
        AttributeValue::Duration(value) => parse_duration(value).is_some(),
        AttributeValue::String(_) | AttributeValue::Long(_) | AttributeValue::Boolean(_) => true,
    }
}

fn parse_date(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year = value[0..4].parse::<i32>().ok()?;
    let month = value[5..7].parse::<u32>().ok()?;
    let day = value[8..10].parse::<u32>().ok()?;
    if year <= 0 || !(1..=12).contains(&month) {
        return None;
    }
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day == 0 || day > days[(month - 1) as usize] {
        return None;
    }
    Some(days_from_civil(year, month, day))
}

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let adjusted_year = year - i32::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    } / 400;
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = month as i32 + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day as i32 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    i64::from(era * 146_097 + day_of_era)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DateTimeKey {
    seconds: i64,
    fraction: String,
}

impl Ord for DateTimeKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.seconds
            .cmp(&other.seconds)
            .then_with(|| compare_fraction(&self.fraction, &other.fraction))
    }
}

impl PartialOrd for DateTimeKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn parse_datetime(value: &str, timezone_required: bool) -> Option<DateTimeKey> {
    let (date, raw_time) = value.split_once('T')?;
    let days = parse_date(date)?;
    let (time, offset_seconds, has_timezone) = if let Some(time) = raw_time.strip_suffix('Z') {
        (time, 0_i64, true)
    } else if let Some(index) = raw_time
        .char_indices()
        .skip(1)
        .find_map(|(index, character)| matches!(character, '+' | '-').then_some(index))
    {
        let (time, offset) = raw_time.split_at(index);
        let sign = if offset.starts_with('-') {
            -1_i64
        } else {
            1_i64
        };
        let (hour, minute, second, fraction) = parse_clock(&offset[1..])?;
        if !fraction.is_empty() || hour > 23 {
            return None;
        }
        (
            time,
            sign * (i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second)),
            true,
        )
    } else {
        (raw_time, 0_i64, false)
    };
    if has_timezone != timezone_required {
        return None;
    }
    let (hour, minute, second, fraction) = parse_clock(time)?;
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))?
        .checked_sub(offset_seconds)?;
    Some(DateTimeKey { seconds, fraction })
}

fn parse_clock(value: &str) -> Option<(u32, u32, u32, String)> {
    let (main, fraction) = value
        .split_once('.')
        .map_or((value, ""), |(main, fraction)| (main, fraction));
    if (!fraction.is_empty() && !fraction.bytes().all(|byte| byte.is_ascii_digit()))
        || value.ends_with('.')
    {
        return None;
    }
    let mut parts = main.split(':');
    let hour = parts.next()?.parse::<u32>().ok()?;
    let minute = parts.next()?.parse::<u32>().ok()?;
    let second = parts.next().map_or(Some(0), |part| part.parse().ok())?;
    if parts.next().is_some() || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some((
        hour,
        minute,
        second,
        fraction.trim_end_matches('0').to_owned(),
    ))
}

fn compare_fraction(left: &str, right: &str) -> Ordering {
    let width = left.len().max(right.len());
    left.bytes()
        .chain(std::iter::repeat_n(b'0', width - left.len()))
        .cmp(
            right
                .bytes()
                .chain(std::iter::repeat_n(b'0', width - right.len())),
        )
}

fn compare_decimal(left: &str, right: &str) -> Option<Ordering> {
    Some(parse_decimal(left)?.compare(&parse_decimal(right)?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct DurationKey {
    months: u64,
    days: u64,
    nanoseconds: u128,
}

fn parse_duration(value: &str) -> Option<DurationKey> {
    let value = value.strip_prefix('P')?;
    if value.is_empty() {
        return None;
    }
    let mut months = 0_u64;
    let mut days = 0_u64;
    let mut nanoseconds = 0_u128;
    let mut number = String::new();
    let mut time = false;
    let mut saw_value = false;
    for character in value.chars() {
        if character == 'T' {
            if time || !number.is_empty() {
                return None;
            }
            time = true;
            continue;
        }
        if character.is_ascii_digit() || character == '.' {
            number.push(character);
            continue;
        }
        if number.is_empty() {
            return None;
        }
        saw_value = true;
        match (time, character) {
            (false, 'Y') => {
                months = months.checked_add(number.parse::<u64>().ok()?.checked_mul(12)?)?
            }
            (false, 'M') => months = months.checked_add(number.parse::<u64>().ok()?)?,
            (false, 'D') => days = days.checked_add(number.parse::<u64>().ok()?)?,
            (true, 'H') => {
                nanoseconds = nanoseconds.checked_add(
                    u128::from(number.parse::<u64>().ok()?).checked_mul(3_600_000_000_000)?,
                )?
            }
            (true, 'M') => {
                nanoseconds = nanoseconds.checked_add(
                    u128::from(number.parse::<u64>().ok()?).checked_mul(60_000_000_000)?,
                )?
            }
            (true, 'S') => {
                let (whole, fraction) = number.split_once('.').unwrap_or((number.as_str(), ""));
                if whole.is_empty()
                    || fraction.len() > 9
                    || !fraction.bytes().all(|byte| byte.is_ascii_digit())
                {
                    return None;
                }
                let seconds = whole.parse::<u64>().ok()?;
                let mut nanos = fraction.parse::<u64>().unwrap_or(0);
                for _ in fraction.len()..9 {
                    nanos *= 10;
                }
                nanoseconds = nanoseconds
                    .checked_add(u128::from(seconds).checked_mul(1_000_000_000)?)?
                    .checked_add(u128::from(nanos))?;
            }
            _ => return None,
        }
        number.clear();
    }
    if !number.is_empty() || !saw_value {
        return None;
    }
    Some(DurationKey {
        months,
        days,
        nanoseconds,
    })
}

/// Running charge of reduction group-key values against the hard ceilings.
#[derive(Default)]
struct EvidenceBudget {
    attribute_values: usize,
    evidence_bytes: usize,
}

impl EvidenceBudget {
    fn charge_attribute_values(&mut self, values: &[AttributeValue]) -> Result<(), MatchError> {
        self.attribute_values =
            self.attribute_values
                .checked_add(values.len())
                .ok_or_else(|| {
                    resource_error(
                        "hydrated_attribute_value_limit",
                        "hydrated attribute value counter overflowed",
                    )
                })?;
        require_limit(
            "hydrated_attribute_value_limit",
            self.attribute_values,
            MAX_REDUCTION_GROUP_VALUES,
        )?;
        for value in values {
            self.evidence_bytes = self
                .evidence_bytes
                .checked_add(attribute_value_bytes(value))
                .ok_or_else(|| {
                    resource_error(
                        "provider_evidence_byte_limit",
                        "provider evidence byte counter overflowed",
                    )
                })?;
            require_limit(
                "provider_evidence_byte_limit",
                self.evidence_bytes,
                MAX_REDUCTION_GROUP_BYTES,
            )?;
        }
        Ok(())
    }
}

fn require_limit(code: &'static str, actual: usize, limit: usize) -> Result<(), MatchError> {
    if actual <= limit {
        Ok(())
    } else {
        Err(
            resource_error(code, "provider result exceeds a hard resource ceiling")
                .with_detail("limit", limit as u64)
                .with_detail("actual", actual as u64),
        )
    }
}

fn attribute_value_bytes(value: &AttributeValue) -> usize {
    match value {
        AttributeValue::String(value)
        | AttributeValue::Date(value)
        | AttributeValue::DateTime(value)
        | AttributeValue::DateTimeTZ(value)
        | AttributeValue::Decimal(value)
        | AttributeValue::Duration(value) => value.len(),
        AttributeValue::Long(_) | AttributeValue::Double(_) => std::mem::size_of::<u64>(),
        AttributeValue::Boolean(_) => 1,
    }
}

fn result_error(code: &'static str, message: &'static str) -> MatchError {
    MatchError::new(MatchErrorCategory::ResultDecode, code, message)
        .at(MatchErrorPathSegment::Result)
}

fn resource_error(code: &'static str, message: &'static str) -> MatchError {
    MatchError::new(MatchErrorCategory::ResourceLimit, code, message)
        .at(MatchErrorPathSegment::Result)
}

#[cfg(test)]
mod tests {
    use type_bridge_contract::query_plan::QueryReductionGroupV2;

    use super::*;
    use crate::_attribute::ValueType;
    use crate::_descriptor::{EntityDescriptor, RelationDescriptor, RoleDescriptor};
    use crate::_entity::Annotation;
    use crate::match_request::ids::FieldId;
    use crate::match_request::model::{BindingPair, MatchBinding, MatchPlan, MatchRequest};
    use crate::match_request::v2_outcome_fixture::{
        distinct_count, distinct_exists, hydrated_page, hydrated_rows, model_reduction,
    };
    use crate::match_request::validation::validate_match_request;

    fn attribute(
        name: &str,
        value_type: ValueType,
        annotations: Vec<Annotation>,
        optional: bool,
        ordered: bool,
    ) -> OwnedAttributeDescriptor {
        OwnedAttributeDescriptor {
            field_name: name.to_owned(),
            attr_name: name.to_owned(),
            value_type,
            annotations,
            is_optional: optional,
            is_ordered: ordered,
            doc: None,
            meta: Default::default(),
        }
    }

    fn registry() -> DescriptorRegistry {
        let registry = DescriptorRegistry::new();
        registry
            .register_entity(EntityDescriptor {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![
                    attribute(
                        "name",
                        ValueType::String,
                        vec![Annotation::Key],
                        false,
                        false,
                    ),
                    attribute("age", ValueType::Long, vec![], true, false),
                    attribute("seen_at", ValueType::DateTime, vec![], true, false),
                ],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        registry
            .register_entity(EntityDescriptor {
                type_name: "employee".into(),
                is_abstract: false,
                parent_type: Some("person".into()),
                // Runtime descriptors carry the effective inherited field set.
                owned_attributes: vec![
                    attribute(
                        "name",
                        ValueType::String,
                        vec![Annotation::Key],
                        false,
                        false,
                    ),
                    attribute(
                        "badge",
                        ValueType::String,
                        vec![Annotation::Unique],
                        false,
                        false,
                    ),
                    attribute(
                        "notes",
                        ValueType::String,
                        vec![Annotation::Card(0, None)],
                        true,
                        true,
                    ),
                    attribute(
                        "tags",
                        ValueType::String,
                        vec![Annotation::Card(0, None), Annotation::Distinct],
                        true,
                        true,
                    ),
                ],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        registry
            .register_entity(EntityDescriptor {
                type_name: "company".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![attribute(
                    "name",
                    ValueType::String,
                    vec![Annotation::Key],
                    false,
                    false,
                )],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        registry
            .register_relation(RelationDescriptor {
                type_name: "employment".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![attribute(
                    "code",
                    ValueType::String,
                    vec![Annotation::Key],
                    false,
                    false,
                )],
                roles: vec![
                    RoleDescriptor {
                        role_name: "employee".into(),
                        player_type_names: vec!["person".into()],
                        cardinality: Some((0, None)),
                        ordered: true,
                        distinct: false,
                        ..Default::default()
                    },
                    RoleDescriptor {
                        role_name: "distinct_employee".into(),
                        player_type_names: vec!["person".into()],
                        cardinality: Some((0, None)),
                        ordered: true,
                        distinct: true,
                        ..Default::default()
                    },
                ],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        registry
    }

    fn binding(
        registry: &DescriptorRegistry,
        id: u16,
        descriptor: &str,
        kind: ThingKind,
        mode: MatchMode,
    ) -> MatchBinding {
        MatchBinding {
            id: BindingId::new(id),
            descriptor: registry.descriptor_id(descriptor).unwrap(),
            thing_kind: kind,
            match_mode: mode,
        }
    }

    fn exact_one_request(
        registry: &DescriptorRegistry,
        bindings: Vec<MatchBinding>,
        predicate: Option<MatchExpr>,
        output: FetchShape,
        cross_joins: BTreeSet<BindingPair>,
    ) -> ValidatedMatchRequest {
        validate_match_request(
            registry,
            MatchRequest::v1(
                MatchPlan {
                    bindings,
                    predicate,
                    allowed_cross_joins: cross_joins,
                },
                MatchOperation::FetchRows {
                    output,
                    order: vec![],
                    window: Window {
                        offset: 0,
                        limit: 1,
                    },
                    cardinality: RowCardinality::ExactlyOne,
                },
            ),
        )
        .unwrap()
    }

    fn field(registry: &DescriptorRegistry, descriptor: &str, name: &str) -> FieldId {
        let descriptor = registry.descriptor_id(descriptor).unwrap();
        registry.field_id(&descriptor, name).unwrap()
    }

    fn hydrated_attribute(
        registry: &DescriptorRegistry,
        descriptor: &str,
        name: &str,
        values: Vec<AttributeValue>,
    ) -> HydratedAttribute {
        HydratedAttribute::new(field(registry, descriptor, name), values)
    }

    fn person(registry: &DescriptorRegistry, concept: &str, name: &str) -> HydratedThing {
        let descriptor = registry.descriptor_id("person").unwrap();
        HydratedThing::new(
            ConceptId::new(concept),
            descriptor.clone(),
            descriptor,
            ThingKind::Entity,
            vec![hydrated_attribute(
                registry,
                "person",
                "name",
                vec![AttributeValue::String(name.into())],
            )],
            vec![],
        )
    }

    fn company(registry: &DescriptorRegistry, concept: &str, name: &str) -> HydratedThing {
        let descriptor = registry.descriptor_id("company").unwrap();
        HydratedThing::new(
            ConceptId::new(concept),
            descriptor.clone(),
            descriptor,
            ThingKind::Entity,
            vec![hydrated_attribute(
                registry,
                "company",
                "name",
                vec![AttributeValue::String(name.into())],
            )],
            vec![],
        )
    }

    fn employment(
        registry: &DescriptorRegistry,
        concept: &str,
        code: &str,
        roles: Vec<HydratedRole>,
    ) -> HydratedThing {
        let descriptor = registry.descriptor_id("employment").unwrap();
        HydratedThing::new(
            ConceptId::new(concept),
            descriptor.clone(),
            descriptor,
            ThingKind::Relation,
            vec![hydrated_attribute(
                registry,
                "employment",
                "code",
                vec![AttributeValue::String(code.into())],
            )],
            roles,
        )
    }

    fn error_code(error: &MatchError) -> &str {
        error.code().as_str()
    }

    #[test]
    fn proof_constructors_remain_owned_by_their_canonical_validators() {
        let source_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("match_request");
        let result_constructor = concat!("ValidatedMatchResult", "::new(");
        let token_issuer = concat!("RequestToken", "::issue(");

        for entry in std::fs::read_dir(source_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(std::ffi::OsStr::to_str) != Some("rs") {
                continue;
            }
            let file_name = path.file_name().unwrap().to_str().unwrap();
            let source = std::fs::read_to_string(&path).unwrap();
            if file_name != "result_validation.rs" {
                assert!(
                    !source.contains(result_constructor),
                    "validated result construction escaped result_validation.rs into {file_name}"
                );
            }
            if file_name != "validation.rs" {
                assert!(
                    !source.contains(token_issuer),
                    "request-token issuance escaped validation.rs into {file_name}"
                );
            }
        }
    }

    #[test]
    fn exactly_one_rows_fail_closed_on_cardinality_and_window() {
        let registry = registry();
        let output = FetchShape::Positional {
            slots: vec![FetchSlot::One {
                binding: BindingId::new(0),
            }],
        };
        let validated = exact_one_request(
            &registry,
            vec![binding(
                &registry,
                0,
                "person",
                ThingKind::Entity,
                MatchMode::Exact,
            )],
            None,
            output.clone(),
            BTreeSet::new(),
        );
        let alice = person(&registry, "0x01", "Alice");
        let bob = person(&registry, "0x02", "Bob");

        let result = validated_match_result_from_v2(
            &registry,
            &validated,
            hydrated_rows(&registry, vec![vec![SlotValue::One(alice.clone())]]),
        )
        .unwrap();
        let MatchResult::Rows { rows } = result.result() else {
            panic!("expected rows")
        };
        assert_eq!(rows[0].slots(), &[SlotValue::One(alice.clone())]);

        let multiple = hydrated_rows(
            &registry,
            vec![
                vec![SlotValue::One(alice.clone())],
                vec![SlotValue::One(bob.clone())],
            ],
        );
        let error = validated_match_result_from_v2(&registry, &validated, multiple).unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::Cardinality);
        assert_eq!(error_code(&error), "not_unique");

        let empty = hydrated_rows(&registry, vec![]);
        let error = validated_match_result_from_v2(&registry, &validated, empty).unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::Cardinality);
        assert_eq!(error_code(&error), "no_result");

        let bounded = validate_match_request(
            &registry,
            MatchRequest::v1(
                MatchPlan {
                    bindings: vec![binding(
                        &registry,
                        0,
                        "person",
                        ThingKind::Entity,
                        MatchMode::Exact,
                    )],
                    predicate: None,
                    allowed_cross_joins: BTreeSet::new(),
                },
                MatchOperation::FetchRows {
                    output,
                    order: vec![],
                    window: Window {
                        offset: 0,
                        limit: 1,
                    },
                    cardinality: RowCardinality::BoundedMany,
                },
            ),
        )
        .unwrap();
        let oversized = hydrated_rows(
            &registry,
            vec![vec![SlotValue::One(alice)], vec![SlotValue::One(bob)]],
        );
        let error = validated_match_result_from_v2(&registry, &bounded, oversized).unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::ResourceLimit);
        assert_eq!(error_code(&error), "row_window_exceeded");
    }

    #[test]
    fn page_pins_root_window_and_same_snapshot_total() {
        let registry = registry();
        let output = FetchShape::Positional {
            slots: vec![
                FetchSlot::One {
                    binding: BindingId::new(0),
                },
                FetchSlot::Collect {
                    binding: BindingId::new(1),
                    distinct: false,
                    order: vec![],
                },
                FetchSlot::Collect {
                    binding: BindingId::new(2),
                    distinct: true,
                    order: vec![],
                },
            ],
        };
        let window = Window {
            offset: 0,
            limit: 10,
        };
        let validated = validate_match_request(
            &registry,
            MatchRequest::v1(
                MatchPlan {
                    bindings: vec![
                        binding(&registry, 0, "person", ThingKind::Entity, MatchMode::Exact),
                        binding(
                            &registry,
                            1,
                            "employment",
                            ThingKind::Relation,
                            MatchMode::Exact,
                        ),
                        binding(&registry, 2, "company", ThingKind::Entity, MatchMode::Exact),
                    ],
                    predicate: None,
                    allowed_cross_joins: BTreeSet::from([
                        BindingPair::new(BindingId::new(0), BindingId::new(1)),
                        BindingPair::new(BindingId::new(0), BindingId::new(2)),
                    ]),
                },
                MatchOperation::PageBy {
                    root: BindingId::new(0),
                    output,
                    order: vec![],
                    window,
                    include_total: true,
                },
            ),
        )
        .unwrap();
        let employment = employment(&registry, "0x03", "code-1", vec![]);
        let entry = vec![
            SlotValue::One(person(&registry, "0x01", "Alice")),
            SlotValue::Many(vec![employment.clone(), employment]),
            SlotValue::Many(vec![company(&registry, "0x02", "Acme")]),
        ];
        let page = |root: u16, window: Window, total: Option<u64>| {
            hydrated_page(
                &registry,
                BindingId::new(root),
                window,
                total,
                vec![entry.clone()],
            )
        };

        let result =
            validated_match_result_from_v2(&registry, &validated, page(0, window, Some(1)))
                .unwrap();
        let MatchResult::Page { entries, total, .. } = result.result() else {
            panic!("expected page")
        };
        assert_eq!(*total, Some(1));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].slots(), entry.as_slice());

        for (outcome, code) in [
            (page(0, window, Some(2)), "page_total_length_mismatch"),
            (page(0, window, None), "missing_page_total"),
            (page(1, window, Some(1)), "page_root_mismatch"),
            (
                page(
                    0,
                    Window {
                        offset: 1,
                        limit: 10,
                    },
                    Some(2),
                ),
                "page_window_mismatch",
            ),
        ] {
            let error = validated_match_result_from_v2(&registry, &validated, outcome).unwrap_err();
            assert_eq!(error_code(&error), code);
        }
    }

    #[test]
    fn aggregate_variants_pin_operation_root_and_lossless_values() {
        let registry = registry();
        let plan = || MatchPlan {
            bindings: vec![binding(
                &registry,
                0,
                "person",
                ThingKind::Entity,
                MatchMode::Exact,
            )],
            predicate: None,
            allowed_cross_joins: BTreeSet::new(),
        };
        let count = validate_match_request(
            &registry,
            MatchRequest::v1(
                plan(),
                MatchOperation::CountBy {
                    root: BindingId::new(0),
                },
            ),
        )
        .unwrap();
        let result = validated_match_result_from_v2(
            &registry,
            &count,
            distinct_count(BindingId::new(0), u64::MAX),
        )
        .unwrap();
        assert!(matches!(
            result.result(),
            MatchResult::Count {
                root,
                value: u64::MAX
            } if *root == BindingId::new(0)
        ));

        assert_eq!(
            error_code(
                &validated_match_result_from_v2(
                    &registry,
                    &count,
                    distinct_count(BindingId::new(1), 0)
                )
                .unwrap_err()
            ),
            "count_root_mismatch"
        );

        let exists = validate_match_request(
            &registry,
            MatchRequest::v1(
                plan(),
                MatchOperation::ExistsBy {
                    root: BindingId::new(0),
                },
            ),
        )
        .unwrap();
        let result = validated_match_result_from_v2(
            &registry,
            &exists,
            distinct_exists(BindingId::new(0), true),
        )
        .unwrap();
        assert!(matches!(
            result.result(),
            MatchResult::Exists {
                root,
                value: true
            } if *root == BindingId::new(0)
        ));
        assert_eq!(
            error_code(
                &validated_match_result_from_v2(
                    &registry,
                    &exists,
                    distinct_exists(BindingId::new(1), true)
                )
                .unwrap_err()
            ),
            "exists_root_mismatch"
        );
    }

    #[test]
    fn manager_multivalue_conjunction_requires_one_shared_member() {
        let registry = registry();
        let root = BindingId::new(0);
        let notes = BoundFieldId::new(root, field(&registry, "employee", "notes"));
        let request = |upper: &str| {
            validate_match_request(
                &registry,
                MatchRequest::v1(
                    MatchPlan {
                        bindings: vec![binding(
                            &registry,
                            0,
                            "employee",
                            ThingKind::Entity,
                            MatchMode::Exact,
                        )],
                        predicate: Some(MatchExpr::And {
                            expressions: vec![
                                MatchExpr::FieldValue {
                                    field: notes.clone(),
                                    operator: ComparisonOp::GreaterThan,
                                    value: AttributeValue::String("b".into()),
                                },
                                MatchExpr::FieldValue {
                                    field: notes.clone(),
                                    operator: ComparisonOp::LessThan,
                                    value: AttributeValue::String(upper.into()),
                                },
                            ],
                        }),
                        allowed_cross_joins: BTreeSet::new(),
                    },
                    MatchOperation::CountBy { root },
                ),
            )
            .unwrap()
        };
        let descriptor = registry.descriptor_id("employee").unwrap();
        let hydrated = HydratedThing::new(
            ConceptId::new("employee-1"),
            descriptor.clone(),
            descriptor,
            ThingKind::Entity,
            vec![
                hydrated_attribute(
                    &registry,
                    "employee",
                    "name",
                    vec![AttributeValue::String("Ada".into())],
                ),
                hydrated_attribute(
                    &registry,
                    "employee",
                    "badge",
                    vec![AttributeValue::String("badge-1".into())],
                ),
                hydrated_attribute(
                    &registry,
                    "employee",
                    "notes",
                    vec![
                        AttributeValue::String("a".into()),
                        AttributeValue::String("c".into()),
                    ],
                ),
            ],
            vec![],
        );

        let impossible = request("b");
        let error = validate_manager_roots(&impossible, root, &[&hydrated]).unwrap_err();
        assert_eq!(error_code(&error), "predicate_evidence_mismatch");

        let witnessed = request("d");
        validate_manager_roots(&witnessed, root, &[&hydrated]).unwrap();
    }

    #[test]
    fn field_grouped_reduction_rejects_wrong_kind_domain_duplicates_and_noncanonical_keys() {
        let registry = registry();
        let root = BindingId::new(0);
        let age = BoundFieldId::new(root, field(&registry, "person", "age"));
        let request_for = |group: BoundFieldId| {
            validate_match_request(
                &registry,
                MatchRequest::v1(
                    MatchPlan {
                        bindings: vec![binding(
                            &registry,
                            0,
                            "person",
                            ThingKind::Entity,
                            MatchMode::Exact,
                        )],
                        predicate: None,
                        allowed_cross_joins: BTreeSet::new(),
                    },
                    MatchOperation::ReduceByField {
                        root,
                        group,
                        reducers: vec![
                            ReduceTerm {
                                reduction: Reduction::Count,
                                input: None,
                            },
                            ReduceTerm {
                                reduction: Reduction::Sum,
                                input: Some(age.clone()),
                            },
                        ],
                    },
                ),
            )
            .unwrap()
        };
        let validated = request_for(age.clone());
        let map = |rows| {
            validated_match_result_from_v2(
                &registry,
                &validated,
                model_reduction(&registry, &validated, rows),
            )
        };

        let result = map(vec![ReductionRow::new_field(
            AttributeValue::Long(20),
            vec![ReducedValue::Count(2), ReducedValue::Long(Some(40))],
        )])
        .unwrap();
        let MatchResult::FieldReduction { group, rows, .. } = result.result() else {
            panic!("expected field-grouped reduction")
        };
        assert_eq!(group, &age);
        assert_eq!(rows[0].field_group(), Some(&AttributeValue::Long(20)));

        let wrong_kind = vec![ReductionRow::new(
            Some(person(&registry, "0x01", "Alice")),
            vec![ReducedValue::Count(1), ReducedValue::Long(Some(20))],
        )];
        assert_eq!(
            error_code(&map(wrong_kind).unwrap_err()),
            "field_reduction_group_kind"
        );

        let wrong_domain = vec![ReductionRow::new_field(
            AttributeValue::String("20".into()),
            vec![ReducedValue::Count(1), ReducedValue::Long(Some(20))],
        )];
        assert_eq!(
            error_code(&map(wrong_domain).unwrap_err()),
            "field_reduction_group_value_type"
        );

        let duplicate = vec![
            ReductionRow::new_field(
                AttributeValue::Long(20),
                vec![ReducedValue::Count(1), ReducedValue::Long(Some(20))],
            ),
            ReductionRow::new_field(
                AttributeValue::Long(20),
                vec![ReducedValue::Count(1), ReducedValue::Long(Some(20))],
            ),
        ];
        assert_eq!(
            error_code(&map(duplicate).unwrap_err()),
            "field_reduction_group_duplicate"
        );

        // The released mapping canonicalizes every remote group key, so the
        // row validator's own canonical-form guard is exercised directly.
        let seen_at = BoundFieldId::new(root, field(&registry, "person", "seen_at"));
        let datetime_validated = request_for(seen_at);
        let noncanonical = vec![ReductionRow::new_field(
            AttributeValue::DateTime("2026-07-28T03:55:00.000000000".into()),
            vec![ReducedValue::Count(1), ReducedValue::Long(Some(20))],
        )];
        assert_eq!(
            error_code(
                &validate_reduction_rows(&registry, &datetime_validated, noncanonical).unwrap_err()
            ),
            "field_reduction_group_value_noncanonical"
        );
    }

    #[test]
    fn tuple_field_grouped_reduction_validates_order_arity_domains_and_uniqueness() {
        let registry = registry();
        let root = BindingId::new(0);
        let age = BoundFieldId::new(root, field(&registry, "person", "age"));
        let name = BoundFieldId::new(root, field(&registry, "person", "name"));
        let groups = vec![age.clone(), name];
        let validated = validate_match_request(
            &registry,
            MatchRequest::v1(
                MatchPlan {
                    bindings: vec![binding(
                        &registry,
                        0,
                        "person",
                        ThingKind::Entity,
                        MatchMode::Exact,
                    )],
                    predicate: None,
                    allowed_cross_joins: BTreeSet::new(),
                },
                MatchOperation::ReduceByFields {
                    root,
                    groups: groups.clone(),
                    reducers: vec![
                        ReduceTerm {
                            reduction: Reduction::Count,
                            input: None,
                        },
                        ReduceTerm {
                            reduction: Reduction::Sum,
                            input: Some(age),
                        },
                    ],
                },
            ),
        )
        .unwrap();
        let reduced = vec![ReducedValue::Count(1), ReducedValue::Long(Some(20))];
        let valid = vec![ReductionRow::new_fields(
            vec![
                AttributeValue::Long(20),
                AttributeValue::String("Alice".into()),
            ],
            reduced.clone(),
        )];
        let map = |rows| {
            validated_match_result_from_v2(
                &registry,
                &validated,
                model_reduction(&registry, &validated, rows),
            )
        };

        let result = map(valid.clone()).unwrap();
        let MatchResult::FieldTupleReduction {
            groups: actual_groups,
            rows,
            ..
        } = result.result()
        else {
            panic!("expected tuple-field-grouped reduction")
        };
        assert_eq!(actual_groups, &groups);
        assert_eq!(
            rows[0].field_groups(),
            Some(
                [
                    AttributeValue::Long(20),
                    AttributeValue::String("Alice".into()),
                ]
                .as_slice()
            )
        );

        let mut wrong_order = model_reduction(&registry, &validated, valid);
        let RemoteOutcomeV2::ModelReduction {
            group: Some(QueryReductionGroupV2::Fields { fields }),
            ..
        } = &mut wrong_order
        else {
            panic!("expected an echoed tuple group contract")
        };
        fields.reverse();
        assert_eq!(
            error_code(
                &validated_match_result_from_v2(&registry, &validated, wrong_order).unwrap_err()
            ),
            "field_tuple_reduction_contract_mismatch"
        );

        let wrong_arity = vec![ReductionRow::new_fields(
            vec![AttributeValue::Long(20)],
            reduced.clone(),
        )];
        assert_eq!(
            error_code(&map(wrong_arity).unwrap_err()),
            "field_tuple_reduction_group_arity"
        );

        let wrong_domain = vec![ReductionRow::new_fields(
            vec![
                AttributeValue::String("20".into()),
                AttributeValue::String("Alice".into()),
            ],
            reduced.clone(),
        )];
        assert_eq!(
            error_code(&map(wrong_domain).unwrap_err()),
            "field_tuple_reduction_group_value_type"
        );

        let duplicate = vec![
            ReductionRow::new_fields(
                vec![
                    AttributeValue::Long(20),
                    AttributeValue::String("Alice".into()),
                ],
                reduced.clone(),
            ),
            ReductionRow::new_fields(
                vec![
                    AttributeValue::Long(20),
                    AttributeValue::String("Alice".into()),
                ],
                reduced,
            ),
        ];
        assert_eq!(
            error_code(&map(duplicate).unwrap_err()),
            "field_tuple_reduction_group_duplicate"
        );
    }

    #[test]
    fn predicate_comparison_is_semantic_and_regex_fails_closed() {
        for (left, right) in [
            ("Alice", "LIC"),
            ("Straße", "STRASSE"),
            ("ﬂour", "FLOUR"),
            ("ος", "ΟΣ"),
            ("İstanbul", "i\u{307}stanbul"),
        ] {
            assert!(
                compare_values(
                    ComparisonOp::Contains,
                    &AttributeValue::String(left.into()),
                    &AttributeValue::String(right.into()),
                )
                .unwrap(),
                "expected {left:?} to contain {right:?} after Unicode case folding"
            );
        }
        assert!(
            !compare_values(
                ComparisonOp::Contains,
                &AttributeValue::String("İstanbul".into()),
                &AttributeValue::String("ISTANBUL".into()),
            )
            .unwrap()
        );
        assert!(
            compare_values(
                ComparisonOp::Equal,
                &AttributeValue::Decimal("1.0dec".into()),
                &AttributeValue::Decimal("1.00".into()),
            )
            .unwrap()
        );
        assert!(
            compare_values(
                ComparisonOp::Equal,
                &AttributeValue::DateTimeTZ("2024-01-01T01:00:00+01:00".into()),
                &AttributeValue::DateTimeTZ("2024-01-01T00:00:00Z".into()),
            )
            .unwrap()
        );
        assert!(
            compare_values(
                ComparisonOp::Regex,
                &AttributeValue::String("Alice".into()),
                &AttributeValue::String("^A.*e$".into()),
            )
            .unwrap()
        );
        let error = compare_values(
            ComparisonOp::Regex,
            &AttributeValue::String("Alice".into()),
            &AttributeValue::String("[".into()),
        )
        .unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::InvalidPlan);
        assert_eq!(error_code(&error), "invalid_regex_pattern");
    }
}
