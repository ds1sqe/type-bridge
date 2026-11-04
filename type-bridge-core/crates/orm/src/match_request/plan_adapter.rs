//! Native plan construction for the released typed-read algebra.

use std::collections::{BTreeMap, BTreeSet};

use super::descriptor_plan::{
    MatchRequestAdapterAuthority, constructible_closure, descriptor_role_id, descriptor_type,
    integrity, registered_attribute,
};
use super::{
    BoundFieldId, ComparisonOp, MatchExpr, MatchFunctionArgument, MatchFunctionCall, MatchMode,
    MatchScalarOperand, ValidatedMatchRequest,
};
use crate::_registry::DescriptorRegistry;
use crate::AttributeValue;
use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::{AttributeId, TypeId};
use type_bridge_contract::migration_assertion::{
    AssertionBinding, AssertionRolePlayer, BindingId, QueryVariable, ValueComparator,
};
use type_bridge_contract::query_plan::v3::{
    ArithOp, Builtin, Expr, Operation, Pattern, PlanSpec, QueryPlanV3, Stage,
};
use type_bridge_contract::query_plan::{
    InputColumn, InputColumnId, InputRow, QueryOperand, QueryOutput, QueryPattern,
};
use type_bridge_contract::value::{CanonicalString, CanonicalValue};

struct Draft<'a> {
    registry: &'a DescriptorRegistry,
    request: &'a ValidatedMatchRequest,
    authority: &'a MatchRequestAdapterAuthority,
    bindings: Vec<AssertionBinding>,
    inputs: Vec<InputColumn>,
    values: Vec<Option<CanonicalValue>>,
    prefix: Vec<Pattern>,
    iids: BTreeMap<super::BindingId, BindingId>,
}

impl<'a> Draft<'a> {
    fn new(
        registry: &'a DescriptorRegistry,
        request: &'a ValidatedMatchRequest,
        authority: &'a MatchRequestAdapterAuthority,
    ) -> Result<Self, Diagnostic> {
        let mut draft = Self {
            registry,
            request,
            authority,
            bindings: vec![],
            inputs: vec![],
            values: vec![],
            prefix: vec![],
            iids: BTreeMap::new(),
        };
        for binding in &request.request().plan.bindings {
            let id = draft.allocate()?;
            if id.get() != binding.id.get() {
                return Err(integrity(
                    "query_plan_adapter_binding",
                    "match bindings are not dense",
                ));
            }
            draft.prefix.push(
                QueryPattern::Isa {
                    binding: id,
                    include_subtypes: binding.match_mode == MatchMode::Subtypes,
                    type_id: descriptor_type(binding.descriptor.as_str(), binding.thing_kind)?,
                }
                .into(),
            );
        }
        if let Some(predicate) = &request.request().plan.predicate {
            let patterns = draft.expression(predicate)?;
            draft.prefix.extend(patterns.into_iter().map(Pattern::Read));
        }
        Ok(draft)
    }

    fn allocate(&mut self) -> Result<BindingId, Diagnostic> {
        let ordinal = u16::try_from(self.bindings.len()).map_err(|_| {
            integrity(
                "query_plan_adapter_binding",
                "adapter bindings exceed the protocol range",
            )
        })?;
        let binding = BindingId::new(ordinal)?;
        self.bindings.push(AssertionBinding::new(
            binding,
            QueryVariable::new(format!("b{ordinal}"))?,
        ));
        Ok(binding)
    }

    fn input(&mut self, value: CanonicalValue) -> Result<QueryOperand, Diagnostic> {
        let ordinal = u16::try_from(self.inputs.len()).map_err(|_| {
            integrity(
                "query_plan_adapter_input",
                "adapter inputs exceed the protocol range",
            )
        })?;
        let column = InputColumnId::new(ordinal);
        self.inputs.push(InputColumn::new(
            column,
            QueryVariable::new(format!("input{ordinal}"))?,
            value.value_type(),
            false,
        ));
        self.values.push(Some(value));
        Ok(QueryOperand::Input { column })
    }

    fn value(&mut self, value: &AttributeValue) -> Result<QueryOperand, Diagnostic> {
        let value = super::result_validation::canonicalize_provider_attribute_value(value.clone())
            .map_err(|_| {
                integrity(
                    "query_plan_adapter_literal",
                    "validated literal cannot be canonicalized",
                )
            })?;
        let canonical = match &value {
            AttributeValue::String(value) => {
                CanonicalString::new_with_limits(value, self.request.limits().codec)
                    .map(CanonicalValue::String)?
            }
            AttributeValue::Duration(value) => super::result_validation::native_duration(value)
                .map(CanonicalValue::Duration)
                .ok_or_else(|| {
                    integrity(
                        "query_plan_adapter_literal",
                        "validated duration exceeds the native scalar domain",
                    )
                })?,
            _ => crate::runtime_projection::canonical_attribute_value(&value).map_err(|_| {
                integrity(
                    "query_plan_adapter_literal",
                    "validated literal is outside its canonical scalar domain",
                )
            })?,
        };
        self.input(canonical)
    }

