//! One-way adaptation of validated V1 match requests onto V2 query plans.
//!
//! **Status: crate-test-only internal experiment.** The module is absent from
//! production builds, is not wired into any execution path, and does not
//! justify any V1 removal; see `docs/guide/v2-deprecations.md`. It maps the
//! V1 vocabulary that the first public V2 vocabulary can express exactly,
//! and rejects everything else by name — V2 features never degrade silently
//! to V1 and a V1 shape without a V2 equivalent is refused, never
//! approximated. There is no reverse adapter, and the V1 executor remains
//! the public default.
//!
//! Adaptation starts from a [`ValidatedMatchRequest`] plus the registry
//! that validated it: attribute identities are the registry's canonical
//! provider labels (renamed members translate correctly), and ordering
//! consumes the validator-proven stable order — including its synthesized
//! unique tie breaker — so windowed pages keep V1 membership. Shapes whose
//! V1 semantics the V2 vocabulary cannot reproduce (subtype-inclusive role
//! edges, missing-value order placement V1 itself rejects) fail with typed
//! diagnostics.
//!
//! Known disclosed divergence: for equal sort keys *without* a window, row
//! order among ties follows the provider, not V1's stable tie order.

use std::collections::BTreeSet;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::limits::StructuralLimits;
use type_bridge_contract::migration_assertion::{
    AssertionBinding, AssertionRolePlayer, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::{
    OrderDirection, OrderTerm, QueryOperand, QueryOperation, QueryOutput, QueryPattern, QueryPlan,
    ReadStage,
};
use type_bridge_contract::temporal::{
    CanonicalDate, CanonicalDateTime, CanonicalDateTimeTz, CanonicalDuration,
};
use type_bridge_contract::value::{CanonicalDouble, CanonicalString, CanonicalValue, DecimalValue};
use type_bridge_query::{MigrationAssertionValidationContext, ValidatedQuery, validate_query_plan};

use crate::AttributeValue;
use crate::match_request::{
    BoundFieldId, ComparisonOp, MatchExpr, MatchMode, MatchOperation, MatchRequest,
    MatchRequestVersion, MissingOrder, SortDirection, ThingKind, ValidatedMatchRequest,
};
use crate::registry::DescriptorRegistry;

/// The V2 program one V1 request adapts to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AdaptedMatchRequest {
    operation: QueryOperation,
    validated: ValidatedQuery,
}

impl AdaptedMatchRequest {
    /// Return the schema-validated adapted query.
    #[must_use]
    pub(crate) const fn validated(&self) -> &ValidatedQuery {
        &self.validated
    }

    /// Return the adapted closed operation.
    #[must_use]
    pub(crate) const fn operation(&self) -> QueryOperation {
        self.operation
    }
}

