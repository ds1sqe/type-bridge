//! Lowering for schema-validated model compatibility plans.
//!
//! The compatibility tree is a closed contract algebra. This module converts
//! it into the existing typed core AST and delegates final TypeQL rendering to
//! the injection-safe compiler. It never accepts raw labels, variables, or
//! TypeQL fragments from a host language.

use std::collections::BTreeMap;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::migration_assertion::{BindingId, ValueComparator};
use type_bridge_contract::query_plan::{
    CompatibilityValueV2, ModelQueryV2, QueryComparatorV2, QueryFieldV2, QueryInvocation,
    QueryMissingOrderV2, QueryModelOutputSlotV2, QueryOperand, QueryOperation,
    QueryOrderDirectionV2, QueryPattern, QueryPatternV2, QueryPlan, QueryStableOrderV2, ReadStage,
};
use type_bridge_contract::value::{CanonicalValue, ValueTypeTag};
use type_bridge_core_lib::ast::{
    TypedCollectionOrder, TypedComparisonOperator, TypedFetchRows, TypedFieldBinding,
    TypedFunctionArgument, TypedFunctionCall, TypedLiteral, TypedMatchOrder, TypedMatchPredicate,
    TypedMatchTarget, TypedMissingOrder, TypedPageRematch, TypedRootScan, TypedScalarOperand,
    TypedSortDirection, TypedThingKind,
};
use type_bridge_core_lib::compiler::QueryCompiler;
use type_bridge_query::ValidatedQuery;

use crate::query_v2::failure;

/// Closed operation-specific provider plan for one compatibility model.
// These short-lived plans retain their typed ASTs by value so lowering and
// execution share one closed ownership boundary without per-stage boxing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CompatibilityProviderPlan {
    /// Selected solution scan, with an optional distinct-public-tuple proof.
    Rows {
        statement: TypedFetchRows,
        tuple_proof: Option<TypedFetchRows>,
    },
    /// Distinct-root selection, optional total, and exact root-batch re-match.
    Page {
        selection: TypedRootScan,
        total: Option<TypedRootScan>,
        rematch: TypedPageRematch,
    },
    /// Complete distinct-root count scan.
    DistinctCount { scan: TypedRootScan },
    /// One-row distinct-root existence probe.
    DistinctExists { scan: TypedRootScan },
    /// Complete distinct-root scan and optional full solution re-match for reduction.
    Reduction {
        scan: TypedRootScan,
        rematch: Option<TypedPageRematch>,
    },
}

/// Closed operation-specific provider statement for one compatibility plan.
pub(crate) struct LoweredCompatibilityQuery {
    provider_plan: CompatibilityProviderPlan,
    operation: QueryOperation,
    typeql: String,
}

impl LoweredCompatibilityQuery {
    pub(crate) const fn operation(&self) -> QueryOperation {
        self.operation
    }

    pub(crate) fn typeql(&self) -> &str {
        &self.typeql
    }

    pub(crate) const fn provider_plan(&self) -> &CompatibilityProviderPlan {
        &self.provider_plan
    }
}