    fn field(&mut self, field: &BoundFieldId) -> Result<(BindingId, QueryPattern), Diagnostic> {
        let attribute = registered_attribute(self.registry, &field.field)?;
        let binding = self.allocate()?;
        Ok((
            binding,
            QueryPattern::Has {
                owner: BindingId::new(field.binding.get())?,
                attribute: binding,
                attribute_id: AttributeId::new(attribute.attr_name)?,
            },
        ))
    }

    fn iid(&mut self, binding: super::BindingId) -> Result<BindingId, Diagnostic> {
        if let Some(id) = self.iids.get(&binding) {
            return Ok(*id);
        }
        let id = self.allocate()?;
        self.prefix.push(Pattern::Let {
            binding: id,
            expr: Expr::Iid {
                binding: BindingId::new(binding.get())?,
            },
        });
        self.iids.insert(binding, id);
        Ok(id)
    }

    fn scalar(
        &mut self,
        value: &MatchScalarOperand,
        patterns: &mut Vec<QueryPattern>,
    ) -> Result<QueryOperand, Diagnostic> {
        Ok(match value {
            MatchScalarOperand::Field { field } => {
                let (binding, has) = self.field(field)?;
                patterns.push(has);
                QueryOperand::Binding { binding }
            }
            MatchScalarOperand::Value { value } => self.input(value.clone())?,
            MatchScalarOperand::Function { call } => QueryOperand::Binding {
                binding: self.call(call, patterns)?,
            },
        })
    }

    fn call(
        &mut self,
        call: &MatchFunctionCall,
        patterns: &mut Vec<QueryPattern>,
    ) -> Result<BindingId, Diagnostic> {
        if self.registry.projected_function(&call.function).is_none() {
            return Err(integrity(
                "query_plan_adapter_function",
                "schema function is absent from the registry authority",
            ));
        }
        let mut arguments = Vec::with_capacity(call.arguments.len());
        for argument in &call.arguments {
            arguments.push(match argument {
                MatchFunctionArgument::Binding { binding } => QueryOperand::Binding {
                    binding: BindingId::new(binding.get())?,
                },
                MatchFunctionArgument::Value { value } => self.input(value.clone())?,
                MatchFunctionArgument::Call { call } => QueryOperand::Binding {
                    binding: self.call(call, patterns)?,
                },
            });
        }
        let assigned = self.allocate()?;
        patterns.push(QueryPattern::FunctionCall {
            arguments,
            assigned,
            function: call.function.clone(),
        });
        Ok(assigned)
    }