/// Adapt one validated V1 request onto the first public V2 vocabulary.
///
/// The registry is rechecked against the request's validation proof before
/// any translation. `context` supplies both the trusted managed identity and
/// resolved schema authority; adaptation returns only after the resulting V2
/// plan validates under `limits`.
pub(crate) fn adapt_match_request(
    validated: &ValidatedMatchRequest,
    registry: &DescriptorRegistry,
    context: &MigrationAssertionValidationContext<'_>,
    limits: StructuralLimits,
) -> Result<AdaptedMatchRequest, Diagnostic> {
    validated.recheck_schema(registry).map_err(|_| {
        reject(
            "query_v2_adapter_registry_mismatch",
            "validated V1 request does not belong to the supplied registry snapshot",
        )
    })?;
    let request = validated.request();
    if request.version != MatchRequestVersion::V1 {
        return Err(reject(
            "query_v2_adapter_version_unsupported",
            "only V1 match requests adapt onto the V2 vocabulary",
        ));
    }
    if !request.plan.allowed_cross_joins.is_empty() {
        return Err(reject(
            "query_v2_adapter_cross_join_unsupported",
            "explicit cross joins have no V2 equivalent under connected topology",
        ));
    }
    for (index, binding) in request.plan.bindings.iter().enumerate() {
        if usize::from(binding.id.get()) != index {
            return Err(reject(
                "query_v2_adapter_bindings_not_dense",
                "V1 bindings must be dense zero-based ordinals",
            ));
        }
    }

    let thing_count = request.plan.bindings.len();
    let mut fields: Vec<BoundFieldId> = Vec::new();
    if let Some(predicate) = &request.plan.predicate {
        collect_fields(predicate, &mut fields);
    }
    // Ordering consumes the validator-proven stable order, not the raw
    // public order: it carries the synthesized unique tie breaker that keeps
    // windowed page membership identical to V1 execution.
    let stable_terms: Vec<&crate::match_request::MatchOrder> =
        if matches!(request.operation, MatchOperation::FetchRows { .. }) {
            validated
                .stable_order()
                .terms()
                .iter()
                .map(|term| term.order())
                .collect()
        } else {
            Vec::new()
        };
    for order in &stable_terms {
        if order.missing != MissingOrder::Reject {
            return Err(reject(
                "query_v2_adapter_missing_order_unsupported",
                "V1 execution rejects missing-value order placement; the adapter preserves that rejection",
            ));
        }
        if !fields.contains(&order.field) {
            fields.push(order.field.clone());
        }
    }

    let mut bindings = Vec::with_capacity(thing_count + fields.len());
    for (index, _) in request.plan.bindings.iter().enumerate() {
        bindings.push(AssertionBinding::new(
            binding_ordinal(index)?,
            QueryVariable::new(format!("b{index}"))?,
        ));
    }
    for (index, _) in fields.iter().enumerate() {
        bindings.push(AssertionBinding::new(
            binding_ordinal(thing_count + index)?,
            QueryVariable::new(format!("f{index}"))?,
        ));
    }
    let field_binding = |field: &BoundFieldId| -> Result<BindingId, Diagnostic> {
        let index = fields
            .iter()
            .position(|candidate| candidate == field)
            .expect("collected field is registered");
        binding_ordinal(thing_count + index)
    };

    let mut patterns = Vec::new();
    for (index, binding) in request.plan.bindings.iter().enumerate() {
        patterns.push(QueryPattern::Isa {
            binding: binding_ordinal(index)?,
            include_subtypes: binding.match_mode == MatchMode::Subtypes,
            type_id: descriptor_type(binding.descriptor.as_str(), binding.thing_kind)?,
        });
    }
    for field in &fields {
        if usize::from(field.binding.get()) >= thing_count {
            return Err(reject(
                "query_v2_adapter_unknown_binding",
                "a bound field references an undeclared V1 binding",
            ));
        }
        // Emit the registry's canonical provider label, never the
        // binding-facing field name: renamed members must match the real
        // attribute type instead of a coincidental same-named one.
        let attribute_label = registry
            .provider_attribute_name(&field.field)
            .ok_or_else(|| {
                reject(
                    "query_v2_adapter_unknown_field",
                    "a bound field has no registered provider attribute",
                )
            })?;
        patterns.push(QueryPattern::Has {
            attribute: field_binding(field)?,
            attribute_id: AttributeId::new(attribute_label)?,
            owner: BindingId::new(field.binding.get())?,
        });
    }
    if let Some(predicate) = &request.plan.predicate {
        patterns.extend(adapt_expression(
            predicate,
            request,
            thing_count,
            &field_binding,
        )?);
    }

    let (operation, output_columns) = match &request.operation {
        MatchOperation::FetchRows {
            output,
            order: _,
            window: _,
            cardinality,
        } => {
            if *cardinality != crate::match_request::RowCardinality::BoundedMany {
                return Err(reject(
                    "query_v2_adapter_cardinality_unsupported",
                    "exactly-one row assertions have no V2 equivalent",
                ));
            }
            let slots = match output {
                crate::match_request::FetchShape::Positional { slots } => slots,
                crate::match_request::FetchShape::Named { .. } => {
                    return Err(reject(
                        "query_v2_adapter_named_shape_unsupported",
                        "named result shapes have no V2 row-projection equivalent",
                    ));
                }
            };
            let mut columns = Vec::with_capacity(slots.len());
            for slot in slots {
                let crate::match_request::FetchSlot::One { binding } = slot else {
                    return Err(reject(
                        "query_v2_adapter_collection_unsupported",
                        "collection slots have no V2 row-projection equivalent",
                    ));
                };
                columns.push(BindingId::new(binding.get())?);
            }
            (QueryOperation::Rows, columns)
        }
        MatchOperation::CountBy { root } => {
            (QueryOperation::Count, vec![BindingId::new(root.get())?])
        }
        MatchOperation::ExistsBy { root } => {
            (QueryOperation::Exists, vec![BindingId::new(root.get())?])
        }
        MatchOperation::PageBy { .. } => {
            return Err(reject(
                "query_v2_adapter_paging_unsupported",
                "root-identity paging has no V2 first-vocabulary equivalent",
            ));
        }
    };

    let mut select: BTreeSet<BindingId> = output_columns.iter().copied().collect();
    let mut sort_terms = Vec::with_capacity(stable_terms.len());
    for term in &stable_terms {
        let binding = field_binding(&term.field)?;
        select.insert(binding);
        sort_terms.push(OrderTerm::new(
            binding,
            match term.direction {
                SortDirection::Ascending => OrderDirection::Ascending,
                SortDirection::Descending => OrderDirection::Descending,
            },
        ));
    }
    let mut pipeline = vec![
        ReadStage::Match { patterns },
        ReadStage::Select {
            bindings: select.into_iter().collect(),
        },
        ReadStage::Distinct,
    ];
    if !sort_terms.is_empty() {
        pipeline.push(ReadStage::Sort { terms: sort_terms });
    }
    if let MatchOperation::FetchRows { window, .. } = &request.operation {
        if stable_terms.is_empty() {
            return Err(reject(
                "query_v2_adapter_unordered_window",
                "a V1 window without a proven stable order has no stable V2 truncation",
            ));
        }
        if window.offset > 0 {
            pipeline.push(ReadStage::Offset {
                rows: window.offset,
            });
        }
        pipeline.push(ReadStage::Limit { rows: window.limit });
    }

    let plan = QueryPlan::new(
        bindings,
        Vec::new(),
        pipeline,
        QueryOutput::Rows {
            columns: output_columns,
        },
        context.managed_state().managed_semantic_schema().clone(),
    )?;
    let validated = validate_query_plan(&plan, context, limits)?;
    Ok(AdaptedMatchRequest {
        operation,
        validated,
    })
}

