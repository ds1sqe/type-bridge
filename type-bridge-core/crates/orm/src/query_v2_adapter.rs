//! Production one-way adaptation of released V1 match requests onto V2 plans.
//!
//! A validated V1 request is translated into one closed V2 compatibility
//! program. The ordinary plan contains only a canonical target/output
//! skeleton; the compatibility tree is the sole predicate source consumed by
//! compatibility lowering. Schema-aware validation recomputes the hydration
//! graph and rejects any second native semantic program before provider I/O.
//!
//! Arbitrary V1 strings predate the V2 canonical artifact ceiling. A request
//! whose exact V2 representation does not fit the session's query limits is
//! refused with a resource-limit diagnostic naming the limit, on the direct
//! path and the remote path alike. There is no fallback executor: a caller
//! whose request legitimately needs more raises the limit.

pub(crate) use crate::match_request::descriptor_plan::*;
use std::collections::BTreeMap;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::AttributeId;
use type_bridge_contract::migration_assertion::{
    AssertionBinding, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::{
    HydrationProjectionV2, InputColumn, InputColumnId, InputRow, ModelQueryV2, QueryBindingPairV2,
    QueryOperand, QueryOperation, QueryOutput, QueryPattern, QueryPlanV2Compatibility,
    QueryReductionGroupV2, QueryRowCardinalityV2, QueryWindowV2, ReadStage,
};
use type_bridge_contract::value::CanonicalValue;
use type_bridge_query::{MigrationAssertionValidationContext, ValidatedQuery};

use crate::_registry::DescriptorRegistry;
use crate::match_request::{
    BoundFieldId, Capability, ComparisonOp, MatchExpr, MatchFunctionArgument, MatchFunctionCall,
    MatchMode, MatchOperation, MatchRequest, MatchRequestVersion, MatchScalarOperand,
    RowCardinality, ValidatedMatchRequest,
};
use crate::query_v2_builder::{QueryCompatibilityPlanInput, QueryPlanBuilder};

/// The V2 program one V1 request adapts to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AdaptedMatchRequest {
    inputs: Vec<InputRow>,
    operation: QueryOperation,
    validated: Box<ValidatedQuery>,
}

impl AdaptedMatchRequest {
    /// Return the exact canonical invocation rows owned by this adaptation.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn inputs(&self) -> &[InputRow] {
        &self.inputs
    }

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