    fn expression(&mut self, expression: &MatchExpr) -> Result<Vec<QueryPattern>, Diagnostic> {
        let mut patterns = Vec::new();
        match expression {
            MatchExpr::FieldValue {
                field,
                operator,
                value,
            } => {
                let (binding, has) = self.field(field)?;
                patterns.push(has);
                let right = self.value(value)?;
                patterns.push(QueryPattern::Value {
                    comparator: comparator(*operator),
                    left: QueryOperand::Binding { binding },
                    right,
                });
            }
            MatchExpr::FieldComparison {
                left,
                operator,
                right,
            } => {
                let (left, left_has) = self.field(left)?;
                let (right, right_has) = self.field(right)?;
                patterns.extend([
                    left_has,
                    right_has,
                    QueryPattern::Value {
                        comparator: comparator(*operator),
                        left: QueryOperand::Binding { binding: left },
                        right: QueryOperand::Binding { binding: right },
                    },
                ]);
            }
            MatchExpr::ScalarComparison {
                left,
                operator,
                right,
            } => {
                let left = self.scalar(left, &mut patterns)?;
                let right = self.scalar(right, &mut patterns)?;
                patterns.push(QueryPattern::Value {
                    comparator: comparator(*operator),
                    left,
                    right,
                });
            }
            MatchExpr::FieldPresence { field, present } => {
                let (_, has) = self.field(field)?;
                patterns.push(if *present {
                    has
                } else {
                    QueryPattern::Not {
                        patterns: vec![has],
                    }
                });
            }
            MatchExpr::BindingIid { binding, iid } => {
                let binding = self.iid(*binding)?;
                let right = self.input(CanonicalValue::String(CanonicalString::new(iid)?))?;
                patterns.push(QueryPattern::Value {
                    comparator: ValueComparator::Equal,
                    left: QueryOperand::Binding { binding },
                    right,
                });
            }
            MatchExpr::RoleEdge {
                relation,
                role,
                player,
                ..
            } => {
                let binding = &self.request.request().plan.bindings[usize::from(relation.get())];
                let declared = descriptor_type(binding.descriptor.as_str(), binding.thing_kind)?;
                let concrete = constructible_closure(
                    self.authority.context().resolved_schema(),
                    &declared,
                    binding.match_mode == MatchMode::Subtypes,
                )?;
                let branches = concrete
                    .into_iter()
                    .map(|relation_id| {
                        Ok(vec![QueryPattern::Links {
                            relation: BindingId::new(relation.get())?,
                            relation_id,
                            players: vec![AssertionRolePlayer::new(
                                descriptor_role_id(role.owner.as_str(), &role.name)?,
                                BindingId::new(player.get())?,
                            )],
                        }])
                    })
                    .collect::<Result<Vec<_>, Diagnostic>>()?;
                match branches.len() {
                    0 => patterns.push(QueryPattern::Not {
                        patterns: vec![QueryPattern::Isa {
                            binding: BindingId::new(relation.get())?,
                            type_id: declared,
                            include_subtypes: true,
                        }],
                    }),
                    1 => patterns.extend(branches.into_iter().flatten()),
                    _ => patterns.push(QueryPattern::Or { branches }),
                }
            }
            MatchExpr::Reachable {
                relation,
                role_from,
                role_to,
                source,
                target,
                min_depth,
                max_depth,
            } => patterns.push(QueryPattern::Reachable {
                relation: descriptor_type(relation.as_str(), super::ThingKind::Relation)?,
                role_from: descriptor_role_id(role_from.owner.as_str(), &role_from.name)?,
                role_to: descriptor_role_id(role_to.owner.as_str(), &role_to.name)?,
                source: BindingId::new(source.get())?,
                target: BindingId::new(target.get())?,
                min_depth: *min_depth,
                max_depth: *max_depth,
            }),
            MatchExpr::And { expressions } => {
                for child in expressions {
                    patterns.extend(self.expression(child)?);
                }
            }
            MatchExpr::Or { expressions } => patterns.push(QueryPattern::Or {
                branches: expressions
                    .iter()
                    .map(|child| self.expression(child))
                    .collect::<Result<_, _>>()?,
            }),
            MatchExpr::Not { expression } => patterns.push(QueryPattern::Not {
                patterns: self.expression(expression)?,
            }),
        }
        Ok(patterns)
    }

    fn finish(
        &self,
        pipeline: Vec<Stage>,
        operation: Operation,
        columns: Vec<BindingId>,
    ) -> Result<QueryPlanV3, Diagnostic> {
        QueryPlanV3::new_with_limits(
            PlanSpec {
                bindings: self.bindings.clone(),
                inputs: self.inputs.clone(),
                given_rows: (!self.inputs.is_empty())
                    .then(|| vec![InputRow::new(self.values.clone())]),
                operation,
                pipeline,
                output: QueryOutput::Rows { columns },
                managed_semantics: self
                    .authority
                    .context()
                    .managed_state()
                    .managed_semantic_schema()
                    .clone(),
                functions: vec![],
            },
            self.authority.declared(),
            {
                let mut limits = self.request.limits();
                limits.structural.selected_slots = limits.structural.bindings;
                limits
            },
        )
    }
}

fn comparator(operator: ComparisonOp) -> ValueComparator {
    match operator {
        ComparisonOp::Equal => ValueComparator::Equal,
        ComparisonOp::NotEqual => ValueComparator::NotEqual,
        ComparisonOp::LessThan => ValueComparator::Less,
        ComparisonOp::LessThanOrEqual => ValueComparator::LessOrEqual,
        ComparisonOp::GreaterThan => ValueComparator::Greater,
        ComparisonOp::GreaterThanOrEqual => ValueComparator::GreaterOrEqual,
        ComparisonOp::Contains => ValueComparator::Contains,
        ComparisonOp::StartsWith => ValueComparator::StartsWith,
        ComparisonOp::EndsWith => ValueComparator::EndsWith,
        ComparisonOp::Regex => ValueComparator::Like,
    }
}

pub(crate) struct AdaptedMatchRequest {
    pub(crate) plans: Vec<QueryPlanV3>,
    pub(crate) layout: Layout,
    pub(crate) hydration: type_bridge_contract::query_plan::HydrationProjectionV2,
    pub(crate) predicate: Option<type_bridge_contract::query_plan::QueryPatternV2>,
}