fn collect_fields(expression: &MatchExpr, fields: &mut Vec<BoundFieldId>) {
    match expression {
        MatchExpr::FieldValue { field, .. } => {
            if !fields.contains(field) {
                fields.push(field.clone());
            }
        }
        MatchExpr::FieldComparison { left, right, .. } => {
            for field in [left, right] {
                if !fields.contains(field) {
                    fields.push(field.clone());
                }
            }
        }
        MatchExpr::RoleEdge { .. } => {}
        MatchExpr::And { expressions } | MatchExpr::Or { expressions } => {
            for child in expressions {
                collect_fields(child, fields);
            }
        }
        MatchExpr::Not { expression } => collect_fields(expression, fields),
    }
}

fn adapt_expression(
    expression: &MatchExpr,
    request: &MatchRequest,
    thing_count: usize,
    field_binding: &impl Fn(&BoundFieldId) -> Result<BindingId, Diagnostic>,
) -> Result<Vec<QueryPattern>, Diagnostic> {
    Ok(match expression {
        MatchExpr::FieldValue {
            field,
            operator,
            value,
        } => vec![QueryPattern::Value {
            comparator: adapt_comparator(*operator)?,
            left: QueryOperand::Binding {
                binding: field_binding(field)?,
            },
            right: QueryOperand::Literal {
                value: adapt_value(value)?,
            },
        }],
        MatchExpr::FieldComparison {
            left,
            operator,
            right,
        } => vec![QueryPattern::Value {
            comparator: adapt_comparator(*operator)?,
            left: QueryOperand::Binding {
                binding: field_binding(left)?,
            },
            right: QueryOperand::Binding {
                binding: field_binding(right)?,
            },
        }],
        MatchExpr::RoleEdge {
            id: _,
            relation,
            role,
            player,
        } => {
            let relation_binding = request
                .plan
                .bindings
                .get(usize::from(relation.get()))
                .filter(|binding| binding.thing_kind == ThingKind::Relation)
                .ok_or_else(|| {
                    reject(
                        "query_v2_adapter_unknown_binding",
                        "a role edge references an undeclared relation binding",
                    )
                })?;
            if relation_binding.match_mode == MatchMode::Subtypes {
                return Err(reject(
                    "query_v2_adapter_subtype_links_unsupported",
                    "V2 links pin the exact relation type; a subtype-inclusive V1 role edge has no faithful V2 equivalent",
                ));
            }
            let relation_id =
                descriptor_type(relation_binding.descriptor.as_str(), ThingKind::Relation)?;
            if usize::from(player.get()) >= thing_count {
                return Err(reject(
                    "query_v2_adapter_unknown_binding",
                    "a role edge references an undeclared player binding",
                ));
            }
            vec![QueryPattern::Links {
                players: vec![AssertionRolePlayer::new(
                    RoleId::new(
                        descriptor_type(role.owner.as_str(), ThingKind::Relation)?
                            .label()
                            .as_str()
                            .to_owned(),
                        role.name.clone(),
                    )?,
                    BindingId::new(player.get())?,
                )],
                relation: BindingId::new(relation.get())?,
                relation_id,
            }]
        }
        MatchExpr::And { expressions } => {
            let mut patterns = Vec::new();
            for child in expressions {
                patterns.extend(adapt_expression(
                    child,
                    request,
                    thing_count,
                    field_binding,
                )?);
            }
            patterns
        }
        MatchExpr::Or { .. } => {
            return Err(reject(
                "query_v2_adapter_disjunction_unsupported",
                "disjunction has no V2 first-vocabulary equivalent",
            ));
        }
        MatchExpr::Not { expression } => vec![QueryPattern::Not {
            patterns: adapt_expression(expression, request, thing_count, field_binding)?,
        }],
    })
}