/// Lower a validated adapter plan, returning `None` for native V2 plans.
pub(crate) fn lower_validated_compatibility_query(
    validated: &ValidatedQuery,
    invocation: &QueryInvocation,
    operation: QueryOperation,
) -> Result<Option<LoweredCompatibilityQuery>, Diagnostic> {
    let plan = validated.plan();
    let Some(compatibility) = plan.v2_compatibility() else {
        return Ok(None);
    };
    let Some(model) = compatibility.model_query() else {
        return Ok(None);
    };
    validate_operation(model, operation)?;

    let targets = compatibility_targets(plan)?;
    let mut fields = Vec::new();
    if let Some(predicate) = compatibility.predicate() {
        collect_pattern_fields(predicate, &mut fields);
    }
    collect_model_order_fields(model, &mut fields);
    let mut typed_fields = fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            Ok(TypedFieldBinding {
                id: u16::try_from(index).map_err(|_| {
                    integrity(
                        "query_v2_compatibility_field_limit",
                        "compatibility field ordinal exceeds the typed compiler range",
                    )
                })?,
                owner: field.binding().get(),
                field_name: field.attribute().label().as_str().to_owned(),
            })
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let mut next_edge = 0_u16;
    let compatibility_predicate = compatibility
        .predicate()
        .map(|predicate| lower_pattern(predicate, &fields, &mut next_edge))
        .transpose()?;
    let native_predicates = lower_native_function_predicates(plan, invocation, &mut typed_fields)?;
    let predicate = combine_predicates(compatibility_predicate, native_predicates);
    let provider_plan = match model {
        ModelQueryV2::Rows {
            cardinality,
            order,
            output,
            window,
            ..
        } => {
            let projection = output
                .slots()
                .into_iter()
                .filter_map(|slot| match slot {
                    QueryModelOutputSlotV2::One { binding, .. } => Some(binding.get()),
                    QueryModelOutputSlotV2::Collect { .. } => None,
                })
                .collect();
            let (order, offset, limit) = if cardinality.is_exactly_one() {
                (Vec::new(), 0, 2)
            } else {
                (
                    lower_order(
                        order.as_ref().ok_or_else(|| {
                            integrity(
                                "query_v2_compatibility_order",
                                "validated bounded rows lack their stable order",
                            )
                        })?,
                        &fields,
                    )?,
                    window.offset(),
                    window.limit(),
                )
            };
            let statement = TypedFetchRows {
                targets,
                fields: typed_fields,
                predicate,
                projection,
                distinct: true,
                order,
                offset,
                limit,
            };
            CompatibilityProviderPlan::Rows {
                tuple_proof: cardinality.is_exactly_one().then(|| statement.clone()),
                statement,
            }
        }
        ModelQueryV2::Page {
            order,
            root,
            output,
            window,
            include_total,
            ..
        } => {
            let graph = TypedRootScan {
                targets,
                fields: typed_fields,
                predicate,
                root: root.get(),
                order: lower_order(order, &fields)?,
                offset: Some(window.offset()),
                limit: Some(window.limit()),
            };
            let total = include_total.then(|| TypedRootScan {
                targets: graph.targets.clone(),
                fields: graph.fields.clone(),
                predicate: graph.predicate.clone(),
                root: graph.root,
                order: Vec::new(),
                offset: None,
                limit: None,
            });
            let collection_orders = output
                .slots()
                .into_iter()
                .filter_map(|slot| match slot {
                    QueryModelOutputSlotV2::One { .. } => None,
                    QueryModelOutputSlotV2::Collect { binding, order, .. } => {
                        Some((*binding, order))
                    }
                })
                .map(|(binding, order)| {
                    Ok(TypedCollectionOrder {
                        binding: binding.get(),
                        order: lower_order(order, &fields)?,
                    })
                })
                .collect::<Result<Vec<_>, Diagnostic>>()?;
            CompatibilityProviderPlan::Page {
                selection: graph.clone(),
                total,
                rematch: TypedPageRematch {
                    targets: graph.targets,
                    fields: graph.fields,
                    predicate: graph.predicate,
                    root: root.get(),
                    root_concept_ids: Vec::new(),
                    collection_orders,
                },
            }
        }
        ModelQueryV2::DistinctCount { root, .. } => CompatibilityProviderPlan::DistinctCount {
            scan: TypedRootScan {
                targets,
                fields: typed_fields,
                predicate,
                root: root.get(),
                order: Vec::new(),
                offset: None,
                limit: None,
            },
        },
        ModelQueryV2::DistinctExists { root, .. } => CompatibilityProviderPlan::DistinctExists {
            scan: TypedRootScan {
                targets,
                fields: typed_fields,
                predicate,
                root: root.get(),
                order: Vec::new(),
                offset: None,
                limit: Some(1),
            },
        },
        ModelQueryV2::Reduction {
            root,
            group,
            reducers,
            ..
        } => {
            let needs_rematch =
                group.is_some() || reducers.iter().any(|term| term.input().is_some());
            let scan = TypedRootScan {
                targets: targets.clone(),
                fields: typed_fields.clone(),
                predicate: predicate.clone(),
                root: root.get(),
                order: Vec::new(),
                offset: None,
                limit: None,
            };
            CompatibilityProviderPlan::Reduction {
                scan,
                rematch: needs_rematch.then(|| TypedPageRematch {
                    targets,
                    fields: typed_fields,
                    predicate,
                    root: root.get(),
                    root_concept_ids: Vec::new(),
                    collection_orders: Vec::new(),
                }),
            }
        }
    };
    let compiler = QueryCompiler::new();
    let uses_functions = provider_plan_uses_functions(&provider_plan);
    let typeql = match &provider_plan {
        CompatibilityProviderPlan::Rows { statement, .. } if uses_functions => {
            compiler
                .prepare_typed_fetch_rows(statement)
                .map_err(compiler_error)?
                .typeql
        }
        CompatibilityProviderPlan::Rows { statement, .. } => compiler
            .compile_typed_fetch_rows(statement)
            .map_err(compiler_error)?,
        CompatibilityProviderPlan::Page { selection, .. } if uses_functions => {
            compiler
                .prepare_typed_root_scan(selection)
                .map_err(compiler_error)?
                .typeql
        }
        CompatibilityProviderPlan::Page { selection, .. } => compiler
            .compile_typed_root_scan(selection)
            .map_err(compiler_error)?,
        CompatibilityProviderPlan::DistinctCount { scan }
        | CompatibilityProviderPlan::DistinctExists { scan }
        | CompatibilityProviderPlan::Reduction { scan, .. }
            if uses_functions =>
        {
            compiler
                .prepare_typed_root_scan(scan)
                .map_err(compiler_error)?
                .typeql
        }
        CompatibilityProviderPlan::DistinctCount { scan }
        | CompatibilityProviderPlan::DistinctExists { scan }
        | CompatibilityProviderPlan::Reduction { scan, .. } => compiler
            .compile_typed_root_scan(scan)
            .map_err(compiler_error)?,
    };
    Ok(Some(LoweredCompatibilityQuery {
        provider_plan,
        operation,
        typeql,
    }))
}

