//! Canonical typed lowering for validated match execution requests.

use std::collections::{BTreeMap, BTreeSet};

use type_bridge_core_lib::ast::{
    TypedCollectionOrder, TypedComparisonOperator, TypedFetchRows, TypedFieldBinding, TypedLiteral,
    TypedMatchOrder, TypedMatchPredicate, TypedMatchTarget, TypedMissingOrder, TypedPageRematch,
    TypedRootScan, TypedSortDirection, TypedThingKind,
};

use super::error::{MatchError, MatchErrorCategory, MatchErrorPathSegment};
use super::ids::{BoundFieldId, DescriptorId, FieldId};
use super::model::{
    ComparisonOp, FetchShape, FetchSlot, MatchExpr, MatchMode, MatchOperation, MissingOrder,
    RowCardinality, SortDirection, ThingKind,
};
use super::validation::{StableOrderSpec, ValidatedMatchRequest};
use crate::descriptor::TypeDescriptorRef;
use crate::registry::DescriptorRegistry;
use crate::value::AttributeValue;

/// One fully preflighted typed operation plan.
#[derive(Debug, Clone)]
pub(crate) enum LoweredMatchExecution {
    FetchRows(TypedFetchRows),
    ExactlyOneBy {
        selection: TypedFetchRows,
        evidence: TypedFetchRows,
    },
    CountBy {
        root: super::ids::BindingId,
        scan: TypedRootScan,
    },
    ReduceBy {
        root: super::ids::BindingId,
        group: Option<super::ids::BindingId>,
        scan: TypedRootScan,
        rematch: Option<TypedPageRematch>,
        terms: Vec<LoweredReduceTerm>,
    },
    ExistsBy {
        root: super::ids::BindingId,
        scan: TypedRootScan,
    },
    PageBy {
        root: super::ids::BindingId,
        total: Option<TypedRootScan>,
        selection: Box<TypedRootScan>,
        rematch: TypedPageRematch,
    },
}

/// The canonical numeric domain of one lowered reducer input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReduceDomain {
    Long,
    Double,
}

/// One reducer term with its registry-resolved input identity and domain.
#[derive(Debug, Clone)]
pub(crate) struct LoweredReduceTerm {
    pub(crate) reduction: super::model::Reduction,
    pub(crate) input: Option<LoweredReduceInput>,
}

/// One registry-resolved reducer input.
#[derive(Debug, Clone)]
pub(crate) struct LoweredReduceInput {
    pub(crate) binding: super::ids::BindingId,
    pub(crate) field: super::ids::FieldId,
    pub(crate) domain: ReduceDomain,
}

#[derive(Debug, Clone)]
struct LoweredGraph {
    targets: Vec<TypedMatchTarget>,
    fields: Vec<TypedFieldBinding>,
    predicate: Option<TypedMatchPredicate>,
    field_ids: BTreeMap<BoundFieldId, u16>,
}

fn reduce_owner_attribute(
    descriptor: &crate::descriptor::TypeDescriptorRef,
    field_name: &str,
) -> Result<crate::descriptor::OwnedAttributeDescriptor, MatchError> {
    let attributes = match descriptor {
        crate::descriptor::TypeDescriptorRef::Entity(entity) => &entity.owned_attributes,
        crate::descriptor::TypeDescriptorRef::Relation(relation) => &relation.owned_attributes,
    };
    attributes
        .iter()
        .find(|attribute| attribute.field_name == field_name)
        .cloned()
        .ok_or_else(|| {
            unsupported(
                "unknown_field",
                "validated reducer input lost its registered field",
            )
        })
}