fn adapt_comparator(operator: ComparisonOp) -> Result<ValueComparator, Diagnostic> {
    Ok(match operator {
        ComparisonOp::Equal => ValueComparator::Equal,
        ComparisonOp::NotEqual => ValueComparator::NotEqual,
        ComparisonOp::LessThan => ValueComparator::Less,
        ComparisonOp::LessThanOrEqual => ValueComparator::LessOrEqual,
        ComparisonOp::GreaterThan => ValueComparator::Greater,
        ComparisonOp::GreaterThanOrEqual => ValueComparator::GreaterOrEqual,
        ComparisonOp::Contains
        | ComparisonOp::StartsWith
        | ComparisonOp::EndsWith
        | ComparisonOp::Regex => {
            return Err(reject(
                "query_v2_adapter_string_operator_unsupported",
                "string pattern operators have no V2 first-vocabulary equivalent",
            ));
        }
    })
}

fn adapt_value(value: &AttributeValue) -> Result<CanonicalValue, Diagnostic> {
    let malformed = || {
        reject(
            "query_v2_adapter_value_not_canonical",
            "V1 literal does not parse as an exact canonical value",
        )
    };
    Ok(match value {
        AttributeValue::String(value) => {
            CanonicalValue::String(CanonicalString::new(value.as_str()).map_err(|_| malformed())?)
        }
        AttributeValue::Long(value) => CanonicalValue::Long(*value),
        AttributeValue::Double(value) => {
            CanonicalValue::Double(CanonicalDouble::new(*value).map_err(|_| malformed())?)
        }
        AttributeValue::Boolean(value) => CanonicalValue::Boolean(*value),
        AttributeValue::Date(value) => {
            CanonicalValue::Date(value.parse::<CanonicalDate>().map_err(|_| malformed())?)
        }
        AttributeValue::DateTime(value) => CanonicalValue::DateTime(
            value
                .parse::<CanonicalDateTime>()
                .map_err(|_| malformed())?,
        ),
        AttributeValue::DateTimeTZ(value) => CanonicalValue::DateTimeTz(
            value
                .parse::<CanonicalDateTimeTz>()
                .map_err(|_| malformed())?,
        ),
        AttributeValue::Decimal(value) => {
            CanonicalValue::Decimal(DecimalValue::new(value.as_str()).map_err(|_| malformed())?)
        }
        AttributeValue::Duration(value) => CanonicalValue::Duration(
            value
                .parse::<CanonicalDuration>()
                .map_err(|_| malformed())?,
        ),
    })
}

fn descriptor_type(descriptor: &str, kind: ThingKind) -> Result<TypeId, Diagnostic> {
    let (prefix, label) = descriptor.split_once(':').ok_or_else(|| {
        reject(
            "query_v2_adapter_descriptor_malformed",
            "descriptor identities carry a kind prefix and a type label",
        )
    })?;
    let expected = match kind {
        ThingKind::Entity => ("entity", TypeKind::Entity),
        ThingKind::Relation => ("relation", TypeKind::Relation),
    };
    if prefix != expected.0 {
        return Err(reject(
            "query_v2_adapter_descriptor_malformed",
            "descriptor kind prefix disagrees with the binding thing kind",
        ));
    }
    TypeId::new(expected.1, label.to_owned())
}

fn binding_ordinal(index: usize) -> Result<BindingId, Diagnostic> {
    u16::try_from(index)
        .map_err(|_| {
            reject(
                "query_v2_adapter_binding_limit",
                "adapted binding count exceeds the dense ordinal range",
            )
        })
        .and_then(BindingId::new)
}

fn reject(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::InvalidContract,
        DiagnosticCode::new(code).expect("static adapter diagnostic code"),
        message,
    )
}

#[cfg(test)]
mod literal_tests {
    use super::*;

    #[test]
    fn released_only_literal_spellings_reject_with_one_stable_diagnostic() {
        for value in [
            AttributeValue::DateTime("2024-01-01T12:30".to_owned()),
            AttributeValue::Duration("P1Y".to_owned()),
            AttributeValue::String("x".repeat(1024 * 1024 + 1)),
        ] {
            let error = adapt_value(&value).expect_err("noncanonical V2 literal must fail closed");
            assert_eq!(
                error.code().as_str(),
                "query_v2_adapter_value_not_canonical"
            );
        }
    }
}