fn provider_plan_uses_functions(plan: &CompatibilityProviderPlan) -> bool {
    let predicate = match plan {
        CompatibilityProviderPlan::Rows { statement, .. } => statement.predicate.as_ref(),
        CompatibilityProviderPlan::Page { selection, .. }
        | CompatibilityProviderPlan::DistinctCount { scan: selection }
        | CompatibilityProviderPlan::DistinctExists { scan: selection }
        | CompatibilityProviderPlan::Reduction {
            scan: selection, ..
        } => selection.predicate.as_ref(),
    };
    predicate.is_some_and(typed_predicate_uses_functions)
}

fn typed_predicate_uses_functions(predicate: &TypedMatchPredicate) -> bool {
    match predicate {
        TypedMatchPredicate::FunctionComparison { .. } => true,
        TypedMatchPredicate::And { expressions } | TypedMatchPredicate::Or { expressions } => {
            expressions.iter().any(typed_predicate_uses_functions)
        }
        TypedMatchPredicate::Not { expression } => typed_predicate_uses_functions(expression),
        TypedMatchPredicate::FieldValue { .. }
        | TypedMatchPredicate::FieldComparison { .. }
        | TypedMatchPredicate::FieldPresence { .. }
        | TypedMatchPredicate::BindingIid { .. }
        | TypedMatchPredicate::RoleEdge { .. }
        | TypedMatchPredicate::Reachable { .. } => false,
    }
}

fn validate_operation(model: &ModelQueryV2, operation: QueryOperation) -> Result<(), Diagnostic> {
    let expected = match model {
        ModelQueryV2::Rows { .. } | ModelQueryV2::Page { .. } | ModelQueryV2::Reduction { .. } => {
            QueryOperation::Rows
        }
        ModelQueryV2::DistinctCount { .. } => QueryOperation::Count,
        ModelQueryV2::DistinctExists { .. } => QueryOperation::Exists,
    };
    if expected == operation {
        Ok(())
    } else {
        Err(failure(
            DiagnosticCategory::InvalidContract,
            "query_v2_compatibility_operation_mismatch",
            "invocation operation contradicts the validated model terminal",
        ))
    }
}