/// Lower one validated operation into its complete bounded typed statement plan.
pub(crate) fn lower_match_execution(
    registry: &DescriptorRegistry,
    validated: &ValidatedMatchRequest,
) -> Result<LoweredMatchExecution, MatchError> {
    let request = validated.request();
    match &request.operation {
        MatchOperation::FetchRows {
            cardinality: RowCardinality::ExactlyOne,
            ..
        } => {
            let evidence = lower_fetch_rows(registry, validated)?;
            let mut selection = evidence.clone();
            selection.order.clear();
            selection.offset = 0;
            selection.limit = 2;
            Ok(LoweredMatchExecution::ExactlyOneBy {
                selection,
                evidence,
            })
        }
        MatchOperation::FetchRows { .. } => Ok(LoweredMatchExecution::FetchRows(lower_fetch_rows(
            registry, validated,
        )?)),
        MatchOperation::CountBy { root } => {
            let graph = lower_graph(registry, validated, &[])?;
            Ok(LoweredMatchExecution::CountBy {
                root: *root,
                scan: root_scan(graph, root.get(), Vec::new(), None, None),
            })
        }
        MatchOperation::ExistsBy { root } => {
            let graph = lower_graph(registry, validated, &[])?;
            Ok(LoweredMatchExecution::ExistsBy {
                root: *root,
                scan: root_scan(graph, root.get(), Vec::new(), None, Some(1)),
            })
        }
        MatchOperation::ReduceBy {
            root,
            group,
            reducers,
        } => {
            let graph = lower_graph(registry, validated, &[])?;
            let mut terms = Vec::with_capacity(reducers.len());
            let mut needs_rematch = group.is_some();
            for term in reducers {
                let input = term
                    .input
                    .as_ref()
                    .map(|field| {
                        needs_rematch = true;
                        let owner_name = registry
                            .descriptor_type_name(&field.field.owner)
                            .ok_or_else(|| {
                                unsupported(
                                    "unknown_descriptor",
                                    "validated reducer input lost its registered owner",
                                )
                            })?;
                        let descriptor = registry.get(&owner_name).ok_or_else(|| {
                            unsupported(
                                "unknown_descriptor",
                                "validated reducer input lost its registered owner",
                            )
                        })?;
                        let attribute = reduce_owner_attribute(&descriptor, &field.field.name)?;
                        let domain = match attribute.value_type.as_str() {
                            "long" | "integer" => ReduceDomain::Long,
                            "double" => ReduceDomain::Double,
                            _ => {
                                return Err(unsupported(
                                    "reduce_input_domain",
                                    "validated reducer input lost its numeric domain",
                                ));
                            }
                        };
                        Ok(LoweredReduceInput {
                            binding: field.binding,
                            field: field.field.clone(),
                            domain,
                        })
                    })
                    .transpose()?;
                terms.push(LoweredReduceTerm {
                    reduction: term.reduction,
                    input,
                });
            }
            let rematch = needs_rematch.then(|| TypedPageRematch {
                targets: graph.targets.clone(),
                fields: graph.fields.clone(),
                predicate: graph.predicate.clone(),
                root: root.get(),
                root_concept_ids: Vec::new(),
                collection_orders: Vec::new(),
            });
            Ok(LoweredMatchExecution::ReduceBy {
                root: *root,
                group: *group,
                scan: root_scan(graph, root.get(), Vec::new(), None, None),
                rematch,
                terms,
            })
        }
        MatchOperation::PageBy {
            root,
            output,
            window,
            include_total,
            ..
        } => {
            let mut specs = vec![validated.stable_order()];
            let mut collection_bindings = BTreeSet::new();
            for slot in output_slots(output) {
                if let FetchSlot::Collect { binding, .. } = slot
                    && collection_bindings.insert(*binding)
                {
                    specs.push(validated.collection_order(*binding).ok_or_else(|| {
                        unsupported(
                            "missing_collection_order_proof",
                            "validated page omitted a collected binding order proof",
                        )
                        .at(MatchErrorPathSegment::Binding(*binding))
                    })?);
                }
            }
            let graph = lower_graph(registry, validated, &specs)?;
            let root_order = lower_order(registry, validated.stable_order(), &graph.field_ids)?;
            let collection_orders = collection_bindings
                .into_iter()
                .map(|binding| {
                    Ok(TypedCollectionOrder {
                        binding: binding.get(),
                        order: lower_order(
                            registry,
                            validated
                                .collection_order(binding)
                                .expect("validated collection proof remains present"),
                            &graph.field_ids,
                        )?,
                    })
                })
                .collect::<Result<Vec<_>, MatchError>>()?;
            let selection = root_scan(
                graph.clone(),
                root.get(),
                root_order,
                Some(window.offset),
                Some(window.limit),
            );
            let total =
                include_total.then(|| root_scan(graph.clone(), root.get(), Vec::new(), None, None));
            Ok(LoweredMatchExecution::PageBy {
                root: *root,
                total,
                selection: Box::new(selection),
                rematch: TypedPageRematch {
                    targets: graph.targets,
                    fields: graph.fields,
                    predicate: graph.predicate,
                    root: root.get(),
                    root_concept_ids: Vec::new(),
                    collection_orders,
                },
            })
        }
    }
}