pub(crate) const LONG_SUM_BASE: i64 = 1_i64 << 32;
pub(crate) const LONG_SUM_MAX_INPUTS: u64 = 1_u64 << 31;

#[derive(Clone, Copy)]
pub(crate) struct ReductionInput {
    pub(crate) plan: usize,
    pub(crate) value: usize,
    pub(crate) count: usize,
    pub(crate) optional: bool,
    pub(crate) sum_high: Option<usize>,
}

pub(crate) enum Layout {
    Count {
        root: super::BindingId,
        answer: usize,
    },
    Exists {
        root: super::BindingId,
        answer: usize,
    },
    WitnessReduction {
        inner: Box<AdaptedMatchRequest>,
        request: Box<ValidatedMatchRequest>,
    },
    Reduction {
        root: BindingId,
        group: Option<type_bridge_contract::query_plan::QueryReductionGroupV2>,
        reducers: Vec<type_bridge_contract::query_plan::QueryReductionTermV2>,
        anchor: usize,
        terms: Vec<Option<ReductionInput>>,
        hydration: Vec<(usize, BindingId, Vec<TypeId>)>,
    },
    Fetch {
        selection: Option<usize>,
        mapping: Option<usize>,
        hydration: Vec<(usize, BindingId, Vec<TypeId>)>,
        total: Option<usize>,
        witnesses: Vec<(type_bridge_contract::query_plan::QueryFieldV2, BindingId)>,
        output: type_bridge_contract::query_plan::QueryModelOutputV2,
        order: type_bridge_contract::query_plan::QueryStableOrderV2,
    },
}