fn compatibility_targets(plan: &QueryPlan) -> Result<Vec<TypedMatchTarget>, Diagnostic> {
    let Some(ReadStage::Match { patterns }) = plan.pipeline().first() else {
        return Err(integrity(
            "query_v2_compatibility_target_skeleton",
            "validated compatibility plan lacks its root target skeleton",
        ));
    };
    patterns
        .iter()
        .filter_map(|pattern| {
            let QueryPattern::Isa {
                binding,
                include_subtypes,
                type_id,
            } = pattern
            else {
                return None;
            };
            Some(Ok(TypedMatchTarget {
                binding: binding.get(),
                kind: match type_id.kind() {
                    type_bridge_contract::id::TypeKind::Entity => TypedThingKind::Entity,
                    type_bridge_contract::id::TypeKind::Relation => TypedThingKind::Relation,
                    type_bridge_contract::id::TypeKind::Attribute
                    | type_bridge_contract::id::TypeKind::Struct => {
                        return Some(Err(integrity(
                            "query_v2_compatibility_target_kind",
                            "validated compatibility target is not a thing type",
                        )));
                    }
                },
                type_name: type_id.label().as_str().to_owned(),
                exact: !include_subtypes,
            }))
        })
        .collect()
}

fn lower_native_function_predicates(
    plan: &QueryPlan,
    invocation: &QueryInvocation,
    typed_fields: &mut Vec<TypedFieldBinding>,
) -> Result<Vec<TypedMatchPredicate>, Diagnostic> {
    let Some(ReadStage::Match { patterns }) = plan.pipeline().first() else {
        return Err(integrity(
            "query_v2_compatibility_function_skeleton",
            "validated compatibility plan lacks its native root conjunction",
        ));
    };
    if !patterns
        .iter()
        .any(|pattern| matches!(pattern, QueryPattern::FunctionCall { .. }))
    {
        return Ok(Vec::new());
    }
    let values = if plan.inputs().is_empty() {
        if !invocation.inputs().is_empty() {
            return Err(integrity(
                "query_v2_compatibility_function_inputs",
                "input-free schema functions require an empty invocation row set",
            ));
        }
        Vec::new()
    } else {
        let [row] = invocation.inputs() else {
            return Err(integrity(
                "query_v2_compatibility_function_inputs",
                "schema-function compatibility execution requires exactly one invocation row",
            ));
        };
        if row.values().len() != plan.inputs().len() {
            return Err(integrity(
                "query_v2_compatibility_function_inputs",
                "schema-function invocation row does not match its input declarations",
            ));
        }
        row.values()
            .iter()
            .enumerate()
            .map(|(index, value)| {
                value.clone().ok_or_else(|| {
                    integrity(
                        "query_v2_compatibility_function_inputs",
                        "schema-function projected inputs cannot be absent",
                    )
                    .with_detail("input", i64::try_from(index).unwrap_or(i64::MAX))
                })
            })
            .collect::<Result<Vec<_>, _>>()?
    };
    let mut attribute_fields = BTreeMap::<BindingId, u16>::new();
    let mut call_results = BTreeMap::<BindingId, u16>::new();
    let mut calls = Vec::<TypedFunctionCall>::new();
    let mut predicates = Vec::new();
    for pattern in patterns {
        match pattern {
            QueryPattern::Has {
                attribute,
                attribute_id,
                owner,
            } => {
                let field = u16::try_from(typed_fields.len()).map_err(|_| {
                    integrity(
                        "query_v2_compatibility_field_limit",
                        "native function field ordinal exceeds the typed compiler range",
                    )
                })?;
                typed_fields.push(TypedFieldBinding {
                    id: field,
                    owner: owner.get(),
                    field_name: attribute_id.label().as_str().to_owned(),
                });
                if attribute_fields.insert(*attribute, field).is_some() {
                    return Err(integrity(
                        "query_v2_compatibility_function_binding",
                        "native function field binding was assigned more than once",
                    ));
                }
            }
            QueryPattern::FunctionCall {
                arguments,
                assigned,
                function,
            } => {
                let result = u16::try_from(calls.len()).map_err(|_| {
                    integrity(
                        "query_v2_compatibility_function_limit",
                        "native schema-function call ordinal exceeds the typed compiler range",
                    )
                })?;
                let arguments = arguments
                    .iter()
                    .map(|argument| {
                        lower_native_function_argument(argument, &call_results, &values)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                calls.push(TypedFunctionCall {
                    result,
                    function: function.label().as_str().to_owned(),
                    arguments,
                });
                if call_results.insert(*assigned, result).is_some() {
                    return Err(integrity(
                        "query_v2_compatibility_function_binding",
                        "native schema-function result binding was assigned more than once",
                    ));
                }
            }
            QueryPattern::Value {
                comparator,
                left,
                right,
            } if !calls.is_empty() => {
                predicates.push(TypedMatchPredicate::FunctionComparison {
                    calls: std::mem::take(&mut calls),
                    left: lower_native_scalar_operand(
                        left,
                        &attribute_fields,
                        &call_results,
                        &values,
                    )?,
                    operator: lower_native_comparator(*comparator),
                    right: lower_native_scalar_operand(
                        right,
                        &attribute_fields,
                        &call_results,
                        &values,
                    )?,
                });
                call_results.clear();
            }
            QueryPattern::Isa { .. } => {}
            QueryPattern::Value { .. }
            | QueryPattern::Links { .. }
            | QueryPattern::Or { .. }
            | QueryPattern::Not { .. }
            | QueryPattern::Try { .. }
            | QueryPattern::Reachable { .. } => {
                return Err(integrity(
                    "query_v2_compatibility_function_skeleton",
                    "validated compatibility function suffix is not canonical",
                ));
            }
        }
    }
    if !calls.is_empty() {
        return Err(integrity(
            "query_v2_compatibility_function_skeleton",
            "native schema-function calls lack their scalar comparison",
        ));
    }
    Ok(predicates)
}

fn lower_native_function_argument(
    operand: &QueryOperand,
    call_results: &BTreeMap<BindingId, u16>,
    values: &[CanonicalValue],
) -> Result<TypedFunctionArgument, Diagnostic> {
    match operand {
        QueryOperand::Binding { binding } => Ok(call_results.get(binding).map_or(
            TypedFunctionArgument::Binding {
                binding: binding.get(),
            },
            |call| TypedFunctionArgument::CallResult { call: *call },
        )),
        QueryOperand::Input { column } => Ok(TypedFunctionArgument::Value {
            value: values
                .get(usize::from(column.get()))
                .cloned()
                .ok_or_else(|| {
                    integrity(
                        "query_v2_compatibility_function_inputs",
                        "native function argument references no invocation input",
                    )
                })?,
        }),
        QueryOperand::Literal { .. } => Err(integrity(
            "query_v2_compatibility_function_literal",
            "adapter-authored schema functions never contain inline literals",
        )),
    }
}

fn lower_native_scalar_operand(
    operand: &QueryOperand,
    attribute_fields: &BTreeMap<BindingId, u16>,
    call_results: &BTreeMap<BindingId, u16>,
    values: &[CanonicalValue],
) -> Result<TypedScalarOperand, Diagnostic> {
    match operand {
        QueryOperand::Binding { binding } => {
            if let Some(field) = attribute_fields.get(binding) {
                Ok(TypedScalarOperand::Field { field: *field })
            } else if let Some(call) = call_results.get(binding) {
                Ok(TypedScalarOperand::CallResult { call: *call })
            } else {
                Err(integrity(
                    "query_v2_compatibility_function_operand",
                    "native scalar comparison references neither a field nor a function result",
                ))
            }
        }
        QueryOperand::Input { column } => Ok(TypedScalarOperand::Value {
            value: values
                .get(usize::from(column.get()))
                .cloned()
                .ok_or_else(|| {
                    integrity(
                        "query_v2_compatibility_function_inputs",
                        "native scalar operand references no invocation input",
                    )
                })?,
        }),
        QueryOperand::Literal { .. } => Err(integrity(
            "query_v2_compatibility_function_literal",
            "adapter-authored schema functions never contain inline literals",
        )),
    }
}

fn combine_predicates(
    compatibility: Option<TypedMatchPredicate>,
    mut native: Vec<TypedMatchPredicate>,
) -> Option<TypedMatchPredicate> {
    if let Some(compatibility) = compatibility {
        native.insert(0, compatibility);
    }
    match native.len() {
        0 => None,
        1 => native.pop(),
        _ => Some(TypedMatchPredicate::And {
            expressions: native,
        }),
    }
}

const fn lower_native_comparator(comparator: ValueComparator) -> TypedComparisonOperator {
    match comparator {
        ValueComparator::Equal => TypedComparisonOperator::Equal,
        ValueComparator::NotEqual => TypedComparisonOperator::NotEqual,
        ValueComparator::Less => TypedComparisonOperator::LessThan,
        ValueComparator::LessOrEqual => TypedComparisonOperator::LessThanOrEqual,
        ValueComparator::Greater => TypedComparisonOperator::GreaterThan,
        ValueComparator::GreaterOrEqual => TypedComparisonOperator::GreaterThanOrEqual,
    }
}

fn collect_pattern_fields<'plan>(
    pattern: &'plan QueryPatternV2,
    fields: &mut Vec<&'plan QueryFieldV2>,
) {
    match pattern {
        QueryPatternV2::FieldValue { field, .. } => insert_field(fields, field),
        QueryPatternV2::FieldComparison { left, right, .. } => {
            insert_field(fields, left);
            insert_field(fields, right);
        }
        QueryPatternV2::FieldPresence { field, .. } => insert_field(fields, field),
        QueryPatternV2::And { patterns } | QueryPatternV2::Or { patterns } => {
            for child in patterns {
                collect_pattern_fields(child, fields);
            }
        }
        QueryPatternV2::Not { pattern } => collect_pattern_fields(pattern, fields),
        QueryPatternV2::BindingIid { .. }
        | QueryPatternV2::RoleEdge { .. }
        | QueryPatternV2::Reachable { .. } => {}
    }
}

fn collect_model_order_fields<'plan>(
    model: &'plan ModelQueryV2,
    fields: &mut Vec<&'plan QueryFieldV2>,
) {
    let mut collect_order = |order: &'plan QueryStableOrderV2| {
        for term in order.terms() {
            insert_field(fields, term.field());
        }
    };
    match model {
        ModelQueryV2::Rows { order, output, .. } => {
            if let Some(order) = order {
                collect_order(order);
            }
            for slot in output.slots() {
                if let QueryModelOutputSlotV2::Collect { order, .. } = slot {
                    collect_order(order);
                }
            }
        }
        ModelQueryV2::Page { order, output, .. } => {
            collect_order(order);
            for slot in output.slots() {
                if let QueryModelOutputSlotV2::Collect { order, .. } = slot {
                    collect_order(order);
                }
            }
        }
        ModelQueryV2::DistinctCount { .. } | ModelQueryV2::DistinctExists { .. } => {}
        ModelQueryV2::Reduction {
            group, reducers, ..
        } => {
            match group {
                Some(type_bridge_contract::query_plan::QueryReductionGroupV2::Field { field }) => {
                    insert_field(fields, field);
                }
                Some(type_bridge_contract::query_plan::QueryReductionGroupV2::Fields {
                    fields: group_fields,
                }) => {
                    for field in group_fields {
                        insert_field(fields, field);
                    }
                }
                None
                | Some(type_bridge_contract::query_plan::QueryReductionGroupV2::Binding {
                    ..
                }) => {}
            }
            for term in reducers {
                if let Some(field) = term.input() {
                    insert_field(fields, field);
                }
            }
        }
    }
}