/// Re-run the released provider-lowering checks without retaining a statement.
///
/// Compatibility execution must preserve failures which historically occurred
/// while lowering V1 requests, even when the validated request can otherwise
/// be represented by an additive V2 plan.
pub(crate) fn preflight_released_match_execution(
    registry: &DescriptorRegistry,
    validated: &ValidatedMatchRequest,
) -> Result<(), MatchError> {
    lower_match_execution(registry, validated).map(|_| ())
}

/// Lower one current validated request into a typed provider statement.
pub(crate) fn lower_fetch_rows(
    registry: &DescriptorRegistry,
    validated: &ValidatedMatchRequest,
) -> Result<TypedFetchRows, MatchError> {
    let request = validated.request();
    let MatchOperation::FetchRows { output, window, .. } = &request.operation else {
        return Err(unsupported(
            "unsupported_selected_operation",
            "selected-row execution supports only FetchRows",
        ));
    };

    let projection = output_slots(output)
        .map(|slot| match slot {
            FetchSlot::One { binding } => Ok(binding.get()),
            FetchSlot::Collect { .. } => Err(unsupported(
                "unsupported_collection_slot",
                "selected-row execution does not support collected output slots",
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;

    let graph = lower_graph(registry, validated, &[validated.stable_order()])?;
    let order = lower_order(registry, validated.stable_order(), &graph.field_ids)?;

    Ok(TypedFetchRows {
        targets: graph.targets,
        fields: graph.fields,
        predicate: graph.predicate,
        projection,
        distinct: true,
        order,
        offset: window.offset,
        limit: window.limit,
    })
}

fn lower_graph(
    registry: &DescriptorRegistry,
    validated: &ValidatedMatchRequest,
    orders: &[&StableOrderSpec],
) -> Result<LoweredGraph, MatchError> {
    let request = validated.request();
    let targets = request
        .plan
        .bindings
        .iter()
        .map(|binding| {
            Ok(TypedMatchTarget {
                binding: binding.id.get(),
                kind: match binding.thing_kind {
                    ThingKind::Entity => TypedThingKind::Entity,
                    ThingKind::Relation => TypedThingKind::Relation,
                },
                type_name: descriptor_type_name(&binding.descriptor)?.to_owned(),
                exact: binding.match_mode == MatchMode::Exact,
            })
        })
        .collect::<Result<Vec<_>, MatchError>>()?;

    let mut referenced_fields = BTreeSet::new();
    if let Some(predicate) = &request.plan.predicate {
        collect_fields(predicate, &mut referenced_fields);
    }
    referenced_fields.extend(
        orders
            .iter()
            .flat_map(|order| order.terms().iter().map(|term| term.order().field.clone())),
    );
    let field_ids = referenced_fields
        .into_iter()
        .enumerate()
        .map(|(index, field)| {
            let id = u16::try_from(index).map_err(|_| {
                unsupported(
                    "too_many_lowered_fields",
                    "typed field ordinal exceeds the provider AST range",
                )
            })?;
            Ok((field, id))
        })
        .collect::<Result<BTreeMap<_, _>, MatchError>>()?;
    let fields = field_ids
        .iter()
        .map(|(field, id)| {
            Ok(TypedFieldBinding {
                id: *id,
                owner: field.binding.get(),
                field_name: provider_attribute_name(registry, &field.field)?,
            })
        })
        .collect::<Result<Vec<_>, MatchError>>()?;

    let predicate = request
        .plan
        .predicate
        .as_ref()
        .map(|predicate| lower_predicate(registry, predicate, &field_ids))
        .transpose()?;
    Ok(LoweredGraph {
        targets,
        fields,
        predicate,
        field_ids,
    })
}

fn lower_order(
    registry: &DescriptorRegistry,
    order: &StableOrderSpec,
    field_ids: &BTreeMap<BoundFieldId, u16>,
) -> Result<Vec<TypedMatchOrder>, MatchError> {
    order
        .terms()
        .iter()
        .map(|term| {
            let order = term.order();
            let attribute = provider_attribute(registry, &order.field.field)?;
            let is_required_scalar = !attribute.is_optional
                && attribute
                    .cardinality()
                    .is_none_or(|(minimum, maximum)| {
                        minimum >= 1 && maximum == Some(1)
                    });
            if !is_required_scalar {
                return Err(MatchError::new(
                    MatchErrorCategory::UnsupportedCapability,
                    "nullable_order_field_unsupported",
                    "the selected provider cannot window by a nullable order field without filtering missing roots",
                )
                .at(MatchErrorPathSegment::Field(order.field.field.clone())));
            }
            let missing = match order.missing {
                MissingOrder::Reject => TypedMissingOrder::Reject,
                MissingOrder::First | MissingOrder::Last => {
                    return Err(MatchError::new(
                        MatchErrorCategory::UnsupportedCapability,
                        "missing_value_order_unsupported",
                        "the selected provider cannot preserve first/last missing-value ordering",
                    )
                    .at(MatchErrorPathSegment::Operation));
                }
            };
            Ok(TypedMatchOrder {
                field: *field_ids.get(&order.field).ok_or_else(|| {
                    unsupported(
                        "missing_lowered_order_field",
                        "stable order field was not assigned a typed field ordinal",
                    )
                })?,
                direction: match order.direction {
                    SortDirection::Ascending => TypedSortDirection::Ascending,
                    SortDirection::Descending => TypedSortDirection::Descending,
                },
                missing,
            })
        })
        .collect()
}

fn root_scan(
    graph: LoweredGraph,
    root: u16,
    order: Vec<TypedMatchOrder>,
    offset: Option<u64>,
    limit: Option<u64>,
) -> TypedRootScan {
    TypedRootScan {
        targets: graph.targets,
        fields: graph.fields,
        predicate: graph.predicate,
        root,
        order,
        offset,
        limit,
    }
}

fn lower_predicate(
    registry: &DescriptorRegistry,
    predicate: &MatchExpr,
    fields: &BTreeMap<BoundFieldId, u16>,
) -> Result<TypedMatchPredicate, MatchError> {
    match predicate {
        MatchExpr::FieldValue {
            field,
            operator,
            value,
        } => Ok(TypedMatchPredicate::FieldValue {
            field: field_id(fields, field)?,
            operator: lower_operator(*operator),
            value: lower_value(value),
        }),
        MatchExpr::FieldComparison {
            left,
            operator,
            right,
        } => Ok(TypedMatchPredicate::FieldComparison {
            left: field_id(fields, left)?,
            operator: lower_operator(*operator),
            right: field_id(fields, right)?,
        }),
        MatchExpr::RoleEdge {
            id,
            relation,
            role,
            player,
        } => Ok(TypedMatchPredicate::RoleEdge {
            edge: id.get(),
            relation: relation.get(),
            role_name: role.name.clone(),
            player: player.get(),
        }),
        MatchExpr::Reachable {
            relation,
            role_from,
            role_to,
            source,
            target,
            min_depth,
            max_depth,
        } => Ok(TypedMatchPredicate::Reachable {
            relation_type: registry.descriptor_type_name(relation).ok_or_else(|| {
                unsupported(
                    "missing_lowered_reachability_relation",
                    "validated reachability relation is no longer registered",
                )
                .at(MatchErrorPathSegment::Predicate)
            })?,
            role_from: role_from.name.clone(),
            role_to: role_to.name.clone(),
            source: source.get(),
            target: target.get(),
            min_depth: *min_depth,
            max_depth: *max_depth,
        }),
        MatchExpr::And { expressions } => Ok(TypedMatchPredicate::And {
            expressions: expressions
                .iter()
                .map(|expression| lower_predicate(registry, expression, fields))
                .collect::<Result<_, _>>()?,
        }),
        MatchExpr::Or { expressions } => Ok(TypedMatchPredicate::Or {
            expressions: expressions
                .iter()
                .map(|expression| lower_predicate(registry, expression, fields))
                .collect::<Result<_, _>>()?,
        }),
        MatchExpr::Not { expression } => Ok(TypedMatchPredicate::Not {
            expression: Box::new(lower_predicate(registry, expression, fields)?),
        }),
    }
}

fn field_id(fields: &BTreeMap<BoundFieldId, u16>, field: &BoundFieldId) -> Result<u16, MatchError> {
    fields.get(field).copied().ok_or_else(|| {
        unsupported(
            "missing_lowered_predicate_field",
            "predicate field was not assigned a typed field ordinal",
        )
        .at(MatchErrorPathSegment::Field(field.field.clone()))
    })
}

fn lower_operator(operator: ComparisonOp) -> TypedComparisonOperator {
    match operator {
        ComparisonOp::Equal => TypedComparisonOperator::Equal,
        ComparisonOp::NotEqual => TypedComparisonOperator::NotEqual,
        ComparisonOp::LessThan => TypedComparisonOperator::LessThan,
        ComparisonOp::LessThanOrEqual => TypedComparisonOperator::LessThanOrEqual,
        ComparisonOp::GreaterThan => TypedComparisonOperator::GreaterThan,
        ComparisonOp::GreaterThanOrEqual => TypedComparisonOperator::GreaterThanOrEqual,
        ComparisonOp::Contains => TypedComparisonOperator::Contains,
        ComparisonOp::StartsWith => TypedComparisonOperator::StartsWith,
        ComparisonOp::EndsWith => TypedComparisonOperator::EndsWith,
        ComparisonOp::Regex => TypedComparisonOperator::Regex,
    }
}

fn lower_value(value: &AttributeValue) -> TypedLiteral {
    match value {
        AttributeValue::String(value) => TypedLiteral::String(value.clone()),
        AttributeValue::Long(value) => TypedLiteral::Long(*value),
        AttributeValue::Double(value) => TypedLiteral::Double(*value),
        AttributeValue::Boolean(value) => TypedLiteral::Boolean(*value),
        AttributeValue::Date(value) => TypedLiteral::Date(value.clone()),
        AttributeValue::DateTime(value) => TypedLiteral::DateTime(value.clone()),
        AttributeValue::DateTimeTZ(value) => TypedLiteral::DateTimeTz(value.clone()),
        AttributeValue::Decimal(value) => TypedLiteral::Decimal(value.clone()),
        AttributeValue::Duration(value) => TypedLiteral::Duration(value.clone()),
    }
}

fn collect_fields(predicate: &MatchExpr, fields: &mut BTreeSet<BoundFieldId>) {
    match predicate {
        MatchExpr::FieldValue { field, .. } => {
            fields.insert(field.clone());
        }
        MatchExpr::FieldComparison { left, right, .. } => {
            fields.insert(left.clone());
            fields.insert(right.clone());
        }
        MatchExpr::And { expressions } | MatchExpr::Or { expressions } => {
            for expression in expressions {
                collect_fields(expression, fields);
            }
        }
        MatchExpr::Not { expression } => collect_fields(expression, fields),
        MatchExpr::RoleEdge { .. } | MatchExpr::Reachable { .. } => {}
    }
}

fn descriptor_type_name(descriptor: &DescriptorId) -> Result<&str, MatchError> {
    descriptor
        .as_str()
        .split_once(':')
        .map(|(_, name)| name)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            unsupported(
                "malformed_validated_descriptor",
                "validated descriptor identity is not kind-qualified",
            )
        })
}

fn provider_attribute_name(
    registry: &DescriptorRegistry,
    field: &FieldId,
) -> Result<String, MatchError> {
    Ok(provider_attribute(registry, field)?.attr_name)
}

fn provider_attribute(
    registry: &DescriptorRegistry,
    field: &FieldId,
) -> Result<crate::descriptor::OwnedAttributeDescriptor, MatchError> {
    let type_name = registry.descriptor_type_name(&field.owner).ok_or_else(|| {
        unsupported(
            "missing_lowered_field_descriptor",
            "validated field owner is no longer registered",
        )
        .at(MatchErrorPathSegment::Field(field.clone()))
    })?;
    let descriptor = registry.get(&type_name).ok_or_else(|| {
        unsupported(
            "missing_lowered_field_descriptor",
            "validated field owner is no longer registered",
        )
        .at(MatchErrorPathSegment::Field(field.clone()))
    })?;
    let attribute = match descriptor {
        TypeDescriptorRef::Entity(descriptor) => descriptor.attribute(&field.name).cloned(),
        TypeDescriptorRef::Relation(descriptor) => descriptor.attribute(&field.name).cloned(),
    }
    .ok_or_else(|| {
        unsupported(
            "missing_lowered_attribute",
            "validated field is no longer owned by its descriptor",
        )
        .at(MatchErrorPathSegment::Field(field.clone()))
    })?;
    Ok(attribute)
}

fn output_slots(output: &FetchShape) -> impl Iterator<Item = &FetchSlot> {
    let slots: Vec<_> = match output {
        FetchShape::Positional { slots } => slots.iter().collect(),
        FetchShape::Named { slots } => slots.iter().map(|named| &named.slot).collect(),
    };
    slots.into_iter()
}

fn unsupported(code: &'static str, message: &'static str) -> MatchError {
    MatchError::new(MatchErrorCategory::InvalidPlan, code, message)
        .at(MatchErrorPathSegment::Operation)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribute::ValueType;
    use crate::descriptor::{
        EntityDescriptor, OwnedAttributeDescriptor, RelationDescriptor, RoleDescriptor,
    };
    use crate::entity::Annotation;
    use crate::match_request::ids::{BindingId, RoleEdgeId, RoleId};
    use crate::match_request::model::{
        FetchShape, FetchSlot, MatchBinding, MatchOrder, MatchPlan, MatchRequest, RowCardinality,
        Window,
    };
    use crate::match_request::validation::validate_match_request;
    use crate::registry::DescriptorRegistry;
    use type_bridge_core_lib::compiler::QueryCompiler;

    fn key(name: &str) -> OwnedAttributeDescriptor {
        OwnedAttributeDescriptor {
            field_name: name.into(),
            attr_name: name.into(),
            value_type: ValueType::String,
            annotations: vec![Annotation::Key],
            is_optional: false,
            is_ordered: false,
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
                    key("name"),
                    OwnedAttributeDescriptor {
                        field_name: "nickname".into(),
                        attr_name: "nickname".into(),
                        value_type: ValueType::String,
                        annotations: Vec::new(),
                        is_optional: true,
                        is_ordered: false,
                        doc: None,
                        meta: Default::default(),
                    },
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
                owned_attributes: vec![key("name")],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        registry
            .register_relation(RelationDescriptor {
                type_name: "employment".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![key("code")],
                roles: vec![
                    RoleDescriptor {
                        role_name: "employee".into(),
                        player_type_names: vec!["person".into()],
                        ..Default::default()
                    },
                    RoleDescriptor {
                        role_name: "employer".into(),
                        player_type_names: vec!["company".into()],
                        ..Default::default()
                    },
                ],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        registry
    }

    fn bound_field(
        registry: &DescriptorRegistry,
        binding: u16,
        descriptor: &str,
        name: &str,
    ) -> BoundFieldId {
        let descriptor = registry.descriptor_id(descriptor).unwrap();
        BoundFieldId::new(
            BindingId::new(binding),
            registry.field_id(&descriptor, name).unwrap(),
        )
    }

    fn graph_request(registry: &DescriptorRegistry, relation_mode: MatchMode) -> MatchRequest {
        let person = registry.descriptor_id("person").unwrap();
        let employment = registry.descriptor_id("employment").unwrap();
        let company = registry.descriptor_id("company").unwrap();
        let person_name = bound_field(registry, 0, "person", "name");
        let company_name = bound_field(registry, 2, "company", "name");
        MatchRequest::v1(
            MatchPlan {
                bindings: vec![
                    MatchBinding {
                        id: BindingId::new(0),
                        descriptor: person,
                        thing_kind: ThingKind::Entity,
                        match_mode: MatchMode::Exact,
                    },
                    MatchBinding {
                        id: BindingId::new(1),
                        descriptor: employment.clone(),
                        thing_kind: ThingKind::Relation,
                        match_mode: relation_mode,
                    },
                    MatchBinding {
                        id: BindingId::new(2),
                        descriptor: company,
                        thing_kind: ThingKind::Entity,
                        match_mode: MatchMode::Exact,
                    },
                ],
                predicate: Some(MatchExpr::And {
                    expressions: vec![
                        MatchExpr::FieldValue {
                            field: person_name.clone(),
                            operator: ComparisonOp::StartsWith,
                            value: AttributeValue::String("Al".into()),
                        },
                        MatchExpr::FieldComparison {
                            left: person_name.clone(),
                            operator: ComparisonOp::NotEqual,
                            right: company_name.clone(),
                        },
                        MatchExpr::RoleEdge {
                            id: RoleEdgeId::new(0),
                            relation: BindingId::new(1),
                            role: RoleId::new(employment.clone(), "employee"),
                            player: BindingId::new(0),
                        },
                        MatchExpr::RoleEdge {
                            id: RoleEdgeId::new(1),
                            relation: BindingId::new(1),
                            role: RoleId::new(employment, "employer"),
                            player: BindingId::new(2),
                        },
                        MatchExpr::Or {
                            expressions: vec![
                                MatchExpr::FieldValue {
                                    field: company_name.clone(),
                                    operator: ComparisonOp::Equal,
                                    value: AttributeValue::String("Acme".into()),
                                },
                                MatchExpr::FieldValue {
                                    field: company_name,
                                    operator: ComparisonOp::Equal,
                                    value: AttributeValue::String("Globex".into()),
                                },
                            ],
                        },
                        MatchExpr::Not {
                            expression: Box::new(MatchExpr::FieldValue {
                                field: person_name.clone(),
                                operator: ComparisonOp::Equal,
                                value: AttributeValue::String("Retired".into()),
                            }),
                        },
                    ],
                }),
                allowed_cross_joins: BTreeSet::new(),
            },
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![
                        FetchSlot::One {
                            binding: BindingId::new(2),
                        },
                        FetchSlot::One {
                            binding: BindingId::new(0),
                        },
                        FetchSlot::One {
                            binding: BindingId::new(1),
                        },
                    ],
                },
                order: vec![MatchOrder {
                    field: person_name,
                    direction: SortDirection::Descending,
                    missing: MissingOrder::Reject,
                }],
                window: Window {
                    offset: 5,
                    limit: 10,
                },
                cardinality: RowCardinality::BoundedMany,
            },
        )
    }

    #[test]
    fn validated_graph_lowers_deterministically_with_owner_and_slot_identity() {
        let registry = registry();
        let validated =
            validate_match_request(&registry, graph_request(&registry, MatchMode::Subtypes))
                .unwrap();
        let first = lower_fetch_rows(&registry, &validated).unwrap();
        let second = lower_fetch_rows(&registry, &validated).unwrap();

        assert_eq!(first, second);
        assert_eq!(first.projection, vec![2, 0, 1]);
        assert!(first.distinct);
        assert_eq!(first.offset, 5);
        assert_eq!(first.limit, 10);
        assert_eq!(first.targets[0].kind, TypedThingKind::Entity);
        assert!(first.targets[0].exact);
        assert_eq!(first.targets[1].kind, TypedThingKind::Relation);
        assert!(!first.targets[1].exact);
        assert_eq!(first.fields.len(), 3);
        assert_eq!(first.order.len(), 3);
        assert!(matches!(
            first.predicate,
            Some(TypedMatchPredicate::And { .. })
        ));
    }

    #[test]
    fn exactly_one_lowers_every_public_slot_into_one_distinct_tuple_proof() {
        let registry = registry();
        let mut request = graph_request(&registry, MatchMode::Subtypes);
        let MatchOperation::FetchRows {
            window,
            cardinality,
            ..
        } = &mut request.operation
        else {
            unreachable!()
        };
        *window = Window {
            offset: 0,
            limit: 1,
        };
        *cardinality = RowCardinality::ExactlyOne;
        let validated = validate_match_request(&registry, request).unwrap();

        let LoweredMatchExecution::ExactlyOneBy {
            selection,
            evidence,
        } = lower_match_execution(&registry, &validated).unwrap()
        else {
            panic!("expected exactly-one tuple proof")
        };
        assert_eq!(selection.projection, vec![2, 0, 1]);
        assert_eq!(selection.projection, evidence.projection);
        assert_eq!(selection.targets, evidence.targets);
        assert_eq!(selection.predicate, evidence.predicate);
        assert!(selection.distinct);
        assert!(selection.order.is_empty());
        assert_eq!(selection.offset, 0);
        assert_eq!(selection.limit, 2);
        assert_eq!(evidence.limit, 1);
    }

    #[test]
    fn exact_relation_target_lowers_to_strict_typed_target() {
        let registry = registry();
        let validated =
            validate_match_request(&registry, graph_request(&registry, MatchMode::Exact)).unwrap();
        let lowered = lower_fetch_rows(&registry, &validated).unwrap();
        assert_eq!(lowered.targets[1].kind, TypedThingKind::Relation);
        assert!(lowered.targets[1].exact);
    }

    #[test]
    fn bounded_reachability_lowers_through_the_typed_ast_compiler() {
        let registry = registry();
        let person = registry.descriptor_id("person").unwrap();
        let company = registry.descriptor_id("company").unwrap();
        let relation = registry.descriptor_id("employment").unwrap();
        let request = MatchRequest::v1(
            MatchPlan {
                bindings: vec![
                    MatchBinding {
                        id: BindingId::new(0),
                        descriptor: person,
                        thing_kind: ThingKind::Entity,
                        match_mode: MatchMode::Exact,
                    },
                    MatchBinding {
                        id: BindingId::new(1),
                        descriptor: company,
                        thing_kind: ThingKind::Entity,
                        match_mode: MatchMode::Exact,
                    },
                ],
                predicate: Some(MatchExpr::Reachable {
                    relation: relation.clone(),
                    role_from: RoleId::new(relation.clone(), "employee"),
                    role_to: RoleId::new(relation, "employer"),
                    source: BindingId::new(0),
                    target: BindingId::new(1),
                    min_depth: 1,
                    max_depth: 2,
                }),
                allowed_cross_joins: BTreeSet::new(),
            },
            MatchOperation::CountBy {
                root: BindingId::new(0),
            },
        );
        let validated = validate_match_request(&registry, request).unwrap();
        let LoweredMatchExecution::CountBy { scan, .. } =
            lower_match_execution(&registry, &validated).unwrap()
        else {
            panic!("expected count scan")
        };
        assert!(matches!(
            scan.predicate.as_ref(),
            Some(TypedMatchPredicate::Reachable {
                relation_type,
                role_from,
                role_to,
                min_depth: 1,
                max_depth: 2,
                ..
            }) if relation_type == "employment"
                && role_from == "employee"
                && role_to == "employer"
        ));
        let typeql = QueryCompiler::new()
            .compile_typed_root_scan(&scan)
            .expect("typed reachability compiles");
        assert!(typeql.contains("(employee: $b0, employer: $b1) isa! employment"));
        assert!(typeql.contains("reduce $RreachableProofCount = count groupby $b0, $b1"));
    }

    #[test]
    fn lowered_field_uses_typedb_attribute_label_not_model_field_name() {
        let registry = DescriptorRegistry::new();
        registry
            .register_entity(EntityDescriptor {
                type_name: "typed-work".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeDescriptor {
                    field_name: "identity".into(),
                    attr_name: "typed-work-identity".into(),
                    ..key("identity")
                }],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        let descriptor = registry.descriptor_id("typed-work").unwrap();
        let request = MatchRequest::v1(
            MatchPlan {
                bindings: vec![MatchBinding {
                    id: BindingId::new(0),
                    descriptor,
                    thing_kind: ThingKind::Entity,
                    match_mode: MatchMode::Exact,
                }],
                predicate: None,
                allowed_cross_joins: BTreeSet::new(),
            },
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One {
                        binding: BindingId::new(0),
                    }],
                },
                order: Vec::new(),
                window: Window {
                    offset: 0,
                    limit: 2,
                },
                cardinality: RowCardinality::BoundedMany,
            },
        );
        let validated = validate_match_request(&registry, request).unwrap();
        let lowered = lower_fetch_rows(&registry, &validated).unwrap();
        assert_eq!(lowered.fields[0].field_name, "typed-work-identity");
    }

    #[test]
    fn nullable_public_order_fails_closed_in_preflight() {
        let registry = registry();
        let mut request = graph_request(&registry, MatchMode::Subtypes);
        let MatchOperation::FetchRows { order, .. } = &mut request.operation else {
            unreachable!()
        };
        *order = vec![MatchOrder {
            field: bound_field(&registry, 0, "person", "nickname"),
            direction: SortDirection::Ascending,
            missing: MissingOrder::Reject,
        }];
        let validated = validate_match_request(&registry, request).unwrap();

        let error = lower_fetch_rows(&registry, &validated).unwrap_err();
        assert_eq!(error.code().as_str(), "nullable_order_field_unsupported");
    }

    #[test]
    fn page_count_and_exists_are_rejected_by_lowerer() {
        let registry = registry();
        for operation in [
            MatchOperation::PageBy {
                root: BindingId::new(0),
                output: FetchShape::Positional {
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
                },
                order: vec![],
                window: Window {
                    offset: 0,
                    limit: 10,
                },
                include_total: false,
            },
            MatchOperation::CountBy {
                root: BindingId::new(0),
            },
            MatchOperation::ExistsBy {
                root: BindingId::new(0),
            },
        ] {
            let mut request = graph_request(&registry, MatchMode::Exact);
            request.operation = operation;
            let validated = validate_match_request(&registry, request).unwrap();
            assert_eq!(
                lower_fetch_rows(&registry, &validated)
                    .unwrap_err()
                    .code()
                    .as_str(),
                "unsupported_selected_operation"
            );
        }
    }
}
