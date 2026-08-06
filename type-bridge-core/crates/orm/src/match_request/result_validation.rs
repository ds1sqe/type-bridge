//! Pure, fail-closed validation of invocation-bound provider result evidence.
//!
//! This module performs no I/O and owns no provider. It defensively verifies a
//! complete evidence envelope against one exact [`ValidatedMatchRequest`], then
//! atomically constructs [`ValidatedMatchResult`]. No partial rows escape on
//! failure.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use type_bridge_contract::id::{TypeId, TypeKind};
use type_bridge_contract::query_plan::{CompatibilityValueV2, ReleasedValueKindV2};
use type_bridge_contract::query_remote_v2::{
    HydrationGraphV2, HydrationNodeKindV2, HydrationReferenceV2, HydrationSlotV2, RemoteOutcomeV2,
};
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration,
};
use type_bridge_contract::value::CanonicalValue;
use type_bridge_core_lib::decimal::parse_decimal;
use unicase::UniCase;

use crate::_descriptor::{OwnedAttributeDescriptor, RoleDescriptor, TypeDescriptorRef};
use crate::_entity::Annotation;
use crate::_registry::DescriptorRegistry;
use crate::value::AttributeValue;

use super::error::{MatchError, MatchErrorCategory, MatchErrorPathSegment};
use super::ids::{BindingId, BoundFieldId, DescriptorId, FieldId, RoleEdgeId, RoleId};
use super::limits::MAX_SEMANTIC_ID_BYTES;
use super::model::{
    ComparisonOp, FetchShape, FetchSlot, MatchBinding, MatchExpr, MatchMode, MatchOperation,
    MatchOrder, MissingOrder, Reduction, RowCardinality, SortDirection, ThingKind, Window,
};
use super::result::{
    ConceptId, HydratedAttribute, HydratedRole, HydratedRolePlayer, HydratedThing, MatchResult,
    MatchRow, ProviderResultEvidence, ProviderResultPayload, ProviderSolutionEvidence,
    ReducedValue, SlotValue, ValidatedMatchResult,
};
use super::validation::{StableOrderSpec, ValidatedMatchRequest};

/// Default hard ceiling for provider solutions entering one result validation.
pub(crate) const MAX_PROVIDER_SOLUTIONS: usize = 100_000;
/// Default hard ceiling for distinct rows or roots produced by validation.
pub(crate) const MAX_RESULT_IDENTITIES: usize = 100_000;
/// Default hard ceiling for hydrated things across assignments and role players.
pub(crate) const MAX_HYDRATED_THINGS: usize = 1_000_000;
/// Default hard ceiling for attribute values across hydrated evidence.
pub(crate) const MAX_HYDRATED_ATTRIBUTE_VALUES: usize = 4_000_000;
/// Default hard ceiling for concepts materialized into collection slots.
pub(crate) const MAX_COLLECTED_CONCEPTS: usize = 1_000_000;
/// Default deterministic in-memory evidence-byte ceiling.
pub(crate) const MAX_PROVIDER_EVIDENCE_BYTES: usize = 64 * 1024 * 1024;

/// Construction authority held exclusively by canonical result validation.
///
/// The type is visible to the proof wrapper, but its field is private so
/// executors and bindings cannot fabricate validated result evidence.
pub(super) struct ValidatedResultSeal(());

/// Executor/session policy ceilings applied while validating provider evidence.
///
/// These values are not part of request equality. Executors may supply tighter
/// limits, but must never exceed their own hard policy ceilings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResultValidationLimits {
    pub(crate) solutions: usize,
    pub(crate) result_identities: usize,
    pub(crate) hydrated_things: usize,
    pub(crate) attribute_values: usize,
    pub(crate) collected_concepts: usize,
    pub(crate) evidence_bytes: usize,
}

impl ResultValidationLimits {
    /// Canonical default executor safety policy.
    pub(crate) const DEFAULT: Self = Self::new(
        MAX_PROVIDER_SOLUTIONS,
        MAX_RESULT_IDENTITIES,
        MAX_HYDRATED_THINGS,
        MAX_HYDRATED_ATTRIBUTE_VALUES,
        MAX_COLLECTED_CONCEPTS,
        MAX_PROVIDER_EVIDENCE_BYTES,
    );

    /// Construct an explicitly tightened policy.
    pub(crate) const fn new(
        solutions: usize,
        result_identities: usize,
        hydrated_things: usize,
        attribute_values: usize,
        collected_concepts: usize,
        evidence_bytes: usize,
    ) -> Self {
        Self {
            solutions: if solutions < MAX_PROVIDER_SOLUTIONS {
                solutions
            } else {
                MAX_PROVIDER_SOLUTIONS
            },
            result_identities: if result_identities < MAX_RESULT_IDENTITIES {
                result_identities
            } else {
                MAX_RESULT_IDENTITIES
            },
            hydrated_things: if hydrated_things < MAX_HYDRATED_THINGS {
                hydrated_things
            } else {
                MAX_HYDRATED_THINGS
            },
            attribute_values: if attribute_values < MAX_HYDRATED_ATTRIBUTE_VALUES {
                attribute_values
            } else {
                MAX_HYDRATED_ATTRIBUTE_VALUES
            },
            collected_concepts: if collected_concepts < MAX_COLLECTED_CONCEPTS {
                collected_concepts
            } else {
                MAX_COLLECTED_CONCEPTS
            },
            evidence_bytes: if evidence_bytes < MAX_PROVIDER_EVIDENCE_BYTES {
                evidence_bytes
            } else {
                MAX_PROVIDER_EVIDENCE_BYTES
            },
        }
    }
}