fn insert_field<'plan>(fields: &mut Vec<&'plan QueryFieldV2>, field: &'plan QueryFieldV2) {
    if !fields.contains(&field) {
        fields.push(field);
    }
}

fn field_id(fields: &[&QueryFieldV2], field: &QueryFieldV2) -> Result<u16, Diagnostic> {
    let index = fields
        .iter()
        .position(|candidate| *candidate == field)
        .ok_or_else(|| {
            integrity(
                "query_v2_compatibility_field_state",
                "validated compatibility field lacks a deterministic ordinal",
            )
        })?;
    u16::try_from(index).map_err(|_| {
        integrity(
            "query_v2_compatibility_field_limit",
            "compatibility field ordinal exceeds the typed compiler range",
        )
    })
}

fn lower_pattern(
    pattern: &QueryPatternV2,
    fields: &[&QueryFieldV2],
    next_edge: &mut u16,
) -> Result<TypedMatchPredicate, Diagnostic> {
    Ok(match pattern {
        QueryPatternV2::FieldValue {
            field,
            comparator,
            value,
        } => TypedMatchPredicate::FieldValue {
            field: field_id(fields, field)?,
            operator: lower_comparator(*comparator),
            value: lower_literal(value)?,
        },
        QueryPatternV2::FieldComparison {
            left,
            comparator,
            right,
        } => TypedMatchPredicate::FieldComparison {
            left: field_id(fields, left)?,
            operator: lower_comparator(*comparator),
            right: field_id(fields, right)?,
        },
        QueryPatternV2::FieldPresence { field, present } => TypedMatchPredicate::FieldPresence {
            field: field_id(fields, field)?,
            present: *present,
        },
        QueryPatternV2::BindingIid { binding, iid } => TypedMatchPredicate::BindingIid {
            binding: binding.get(),
            iid: iid.clone(),
        },
        QueryPatternV2::RoleEdge {
            relation,
            role,
            player,
            ..
        } => {
            let edge = *next_edge;
            *next_edge = next_edge.checked_add(1).ok_or_else(|| {
                integrity(
                    "query_v2_compatibility_role_edge_limit",
                    "compatibility role-edge ordinal overflowed",
                )
            })?;
            TypedMatchPredicate::RoleEdge {
                edge,
                relation: relation.get(),
                role_name: role.label().as_str().to_owned(),
                player: player.get(),
            }
        }
        QueryPatternV2::Reachable {
            min_depth,
            max_depth,
            relation,
            role_from,
            role_to,
            source,
            target,
        } => TypedMatchPredicate::Reachable {
            relation_type: relation.label().as_str().to_owned(),
            role_from: role_from.label().as_str().to_owned(),
            role_to: role_to.label().as_str().to_owned(),
            source: source.get(),
            target: target.get(),
            min_depth: *min_depth,
            max_depth: *max_depth,
        },
        QueryPatternV2::And { patterns } => TypedMatchPredicate::And {
            expressions: patterns
                .iter()
                .map(|child| lower_pattern(child, fields, next_edge))
                .collect::<Result<Vec<_>, _>>()?,
        },
        QueryPatternV2::Or { patterns } => TypedMatchPredicate::Or {
            expressions: patterns
                .iter()
                .map(|child| lower_pattern(child, fields, next_edge))
                .collect::<Result<Vec<_>, _>>()?,
        },
        QueryPatternV2::Not { pattern } => TypedMatchPredicate::Not {
            expression: Box::new(lower_pattern(pattern, fields, next_edge)?),
        },
    })
}