/// Adapt one canonically validated released V1 request.
///
/// Semantic translation failures are diagnostics and indicate a bridge bug or
/// stale/contradictory authority. Only a fixed V2 representation-envelope
/// mismatch is refused with a resource-limit diagnostic naming the limit.
pub(crate) fn adapt_match_request(
    validated: &ValidatedMatchRequest,
    registry: &DescriptorRegistry,
    context: &MigrationAssertionValidationContext<'_>,
) -> Result<AdaptedMatchRequest, Diagnostic> {
    // The limits V1 validation ran under govern the adapted plan too.
    let limits = validated.limits();
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
    for capability in validated.capabilities().iter().copied() {
        assert_released_capability_mapped(capability);
    }
    for (index, binding) in request.plan.bindings.iter().enumerate() {
        if usize::from(binding.id.get()) != index {
            return Err(reject(
                "query_v2_adapter_bindings_not_dense",
                "V1 bindings must be dense zero-based ordinals",
            ));
        }
    }
    validate_match_literal_limits(request.plan.predicate.as_ref(), limits)?;

    let schema = context.resolved_schema();
    let hydration = build_hydration(request, registry, schema)?;
    let has_schema_function = request
        .plan
        .predicate
        .as_ref()
        .is_some_and(predicate_has_schema_function);
    let predicate = request
        .plan
        .predicate
        .as_ref()
        .map(|expression| {
            if has_schema_function {
                adapt_compatibility_expression(expression, request, registry)
            } else {
                adapt_expression(expression, request, registry).map(Some)
            }
        })
        .transpose()?
        .flatten();
    let cross_joins = request
        .plan
        .allowed_cross_joins
        .iter()
        .map(|pair| {
            Ok(QueryBindingPairV2::new(
                BindingId::new(pair.left.get())?,
                BindingId::new(pair.right.get())?,
            ))
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let (operation, model_query, output_columns) = adapt_operation(validated, registry, hydration)?;

    let mut bindings = request
        .plan
        .bindings
        .iter()
        .enumerate()
        .map(|(index, _)| {
            Ok(AssertionBinding::new(
                binding_ordinal(index)?,
                QueryVariable::new(format!("b{index}"))?,
            ))
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let mut patterns = request
        .plan
        .bindings
        .iter()
        .map(|binding| {
            Ok(QueryPattern::Isa {
                binding: BindingId::new(binding.id.get())?,
                include_subtypes: binding.match_mode == MatchMode::Subtypes,
                type_id: descriptor_type(binding.descriptor.as_str(), binding.thing_kind)?,
            })
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let mut native = NativeFunctionAdaptation::new(bindings.len());
    if let Some(predicate) = &request.plan.predicate {
        native.adapt_expression(predicate, request, registry)?;
    }
    bindings.append(&mut native.bindings);
    patterns.append(&mut native.patterns);
    let inputs = native.inputs;
    let invocation_inputs = if inputs.is_empty() {
        Vec::new()
    } else {
        vec![InputRow::new(native.values)]
    };
    let mut selected = output_columns.clone();
    selected.sort_unstable();
    selected.dedup();
    let pipeline = vec![
        ReadStage::Match { patterns },
        ReadStage::Select { bindings: selected },
        ReadStage::Distinct,
    ];
    let compatibility = QueryPlanV2Compatibility::new(predicate, cross_joins, Some(model_query));
    let validated = QueryPlanBuilder::finalize_compatibility(
        QueryCompatibilityPlanInput::new(
            bindings,
            inputs,
            pipeline,
            QueryOutput::Rows {
                columns: output_columns,
            },
            compatibility,
        ),
        context,
        limits,
    )
    .map_err(|error| limits.annotate(error))?;
    // The plan must also encode: a remote peer and the model executor both
    // read its canonical bytes.
    validated
        .plan()
        .canonical_bytes()
        .map_err(|error| limits.annotate(error))?;
    Ok(AdaptedMatchRequest {
        inputs: invocation_inputs,
        operation,
        validated: Box::new(validated),
    })
}

/// Compile-time completeness guard for the released provider vocabulary.
const fn assert_released_capability_mapped(capability: Capability) {
    match capability {
        Capability::ResourceBoundedStreaming
        | Capability::ExactEntityTarget
        | Capability::ExactRelationTarget
        | Capability::SubtypeEntityTarget
        | Capability::SubtypeRelationTarget
        | Capability::FieldComparison
        | Capability::BooleanPattern
        | Capability::SelectedTupleDistinct
        | Capability::StableSelectedOrder
        | Capability::DistinctRootSelection
        | Capability::StableRootOrder
        | Capability::SameTransactionRehydration
        | Capability::BatchIdentityRebind
        | Capability::DistinctRootCount
        | Capability::DistinctRootExists
        | Capability::Collect
        | Capability::CollectDistinct
        | Capability::StableCollectionOrder
        | Capability::BoundedReachability
        | Capability::TypedReduction
        | Capability::SchemaFunctionCall => {}
    }
}

fn adapt_operation(
    validated: &ValidatedMatchRequest,
    registry: &DescriptorRegistry,
    hydration: HydrationProjectionV2,
) -> Result<(QueryOperation, ModelQueryV2, Vec<BindingId>), Diagnostic> {
    let request = validated.request();
    Ok(match &request.operation {
        MatchOperation::FetchRows {
            output,
            window,
            cardinality,
            ..
        } => {
            let model_output = adapt_output(output, validated, registry, request)?;
            let columns = output_slots(output)
                .map(|slot| BindingId::new(slot.binding().get()))
                .collect::<Result<Vec<_>, _>>()?;
            let (cardinality, order) = match cardinality {
                RowCardinality::ExactlyOne => (QueryRowCardinalityV2::ExactlyOne, None),
                RowCardinality::Complete => (QueryRowCardinalityV2::Complete, None),
                RowCardinality::BoundedMany => (
                    QueryRowCardinalityV2::BoundedMany,
                    Some(adapt_stable_order(
                        validated.stable_order(),
                        columns.clone(),
                        registry,
                        request,
                    )?),
                ),
            };
            (
                QueryOperation::Rows,
                ModelQueryV2::Rows {
                    cardinality,
                    hydration,
                    order,
                    output: model_output,
                    window: QueryWindowV2::new(window.offset, window.limit),
                },
                columns,
            )
        }
        MatchOperation::PageBy {
            root,
            output,
            window,
            include_total,
            ..
        } => {
            let root = BindingId::new(root.get())?;
            (
                QueryOperation::Rows,
                ModelQueryV2::Page {
                    hydration,
                    include_total: *include_total,
                    order: adapt_stable_order(
                        validated.stable_order(),
                        vec![root],
                        registry,
                        request,
                    )?,
                    output: adapt_output(output, validated, registry, request)?,
                    root,
                    window: QueryWindowV2::new(window.offset, window.limit),
                },
                vec![root],
            )
        }
        MatchOperation::CountBy { root } => {
            let root = BindingId::new(root.get())?;
            (
                QueryOperation::Count,
                ModelQueryV2::DistinctCount { hydration, root },
                vec![root],
            )
        }
        MatchOperation::ReduceBy {
            root,
            group,
            reducers,
        } => {
            let root_v2 = BindingId::new(root.get())?;
            let group = group
                .map(|binding| BindingId::new(binding.get()).map(QueryReductionGroupV2::binding))
                .transpose()?;
            (
                QueryOperation::Rows,
                ModelQueryV2::Reduction {
                    hydration,
                    root: root_v2,
                    group,
                    reducers: adapt_reducers(reducers, request, registry)?,
                },
                vec![root_v2],
            )
        }
        MatchOperation::ReduceByField {
            root,
            group,
            reducers,
        } => {
            let root = BindingId::new(root.get())?;
            (
                QueryOperation::Rows,
                ModelQueryV2::Reduction {
                    hydration,
                    root,
                    group: Some(QueryReductionGroupV2::field(adapt_field(
                        group, request, registry,
                    )?)),
                    reducers: adapt_reducers(reducers, request, registry)?,
                },
                vec![root],
            )
        }
        MatchOperation::ReduceByFields {
            root,
            groups,
            reducers,
        } => {
            let root = BindingId::new(root.get())?;
            (
                QueryOperation::Rows,
                ModelQueryV2::Reduction {
                    hydration,
                    root,
                    group: Some(QueryReductionGroupV2::fields(
                        groups
                            .iter()
                            .map(|group| adapt_field(group, request, registry))
                            .collect::<Result<Vec<_>, _>>()?,
                    )),
                    reducers: adapt_reducers(reducers, request, registry)?,
                },
                vec![root],
            )
        }
        MatchOperation::ExistsBy { root } => {
            let root = BindingId::new(root.get())?;
            (
                QueryOperation::Exists,
                ModelQueryV2::DistinctExists { hydration, root },
                vec![root],
            )
        }
    })
}

/// Native low-level V2 fragment for the schema-function-only portion of one
/// released predicate. Compatibility predicates remain the sole authority for
/// every other released expression.
struct NativeFunctionAdaptation {
    bindings: Vec<AssertionBinding>,
    fields: BTreeMap<BoundFieldId, BindingId>,
    inputs: Vec<InputColumn>,
    patterns: Vec<QueryPattern>,
    values: Vec<Option<CanonicalValue>>,
    next_binding: usize,
}

impl NativeFunctionAdaptation {
    fn new(next_binding: usize) -> Self {
        Self {
            bindings: Vec::new(),
            fields: BTreeMap::new(),
            inputs: Vec::new(),
            patterns: Vec::new(),
            values: Vec::new(),
            next_binding,
        }
    }

    fn adapt_expression(
        &mut self,
        expression: &MatchExpr,
        request: &MatchRequest,
        registry: &DescriptorRegistry,
    ) -> Result<(), Diagnostic> {
        match expression {
            MatchExpr::ScalarComparison {
                left,
                operator,
                right,
            } => {
                let left = self.adapt_scalar_operand(left, request, registry)?;
                let right = self.adapt_scalar_operand(right, request, registry)?;
                self.patterns.push(QueryPattern::Value {
                    comparator: function_comparator(*operator)?,
                    left,
                    right,
                });
            }
            MatchExpr::And { expressions } => {
                for child in expressions {
                    self.adapt_expression(child, request, registry)?;
                }
            }
            MatchExpr::FieldValue { .. }
            | MatchExpr::FieldComparison { .. }
            | MatchExpr::FieldPresence { .. }
            | MatchExpr::BindingIid { .. }
            | MatchExpr::RoleEdge { .. }
            | MatchExpr::Reachable { .. } => {}
            MatchExpr::Or { .. } | MatchExpr::Not { .. } => {
                if predicate_has_schema_function(expression) {
                    return Err(integrity(
                        "query_v2_adapter_function_scope",
                        "schema-function predicates must remain in the root conjunction",
                    ));
                }
            }
        }
        Ok(())
    }

    fn adapt_scalar_operand(
        &mut self,
        operand: &MatchScalarOperand,
        request: &MatchRequest,
        registry: &DescriptorRegistry,
    ) -> Result<QueryOperand, Diagnostic> {
        match operand {
            MatchScalarOperand::Field { field } => {
                let binding = if let Some(binding) = self.fields.get(field) {
                    *binding
                } else {
                    let owner = BindingId::new(field.binding.get())?;
                    let attribute = registered_attribute(registry, &field.field)?;
                    let binding = self.allocate_binding()?;
                    self.patterns.push(QueryPattern::Has {
                        attribute: binding,
                        attribute_id: AttributeId::new(attribute.attr_name.clone())?,
                        owner,
                    });
                    self.fields.insert(field.clone(), binding);
                    binding
                };
                Ok(QueryOperand::Binding { binding })
            }
            MatchScalarOperand::Value { value } => Ok(self.adapt_value(value.clone())?),
            MatchScalarOperand::Function { call } => Ok(QueryOperand::Binding {
                binding: self.adapt_call(call, request, registry)?,
            }),
        }
    }

    fn adapt_call(
        &mut self,
        call: &MatchFunctionCall,
        request: &MatchRequest,
        registry: &DescriptorRegistry,
    ) -> Result<BindingId, Diagnostic> {
        if registry.projected_function(&call.function).is_none() {
            return Err(integrity(
                "query_v2_adapter_function_authority",
                "validated schema function is absent from the installed projection authority",
            ));
        }
        let mut arguments = Vec::with_capacity(call.arguments.len());
        for argument in &call.arguments {
            arguments.push(match argument {
                MatchFunctionArgument::Binding { binding } => {
                    request
                        .plan
                        .bindings
                        .get(usize::from(binding.get()))
                        .ok_or_else(|| {
                            integrity(
                                "query_v2_adapter_unknown_binding",
                                "validated function argument references no declared binding",
                            )
                        })?;
                    QueryOperand::Binding {
                        binding: BindingId::new(binding.get())?,
                    }
                }
                MatchFunctionArgument::Value { value } => self.adapt_value(value.clone())?,
                MatchFunctionArgument::Call { call } => QueryOperand::Binding {
                    binding: self.adapt_call(call, request, registry)?,
                },
            });
        }
        let assigned = self.allocate_binding()?;
        self.patterns.push(QueryPattern::FunctionCall {
            arguments,
            assigned,
            function: call.function.clone(),
        });
        Ok(assigned)
    }

    fn adapt_value(&mut self, value: CanonicalValue) -> Result<QueryOperand, Diagnostic> {
        let ordinal = u16::try_from(self.inputs.len()).map_err(|_| {
            integrity(
                "query_v2_adapter_function_input_limit",
                "schema-function input ordinal exceeds the canonical range",
            )
        })?;
        let column = InputColumnId::new(ordinal);
        self.inputs.push(InputColumn::new(
            column,
            QueryVariable::new(format!("g{ordinal}"))?,
            value.value_type(),
            false,
        ));
        self.values.push(Some(value));
        Ok(QueryOperand::Input { column })
    }

    fn allocate_binding(&mut self) -> Result<BindingId, Diagnostic> {
        let ordinal = binding_ordinal(self.next_binding)?;
        self.next_binding = self.next_binding.checked_add(1).ok_or_else(|| {
            integrity(
                "query_v2_adapter_function_binding_limit",
                "schema-function binding ordinal overflowed",
            )
        })?;
        self.bindings.push(AssertionBinding::new(
            ordinal,
            QueryVariable::new(format!("b{}", ordinal.get()))?,
        ));
        Ok(ordinal)
    }
}

fn function_comparator(operator: ComparisonOp) -> Result<ValueComparator, Diagnostic> {
    match operator {
        ComparisonOp::Equal => Ok(ValueComparator::Equal),
        ComparisonOp::NotEqual => Ok(ValueComparator::NotEqual),
        ComparisonOp::LessThan => Ok(ValueComparator::Less),
        ComparisonOp::LessThanOrEqual => Ok(ValueComparator::LessOrEqual),
        ComparisonOp::GreaterThan => Ok(ValueComparator::Greater),
        ComparisonOp::GreaterThanOrEqual => Ok(ValueComparator::GreaterOrEqual),
        ComparisonOp::Contains
        | ComparisonOp::StartsWith
        | ComparisonOp::EndsWith
        | ComparisonOp::Regex => Err(integrity(
            "query_v2_adapter_function_comparator",
            "schema-function comparisons admit only scalar order comparators",
        )),
    }
}