impl Default for ResultValidationLimits {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Validate provider evidence with the default hard result policy.
pub(crate) fn validate_provider_result(
    registry: &DescriptorRegistry,
    validated: &ValidatedMatchRequest,
    evidence: ProviderResultEvidence,
) -> Result<ValidatedMatchResult, MatchError> {
    validate_provider_result_with_limits(
        registry,
        validated,
        evidence,
        ResultValidationLimits::DEFAULT,
    )
}

/// Validate provider evidence with an executor-supplied tighter policy.
pub(crate) fn validate_provider_result_with_limits(
    registry: &DescriptorRegistry,
    validated: &ValidatedMatchRequest,
    evidence: ProviderResultEvidence,
    limits: ResultValidationLimits,
) -> Result<ValidatedMatchResult, MatchError> {
    // Invocation and shape are always checked before schema, operation, or data
    // evidence so same-shaped cross-invocation reuse fails closed deterministically.
    if evidence.request_token() != validated.request_token() {
        return Err(result_error(
            "request_token_mismatch",
            "provider result belongs to a different validated request invocation",
        ));
    }
    if evidence.shape_id() != validated.shape_id() {
        return Err(result_error(
            "result_shape_mismatch",
            "provider result shape does not match the validated request",
        )
        .with_detail("expected", validated.shape_id().as_str())
        .with_detail("actual", evidence.shape_id().as_str()));
    }

    validated.recheck_schema(registry)?;

    let request = validated.request();
    let mut budget = EvidenceBudget::new(limits);
    let result = match (&request.operation, evidence.into_payload()) {
        (
            MatchOperation::FetchRows {
                output,
                window,
                cardinality,
                ..
            },
            ProviderResultPayload::Rows {
                solutions,
                apply_window,
            },
        ) => validate_rows(
            registry,
            validated,
            RowsValidationContract {
                output,
                window: *window,
                cardinality: *cardinality,
                apply_window,
            },
            solutions,
            &mut budget,
        )?,
        (
            MatchOperation::PageBy {
                root,
                output,
                window,
                include_total,
                ..
            },
            ProviderResultPayload::Page {
                root: actual_root,
                selected_roots,
                solutions,
                window: actual_window,
                total,
            },
        ) => {
            if actual_root != *root {
                return Err(result_error(
                    "page_root_mismatch",
                    "provider page root does not match the validated operation",
                )
                .at(MatchErrorPathSegment::Binding(actual_root)));
            }
            if actual_window != *window {
                return Err(result_error(
                    "page_window_mismatch",
                    "provider page window does not match the validated operation",
                ));
            }
            validate_page(
                registry,
                validated,
                PageValidationContract {
                    root: *root,
                    output,
                    window: *window,
                    include_total: *include_total,
                    total,
                    selected_roots,
                },
                solutions,
                &mut budget,
            )?
        }
        (
            MatchOperation::CountBy { root },
            ProviderResultPayload::Count {
                root: actual_root,
                value,
            },
        ) => {
            require_root(*root, actual_root, "count_root_mismatch")?;
            MatchResult::Count { root: *root, value }
        }
        (
            MatchOperation::ExistsBy { root },
            ProviderResultPayload::Exists {
                root: actual_root,
                value,
            },
        ) => {
            require_root(*root, actual_root, "exists_root_mismatch")?;
            MatchResult::Exists { root: *root, value }
        }
        (
            MatchOperation::ReduceBy {
                root,
                group,
                reducers,
            },
            ProviderResultPayload::Reduction {
                root: actual_root,
                group: actual_group,
                rows,
            },
        ) => {
            require_root(*root, actual_root, "reduction_root_mismatch")?;
            if *group != actual_group {
                return Err(result_error(
                    "reduction_group_mismatch",
                    "provider reduction echoed the wrong group binding",
                ));
            }
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
        (
            MatchOperation::ReduceByField {
                root,
                group,
                reducers,
            },
            ProviderResultPayload::FieldReduction {
                root: actual_root,
                group: actual_group,
                rows,
            },
        ) => {
            require_root(*root, actual_root, "field_reduction_root_mismatch")?;
            if *group != actual_group {
                return Err(result_error(
                    "field_reduction_group_mismatch",
                    "provider reduction echoed the wrong descriptor-qualified group field",
                ));
            }
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
        (
            MatchOperation::ReduceByFields {
                root,
                groups,
                reducers,
            },
            ProviderResultPayload::FieldTupleReduction {
                root: actual_root,
                groups: actual_groups,
                rows,
            },
        ) => {
            require_root(*root, actual_root, "field_tuple_reduction_root_mismatch")?;
            if *groups != actual_groups {
                return Err(result_error(
                    "field_tuple_reduction_groups_mismatch",
                    "provider reduction echoed the wrong ordered group fields",
                ));
            }
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
    };

    Ok(ValidatedMatchResult::new(
        ValidatedResultSeal(()),
        validated.request_token(),
        validated.shape_id().clone(),
        result,
    ))
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
/// and shallow role-player closure. It therefore cannot safely masquerade as
/// [`ProviderResultEvidence`], whose contract requires every positive V1
/// binding. The V2 validator has already proven the complete model projection;
/// this function performs only the lossless released representation mapping.
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

fn released_attribute_value(value: &CompatibilityValueV2) -> Result<AttributeValue, MatchError> {
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

struct RowsValidationContract<'a> {
    output: &'a FetchShape,
    window: Window,
    cardinality: RowCardinality,
    apply_window: bool,
}

fn validate_rows(
    registry: &DescriptorRegistry,
    validated: &ValidatedMatchRequest,
    contract: RowsValidationContract<'_>,
    solutions: Vec<ProviderSolutionEvidence>,
    budget: &mut EvidenceBudget,
) -> Result<MatchResult, MatchError> {
    let RowsValidationContract {
        output,
        window,
        cardinality,
        apply_window,
    } = contract;
    let checked = validate_solutions(registry, validated, &solutions, budget)?;
    let slots = output_slots(output).collect::<Vec<_>>();
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    let mut row_assignments = Vec::new();

    for solution in &checked {
        let identity: Vec<_> = slots
            .iter()
            .map(|slot| {
                solution
                    .bindings
                    .get(&slot.binding())
                    .expect("complete assignment validated")
                    .concept_id()
                    .clone()
            })
            .collect();
        if !seen.insert(identity) {
            continue;
        }
        budget.charge_result_identity()?;
        let row = slots
            .iter()
            .map(|slot| {
                let thing = solution.bindings[&slot.binding()].clone();
                SlotValue::One(thing)
            })
            .collect();
        rows.push(MatchRow::new(row));
        row_assignments.push(solution);
    }

    if cardinality == RowCardinality::ExactlyOne
        && let Some(error) = exactly_one_cardinality_error(rows.len())
    {
        return Err(error);
    }

    if !apply_window && rows.len() as u64 > window.limit {
        return Err(resource_error(
            "row_window_exceeded",
            "provider returned more distinct rows than the validated window permits",
        )
        .with_detail("limit", window.limit)
        .with_detail("actual", rows.len() as u64));
    }

    validate_solution_order(validated.stable_order(), &row_assignments)?;
    if apply_window && cardinality == RowCardinality::BoundedMany {
        let offset = usize::try_from(window.offset).unwrap_or(usize::MAX);
        let limit = usize::try_from(window.limit).unwrap_or(usize::MAX);
        rows = rows.into_iter().skip(offset).take(limit).collect();
    }
    Ok(MatchResult::Rows { rows })
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

struct PageValidationContract<'a> {
    root: BindingId,
    output: &'a FetchShape,
    window: Window,
    include_total: bool,
    total: Option<u64>,
    selected_roots: Vec<ConceptId>,
}

fn validate_page(
    registry: &DescriptorRegistry,
    validated: &ValidatedMatchRequest,
    contract: PageValidationContract<'_>,
    solutions: Vec<ProviderSolutionEvidence>,
    budget: &mut EvidenceBudget,
) -> Result<MatchResult, MatchError> {
    let PageValidationContract {
        root,
        output,
        window,
        include_total,
        total,
        selected_roots,
    } = contract;
    match (include_total, total) {
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

    let selected_set = selected_roots.iter().cloned().collect::<BTreeSet<_>>();
    if selected_set.len() != selected_roots.len() {
        return Err(result_error(
            "duplicate_selected_root",
            "page root selection contains a duplicate concept identity",
        )
        .at(MatchErrorPathSegment::Binding(root)));
    }
    if selected_roots.len() as u64 > window.limit {
        return Err(resource_error(
            "page_window_exceeded",
            "root selection returned more distinct roots than the validated page window permits",
        )
        .with_detail("limit", window.limit)
        .with_detail("actual", selected_roots.len() as u64));
    }

    let checked = validate_solutions(registry, validated, &solutions, budget)?;
    let mut grouped: BTreeMap<ConceptId, Vec<&CheckedSolution<'_>>> = BTreeMap::new();
    for solution in &checked {
        let root_id = solution.bindings[&root].concept_id();
        if !selected_set.contains(root_id) {
            return Err(result_error(
                "unexpected_hydrated_root",
                "page re-match returned a root outside the selected identity set",
            )
            .at(MatchErrorPathSegment::Binding(root)));
        }
        grouped.entry(root_id.clone()).or_default().push(solution);
    }
    if grouped.keys().cloned().collect::<BTreeSet<_>>() != selected_set {
        return Err(result_error(
            "selected_root_set_mismatch",
            "page re-match root set does not exactly equal root selection",
        )
        .at(MatchErrorPathSegment::Binding(root)));
    }

    let mut groups = Vec::with_capacity(selected_roots.len());
    for root_id in &selected_roots {
        budget.charge_result_identity()?;
        groups.push(
            grouped
                .remove(root_id)
                .expect("selected root equality was proven"),
        );
    }
    let representatives = groups.iter().map(|group| group[0]).collect::<Vec<_>>();
    validate_solution_order(validated.stable_order(), &representatives)?;

    let slots = output_slots(output).collect::<Vec<_>>();
    let mut entries = Vec::with_capacity(groups.len());
    for group in groups {
        let mut values = Vec::with_capacity(slots.len());
        for slot in &slots {
            match slot {
                FetchSlot::One { binding } => {
                    values.push(SlotValue::One(group[0].bindings[binding].clone()));
                }
                FetchSlot::Collect {
                    binding, distinct, ..
                } => {
                    let mut things = group
                        .iter()
                        .map(|solution| solution.bindings[binding].clone())
                        .collect::<Vec<_>>();
                    if *distinct {
                        let mut seen = BTreeSet::new();
                        things.retain(|thing| seen.insert(thing.concept_id().clone()));
                    }
                    budget.charge_collected(things.len())?;
                    let stable = validated.collection_order(*binding).ok_or_else(|| {
                        result_error(
                            "missing_collection_order_proof",
                            "validated page omitted a collected binding order proof",
                        )
                        .at(MatchErrorPathSegment::Binding(*binding))
                    })?;
                    canonical_sort_things(stable, &mut things)?;
                    values.push(SlotValue::Many(things));
                }
            }
        }
        entries.push(MatchRow::new(values));
    }

    if let Some(total) = total {
        let expected = window.limit.min(total.saturating_sub(window.offset));
        let actual = entries.len() as u64;
        if actual != expected {
            return Err(result_error(
                "page_total_length_mismatch",
                "provider page length is inconsistent with its same-snapshot total and window",
            )
            .with_detail("total", total)
            .with_detail("expected", expected)
            .with_detail("actual", actual));
        }
    }

    Ok(MatchResult::Page {
        root,
        entries,
        window,
        total,
    })
}

struct CheckedSolution<'a> {
    bindings: BTreeMap<BindingId, &'a HydratedThing>,
}

fn validate_solutions<'a>(
    registry: &DescriptorRegistry,
    validated: &ValidatedMatchRequest,
    solutions: &'a [ProviderSolutionEvidence],
    budget: &mut EvidenceBudget,
) -> Result<Vec<CheckedSolution<'a>>, MatchError> {
    budget.charge_solutions(solutions.len())?;
    let request = validated.request();
    let expected: BTreeMap<_, _> = request
        .plan
        .bindings
        .iter()
        .map(|binding| (binding.id, binding))
        .collect();
    let known_role_edges = request
        .plan
        .predicate
        .as_ref()
        .map(role_edge_ids)
        .unwrap_or_default();
    let mut consistent: BTreeMap<ConceptId, GlobalHydration<'_>> = BTreeMap::new();
    let mut checked = Vec::with_capacity(solutions.len());

    for (solution_index, solution) in solutions.iter().enumerate() {
        let mut bindings = BTreeMap::new();
        for assignment in solution.bindings() {
            let binding_id = assignment.binding();
            let Some(binding) = expected.get(&binding_id) else {
                return Err(result_error(
                    "unknown_provider_binding",
                    "provider solution contains an undeclared binding assignment",
                )
                .at(MatchErrorPathSegment::ProviderEvidence)
                .at(MatchErrorPathSegment::Index(solution_index))
                .at(MatchErrorPathSegment::Binding(binding_id)));
            };
            if bindings.insert(binding_id, assignment.thing()).is_some() {
                return Err(result_error(
                    "duplicate_provider_binding",
                    "provider solution assigns one binding more than once",
                )
                .at(MatchErrorPathSegment::ProviderEvidence)
                .at(MatchErrorPathSegment::Index(solution_index))
                .at(MatchErrorPathSegment::Binding(binding_id)));
            }
            validate_bound_thing(registry, binding, assignment.thing(), budget).map_err(
                |error| {
                    error
                        .at(MatchErrorPathSegment::ProviderEvidence)
                        .at(MatchErrorPathSegment::Index(solution_index))
                        .at(MatchErrorPathSegment::Binding(binding_id))
                },
            )?;
            require_global_hydration_consistency(
                &mut consistent,
                assignment.thing().concept_id(),
                GlobalHydration::thing(assignment.thing()),
            )
            .map_err(|error| error.at(MatchErrorPathSegment::Binding(binding_id)))?;
            for role in assignment.thing().roles() {
                for player in role.players() {
                    require_global_hydration_consistency(
                        &mut consistent,
                        player.concept_id(),
                        GlobalHydration::player(player),
                    )
                    .map_err(|error| {
                        error
                            .at(MatchErrorPathSegment::Role(role.role().clone()))
                            .at(MatchErrorPathSegment::Binding(binding_id))
                    })?;
                }
            }
        }

        if bindings.len() != expected.len() {
            let missing = expected
                .keys()
                .find(|binding| !bindings.contains_key(binding))
                .copied()
                .expect("incomplete assignment has a missing binding");
            return Err(result_error(
                "missing_provider_binding",
                "provider solution omits a required positive binding assignment",
            )
            .at(MatchErrorPathSegment::ProviderEvidence)
            .at(MatchErrorPathSegment::Index(solution_index))
            .at(MatchErrorPathSegment::Binding(missing)));
        }

        let mut satisfied = BTreeSet::new();
        for edge in solution.satisfied_role_edges() {
            if !known_role_edges.contains(edge) {
                return Err(result_error(
                    "unknown_role_edge_evidence",
                    "provider solution reports an undeclared role-edge identity",
                )
                .at(MatchErrorPathSegment::RoleEdge(*edge)));
            }
            if !satisfied.insert(*edge) {
                return Err(result_error(
                    "duplicate_role_edge_evidence",
                    "provider solution reports one role edge more than once",
                )
                .at(MatchErrorPathSegment::RoleEdge(*edge)));
            }
            let expression = find_role_edge(request.plan.predicate.as_ref(), *edge)
                .expect("known role edge was collected from the predicate");
            validate_role_edge_link(registry, expression, &bindings)?;
        }

        if let Some(predicate) = &request.plan.predicate
            && !evaluate_expression(predicate, &bindings, &satisfied)?
        {
            return Err(result_error(
                "predicate_evidence_mismatch",
                "provider solution does not satisfy the validated predicate",
            )
            .at(MatchErrorPathSegment::ProviderEvidence)
            .at(MatchErrorPathSegment::Index(solution_index)));
        }
        checked.push(CheckedSolution { bindings });
    }
    Ok(checked)
}

#[derive(Clone, Copy)]
struct GlobalHydration<'a> {
    concrete_descriptor: &'a DescriptorId,
    kind: ThingKind,
    attributes: &'a [HydratedAttribute],
    roles: Option<&'a [HydratedRole]>,
}

impl<'a> GlobalHydration<'a> {
    fn thing(thing: &'a HydratedThing) -> Self {
        Self {
            concrete_descriptor: thing.concrete_descriptor(),
            kind: thing.kind(),
            attributes: thing.attributes(),
            roles: Some(thing.roles()),
        }
    }

    fn player(player: &'a HydratedRolePlayer) -> Self {
        Self {
            concrete_descriptor: player.concrete_descriptor(),
            kind: player.kind(),
            attributes: player.attributes(),
            roles: None,
        }
    }
}

fn require_global_hydration_consistency<'a>(
    consistent: &mut BTreeMap<ConceptId, GlobalHydration<'a>>,
    concept_id: &ConceptId,
    current: GlobalHydration<'a>,
) -> Result<(), MatchError> {
    if let Some(previous) = consistent.get(concept_id) {
        let same_base = previous.concrete_descriptor == current.concrete_descriptor
            && previous.kind == current.kind
            && previous.attributes == current.attributes;
        let same_complete_roles = match (previous.roles, current.roles) {
            (Some(left), Some(right)) => left == right,
            _ => true,
        };
        if !same_base || !same_complete_roles {
            return Err(result_error(
                "inconsistent_concept_hydration",
                "one global concept identity has conflicting hydrated evidence",
            ));
        }
        if previous.roles.is_none() && current.roles.is_some() {
            consistent.insert(concept_id.clone(), current);
        }
    } else {
        consistent.insert(concept_id.clone(), current);
    }
    Ok(())
}

fn validate_bound_thing(
    registry: &DescriptorRegistry,
    binding: &MatchBinding,
    thing: &HydratedThing,
    budget: &mut EvidenceBudget,
) -> Result<(), MatchError> {
    budget.charge_thing(thing)?;
    validate_concept_id(thing.concept_id())?;
    if thing.declared_descriptor() != &binding.descriptor {
        return Err(result_error(
            "declared_descriptor_mismatch",
            "hydrated thing's declared descriptor does not match its binding",
        )
        .with_detail("expected", binding.descriptor.as_str())
        .with_detail("actual", thing.declared_descriptor().as_str()));
    }
    if thing.kind() != binding.thing_kind {
        return Err(result_error(
            "thing_kind_mismatch",
            "hydrated thing kind does not match its binding",
        ));
    }

    let declared = resolve_descriptor(registry, thing.declared_descriptor())?;
    let concrete = resolve_descriptor(registry, thing.concrete_descriptor())?;
    require_descriptor_kind(&declared, thing.kind())?;
    require_descriptor_kind(&concrete, thing.kind())?;
    match binding.match_mode {
        MatchMode::Exact if thing.concrete_descriptor() != thing.declared_descriptor() => {
            return Err(result_error(
                "exact_descriptor_mismatch",
                "exact binding returned a different concrete descriptor",
            )
            .with_detail("declared", thing.declared_descriptor().as_str())
            .with_detail("concrete", thing.concrete_descriptor().as_str()));
        }
        MatchMode::Subtypes
            if !is_descriptor_or_subtype(
                registry,
                thing.concrete_descriptor(),
                thing.declared_descriptor(),
            ) =>
        {
            return Err(result_error(
                "invalid_concrete_subtype",
                "concrete descriptor is not the declared descriptor or one of its subtypes",
            )
            .with_detail("declared", thing.declared_descriptor().as_str())
            .with_detail("concrete", thing.concrete_descriptor().as_str()));
        }
        MatchMode::Exact | MatchMode::Subtypes => {}
    }

    validate_attributes(
        registry,
        thing.concrete_descriptor(),
        &concrete,
        thing.attributes(),
        budget,
    )?;
    match concrete {
        TypeDescriptorRef::Entity(_) => {
            if !thing.roles().is_empty() {
                return Err(result_error(
                    "entity_has_role_evidence",
                    "entity hydration cannot contain relation-role evidence",
                ));
            }
        }
        TypeDescriptorRef::Relation(descriptor) => validate_roles(
            registry,
            thing.concrete_descriptor(),
            &descriptor.roles,
            thing.roles(),
            budget,
        )?,
    }
    Ok(())
}

fn validate_nested_player(
    registry: &DescriptorRegistry,
    player: &HydratedRolePlayer,
    allowed_types: &[String],
    budget: &mut EvidenceBudget,
) -> Result<(), MatchError> {
    budget.charge_role_player(player)?;
    validate_concept_id(player.concept_id())?;
    let declared = resolve_descriptor(registry, player.declared_descriptor())?;
    let concrete = resolve_descriptor(registry, player.concrete_descriptor())?;
    require_descriptor_kind(&declared, player.kind())?;
    require_descriptor_kind(&concrete, player.kind())?;
    if !is_descriptor_or_subtype(
        registry,
        player.concrete_descriptor(),
        player.declared_descriptor(),
    ) {
        return Err(result_error(
            "invalid_role_player_subtype",
            "role player's concrete descriptor is not compatible with its declared descriptor",
        ));
    }
    let concrete_name = descriptor_name(player.concrete_descriptor())?;
    if !allowed_types
        .iter()
        .any(|allowed| is_type_name_or_subtype(registry, concrete_name, allowed))
    {
        return Err(result_error(
            "incompatible_hydrated_role_player",
            "hydrated role player is not compatible with the registered role",
        ));
    }
    validate_attributes(
        registry,
        player.concrete_descriptor(),
        &concrete,
        player.attributes(),
        budget,
    )
}

fn validate_attributes(
    registry: &DescriptorRegistry,
    declared_id: &DescriptorId,
    descriptor: &TypeDescriptorRef,
    attributes: &[HydratedAttribute],
    budget: &mut EvidenceBudget,
) -> Result<(), MatchError> {
    let expected = descriptor_attributes(descriptor);
    let mut present = BTreeSet::new();
    for attribute in attributes {
        budget.charge_bytes(attribute.field().owner.as_str().len())?;
        budget.charge_bytes(attribute.field().name.len())?;
        if attribute.field().owner != *declared_id {
            return Err(result_error(
                "attribute_owner_mismatch",
                "hydrated field identity is not owned by the declared descriptor",
            )
            .at(MatchErrorPathSegment::Field(attribute.field().clone())));
        }
        if !present.insert(attribute.field().clone()) {
            return Err(result_error(
                "duplicate_hydrated_attribute",
                "hydrated thing contains one field more than once",
            )
            .at(MatchErrorPathSegment::Field(attribute.field().clone())));
        }
        let canonical = registry
            .field_id(declared_id, &attribute.field().name)
            .ok_or_else(|| {
                result_error(
                    "unknown_hydrated_attribute",
                    "hydrated field does not exist on the declared descriptor",
                )
                .at(MatchErrorPathSegment::Field(attribute.field().clone()))
            })?;
        if canonical != *attribute.field() {
            return Err(result_error(
                "non_canonical_hydrated_attribute",
                "hydrated field identity is not canonical",
            )
            .at(MatchErrorPathSegment::Field(attribute.field().clone())));
        }
        let field = expected
            .iter()
            .find(|field| field.field_name == attribute.field().name)
            .expect("canonical field is present on resolved descriptor");
        let (minimum, maximum) = attribute_cardinality(field);
        let actual = attribute.values().len();
        if actual < minimum as usize || maximum.is_some_and(|maximum| actual > maximum as usize) {
            return Err(result_error(
                "hydrated_attribute_cardinality",
                "hydrated field value count violates its registered cardinality",
            )
            .at(MatchErrorPathSegment::Field(attribute.field().clone()))
            .with_detail("actual", actual as u64));
        }
        budget.charge_attribute_values(attribute.values())?;
        if field
            .annotations
            .iter()
            .any(|annotation| matches!(annotation, Annotation::Distinct))
            && attribute
                .values()
                .iter()
                .enumerate()
                .any(|(index, value)| attribute.values()[..index].contains(value))
        {
            return Err(result_error(
                "duplicate_distinct_attribute_value",
                "hydrated distinct attribute contains duplicate values",
            )
            .at(MatchErrorPathSegment::Field(attribute.field().clone())));
        }
        for value in attribute.values() {
            if field.value_type.as_str() != value.value_type_name() {
                return Err(result_error(
                    "hydrated_attribute_type_mismatch",
                    "hydrated value type does not match its registered field",
                )
                .at(MatchErrorPathSegment::Field(attribute.field().clone())));
            }
            if !safe_value(value) {
                return Err(result_error(
                    "unsafe_hydrated_value",
                    "hydrated value contains unsafe numeric or malformed temporal data",
                )
                .at(MatchErrorPathSegment::Field(attribute.field().clone())));
            }
        }
    }

    for field in expected {
        let (minimum, _) = attribute_cardinality(field);
        if minimum > 0 {
            let field_id = registry
                .field_id(declared_id, &field.field_name)
                .expect("descriptor field remains registered");
            if !present.contains(&field_id) {
                return Err(result_error(
                    "missing_required_attribute",
                    "hydrated thing omits a required registered field",
                )
                .at(MatchErrorPathSegment::Field(field_id)));
            }
        }
    }
    Ok(())
}

fn validate_roles(
    registry: &DescriptorRegistry,
    declared_id: &DescriptorId,
    expected: &[RoleDescriptor],
    roles: &[HydratedRole],
    budget: &mut EvidenceBudget,
) -> Result<(), MatchError> {
    let mut present = BTreeSet::new();
    for role in roles {
        budget.charge_bytes(role.role().owner.as_str().len())?;
        budget.charge_bytes(role.role().name.len())?;
        if role.role().owner != *declared_id {
            return Err(result_error(
                "role_owner_mismatch",
                "hydrated role identity is not owned by the declared relation",
            )
            .at(MatchErrorPathSegment::Role(role.role().clone())));
        }
        if !present.insert(role.role().clone()) {
            return Err(result_error(
                "duplicate_hydrated_role",
                "hydrated relation contains one role more than once",
            )
            .at(MatchErrorPathSegment::Role(role.role().clone())));
        }
        let canonical = registry
            .role_id(declared_id, &role.role().name)
            .ok_or_else(|| {
                result_error(
                    "unknown_hydrated_role",
                    "hydrated role does not exist on the declared relation",
                )
                .at(MatchErrorPathSegment::Role(role.role().clone()))
            })?;
        if canonical != *role.role() {
            return Err(result_error(
                "non_canonical_hydrated_role",
                "hydrated role identity is not canonical",
            )
            .at(MatchErrorPathSegment::Role(role.role().clone())));
        }
        let descriptor = expected
            .iter()
            .find(|candidate| candidate.role_name == role.role().name)
            .expect("canonical role is present on resolved descriptor");
        let actual = role.players().len();
        if let Some((minimum, maximum)) = descriptor.cardinality
            && (actual < minimum as usize
                || maximum.is_some_and(|maximum| actual > maximum as usize))
        {
            return Err(result_error(
                "hydrated_role_cardinality",
                "hydrated role-player count violates its registered cardinality",
            )
            .at(MatchErrorPathSegment::Role(role.role().clone()))
            .with_detail("actual", actual as u64));
        }
        let mut players = BTreeSet::new();
        for player in role.players() {
            if descriptor.distinct && !players.insert(player.concept_id().clone()) {
                return Err(result_error(
                    "duplicate_hydrated_role_player",
                    "one concept appears more than once for a distinct hydrated relation role",
                )
                .at(MatchErrorPathSegment::Role(role.role().clone())));
            }
            validate_nested_player(registry, player, &descriptor.player_type_names, budget)
                .map_err(|error| error.at(MatchErrorPathSegment::Role(role.role().clone())))?;
        }
    }

    for descriptor in expected {
        if descriptor
            .cardinality
            .is_some_and(|(minimum, _)| minimum > 0)
        {
            let role_id = registry
                .role_id(declared_id, &descriptor.role_name)
                .expect("descriptor role remains registered");
            if !present.contains(&role_id) {
                return Err(result_error(
                    "missing_required_role",
                    "hydrated relation omits a required registered role",
                )
                .at(MatchErrorPathSegment::Role(role_id)));
            }
        }
    }
    Ok(())
}

fn attribute_cardinality(attribute: &OwnedAttributeDescriptor) -> (u32, Option<u32>) {
    attribute.cardinality().unwrap_or({
        if attribute.is_optional {
            (0, Some(1))
        } else {
            (1, Some(1))
        }
    })
}

fn resolve_descriptor(
    registry: &DescriptorRegistry,
    id: &DescriptorId,
) -> Result<TypeDescriptorRef, MatchError> {
    let name = descriptor_name(id)?;
    let descriptor = registry.get(name).ok_or_else(|| {
        result_error(
            "unknown_hydrated_descriptor",
            "hydrated descriptor is not registered in the current schema",
        )
        .with_detail("descriptor", id.as_str())
    })?;
    if registry.descriptor_id(name).as_ref() != Some(id) {
        return Err(result_error(
            "hydrated_descriptor_kind_mismatch",
            "hydrated descriptor kind does not match the registered descriptor",
        ));
    }
    Ok(descriptor)
}

fn descriptor_name(id: &DescriptorId) -> Result<&str, MatchError> {
    let Some((kind, name)) = id.as_str().split_once(':') else {
        return Err(result_error(
            "malformed_hydrated_descriptor",
            "hydrated descriptor identity is not kind-qualified",
        ));
    };
    if name.is_empty() || !matches!(kind, "entity" | "relation") {
        return Err(result_error(
            "malformed_hydrated_descriptor",
            "hydrated descriptor identity has an unsupported kind or empty name",
        ));
    }
    Ok(name)
}

fn require_descriptor_kind(
    descriptor: &TypeDescriptorRef,
    expected: ThingKind,
) -> Result<(), MatchError> {
    let actual = match descriptor {
        TypeDescriptorRef::Entity(_) => ThingKind::Entity,
        TypeDescriptorRef::Relation(_) => ThingKind::Relation,
    };
    if actual == expected {
        Ok(())
    } else {
        Err(result_error(
            "hydrated_thing_kind_mismatch",
            "hydrated descriptor kind does not match the reported thing kind",
        ))
    }
}

fn is_descriptor_or_subtype(
    registry: &DescriptorRegistry,
    actual: &DescriptorId,
    expected: &DescriptorId,
) -> bool {
    let (Ok(actual), Ok(expected)) = (descriptor_name(actual), descriptor_name(expected)) else {
        return false;
    };
    is_type_name_or_subtype(registry, actual, expected)
}

fn is_type_name_or_subtype(registry: &DescriptorRegistry, actual: &str, expected: &str) -> bool {
    let mut current = Some(actual.to_owned());
    let mut visited = BTreeSet::new();
    while let Some(name) = current {
        if name == expected {
            return true;
        }
        if !visited.insert(name.clone()) {
            return false;
        }
        current = registry.get(&name).and_then(|descriptor| match descriptor {
            TypeDescriptorRef::Entity(descriptor) => descriptor.parent_type.clone(),
            TypeDescriptorRef::Relation(descriptor) => descriptor.parent_type.clone(),
        });
    }
    false
}

fn descriptor_attributes(descriptor: &TypeDescriptorRef) -> &[OwnedAttributeDescriptor] {
    match descriptor {
        TypeDescriptorRef::Entity(descriptor) => &descriptor.owned_attributes,
        TypeDescriptorRef::Relation(descriptor) => &descriptor.owned_attributes,
    }
}

fn validate_concept_id(concept: &ConceptId) -> Result<(), MatchError> {
    if concept.as_str().is_empty() || concept.as_str().len() > MAX_SEMANTIC_ID_BYTES {
        Err(result_error(
            "invalid_concept_id",
            "provider concept identity is empty or exceeds the semantic-ID byte ceiling",
        ))
    } else {
        Ok(())
    }
}

fn role_edge_ids(expression: &MatchExpr) -> BTreeSet<RoleEdgeId> {
    let mut edges = BTreeSet::new();
    collect_role_edge_ids(expression, &mut edges);
    edges
}

fn collect_role_edge_ids(expression: &MatchExpr, edges: &mut BTreeSet<RoleEdgeId>) {
    match expression {
        MatchExpr::RoleEdge { id, .. } => {
            edges.insert(*id);
        }
        MatchExpr::And { expressions } | MatchExpr::Or { expressions } => {
            for child in expressions {
                collect_role_edge_ids(child, edges);
            }
        }
        MatchExpr::Not { expression } => collect_role_edge_ids(expression, edges),
        MatchExpr::FieldValue { .. }
        | MatchExpr::FieldComparison { .. }
        | MatchExpr::FieldPresence { .. }
        | MatchExpr::BindingIid { .. }
        | MatchExpr::Reachable { .. } => {}
    }
}

fn find_role_edge(expression: Option<&MatchExpr>, id: RoleEdgeId) -> Option<&MatchExpr> {
    let expression = expression?;
    match expression {
        MatchExpr::RoleEdge { id: candidate, .. } if *candidate == id => Some(expression),
        MatchExpr::And { expressions } | MatchExpr::Or { expressions } => expressions
            .iter()
            .find_map(|child| find_role_edge(Some(child), id)),
        MatchExpr::Not { expression } => find_role_edge(Some(expression), id),
        MatchExpr::FieldValue { .. }
        | MatchExpr::FieldComparison { .. }
        | MatchExpr::FieldPresence { .. }
        | MatchExpr::BindingIid { .. }
        | MatchExpr::RoleEdge { .. }
        | MatchExpr::Reachable { .. } => None,
    }
}

fn validate_role_edge_link(
    registry: &DescriptorRegistry,
    expression: &MatchExpr,
    bindings: &BTreeMap<BindingId, &HydratedThing>,
) -> Result<(), MatchError> {
    let MatchExpr::RoleEdge {
        id,
        relation,
        role,
        player,
    } = expression
    else {
        unreachable!("role-edge lookup returned a different expression variant")
    };
    let relation_thing = bindings
        .get(relation)
        .expect("complete assignment includes relation binding");
    let player_thing = bindings
        .get(player)
        .expect("complete assignment includes player binding");
    let linked = relation_thing.roles().iter().any(|hydrated_role| {
        hydrated_role.role().name == role.name
            && hydrated_role.players().iter().any(|candidate| {
                candidate.concept_id() == player_thing.concept_id()
                    && candidate.concrete_descriptor() == player_thing.concrete_descriptor()
                    && declared_descriptors_share_lineage(
                        registry,
                        candidate.declared_descriptor(),
                        player_thing.declared_descriptor(),
                    )
                    && candidate.kind() == player_thing.kind()
                    && candidate.attributes() == player_thing.attributes()
            })
    });
    if linked {
        Ok(())
    } else {
        Err(result_error(
            "role_edge_hydration_mismatch",
            "satisfied role-edge evidence is absent from hydrated relation players",
        )
        .at(MatchErrorPathSegment::RoleEdge(*id)))
    }
}

fn declared_descriptors_share_lineage(
    registry: &DescriptorRegistry,
    left: &DescriptorId,
    right: &DescriptorId,
) -> bool {
    registry.is_same_or_subtype(left, right) || registry.is_same_or_subtype(right, left)
}

fn evaluate_expression(
    expression: &MatchExpr,
    bindings: &BTreeMap<BindingId, &HydratedThing>,
    satisfied_role_edges: &BTreeSet<RoleEdgeId>,
) -> Result<bool, MatchError> {
    match expression {
        MatchExpr::FieldValue {
            field,
            operator,
            value,
        } => {
            for candidate in field_values(bindings, field) {
                if compare_values(*operator, candidate, value)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        MatchExpr::FieldComparison {
            left,
            operator,
            right,
        } => {
            let left = field_values(bindings, left);
            let right = field_values(bindings, right);
            for left_value in left {
                for right_value in right {
                    if compare_values(*operator, left_value, right_value)? {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        }
        MatchExpr::FieldPresence { field, present } => {
            Ok(field_values(bindings, field).is_empty() != *present)
        }
        MatchExpr::BindingIid { binding, iid } => Ok(bindings
            .get(binding)
            .is_some_and(|thing| thing.concept_id().as_str().eq_ignore_ascii_case(iid))),
        MatchExpr::RoleEdge { id, .. } => Ok(satisfied_role_edges.contains(id)),
        MatchExpr::Reachable {
            source,
            target,
            max_depth,
            ..
        } => {
            // The zero-only case remains fully claim-checkable after proof
            // projection: TypeQL `is` means exact concept identity.
            if *max_depth == 0 {
                return Ok(bindings
                    .get(source)
                    .zip(bindings.get(target))
                    .is_some_and(|(source, target)| source.concept_id() == target.concept_id()));
            }
            // Trust boundary: positive proof variables are intentionally
            // existential and are projected away by the typed compiler.
            // Result validation therefore trusts the already-authenticated
            // provider execution for path existence while independently
            // claim-checking every public endpoint. This is not evidence that
            // an arbitrary caller may forge after the provider boundary.
            Ok(true)
        }
        MatchExpr::And { expressions } => {
            for child in expressions {
                if !evaluate_expression(child, bindings, satisfied_role_edges)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        MatchExpr::Or { expressions } => {
            for child in expressions {
                if evaluate_expression(child, bindings, satisfied_role_edges)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        MatchExpr::Not { expression } => Ok(!evaluate_expression(
            expression,
            bindings,
            satisfied_role_edges,
        )?),
    }
}

fn field_values<'a>(
    bindings: &'a BTreeMap<BindingId, &'a HydratedThing>,
    field: &BoundFieldId,
) -> &'a [AttributeValue] {
    bindings
        .get(&field.binding)
        .and_then(|thing| {
            thing
                .attributes()
                .iter()
                .find(|attribute| attribute.field().name == field.field.name)
        })
        .map(HydratedAttribute::values)
        .unwrap_or_default()
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

fn value_order(left: &AttributeValue, right: &AttributeValue) -> Option<Ordering> {
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

fn validate_solution_order(
    stable: &StableOrderSpec,
    solutions: &[&CheckedSolution<'_>],
) -> Result<(), MatchError> {
    if stable.is_empty() || solutions.len() < 2 {
        return Ok(());
    }
    for pair in solutions.windows(2) {
        let ordering = compare_assignment_order(pair[0], pair[1], stable)?;
        if ordering == Ordering::Greater {
            return Err(result_error(
                "unstable_provider_order",
                "provider rows or roots are not in the validated stable order",
            ));
        }
        if ordering == Ordering::Equal && selected_identity(pair[0]) != selected_identity(pair[1]) {
            return Err(result_error(
                "non_total_provider_order",
                "validated stable order does not distinguish two provider identities",
            ));
        }
    }
    Ok(())
}

fn compare_assignment_order(
    left: &CheckedSolution<'_>,
    right: &CheckedSolution<'_>,
    stable: &StableOrderSpec,
) -> Result<Ordering, MatchError> {
    for term in stable.terms() {
        let order = term.order();
        let ordering = compare_optional_order_values(
            singular_field_value(&left.bindings, &order.field)?,
            singular_field_value(&right.bindings, &order.field)?,
            order,
        )?;
        if ordering != Ordering::Equal {
            return Ok(ordering);
        }
    }
    Ok(Ordering::Equal)
}

fn selected_identity(solution: &CheckedSolution<'_>) -> Vec<(BindingId, ConceptId)> {
    solution
        .bindings
        .iter()
        .map(|(binding, thing)| (*binding, thing.concept_id().clone()))
        .collect()
}

fn singular_field_value<'a>(
    bindings: &'a BTreeMap<BindingId, &'a HydratedThing>,
    field: &BoundFieldId,
) -> Result<Option<&'a AttributeValue>, MatchError> {
    let values = field_values(bindings, field);
    if values.len() > 1 {
        return Err(result_error(
            "non_scalar_order_evidence",
            "provider returned multiple values for a validated scalar order field",
        )
        .at(MatchErrorPathSegment::Field(field.field.clone())));
    }
    Ok(values.first())
}

fn compare_optional_order_values(
    left: Option<&AttributeValue>,
    right: Option<&AttributeValue>,
    order: &MatchOrder,
) -> Result<Ordering, MatchError> {
    let ordering = match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => match order.missing {
            MissingOrder::Reject => {
                return Err(result_error(
                    "missing_order_value",
                    "provider omitted a value required by stable ordering",
                )
                .at(MatchErrorPathSegment::Field(order.field.field.clone())));
            }
            MissingOrder::First => Ordering::Less,
            MissingOrder::Last => Ordering::Greater,
        },
        (Some(_), None) => match order.missing {
            MissingOrder::Reject => {
                return Err(result_error(
                    "missing_order_value",
                    "provider omitted a value required by stable ordering",
                )
                .at(MatchErrorPathSegment::Field(order.field.field.clone())));
            }
            MissingOrder::First => Ordering::Greater,
            MissingOrder::Last => Ordering::Less,
        },
        (Some(left), Some(right)) => {
            let ordering = value_order(left, right).ok_or_else(|| {
                result_error(
                    "order_value_type_mismatch",
                    "provider order values are not mutually comparable",
                )
                .at(MatchErrorPathSegment::Field(order.field.field.clone()))
            })?;
            if order.direction == SortDirection::Descending {
                ordering.reverse()
            } else {
                ordering
            }
        }
    };
    Ok(ordering)
}

fn canonical_sort_things(
    stable: &StableOrderSpec,
    things: &mut [HydratedThing],
) -> Result<(), MatchError> {
    let order = stable
        .terms()
        .iter()
        .map(|term| term.order().clone())
        .collect::<Vec<_>>();
    for term in &order {
        let values = things
            .iter()
            .map(|thing| thing_field_value(thing, &term.field.field))
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(reference) = values.first().copied() {
            for value in values {
                compare_optional_order_values(reference, value, term)?;
            }
        }
    }
    things
        .sort_by(|left, right| compare_thing_order(&order, left, right).unwrap_or(Ordering::Equal));
    validate_thing_order(&order, things)
}

fn compare_thing_order(
    order: &[MatchOrder],
    left: &HydratedThing,
    right: &HydratedThing,
) -> Result<Ordering, MatchError> {
    for term in order {
        let comparison = compare_optional_order_values(
            thing_field_value(left, &term.field.field)?,
            thing_field_value(right, &term.field.field)?,
            term,
        )?;
        if comparison != Ordering::Equal {
            return Ok(comparison);
        }
    }
    Ok(Ordering::Equal)
}

fn validate_thing_order(order: &[MatchOrder], things: &[HydratedThing]) -> Result<(), MatchError> {
    if order.is_empty() || things.len() < 2 {
        return Ok(());
    }
    for pair in things.windows(2) {
        let mut final_order = Ordering::Equal;
        for term in order {
            let left = thing_field_value(&pair[0], &term.field.field)?;
            let right = thing_field_value(&pair[1], &term.field.field)?;
            let ordering = compare_optional_order_values(left, right, term)?;
            if ordering != Ordering::Equal {
                final_order = ordering;
                break;
            }
        }
        if final_order == Ordering::Greater {
            return Err(result_error(
                "unstable_collection_order",
                "provider collection members are not in validated stable order",
            ));
        }
        if final_order == Ordering::Equal && pair[0].concept_id() != pair[1].concept_id() {
            return Err(result_error(
                "non_total_collection_order",
                "validated collection order does not distinguish two concept identities",
            ));
        }
    }
    Ok(())
}

fn thing_field_value<'a>(
    thing: &'a HydratedThing,
    field: &FieldId,
) -> Result<Option<&'a AttributeValue>, MatchError> {
    let Some(attribute) = thing
        .attributes()
        .iter()
        .find(|attribute| attribute.field().name == field.name)
    else {
        return Ok(None);
    };
    if attribute.values().len() > 1 {
        return Err(result_error(
            "non_scalar_collection_order_evidence",
            "provider returned multiple values for a scalar collection order field",
        )
        .at(MatchErrorPathSegment::Field(field.clone())));
    }
    Ok(attribute.values().first())
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
                let (whole, fraction) = number
                    .split_once('.')
                    .map_or((number.as_str(), ""), |parts| parts);
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

struct EvidenceBudget {
    limits: ResultValidationLimits,
    hydrated_things: usize,
    attribute_values: usize,
    collected_concepts: usize,
    evidence_bytes: usize,
    result_identities: usize,
}

impl EvidenceBudget {
    fn new(limits: ResultValidationLimits) -> Self {
        Self {
            limits,
            hydrated_things: 0,
            attribute_values: 0,
            collected_concepts: 0,
            evidence_bytes: 0,
            result_identities: 0,
        }
    }

    fn charge_solutions(&mut self, count: usize) -> Result<(), MatchError> {
        self.require_limit("provider_solution_limit", count, self.limits.solutions)
    }

    fn charge_result_identity(&mut self) -> Result<(), MatchError> {
        self.result_identities = self.result_identities.checked_add(1).ok_or_else(|| {
            resource_error(
                "result_identity_limit",
                "distinct result identity counter overflowed",
            )
        })?;
        self.require_limit(
            "result_identity_limit",
            self.result_identities,
            self.limits.result_identities,
        )
    }

    fn charge_collected(&mut self, count: usize) -> Result<(), MatchError> {
        self.collected_concepts = self.collected_concepts.checked_add(count).ok_or_else(|| {
            resource_error(
                "collected_concept_limit",
                "collected concept counter overflowed",
            )
        })?;
        self.require_limit(
            "collected_concept_limit",
            self.collected_concepts,
            self.limits.collected_concepts,
        )
    }

    fn charge_thing(&mut self, thing: &HydratedThing) -> Result<(), MatchError> {
        self.hydrated_things = self.hydrated_things.checked_add(1).ok_or_else(|| {
            resource_error("hydrated_thing_limit", "hydrated thing counter overflowed")
        })?;
        self.require_limit(
            "hydrated_thing_limit",
            self.hydrated_things,
            self.limits.hydrated_things,
        )?;
        self.charge_bytes(thing.concept_id().as_str().len())?;
        self.charge_bytes(thing.declared_descriptor().as_str().len())?;
        self.charge_bytes(thing.concrete_descriptor().as_str().len())
    }

    fn charge_role_player(&mut self, player: &HydratedRolePlayer) -> Result<(), MatchError> {
        self.hydrated_things = self.hydrated_things.checked_add(1).ok_or_else(|| {
            resource_error("hydrated_thing_limit", "hydrated thing counter overflowed")
        })?;
        self.require_limit(
            "hydrated_thing_limit",
            self.hydrated_things,
            self.limits.hydrated_things,
        )?;
        self.charge_bytes(player.concept_id().as_str().len())?;
        self.charge_bytes(player.declared_descriptor().as_str().len())?;
        self.charge_bytes(player.concrete_descriptor().as_str().len())
    }

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
        self.require_limit(
            "hydrated_attribute_value_limit",
            self.attribute_values,
            self.limits.attribute_values,
        )?;
        for value in values {
            self.charge_bytes(attribute_value_bytes(value))?;
        }
        Ok(())
    }

    fn charge_bytes(&mut self, bytes: usize) -> Result<(), MatchError> {
        self.evidence_bytes = self.evidence_bytes.checked_add(bytes).ok_or_else(|| {
            resource_error(
                "provider_evidence_byte_limit",
                "provider evidence byte counter overflowed",
            )
        })?;
        self.require_limit(
            "provider_evidence_byte_limit",
            self.evidence_bytes,
            self.limits.evidence_bytes,
        )
    }

    fn require_limit(
        &self,
        code: &'static str,
        actual: usize,
        limit: usize,
    ) -> Result<(), MatchError> {
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
    use super::*;
    use crate::_attribute::ValueType;
    use crate::_descriptor::{EntityDescriptor, RelationDescriptor};
    use crate::match_request::ids::{ResultShapeId, RoleId};
    use crate::match_request::model::{BindingPair, MatchPlan, MatchRequest};
    use crate::match_request::result::BoundConceptEvidence;
    use crate::match_request::validation::{request_token_for_test, validate_match_request};

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

    fn player_from_thing(thing: &HydratedThing) -> HydratedRolePlayer {
        HydratedRolePlayer::new(
            thing.concept_id().clone(),
            thing.declared_descriptor().clone(),
            thing.concrete_descriptor().clone(),
            thing.kind(),
            thing.attributes().to_vec(),
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

    fn solution(bindings: Vec<(u16, HydratedThing)>, edges: Vec<u16>) -> ProviderSolutionEvidence {
        ProviderSolutionEvidence::new(
            bindings
                .into_iter()
                .map(|(binding, thing)| BoundConceptEvidence::new(BindingId::new(binding), thing))
                .collect(),
            edges.into_iter().map(RoleEdgeId::new).collect(),
        )
    }

    fn rows_evidence(
        validated: &ValidatedMatchRequest,
        solutions: Vec<ProviderSolutionEvidence>,
    ) -> ProviderResultEvidence {
        ProviderResultEvidence::rows(
            validated.request_token(),
            validated.shape_id().clone(),
            solutions,
        )
    }

    fn error_code(error: &MatchError) -> &str {
        error.code().as_str()
    }

    #[test]
    fn invocation_and_shape_are_rejected_before_evidence_interpretation() {
        let registry = registry();
        let declared = registry.descriptor_id("person").unwrap();
        let validated = exact_one_request(
            &registry,
            vec![binding(
                &registry,
                0,
                "person",
                ThingKind::Entity,
                MatchMode::Exact,
            )],
            Some(MatchExpr::FieldValue {
                field: BoundFieldId::new(
                    BindingId::new(0),
                    registry.field_id(&declared, "name").unwrap(),
                ),
                operator: ComparisonOp::Equal,
                value: AttributeValue::String("Alice".into()),
            }),
            FetchShape::Positional {
                slots: vec![FetchSlot::One {
                    binding: BindingId::new(0),
                }],
            },
            BTreeSet::new(),
        );
        let wrong_token = ProviderResultEvidence::rows(
            request_token_for_test([0xff; 16]),
            ResultShapeId::new("wrong-shape-too"),
            vec![],
        );
        let error = validate_provider_result(&registry, &validated, wrong_token).unwrap_err();
        assert_eq!(error_code(&error), "request_token_mismatch");

        let wrong_shape = ProviderResultEvidence::rows(
            validated.request_token(),
            ResultShapeId::new("wrong-shape"),
            vec![],
        );
        let error = validate_provider_result(&registry, &validated, wrong_shape).unwrap_err();
        assert_eq!(error_code(&error), "result_shape_mismatch");
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
    fn selected_tuple_dedup_ignores_hidden_witness_identity() {
        let registry = registry();
        let validated = exact_one_request(
            &registry,
            vec![
                binding(&registry, 0, "person", ThingKind::Entity, MatchMode::Exact),
                binding(&registry, 1, "company", ThingKind::Entity, MatchMode::Exact),
            ],
            None,
            FetchShape::Positional {
                slots: vec![FetchSlot::One {
                    binding: BindingId::new(0),
                }],
            },
            BTreeSet::from([BindingPair::new(BindingId::new(0), BindingId::new(1))]),
        );
        let evidence = rows_evidence(
            &validated,
            vec![
                solution(
                    vec![
                        (0, person(&registry, "person-1", "Alice")),
                        (1, company(&registry, "company-1", "Acme")),
                    ],
                    vec![],
                ),
                solution(
                    vec![
                        (0, person(&registry, "person-1", "Alice")),
                        (1, company(&registry, "company-2", "Globex")),
                    ],
                    vec![],
                ),
            ],
        );

        let result = validate_provider_result(&registry, &validated, evidence).unwrap();
        let MatchResult::Rows { rows } = result.result() else {
            panic!("expected rows")
        };
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn incomplete_duplicate_and_cardinality_hostility_fail_atomically() {
        let registry = registry();
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
            FetchShape::Positional {
                slots: vec![FetchSlot::One {
                    binding: BindingId::new(0),
                }],
            },
            BTreeSet::new(),
        );

        let missing = rows_evidence(&validated, vec![solution(vec![], vec![])]);
        assert_eq!(
            error_code(&validate_provider_result(&registry, &validated, missing).unwrap_err()),
            "missing_provider_binding"
        );

        let duplicate = rows_evidence(
            &validated,
            vec![solution(
                vec![
                    (0, person(&registry, "person-1", "Alice")),
                    (0, person(&registry, "person-1", "Alice")),
                ],
                vec![],
            )],
        );
        assert_eq!(
            error_code(&validate_provider_result(&registry, &validated, duplicate).unwrap_err()),
            "duplicate_provider_binding"
        );

        let multiple = rows_evidence(
            &validated,
            vec![
                solution(vec![(0, person(&registry, "person-1", "Alice"))], vec![]),
                solution(vec![(0, person(&registry, "person-2", "Bob"))], vec![]),
            ],
        );
        let error = validate_provider_result(&registry, &validated, multiple).unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::Cardinality);
        assert_eq!(error_code(&error), "not_unique");

        let empty = rows_evidence(&validated, vec![]);
        let error = validate_provider_result(&registry, &validated, empty).unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::Cardinality);
        assert_eq!(error_code(&error), "no_result");
    }

    #[test]
    fn repeated_bindings_reject_conflicting_hydration_for_one_global_iid() {
        let registry = registry();
        let validated = exact_one_request(
            &registry,
            vec![
                binding(&registry, 0, "person", ThingKind::Entity, MatchMode::Exact),
                binding(&registry, 1, "person", ThingKind::Entity, MatchMode::Exact),
            ],
            None,
            FetchShape::Positional {
                slots: vec![
                    FetchSlot::One {
                        binding: BindingId::new(0),
                    },
                    FetchSlot::One {
                        binding: BindingId::new(1),
                    },
                ],
            },
            BTreeSet::from([BindingPair::new(BindingId::new(0), BindingId::new(1))]),
        );
        let evidence = rows_evidence(
            &validated,
            vec![solution(
                vec![
                    (0, person(&registry, "0x01", "Alice")),
                    (1, person(&registry, "0x01", "Mallory")),
                ],
                vec![],
            )],
        );
        assert_eq!(
            error_code(&validate_provider_result(&registry, &validated, evidence).unwrap_err()),
            "inconsistent_concept_hydration"
        );
    }

    #[test]
    fn nested_role_players_share_global_iid_hydration_consistency() {
        let registry = registry();
        let person_and_relation = exact_one_request(
            &registry,
            vec![
                binding(&registry, 0, "person", ThingKind::Entity, MatchMode::Exact),
                binding(
                    &registry,
                    1,
                    "employment",
                    ThingKind::Relation,
                    MatchMode::Exact,
                ),
            ],
            None,
            FetchShape::Positional {
                slots: vec![
                    FetchSlot::One {
                        binding: BindingId::new(0),
                    },
                    FetchSlot::One {
                        binding: BindingId::new(1),
                    },
                ],
            },
            BTreeSet::from([BindingPair::new(BindingId::new(0), BindingId::new(1))]),
        );
        let relation = registry.descriptor_id("employment").unwrap();
        let conflicting_nested = rows_evidence(
            &person_and_relation,
            vec![solution(
                vec![
                    (0, person(&registry, "0x01", "Alice")),
                    (
                        1,
                        employment(
                            &registry,
                            "0x10",
                            "code-1",
                            vec![HydratedRole::new(
                                RoleId::new(relation.clone(), "employee"),
                                vec![player_from_thing(&person(&registry, "0x01", "Mallory"))],
                            )],
                        ),
                    ),
                ],
                vec![],
            )],
        );
        assert_eq!(
            error_code(
                &validate_provider_result(&registry, &person_and_relation, conflicting_nested)
                    .unwrap_err()
            ),
            "inconsistent_concept_hydration"
        );

        let relation_only = exact_one_request(
            &registry,
            vec![binding(
                &registry,
                0,
                "employment",
                ThingKind::Relation,
                MatchMode::Exact,
            )],
            None,
            FetchShape::Positional {
                slots: vec![FetchSlot::One {
                    binding: BindingId::new(0),
                }],
            },
            BTreeSet::new(),
        );
        let conflicting_roles = rows_evidence(
            &relation_only,
            vec![solution(
                vec![(
                    0,
                    employment(
                        &registry,
                        "0x10",
                        "code-1",
                        vec![
                            HydratedRole::new(
                                RoleId::new(relation.clone(), "employee"),
                                vec![player_from_thing(&person(&registry, "0x01", "Alice"))],
                            ),
                            HydratedRole::new(
                                RoleId::new(relation, "distinct_employee"),
                                vec![player_from_thing(&person(&registry, "0x01", "Mallory"))],
                            ),
                        ],
                    ),
                )],
                vec![],
            )],
        );
        assert_eq!(
            error_code(
                &validate_provider_result(&registry, &relation_only, conflicting_roles)
                    .unwrap_err()
            ),
            "inconsistent_concept_hydration"
        );
    }

    fn subtype_employee(
        registry: &DescriptorRegistry,
        concept: &str,
        attribute_owner: &str,
        notes: Vec<&str>,
        tags: Vec<&str>,
    ) -> HydratedThing {
        let declared = registry.descriptor_id("person").unwrap();
        let concrete = registry.descriptor_id("employee").unwrap();
        let values = |values: Vec<&str>| {
            values
                .into_iter()
                .map(|value| AttributeValue::String(value.into()))
                .collect()
        };
        let mut attributes = vec![
            hydrated_attribute(
                registry,
                attribute_owner,
                "name",
                vec![AttributeValue::String("Alice".into())],
            ),
            hydrated_attribute(
                registry,
                "employee",
                "badge",
                vec![AttributeValue::String("badge-1".into())],
            ),
        ];
        if !notes.is_empty() {
            attributes.push(hydrated_attribute(
                registry,
                "employee",
                "notes",
                values(notes),
            ));
        }
        if !tags.is_empty() {
            attributes.push(hydrated_attribute(
                registry,
                "employee",
                "tags",
                values(tags),
            ));
        }
        HydratedThing::new(
            ConceptId::new(concept),
            declared,
            concrete,
            ThingKind::Entity,
            attributes,
            vec![],
        )
    }

    #[test]
    fn subtype_hydration_uses_concrete_fields_and_attribute_distinctness() {
        let registry = registry();
        let declared = registry.descriptor_id("person").unwrap();
        let validated = exact_one_request(
            &registry,
            vec![binding(
                &registry,
                0,
                "person",
                ThingKind::Entity,
                MatchMode::Subtypes,
            )],
            Some(MatchExpr::FieldValue {
                field: BoundFieldId::new(
                    BindingId::new(0),
                    registry.field_id(&declared, "name").unwrap(),
                ),
                operator: ComparisonOp::Equal,
                value: AttributeValue::String("Alice".into()),
            }),
            FetchShape::Positional {
                slots: vec![FetchSlot::One {
                    binding: BindingId::new(0),
                }],
            },
            BTreeSet::new(),
        );

        let multiplicity = rows_evidence(
            &validated,
            vec![solution(
                vec![(
                    0,
                    subtype_employee(
                        &registry,
                        "employee-1",
                        "employee",
                        vec!["same", "same"],
                        vec![],
                    ),
                )],
                vec![],
            )],
        );
        validate_provider_result(&registry, &validated, multiplicity).unwrap();

        let declared_owner = rows_evidence(
            &validated,
            vec![solution(
                vec![(
                    0,
                    subtype_employee(&registry, "employee-1", "person", vec![], vec![]),
                )],
                vec![],
            )],
        );
        assert_eq!(
            error_code(
                &validate_provider_result(&registry, &validated, declared_owner).unwrap_err()
            ),
            "attribute_owner_mismatch"
        );

        let duplicate_distinct = rows_evidence(
            &validated,
            vec![solution(
                vec![(
                    0,
                    subtype_employee(
                        &registry,
                        "employee-1",
                        "employee",
                        vec![],
                        vec!["same", "same"],
                    ),
                )],
                vec![],
            )],
        );
        assert_eq!(
            error_code(
                &validate_provider_result(&registry, &validated, duplicate_distinct).unwrap_err()
            ),
            "duplicate_distinct_attribute_value"
        );
    }

    #[test]
    fn role_multiplicity_is_preserved_unless_role_is_distinct() {
        let registry = registry();
        let validated = exact_one_request(
            &registry,
            vec![binding(
                &registry,
                0,
                "employment",
                ThingKind::Relation,
                MatchMode::Exact,
            )],
            None,
            FetchShape::Positional {
                slots: vec![FetchSlot::One {
                    binding: BindingId::new(0),
                }],
            },
            BTreeSet::new(),
        );
        let person = person(&registry, "person-1", "Alice");
        let duplicate_players = vec![player_from_thing(&person), player_from_thing(&person)];
        let relation = registry.descriptor_id("employment").unwrap();

        let preserved = rows_evidence(
            &validated,
            vec![solution(
                vec![(
                    0,
                    employment(
                        &registry,
                        "employment-1",
                        "code-1",
                        vec![HydratedRole::new(
                            RoleId::new(relation.clone(), "employee"),
                            duplicate_players.clone(),
                        )],
                    ),
                )],
                vec![],
            )],
        );
        validate_provider_result(&registry, &validated, preserved).unwrap();

        let rejected = rows_evidence(
            &validated,
            vec![solution(
                vec![(
                    0,
                    employment(
                        &registry,
                        "employment-1",
                        "code-1",
                        vec![HydratedRole::new(
                            RoleId::new(relation, "distinct_employee"),
                            duplicate_players,
                        )],
                    ),
                )],
                vec![],
            )],
        );
        assert_eq!(
            error_code(&validate_provider_result(&registry, &validated, rejected).unwrap_err()),
            "duplicate_hydrated_role_player"
        );
    }

    #[test]
    fn role_edge_and_hydration_evidence_must_satisfy_the_predicate() {
        let registry = registry();
        let role_owner = registry.descriptor_id("employment").unwrap();
        let predicate = MatchExpr::RoleEdge {
            id: RoleEdgeId::new(0),
            relation: BindingId::new(1),
            role: RoleId::new(role_owner.clone(), "employee"),
            player: BindingId::new(0),
        };
        let validated = exact_one_request(
            &registry,
            vec![
                binding(&registry, 0, "person", ThingKind::Entity, MatchMode::Exact),
                binding(
                    &registry,
                    1,
                    "employment",
                    ThingKind::Relation,
                    MatchMode::Exact,
                ),
            ],
            Some(predicate),
            FetchShape::Positional {
                slots: vec![
                    FetchSlot::One {
                        binding: BindingId::new(0),
                    },
                    FetchSlot::One {
                        binding: BindingId::new(1),
                    },
                ],
            },
            BTreeSet::new(),
        );
        let person = person(&registry, "person-1", "Alice");
        let relation = employment(
            &registry,
            "employment-1",
            "code-1",
            vec![HydratedRole::new(
                RoleId::new(role_owner, "employee"),
                vec![player_from_thing(&person)],
            )],
        );
        let missing_edge = rows_evidence(
            &validated,
            vec![solution(vec![(0, person), (1, relation)], vec![])],
        );
        assert_eq!(
            error_code(&validate_provider_result(&registry, &validated, missing_edge).unwrap_err()),
            "predicate_evidence_mismatch"
        );
    }

    #[test]
    fn zero_hop_reachability_claim_checks_exact_endpoint_identity() {
        let registry = registry();
        let relation = registry.descriptor_id("employment").unwrap();
        let role = RoleId::new(relation.clone(), "employee");
        let validated = exact_one_request(
            &registry,
            vec![
                binding(&registry, 0, "person", ThingKind::Entity, MatchMode::Exact),
                binding(&registry, 1, "person", ThingKind::Entity, MatchMode::Exact),
            ],
            Some(MatchExpr::Reachable {
                relation,
                role_from: role.clone(),
                role_to: role,
                source: BindingId::new(0),
                target: BindingId::new(1),
                min_depth: 0,
                max_depth: 0,
            }),
            FetchShape::Positional {
                slots: vec![
                    FetchSlot::One {
                        binding: BindingId::new(0),
                    },
                    FetchSlot::One {
                        binding: BindingId::new(1),
                    },
                ],
            },
            BTreeSet::new(),
        );

        let forged = rows_evidence(
            &validated,
            vec![solution(
                vec![
                    (0, person(&registry, "person-1", "Alice")),
                    (1, person(&registry, "person-2", "Bob")),
                ],
                vec![],
            )],
        );
        assert_eq!(
            error_code(&validate_provider_result(&registry, &validated, forged).unwrap_err()),
            "predicate_evidence_mismatch"
        );

        let valid = rows_evidence(
            &validated,
            vec![solution(
                vec![
                    (0, person(&registry, "person-1", "Alice")),
                    (1, person(&registry, "person-1", "Alice")),
                ],
                vec![],
            )],
        );
        validate_provider_result(&registry, &validated, valid)
            .expect("zero-hop evidence with one endpoint identity");
    }

    #[test]
    fn positive_reachability_proof_is_a_trusted_provider_projection_boundary() {
        let registry = registry();
        let relation = registry.descriptor_id("employment").unwrap();
        let role = RoleId::new(relation.clone(), "employee");
        let validated = exact_one_request(
            &registry,
            vec![
                binding(&registry, 0, "person", ThingKind::Entity, MatchMode::Exact),
                binding(&registry, 1, "person", ThingKind::Entity, MatchMode::Exact),
            ],
            Some(MatchExpr::Reachable {
                relation,
                role_from: role.clone(),
                role_to: role,
                source: BindingId::new(0),
                target: BindingId::new(1),
                min_depth: 1,
                max_depth: 1,
            }),
            FetchShape::Positional {
                slots: vec![
                    FetchSlot::One {
                        binding: BindingId::new(0),
                    },
                    FetchSlot::One {
                        binding: BindingId::new(1),
                    },
                ],
            },
            BTreeSet::new(),
        );

        // No positive-path proof survives projection into provider evidence.
        // This row is admissible only because this validator consumes evidence
        // produced inside the trusted provider execution boundary; endpoint
        // types and identities remain independently checked above.
        let projected = rows_evidence(
            &validated,
            vec![solution(
                vec![
                    (0, person(&registry, "person-1", "Alice")),
                    (1, person(&registry, "person-2", "Bob")),
                ],
                vec![],
            )],
        );
        validate_provider_result(&registry, &validated, projected)
            .expect("trusted provider owns the projected positive path proof");
    }

    #[test]
    fn role_edge_accepts_base_declared_view_of_exact_subtype_player() {
        let registry = registry();
        let person_descriptor = registry.descriptor_id("person").unwrap();
        let employee_descriptor = registry.descriptor_id("employee").unwrap();
        let role_owner = registry.descriptor_id("employment").unwrap();
        let predicate = MatchExpr::RoleEdge {
            id: RoleEdgeId::new(0),
            relation: BindingId::new(1),
            role: RoleId::new(role_owner.clone(), "employee"),
            player: BindingId::new(0),
        };
        let validated = exact_one_request(
            &registry,
            vec![
                binding(
                    &registry,
                    0,
                    "employee",
                    ThingKind::Entity,
                    MatchMode::Exact,
                ),
                binding(
                    &registry,
                    1,
                    "employment",
                    ThingKind::Relation,
                    MatchMode::Exact,
                ),
            ],
            Some(predicate),
            FetchShape::Positional {
                slots: vec![
                    FetchSlot::One {
                        binding: BindingId::new(0),
                    },
                    FetchSlot::One {
                        binding: BindingId::new(1),
                    },
                ],
            },
            BTreeSet::new(),
        );
        let attributes = vec![
            hydrated_attribute(
                &registry,
                "employee",
                "name",
                vec![AttributeValue::String("Alice".into())],
            ),
            hydrated_attribute(
                &registry,
                "employee",
                "badge",
                vec![AttributeValue::String("A-1".into())],
            ),
        ];
        let selected_employee = HydratedThing::new(
            ConceptId::new("employee-1"),
            employee_descriptor.clone(),
            employee_descriptor.clone(),
            ThingKind::Entity,
            attributes.clone(),
            vec![],
        );
        let nested_employee = HydratedRolePlayer::new(
            ConceptId::new("employee-1"),
            person_descriptor,
            employee_descriptor,
            ThingKind::Entity,
            attributes,
        );
        let relation = employment(
            &registry,
            "employment-1",
            "code-1",
            vec![HydratedRole::new(
                RoleId::new(role_owner, "employee"),
                vec![nested_employee],
            )],
        );
        let evidence = rows_evidence(
            &validated,
            vec![solution(
                vec![(0, selected_employee), (1, relation)],
                vec![0],
            )],
        );

        validate_provider_result(&registry, &validated, evidence).unwrap();
    }

    #[test]
    fn page_preserves_collection_multiplicity_and_dedupes_distinct_slots() {
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
                    window: Window {
                        offset: 0,
                        limit: 10,
                    },
                    include_total: true,
                },
            ),
        )
        .unwrap();
        let make_solution = || {
            solution(
                vec![
                    (0, person(&registry, "person-1", "Alice")),
                    (1, employment(&registry, "employment-1", "code-1", vec![])),
                    (2, company(&registry, "company-1", "Acme")),
                ],
                vec![],
            )
        };
        let evidence = ProviderResultEvidence::page(
            validated.request_token(),
            validated.shape_id().clone(),
            BindingId::new(0),
            vec![make_solution(), make_solution()],
            Window {
                offset: 0,
                limit: 10,
            },
            Some(1),
        );
        let result = validate_provider_result(&registry, &validated, evidence).unwrap();
        let MatchResult::Page { entries, .. } = result.result() else {
            panic!("expected page")
        };
        assert_eq!(entries.len(), 1);
        let SlotValue::Many(employments) = &entries[0].slots()[1] else {
            panic!("expected employment collection")
        };
        let SlotValue::Many(companies) = &entries[0].slots()[2] else {
            panic!("expected company collection")
        };
        assert_eq!(employments.len(), 2);
        assert_eq!(companies.len(), 1);

        let invalid_total = ProviderResultEvidence::page(
            validated.request_token(),
            validated.shape_id().clone(),
            BindingId::new(0),
            vec![make_solution()],
            Window {
                offset: 0,
                limit: 10,
            },
            Some(2),
        );
        assert_eq!(
            error_code(
                &validate_provider_result(&registry, &validated, invalid_total).unwrap_err()
            ),
            "page_total_length_mismatch"
        );
    }

    #[test]
    fn stable_order_and_resource_policy_fail_closed() {
        let registry = registry();
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
                MatchOperation::FetchRows {
                    output: FetchShape::Positional {
                        slots: vec![FetchSlot::One {
                            binding: BindingId::new(0),
                        }],
                    },
                    order: vec![],
                    window: Window {
                        offset: 0,
                        limit: 10,
                    },
                    cardinality: RowCardinality::BoundedMany,
                },
            ),
        )
        .unwrap();
        let descending = rows_evidence(
            &validated,
            vec![
                solution(vec![(0, person(&registry, "person-2", "Bob"))], vec![]),
                solution(vec![(0, person(&registry, "person-1", "Alice"))], vec![]),
            ],
        );
        assert_eq!(
            error_code(&validate_provider_result(&registry, &validated, descending).unwrap_err()),
            "unstable_provider_order"
        );

        let evidence = rows_evidence(
            &validated,
            vec![solution(
                vec![(0, person(&registry, "person-1", "Alice"))],
                vec![],
            )],
        );
        let limits = ResultValidationLimits::new(
            0,
            usize::MAX,
            usize::MAX,
            usize::MAX,
            usize::MAX,
            usize::MAX,
        );
        let error = validate_provider_result_with_limits(&registry, &validated, evidence, limits)
            .unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::ResourceLimit);
        assert_eq!(error_code(&error), "provider_solution_limit");

        let clamped = ResultValidationLimits::new(
            usize::MAX,
            usize::MAX,
            usize::MAX,
            usize::MAX,
            usize::MAX,
            usize::MAX,
        );
        assert_eq!(clamped, ResultValidationLimits::DEFAULT);
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
        let evidence = ProviderResultEvidence::count(
            count.request_token(),
            count.shape_id().clone(),
            BindingId::new(0),
            u64::MAX,
        );
        let result = validate_provider_result(&registry, &count, evidence).unwrap();
        assert!(matches!(
            result.result(),
            MatchResult::Count {
                root,
                value: u64::MAX
            } if *root == BindingId::new(0)
        ));

        let wrong_root = ProviderResultEvidence::count(
            count.request_token(),
            count.shape_id().clone(),
            BindingId::new(1),
            0,
        );
        assert_eq!(
            error_code(&validate_provider_result(&registry, &count, wrong_root).unwrap_err()),
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
        let evidence = ProviderResultEvidence::exists(
            exists.request_token(),
            exists.shape_id().clone(),
            BindingId::new(0),
            true,
        );
        let result = validate_provider_result(&registry, &exists, evidence).unwrap();
        assert!(matches!(
            result.result(),
            MatchResult::Exists {
                root,
                value: true
            } if *root == BindingId::new(0)
        ));
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
                            super::super::model::ReduceTerm {
                                reduction: Reduction::Count,
                                input: None,
                            },
                            super::super::model::ReduceTerm {
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
        let evidence = |group: BoundFieldId, rows| {
            ProviderResultEvidence::field_reduction(
                validated.request_token(),
                validated.shape_id().clone(),
                root,
                group,
                rows,
            )
        };

        let valid = evidence(
            age.clone(),
            vec![super::super::result::ReductionRow::new_field(
                AttributeValue::Long(20),
                vec![ReducedValue::Count(2), ReducedValue::Long(Some(40))],
            )],
        );
        let result = validate_provider_result(&registry, &validated, valid).unwrap();
        let MatchResult::FieldReduction { group, rows, .. } = result.result() else {
            panic!("expected field-grouped reduction")
        };
        assert_eq!(group, &age);
        assert_eq!(rows[0].field_group(), Some(&AttributeValue::Long(20)));

        let wrong_kind = evidence(
            age.clone(),
            vec![super::super::result::ReductionRow::new(
                Some(person(&registry, "person-1", "Alice")),
                vec![ReducedValue::Count(1), ReducedValue::Long(Some(20))],
            )],
        );
        assert_eq!(
            error_code(&validate_provider_result(&registry, &validated, wrong_kind).unwrap_err()),
            "field_reduction_group_kind"
        );

        let wrong_domain = evidence(
            age.clone(),
            vec![super::super::result::ReductionRow::new_field(
                AttributeValue::String("20".into()),
                vec![ReducedValue::Count(1), ReducedValue::Long(Some(20))],
            )],
        );
        assert_eq!(
            error_code(&validate_provider_result(&registry, &validated, wrong_domain).unwrap_err()),
            "field_reduction_group_value_type"
        );

        let duplicate = evidence(
            age.clone(),
            vec![
                super::super::result::ReductionRow::new_field(
                    AttributeValue::Long(20),
                    vec![ReducedValue::Count(1), ReducedValue::Long(Some(20))],
                ),
                super::super::result::ReductionRow::new_field(
                    AttributeValue::Long(20),
                    vec![ReducedValue::Count(1), ReducedValue::Long(Some(20))],
                ),
            ],
        );
        assert_eq!(
            error_code(&validate_provider_result(&registry, &validated, duplicate).unwrap_err()),
            "field_reduction_group_duplicate"
        );

        let seen_at = BoundFieldId::new(root, field(&registry, "person", "seen_at"));
        let datetime_validated = request_for(seen_at.clone());
        let noncanonical = ProviderResultEvidence::field_reduction(
            datetime_validated.request_token(),
            datetime_validated.shape_id().clone(),
            root,
            seen_at,
            vec![super::super::result::ReductionRow::new_field(
                AttributeValue::DateTime("2026-07-28T03:55:00.000000000".into()),
                vec![ReducedValue::Count(1), ReducedValue::Long(Some(20))],
            )],
        );
        assert_eq!(
            error_code(
                &validate_provider_result(&registry, &datetime_validated, noncanonical)
                    .unwrap_err()
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
        let groups = vec![age.clone(), name.clone()];
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
                        super::super::model::ReduceTerm {
                            reduction: Reduction::Count,
                            input: None,
                        },
                        super::super::model::ReduceTerm {
                            reduction: Reduction::Sum,
                            input: Some(age.clone()),
                        },
                    ],
                },
            ),
        )
        .unwrap();
        let evidence = |echoed_groups: Vec<BoundFieldId>, rows| {
            ProviderResultEvidence::field_tuple_reduction(
                validated.request_token(),
                validated.shape_id().clone(),
                root,
                echoed_groups,
                rows,
            )
        };
        let reduced = vec![ReducedValue::Count(1), ReducedValue::Long(Some(20))];

        let valid = evidence(
            groups.clone(),
            vec![super::super::result::ReductionRow::new_fields(
                vec![
                    AttributeValue::Long(20),
                    AttributeValue::String("Alice".into()),
                ],
                reduced.clone(),
            )],
        );
        let result = validate_provider_result(&registry, &validated, valid).unwrap();
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

        let wrong_order = evidence(
            vec![name.clone(), age.clone()],
            vec![super::super::result::ReductionRow::new_fields(
                vec![
                    AttributeValue::String("Alice".into()),
                    AttributeValue::Long(20),
                ],
                reduced.clone(),
            )],
        );
        assert_eq!(
            error_code(&validate_provider_result(&registry, &validated, wrong_order).unwrap_err()),
            "field_tuple_reduction_groups_mismatch"
        );

        let wrong_arity = evidence(
            groups.clone(),
            vec![super::super::result::ReductionRow::new_fields(
                vec![AttributeValue::Long(20)],
                reduced.clone(),
            )],
        );
        assert_eq!(
            error_code(&validate_provider_result(&registry, &validated, wrong_arity).unwrap_err()),
            "field_tuple_reduction_group_arity"
        );

        let wrong_domain = evidence(
            groups.clone(),
            vec![super::super::result::ReductionRow::new_fields(
                vec![
                    AttributeValue::String("20".into()),
                    AttributeValue::String("Alice".into()),
                ],
                reduced.clone(),
            )],
        );
        assert_eq!(
            error_code(&validate_provider_result(&registry, &validated, wrong_domain).unwrap_err()),
            "field_tuple_reduction_group_value_type"
        );

        let duplicate = evidence(
            groups,
            vec![
                super::super::result::ReductionRow::new_fields(
                    vec![
                        AttributeValue::Long(20),
                        AttributeValue::String("Alice".into()),
                    ],
                    reduced.clone(),
                ),
                super::super::result::ReductionRow::new_fields(
                    vec![
                        AttributeValue::Long(20),
                        AttributeValue::String("Alice".into()),
                    ],
                    reduced,
                ),
            ],
        );
        assert_eq!(
            error_code(&validate_provider_result(&registry, &validated, duplicate).unwrap_err()),
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