fn lower_order(
    order: &QueryStableOrderV2,
    fields: &[&QueryFieldV2],
) -> Result<Vec<TypedMatchOrder>, Diagnostic> {
    order
        .terms()
        .iter()
        .map(|term| {
            Ok(TypedMatchOrder {
                field: field_id(fields, term.field())?,
                direction: match term.direction() {
                    QueryOrderDirectionV2::Ascending => TypedSortDirection::Ascending,
                    QueryOrderDirectionV2::Descending => TypedSortDirection::Descending,
                },
                missing: match term.missing() {
                    QueryMissingOrderV2::Reject => TypedMissingOrder::Reject,
                    QueryMissingOrderV2::First => TypedMissingOrder::First,
                    QueryMissingOrderV2::Last => TypedMissingOrder::Last,
                },
            })
        })
        .collect()
}

const fn lower_comparator(comparator: QueryComparatorV2) -> TypedComparisonOperator {
    match comparator {
        QueryComparatorV2::Equal => TypedComparisonOperator::Equal,
        QueryComparatorV2::NotEqual => TypedComparisonOperator::NotEqual,
        QueryComparatorV2::Less => TypedComparisonOperator::LessThan,
        QueryComparatorV2::LessOrEqual => TypedComparisonOperator::LessThanOrEqual,
        QueryComparatorV2::Greater => TypedComparisonOperator::GreaterThan,
        QueryComparatorV2::GreaterOrEqual => TypedComparisonOperator::GreaterThanOrEqual,
        QueryComparatorV2::Contains => TypedComparisonOperator::Contains,
        QueryComparatorV2::StartsWith => TypedComparisonOperator::StartsWith,
        QueryComparatorV2::EndsWith => TypedComparisonOperator::EndsWith,
        QueryComparatorV2::Regex => TypedComparisonOperator::Regex,
    }
}