impl Draft<'_> {
    fn count(&mut self, root: super::BindingId) -> Result<QueryPlanV3, Diagnostic> {
        use type_bridge_contract::query_plan::{ReduceAssignment, Reducer};
        let result = self.allocate()?;
        self.finish(
            vec![
                Stage::Match {
                    patterns: self.prefix.clone(),
                },
                Stage::Select {
                    bindings: vec![BindingId::new(root.get())?],
                },
                Stage::Distinct,
                Stage::Reduce {
                    assignments: vec![ReduceAssignment::new(result, Reducer::Count, None)],
                    groups: vec![],
                },
            ],
            Operation::Rows,
            vec![result],
        )
    }

    fn window(
        &mut self,
        selected: &[super::BindingId],
        order: &super::StableOrderSpec,
        window: super::Window,
        stable: bool,
    ) -> Result<(Vec<Stage>, Vec<BindingId>), Diagnostic> {
        use type_bridge_contract::query_plan::{OrderDirection, OrderTerm};
        let mut retained = selected
            .iter()
            .map(|binding| BindingId::new(binding.get()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut terms = Vec::new();
        if stable {
            for term in order.terms() {
                let (binding, has) = self.field(&term.order().field)?;
                self.prefix.push(has.into());
                retained.push(binding);
                terms.push(OrderTerm::new(
                    binding,
                    match term.order().direction {
                        super::SortDirection::Ascending => OrderDirection::Ascending,
                        super::SortDirection::Descending => OrderDirection::Descending,
                    },
                ));
            }
        }
        for selected in selected {
            let binding = self.iid(*selected)?;
            retained.push(binding);
            terms.push(OrderTerm::new(binding, OrderDirection::Ascending));
        }
        retained.sort();
        retained.dedup();
        let mut pipeline = vec![
            Stage::Match {
                patterns: self.prefix.clone(),
            },
            Stage::Select {
                bindings: retained.clone(),
            },
            Stage::Distinct,
        ];
        if !terms.is_empty() {
            pipeline.push(Stage::Sort { terms });
        }
        if window.offset > 0 {
            pipeline.push(Stage::Offset {
                rows: window.offset,
            });
        }
        pipeline.push(Stage::Limit { rows: window.limit });
        Ok((pipeline, retained))
    }
}

pub(crate) fn adapt_match_request(
    registry: &DescriptorRegistry,
    request: &ValidatedMatchRequest,
) -> Result<AdaptedMatchRequest, Diagnostic> {
    let authority = MatchRequestAdapterAuthority::from_registry(registry)?;
    adapt_match_request_under_authority(registry, request, &authority)
}

pub(crate) fn adapt_match_request_under_authority(
    registry: &DescriptorRegistry,
    request: &ValidatedMatchRequest,
    authority: &MatchRequestAdapterAuthority,
) -> Result<AdaptedMatchRequest, Diagnostic> {
    use super::descriptor_plan::{
        adapt_compatibility_expression, adapt_output, adapt_stable_order, build_hydration,
        output_slots, validate_match_literal_limits,
    };
    use super::{MatchOperation, RowCardinality};
    request.recheck_schema(registry).map_err(|_| {
        integrity(
            "query_plan_adapter_stale_schema",
            "registry changed after match validation",
        )
    })?;
    validate_match_literal_limits(request.request().plan.predicate.as_ref(), request.limits())?;
    let hydration = build_hydration(
        request.request(),
        registry,
        authority.context().resolved_schema(),
    )?;
    let predicate = request
        .request()
        .plan
        .predicate
        .as_ref()
        .map(|predicate| adapt_compatibility_expression(predicate, request.request(), registry))
        .transpose()?
        .flatten();
    let mut draft = Draft::new(registry, request, authority)?;
    let mut plans = Vec::new();
    let layout = match &request.request().operation {
        MatchOperation::CountBy { root } => {
            plans.push(draft.count(*root)?);
            Layout::Count {
                root: *root,
                answer: 0,
            }
        }
        MatchOperation::ExistsBy { root } => {
            plans.push(draft.finish(
                vec![
                    Stage::Match {
                        patterns: draft.prefix.clone(),
                    },
                    Stage::Select {
                        bindings: vec![BindingId::new(root.get())?],
                    },
                    Stage::Distinct,
                ],
                Operation::Exists,
                vec![],
            )?);
            Layout::Exists {
                root: *root,
                answer: 0,
            }
        }
        operation @ (MatchOperation::FetchRows { output, window, .. }
        | MatchOperation::PageBy { output, window, .. }) => {
            let total = if let MatchOperation::PageBy {
                root,
                include_total: true,
                ..
            } = operation
            {
                plans.push(draft.count(*root)?);
                Some(0)
            } else {
                None
            };
            let mut selected = match operation {
                MatchOperation::PageBy { root, .. } => vec![*root],
                _ => output_slots(output).map(|slot| slot.binding()).collect(),
            };
            selected.sort();
            selected.dedup();
            let stable = matches!(
                operation,
                MatchOperation::PageBy { .. }
                    | MatchOperation::FetchRows {
                        cardinality: RowCardinality::BoundedMany,
                        ..
                    }
            );
            let actual_window = super::Window {
                offset: window.offset,
                limit: match operation {
                    MatchOperation::FetchRows {
                        cardinality: RowCardinality::ExactlyOne,
                        ..
                    } => 2,
                    MatchOperation::FetchRows {
                        cardinality: RowCardinality::Complete,
                        ..
                    } => window.limit.saturating_add(1),
                    _ => window.limit,
                },
            };
            let mut hydrated = output_slots(output)
                .map(|slot| slot.binding().get())
                .collect::<BTreeSet<_>>();
            hydrated.extend(selected.iter().map(|binding| binding.get()));
            let mut fields = Vec::new();
            if let Some(predicate) = &predicate {
                hidden_predicate_fields(predicate, &hydrated, &mut fields);
            }
            fields.sort_by_key(|field| (field.binding(), field.attribute().clone()));
            fields.dedup();
            let mut witnesses = Vec::new();
            for field in fields {
                let binding = draft.allocate()?;
                draft.prefix.push(
                    QueryPattern::Try {
                        patterns: vec![QueryPattern::Has {
                            owner: field.binding(),
                            attribute: binding,
                            attribute_id: field.attribute().clone(),
                        }],
                    }
                    .into(),
                );
                witnesses.push((field, binding));
            }
            let (prefix, _retained) =
                draft.window(&selected, request.stable_order(), actual_window, stable)?;
            let model_output = adapt_output(output, request, registry, request.request())?;
            let order = adapt_stable_order(
                request.stable_order(),
                selected
                    .iter()
                    .map(|binding| BindingId::new(binding.get()))
                    .collect::<Result<_, _>>()?,
                registry,
                request.request(),
            )?;
            let source = draft.finish(
                prefix,
                Operation::Rows,
                selected
                    .iter()
                    .map(|binding| BindingId::new(binding.get()))
                    .collect::<Result<_, _>>()?,
            )?;
            let model_slots = request
                .request()
                .plan
                .bindings
                .iter()
                .filter(|binding| hydrated.contains(&binding.id.get()))
                .map(|binding| {
                    Ok((
                        BindingId::new(binding.id.get())?,
                        binding.descriptor.clone(),
                    ))
                })
                .collect::<Result<Vec<_>, Diagnostic>>()?;
            let stack = super::plan_fetch_stack::build_under_authority(
                registry,
                &source,
                &model_slots,
                authority,
                true,
                &witnesses
                    .iter()
                    .map(|(_, binding)| *binding)
                    .collect::<Vec<_>>(),
                super::plan_fetch_stack::FetchProof::CorrelatedMatch,
            )?;
            let offset = plans.len();
            plans.extend(stack.plans);
            Layout::Fetch {
                selection: stack.selection.map(|index| offset + index),
                mapping: stack.mapping.map(|index| offset + index),
                hydration: stack
                    .partitions
                    .into_iter()
                    .map(|(index, binding, concrete)| (offset + index, binding, concrete))
                    .collect(),
                total,
                witnesses,
                output: model_output,
                order,
            }
        }

        operation @ (MatchOperation::ReduceBy { root, reducers, .. }
        | MatchOperation::ReduceByField { root, reducers, .. }
        | MatchOperation::ReduceByFields { root, reducers, .. }) => {
            use super::descriptor_plan::{adapt_field, adapt_reducers};
            use type_bridge_contract::query_plan::QueryReductionGroupV2;
            let group = match operation {
                MatchOperation::ReduceBy { group, .. } => group
                    .map(|binding| {
                        BindingId::new(binding.get()).map(QueryReductionGroupV2::binding)
                    })
                    .transpose()?,
                MatchOperation::ReduceByField { group, .. } => Some(QueryReductionGroupV2::field(
                    adapt_field(group, request.request(), registry)?,
                )),
                MatchOperation::ReduceByFields { groups, .. } => {
                    Some(QueryReductionGroupV2::fields(
                        groups
                            .iter()
                            .map(|field| adapt_field(field, request.request(), registry))
                            .collect::<Result<_, _>>()?,
                    ))
                }
                _ => unreachable!(),
            };
            let witness_dependent =
                reducers
                    .iter()
                    .filter_map(|term| term.input.as_ref())
                    .any(|input| {
                        input.binding != *root
                            && match operation {
                                MatchOperation::ReduceBy { group, .. } => {
                                    *group != Some(input.binding)
                                }
                                MatchOperation::ReduceByField { group, .. } => input != group,
                                MatchOperation::ReduceByFields { groups, .. } => {
                                    !groups.contains(input)
                                }
                                _ => unreachable!(),
                            }
                    });
            if witness_dependent {
                let mut inner_request = request.request().clone();
                inner_request.operation = MatchOperation::FetchRows {
                    output: super::FetchShape::Positional {
                        slots: inner_request
                            .plan
                            .bindings
                            .iter()
                            .map(|binding| super::FetchSlot::One {
                                binding: binding.id,
                            })
                            .collect(),
                    },
                    order: vec![],
                    window: super::Window {
                        offset: 0,
                        limit: i64::MAX as u64 - 1,
                    },
                    cardinality: super::RowCardinality::Complete,
                };
                let inner_request = super::validate_match_request_with_limits(
                    registry,
                    inner_request,
                    request.limits(),
                )
                .map_err(|_| {
                    integrity(
                        "query_plan_adapter_reduction",
                        "witness reduction evidence request cannot be validated",
                    )
                })?;
                let inner =
                    adapt_match_request_under_authority(registry, &inner_request, authority)?;
                return Ok(AdaptedMatchRequest {
                    plans: inner.plans.clone(),
                    layout: Layout::WitnessReduction {
                        inner: Box::new(inner),
                        request: Box::new(inner_request),
                    },
                    hydration,
                    predicate,
                });
            }
            let root = BindingId::new(root.get())?;
            let anchor = plans.len();
            plans.push(draft.reduction_plan(root, group.as_ref(), &[], false)?);
            let mut terms = vec![None; reducers.len()];
            let mut inputs = BTreeMap::<_, Vec<_>>::new();
            for (index, term) in reducers.iter().enumerate() {
                if let Some(input) = &term.input {
                    inputs.entry(input.clone()).or_default().push((index, term));
                }
            }
            let width = match &group {
                None => 0,
                Some(QueryReductionGroupV2::Fields { fields }) => fields.len(),
                Some(_) => 1,
            };
            for (input, input_terms) in inputs {
                let index = plans.len();
                let attribute = registered_attribute(registry, &input.field)?;
                let optional = attribute.is_optional
                    || attribute.cardinality().is_some_and(|(min, _)| min == 0);
                let long_input = attribute.value_type == crate::_attribute::ValueType::Long;
                let exact_terms = input_terms
                    .iter()
                    .filter(|(_, term)| {
                        long_input
                            && matches!(
                                term.reduction,
                                super::Reduction::Sum | super::Reduction::Mean
                            )
                    })
                    .count();
                let count_column = width + input_terms.len() - exact_terms.saturating_sub(1);
                let mut next_column = width;
                let mut exact_column = None;
                for (term, reduction) in &input_terms {
                    let exact = long_input
                        && matches!(
                            reduction.reduction,
                            super::Reduction::Sum | super::Reduction::Mean
                        );
                    let column = if exact {
                        *exact_column.get_or_insert_with(|| {
                            let column = next_column;
                            next_column += 1;
                            column
                        })
                    } else {
                        let column = next_column;
                        next_column += 1;
                        column
                    };
                    terms[*term] = Some(ReductionInput {
                        plan: index,
                        value: column,
                        count: count_column,
                        optional,
                        sum_high: exact.then_some(count_column + 1),
                    });
                }
                plans.push(
                    draft.reduction_plan(
                        root,
                        group.as_ref(),
                        &input_terms
                            .iter()
                            .map(|(_, term)| *term)
                            .collect::<Vec<_>>(),
                        long_input,
                    )?,
                );
            }
            let mut document_plans = Vec::new();
            if let Some(QueryReductionGroupV2::Binding { binding }) = &group {
                let metadata = hydration
                    .bindings()
                    .iter()
                    .find(|item| item.binding() == *binding)
                    .ok_or_else(|| {
                        integrity(
                            "query_plan_adapter_reduction",
                            "group binding has no hydration authority",
                        )
                    })?;
                let prefix = vec![
                    Stage::Match {
                        patterns: draft.prefix.clone(),
                    },
                    Stage::Select {
                        bindings: vec![*binding],
                    },
                    Stage::Distinct,
                ];
                let source = draft.finish(prefix.clone(), Operation::Rows, vec![*binding])?;
                document_plans = super::plan_fetch_stack::append_hydration_partitions(
                    super::plan_fetch_stack::FetchPrefix {
                        source: &source,
                        stages: &prefix,
                        rematch: &[],
                        baseline: Some(*binding),
                    },
                    std::slice::from_ref(metadata),
                    &hydration,
                    authority,
                    &mut plans,
                )?;
            }
            Layout::Reduction {
                root,
                group,
                reducers: adapt_reducers(reducers, request.request(), registry)?,
                anchor,
                terms,
                hydration: document_plans,
            }
        }
    };
    Ok(AdaptedMatchRequest {
        plans,
        layout,
        hydration,
        predicate,
    })
}

impl Draft<'_> {
    fn reduction_plan(
        &mut self,
        root: BindingId,
        group: Option<&type_bridge_contract::query_plan::QueryReductionGroupV2>,
        terms: &[&super::ReduceTerm],
        long_input: bool,
    ) -> Result<QueryPlanV3, Diagnostic> {
        use type_bridge_contract::query_plan::{ReduceAssignment, Reducer};
        let mut patterns = self.prefix.clone();
        let mut groups = Vec::new();
        let mut group_fields = Vec::new();
        match group {
            Some(type_bridge_contract::query_plan::QueryReductionGroupV2::Binding { binding }) => {
                groups.push(*binding)
            }
            Some(type_bridge_contract::query_plan::QueryReductionGroupV2::Field { field }) => {
                group_fields.push(field)
            }
            Some(type_bridge_contract::query_plan::QueryReductionGroupV2::Fields { fields }) => {
                group_fields.extend(fields)
            }
            None => {}
        }
        for field in group_fields {
            let binding = self.allocate()?;
            patterns.push(
                QueryPattern::Has {
                    owner: field.binding(),
                    attribute: binding,
                    attribute_id: field.attribute().clone(),
                }
                .into(),
            );
            groups.push(binding);
        }
        let input = if let Some(field) = terms.first().and_then(|term| term.input.as_ref()) {
            let (binding, has) = self.field(field)?;
            patterns.push(has.into());
            Some(binding)
        } else {
            None
        };
        // A constant group preserves absence on empty statistical input streams.
        let constant_group = groups.is_empty()
            && terms.iter().any(|term| {
                !matches!(
                    term.reduction,
                    super::Reduction::Count | super::Reduction::Sum
                )
            });
        if constant_group {
            let binding = self.allocate()?;
            patterns.push(Pattern::Let {
                binding,
                expr: Expr::Operand(QueryOperand::Literal {
                    value: CanonicalValue::Long(1),
                }),
            });
            groups.push(binding);
        }
        let exact_sum = long_input
            && terms.iter().any(|term| {
                matches!(
                    term.reduction,
                    super::Reduction::Sum | super::Reduction::Mean
                )
            });
        let halves = if exact_sum {
            let low = self.allocate()?;
            let high = self.allocate()?;
            let value = |binding| Expr::Operand(QueryOperand::Binding { binding });
            let base = || {
                Expr::Operand(QueryOperand::Literal {
                    value: CanonicalValue::Long(LONG_SUM_BASE),
                })
            };
            patterns.push(Pattern::Let {
                binding: low,
                expr: Expr::Arith {
                    op: ArithOp::Mod,
                    lhs: Box::new(value(input.expect("sum input"))),
                    rhs: Box::new(base()),
                },
            });
            // TypeDB uses floored modulo and Double division; the multiple of 2^32 stays exact before floor returns Long.
            patterns.push(Pattern::Let {
                binding: high,
                expr: Expr::Call {
                    func: Builtin::Floor,
                    args: vec![Expr::Arith {
                        op: ArithOp::Div,
                        lhs: Box::new(Expr::Arith {
                            op: ArithOp::Sub,
                            lhs: Box::new(value(input.expect("sum input"))),
                            rhs: Box::new(value(low)),
                        }),
                        rhs: Box::new(base()),
                    }],
                },
            });
            Some((low, high))
        } else {
            None
        };
        let floating_input = if long_input
            && terms.iter().any(|term| {
                matches!(
                    term.reduction,
                    super::Reduction::Std | super::Reduction::Median
                )
            }) {
            let binding = self.allocate()?;
            patterns.push(Pattern::Let {
                binding,
                expr: Expr::Arith {
                    op: ArithOp::Mul,
                    lhs: Box::new(Expr::Operand(QueryOperand::Binding {
                        binding: input.expect("statistical input"),
                    })),
                    rhs: Box::new(Expr::Operand(QueryOperand::Literal {
                        value: CanonicalValue::Double(
                            type_bridge_contract::value::CanonicalDouble::new(1.0)
                                .expect("finite constant"),
                        ),
                    })),
                },
            });
            Some(binding)
        } else {
            None
        };
        let mut selected = vec![root];
        selected.extend(&groups);
        selected.extend(input);
        if let Some((low, high)) = halves {
            selected.extend([low, high]);
        }
        selected.extend(floating_input);
        selected.sort();
        selected.dedup();
        let mut columns = if constant_group {
            vec![]
        } else {
            groups.clone()
        };
        let mut assignments = Vec::new();
        let mut exact_assigned = false;
        for term in terms {
            let exact = exact_sum
                && matches!(
                    term.reduction,
                    super::Reduction::Sum | super::Reduction::Mean
                );
            if exact && std::mem::replace(&mut exact_assigned, true) {
                continue;
            }
            let reducer = if exact {
                Reducer::Sum
            } else {
                match term.reduction {
                    super::Reduction::Count => Reducer::Count,
                    super::Reduction::Sum => Reducer::Sum,
                    super::Reduction::Min => Reducer::Min,
                    super::Reduction::Max => Reducer::Max,
                    super::Reduction::Mean => Reducer::Mean,
                    super::Reduction::Median => Reducer::Median,
                    super::Reduction::Std => Reducer::Std,
                }
            };
            let result = self.allocate()?;
            let operand = if exact {
                halves.map(|(low, _)| low)
            } else if matches!(
                term.reduction,
                super::Reduction::Std | super::Reduction::Median
            ) {
                floating_input.or(input)
            } else {
                input
            };
            assignments.push(ReduceAssignment::new(result, reducer, operand));
            columns.push(result);
        }
        // One shared input count covers emptiness and sample-deviation cardinality.
        let size = self.allocate()?;
        assignments.push(ReduceAssignment::new(size, Reducer::Count, None));
        columns.push(size);
        if let Some((_, high)) = halves {
            let result = self.allocate()?;
            assignments.push(ReduceAssignment::new(result, Reducer::Sum, Some(high)));
            columns.push(result);
        }
        self.finish(
            vec![
                Stage::Match { patterns },
                Stage::Select { bindings: selected },
                Stage::Distinct,
                Stage::Reduce {
                    assignments,
                    groups,
                },
            ],
            Operation::Rows,
            columns,
        )
    }
}

fn hidden_predicate_fields(
    pattern: &type_bridge_contract::query_plan::QueryPatternV2,
    selected: &BTreeSet<u16>,
    fields: &mut Vec<type_bridge_contract::query_plan::QueryFieldV2>,
) {
    use type_bridge_contract::query_plan::QueryPatternV2;
    let mut add = |field: &type_bridge_contract::query_plan::QueryFieldV2| {
        if !selected.contains(&field.binding().get()) {
            fields.push(field.clone());
        }
    };
    match pattern {
        QueryPatternV2::FieldValue { field, .. } | QueryPatternV2::FieldPresence { field, .. } => {
            add(field)
        }
        QueryPatternV2::FieldComparison { left, right, .. } => {
            add(left);
            add(right);
        }
        QueryPatternV2::And { patterns } | QueryPatternV2::Or { patterns } => {
            for pattern in patterns {
                hidden_predicate_fields(pattern, selected, fields);
            }
        }
        QueryPatternV2::Not { pattern } => hidden_predicate_fields(pattern, selected, fields),
        _ => {}
    }
}