fn lower_literal(value: &CompatibilityValueV2) -> Result<TypedLiteral, Diagnostic> {
    if let Some(value) = value.canonical_value() {
        return Ok(match value {
            CanonicalValue::String(value) => TypedLiteral::String(value.as_str().to_owned()),
            CanonicalValue::Long(value) => TypedLiteral::Long(*value),
            CanonicalValue::Double(value) => TypedLiteral::Double(value.get()),
            CanonicalValue::Boolean(value) => TypedLiteral::Boolean(*value),
            CanonicalValue::Date(value) => TypedLiteral::Date(value.to_string()),
            CanonicalValue::DateTime(value) => TypedLiteral::DateTime(value.to_string()),
            CanonicalValue::DateTimeTz(value) => TypedLiteral::DateTimeTz(value.to_string()),
            CanonicalValue::Decimal(value) => TypedLiteral::Decimal(value.as_str().to_owned()),
            CanonicalValue::Duration(value) => TypedLiteral::Duration(value.to_string()),
        });
    }
    let text = value.released_text().ok_or_else(|| {
        integrity(
            "query_v2_compatibility_literal_state",
            "validated compatibility literal has no canonical or released representation",
        )
    })?;
    Ok(match value.value_type() {
        ValueTypeTag::String => TypedLiteral::String(text),
        ValueTypeTag::DateTime => TypedLiteral::DateTime(text),
        ValueTypeTag::DateTimeTz => TypedLiteral::DateTimeTz(text),
        ValueTypeTag::Decimal => TypedLiteral::Decimal(text),
        ValueTypeTag::Duration => TypedLiteral::Duration(text),
        ValueTypeTag::Long | ValueTypeTag::Double | ValueTypeTag::Boolean | ValueTypeTag::Date => {
            return Err(integrity(
                "query_v2_compatibility_literal_state",
                "validated released literal uses an unsupported scalar domain",
            ));
        }
    })
}

fn compiler_error(error: impl std::fmt::Display) -> Diagnostic {
    failure(
        DiagnosticCategory::InvalidContract,
        "query_v2_compatibility_provider_unsupported",
        "the selected provider cannot lower this validated released query shape",
    )
    .with_detail("compiler", error.to_string())
}

fn integrity(code: &'static str, message: &'static str) -> Diagnostic {
    failure(DiagnosticCategory::Integrity, code, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn released_decimal_literal_reaches_the_typed_compiler_without_canonical_v2_relaxation() {
        let value = CompatibilityValueV2::released_decimal("00123.4500dec")
            .expect("released V1 decimal spelling");
        assert_eq!(
            lower_literal(&value).expect("compatibility literal lowers"),
            TypedLiteral::Decimal("00123.4500dec".into()),
        );
    }
}
