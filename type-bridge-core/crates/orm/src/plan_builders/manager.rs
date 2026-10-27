//! Descriptor managers build native plans under their registry's authority.

use std::sync::Arc;

use type_bridge_contract::diagnostic::Diagnostic;
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::{AssertionRolePlayer, BindingId, ValueComparator};
use type_bridge_contract::query_plan::v3::{
    AttachItem, DeleteItem, DocumentRole, Expr, InsertThing, Operation, Pattern, Stage,
    ThingDocument, WriteReturns,
};
use type_bridge_contract::query_plan::{
    OrderDirection, OrderTerm, QueryOperand, QueryOutput, QueryPattern, ReduceAssignment, Reducer,
};
use type_bridge_contract::schema::{OwnsFactId, SchemaFactId};
use type_bridge_contract::value::CanonicalValue;

use crate::_descriptor::TypeDescriptorRef;
use crate::_dynamic::{
    DynamicAggregate, DynamicAttributeMap, DynamicComparisonOp, DynamicExpr,
    DynamicRolePlayerInput, DynamicSort,
};
use crate::_registry::DescriptorRegistry;
use crate::error::{OrmError, Result};
use crate::expr::SortDir;
use crate::match_request::{MatchError, MatchErrorCategory, PlanFetchStack};

use super::authority::PlanAuthority;
use super::reference::{Reference, ReferenceIdentity, reference_binding};
use super::{BuiltPlan, PlanDraft, ValueTransport};

/// Shared across owned and borrowed forms of one registry-backed manager.
#[derive(Clone)]
pub(crate) struct ManagerPlans {
    pub(crate) registry: Arc<DescriptorRegistry>,
    pub(crate) transport: ValueTransport,
}

pub(crate) fn invalid(code: &'static str, message: &'static str) -> OrmError {
    MatchError::new(MatchErrorCategory::InvalidPlan, code, message).into()
}

pub(crate) fn diagnostic(error: Diagnostic) -> OrmError {
    MatchError::new(
        MatchErrorCategory::InvalidPlan,
        error.code().as_str(),
        "Manager plan construction failed",
    )
    .into()
}

fn canonical(value: &crate::AttributeValue) -> Result<CanonicalValue> {
    crate::runtime_projection::canonical_attribute_value(value).map_err(|_| {
        invalid(
            "manager_value_invalid",
            "A manager value is outside its canonical scalar domain",
        )
    })
}

impl ManagerPlans {
    pub(crate) fn new(registry: Arc<DescriptorRegistry>) -> Self {
        Self {
            registry,
            transport: ValueTransport::Given,
        }
    }

    pub(crate) fn authority(&self) -> Result<&PlanAuthority> {
        let installed = self.registry.installed_schema_authority().ok_or_else(|| {
            invalid(
                "manager_plan_authority_unavailable",
                "A registry-backed manager requires installed schema authority",
            )
        })?;
        Ok(installed.plan_authority())
    }

    pub(crate) fn type_id(&self, name: &str) -> Result<TypeId> {
        self.authority()?;
        let kind = match self
            .registry
            .get(name)
            .ok_or_else(|| OrmError::DescriptorNotFound(name.into()))?
        {
            TypeDescriptorRef::Entity(_) => TypeKind::Entity,
            TypeDescriptorRef::Relation(_) => TypeKind::Relation,
        };
        TypeId::new(kind, name).map_err(diagnostic)
    }

    fn attributes(&self, type_id: &TypeId) -> Result<TypeDescriptorRef> {
        self.registry
            .get(type_id.label().as_str())
            .ok_or_else(|| OrmError::DescriptorNotFound(type_id.label().as_str().into()))
    }

    fn attribute(&self, type_id: &TypeId, name: &str) -> Result<AttributeId> {
        let descriptor = self
            .registry
            .get(type_id.label().as_str())
            .ok_or_else(|| OrmError::DescriptorNotFound(type_id.label().as_str().into()))?;
        let attribute = match &descriptor {
            TypeDescriptorRef::Entity(descriptor) => descriptor.attribute(name),
            TypeDescriptorRef::Relation(descriptor) => descriptor.attribute(name),
        }
        .ok_or_else(|| {
            invalid(
                "manager_field_unknown",
                "The manager field is absent from its descriptor",
            )
        })?;
        AttributeId::new(&attribute.attr_name).map_err(diagnostic)
    }

    fn ownership(&self, type_id: &TypeId, name: &str) -> Result<OwnsFactId> {
        let attribute = self.attribute(type_id, name)?;
        let installed = self
            .registry
            .installed_schema_authority()
            .expect("authority checked");
        let effective = installed
            .resolved()
            .types()
            .get(type_id)
            .and_then(|model| model.owns().get(&attribute))
            .ok_or_else(|| {
                invalid(
                    "manager_field_authority_mismatch",
                    "The manager field is absent from schema authority",
                )
            })?;
        match effective.origin().declared() {
            SchemaFactId::Owns(id) => Ok(id.clone()),
            _ => Err(invalid(
                "manager_field_authority_mismatch",
                "The field has invalid declaring authority",
            )),
        }
    }

    fn role(&self, type_id: &TypeId, name: &str) -> Result<RoleId> {
        let installed = self
            .registry
            .installed_schema_authority()
            .expect("authority checked");
        installed
            .resolved()
            .types()
            .get(type_id)
            .and_then(|model| {
                model
                    .relates()
                    .keys()
                    .find(|role| role.label().as_str() == name)
            })
            .cloned()
            .ok_or_else(|| {
                invalid(
                    "manager_role_unknown",
                    "The manager role is absent from schema authority",
                )
            })
    }

    pub(crate) fn document_roles(&self, type_id: &TypeId) -> Result<Vec<DocumentRole>> {
        let installed = self
            .registry
            .installed_schema_authority()
            .expect("authority checked");
        let model = installed.resolved().types().get(type_id).ok_or_else(|| {
            invalid(
                "manager_type_unknown",
                "The manager type is absent from schema authority",
            )
        })?;
        Ok(model
            .relates()
            .keys()
            .cloned()
            .map(|role| DocumentRole::new(role, false))
            .collect())
    }

    fn fields(
        &self,
        type_id: &TypeId,
        attributes: &DynamicAttributeMap,
    ) -> Result<Vec<(OwnsFactId, CanonicalValue)>> {
        attributes
            .iter()
            .map(|(name, value)| Ok((self.ownership(type_id, name)?, canonical(value)?)))
            .collect()
    }

    fn field_pattern(
        &self,
        draft: &mut PlanDraft,
        owner: BindingId,
        type_id: &TypeId,
        name: &str,
    ) -> Result<(BindingId, QueryPattern)> {
        let attribute = draft.bind("field").map_err(diagnostic)?;
        Ok((
            attribute,
            QueryPattern::Has {
                owner,
                attribute,
                attribute_id: self.attribute(type_id, name)?,
            },
        ))
    }

    fn links(
        &self,
        relation: BindingId,
        type_id: &TypeId,
        players: Vec<AssertionRolePlayer>,
    ) -> QueryPattern {
        let model = &self
            .registry
            .installed_schema_authority()
            .expect("authority checked")
            .resolved()
            .types()[type_id];
        if model.subtypes().is_empty() {
            return QueryPattern::Links {
                relation,
                relation_id: type_id.clone(),
                players,
            };
        }
        QueryPattern::Or {
            branches: std::iter::once(type_id)
                .chain(model.subtypes())
                .map(|candidate| {
                    vec![QueryPattern::Links {
                        relation,
                        relation_id: candidate.clone(),
                        players: players.clone(),
                    }]
                })
                .collect(),
        }
    }

    fn expression(
        &self,
        draft: &mut PlanDraft,
        thing: BindingId,
        type_id: &TypeId,
        expression: &DynamicExpr,
    ) -> Result<Vec<QueryPattern>> {
        Ok(match expression {
            DynamicExpr::Compare {
                attr_name,
                operator,
                value,
            } => {
                let (attribute, has) = self.field_pattern(draft, thing, type_id, attr_name)?;
                let right = draft.value(canonical(value)?).map_err(diagnostic)?;
                let comparator = match operator {
                    DynamicComparisonOp::Eq => ValueComparator::Equal,
                    DynamicComparisonOp::Neq => ValueComparator::NotEqual,
                    DynamicComparisonOp::Gt => ValueComparator::Greater,
                    DynamicComparisonOp::Gte => ValueComparator::GreaterOrEqual,
                    DynamicComparisonOp::Lt => ValueComparator::Less,
                    DynamicComparisonOp::Lte => ValueComparator::LessOrEqual,
                    DynamicComparisonOp::Contains => ValueComparator::Contains,
                    DynamicComparisonOp::Like => ValueComparator::Like,
                    DynamicComparisonOp::StartsWith => ValueComparator::StartsWith,
                    DynamicComparisonOp::EndsWith => ValueComparator::EndsWith,
                };
                vec![
                    has,
                    QueryPattern::Value {
                        comparator,
                        left: QueryOperand::Binding { binding: attribute },
                        right,
                    },
                ]
            }
            DynamicExpr::Iid { iid } => {
                if !type_bridge_contract::id::is_canonical_thing_iid(iid) {
                    return Err(invalid(
                        "query_plan_invalid_iid",
                        "Manager IID filters require a canonical IID",
                    ));
                }
                vec![QueryPattern::Iid {
                    binding: thing,
                    iid: QueryOperand::Literal {
                        value: CanonicalValue::String(
                            type_bridge_contract::value::CanonicalString::new(iid)
                                .map_err(diagnostic)?,
                        ),
                    },
                }]
            }
            DynamicExpr::IsNull { attr_name, is_null } => {
                let (_, has) = self.field_pattern(draft, thing, type_id, attr_name)?;
                if *is_null {
                    vec![QueryPattern::Not {
                        patterns: vec![has],
                    }]
                } else {
                    vec![has]
                }
            }
            DynamicExpr::And { exprs } => {
                let mut patterns = Vec::new();
                for child in exprs {
                    patterns.extend(self.expression(draft, thing, type_id, child)?);
                }
                patterns
            }
            DynamicExpr::Or { exprs } => vec![QueryPattern::Or {
                branches: exprs
                    .iter()
                    .map(|child| self.expression(draft, thing, type_id, child))
                    .collect::<Result<_>>()?,
            }],
            DynamicExpr::Not { expr } => vec![QueryPattern::Not {
                patterns: self.expression(draft, thing, type_id, expr)?,
            }],
            DynamicExpr::RolePlayer { role_name, expr } => {
                let role = self.role(type_id, role_name)?;
                let installed = self
                    .registry
                    .installed_schema_authority()
                    .expect("authority checked");
                let accepted = installed.resolved().roles().get(&role).ok_or_else(|| {
                    invalid(
                        "manager_role_unknown",
                        "The manager role has no player authority",
                    )
                })?;
                let player = draft.bind("player").map_err(diagnostic)?;
                let mut branches = Vec::new();
                for candidate in accepted.accepted_players() {
                    let mut branch = vec![
                        QueryPattern::Isa {
                            binding: player,
                            type_id: candidate.clone(),
                            include_subtypes: false,
                        },
                        self.links(
                            thing,
                            type_id,
                            vec![AssertionRolePlayer::new(role.clone(), player)],
                        ),
                    ];
                    branch.extend(self.expression(draft, player, candidate, expr)?);
                    branches.push(branch);
                }
                vec![QueryPattern::Or { branches }]
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn read(
        &self,
        name: &str,
        exact: bool,
        expressions: &[DynamicExpr],
        sorts: &[DynamicSort],
        limit: Option<u64>,
        offset: Option<u64>,
        role_filters: &[DynamicRolePlayerInput],
        terminal: ReadTerminal,
    ) -> Result<ManagerRead> {
        let type_id = self.type_id(name)?;
        let mut draft = PlanDraft::new(self.transport);
        let root = draft.bind("root").map_err(diagnostic)?;
        let mut patterns = vec![Pattern::Read(QueryPattern::Isa {
            binding: root,
            type_id: type_id.clone(),
            include_subtypes: !exact,
        })];
        for expression in expressions {
            if let DynamicExpr::Iid { iid } = expression {
                patterns.push(Pattern::Iid {
                    binding: root,
                    iid: iid.clone(),
                });
            } else {
                patterns.extend(
                    self.expression(&mut draft, root, &type_id, expression)?
                        .into_iter()
                        .map(Pattern::Read),
                );
            }
        }
        for (role, player) in
            self.player_bindings(&mut draft, &type_id, role_filters, !exact, &mut patterns)?
        {
            patterns.push(
                self.links(root, &type_id, vec![AssertionRolePlayer::new(role, player)])
                    .into(),
            );
        }
        let mut terms = Vec::new();
        for sort in sorts {
            let (attribute, direction) = match sort {
                DynamicSort::Attribute {
                    attr_name,
                    direction,
                } => {
                    let (attribute, has) =
                        self.field_pattern(&mut draft, root, &type_id, attr_name)?;
                    patterns.push(has.into());
                    (attribute, direction)
                }
                DynamicSort::RolePlayerAttribute {
                    role_name,
                    attr_name,
                    direction,
                } => {
                    let role = self.role(&type_id, role_name)?;
                    let installed = self
                        .registry
                        .installed_schema_authority()
                        .expect("authority checked");
                    let accepted = installed.resolved().roles().get(&role).ok_or_else(|| {
                        invalid(
                            "manager_role_unknown",
                            "The manager role has no player authority",
                        )
                    })?;
                    let player = draft.bind("sortplayer").map_err(diagnostic)?;
                    let attribute = draft.bind("sortfield").map_err(diagnostic)?;
                    let mut branches = Vec::new();
                    for candidate in accepted.accepted_players() {
                        if let Ok(attribute_id) = self.attribute(candidate, attr_name) {
                            branches.push(vec![
                                QueryPattern::Isa {
                                    binding: player,
                                    type_id: candidate.clone(),
                                    include_subtypes: false,
                                },
                                self.links(
                                    root,
                                    &type_id,
                                    vec![AssertionRolePlayer::new(role.clone(), player)],
                                ),
                                QueryPattern::Has {
                                    owner: player,
                                    attribute,
                                    attribute_id,
                                },
                            ]);
                        }
                    }
                    if branches.is_empty() {
                        return Err(invalid(
                            "manager_field_unknown",
                            "No role player owns the requested sort field",
                        ));
                    }
                    patterns.push(QueryPattern::Or { branches }.into());
                    (attribute, direction)
                }
            };
            terms.push(OrderTerm::new(
                attribute,
                match direction {
                    SortDir::Asc => OrderDirection::Ascending,
                    SortDir::Desc => OrderDirection::Descending,
                },
            ));
        }
        let mut pipeline = Vec::new();
        let output = match terminal {
            ReadTerminal::Fetch | ReadTerminal::Identity => {
                if limit.is_some() || offset.is_some() || !terms.is_empty() {
                    let iid = draft.bind("identity").map_err(diagnostic)?;
                    patterns.push(Pattern::Let {
                        binding: iid,
                        expr: Expr::Iid { binding: root },
                    });
                    terms.push(OrderTerm::new(iid, OrderDirection::Ascending));
                }
                pipeline.push(Stage::Match { patterns });
                let mut retained = vec![root];
                retained.extend(terms.iter().map(OrderTerm::binding));
                retained.sort();
                retained.dedup();
                pipeline.push(Stage::Select { bindings: retained });
                pipeline.push(Stage::Distinct);
                if !terms.is_empty() {
                    pipeline.push(Stage::Sort { terms });
                }
                if let Some(offset) = offset.filter(|offset| *offset != 0) {
                    pipeline.push(Stage::Offset { rows: offset });
                }
                if let Some(limit) = limit {
                    pipeline.push(Stage::Limit { rows: limit });
                }
                QueryOutput::Rows {
                    columns: vec![root],
                }
            }
            ReadTerminal::Count => {
                let count = draft.bind("count").map_err(diagnostic)?;
                pipeline.extend([
                    Stage::Match { patterns },
                    Stage::Select {
                        bindings: vec![root],
                    },
                    Stage::Distinct,
                    Stage::Reduce {
                        assignments: vec![ReduceAssignment::new(count, Reducer::Count, None)],
                        groups: vec![],
                    },
                ]);
                QueryOutput::Rows {
                    columns: vec![count],
                }
            }
            ReadTerminal::Exists => {
                pipeline.extend([
                    Stage::Match { patterns },
                    Stage::Select {
                        bindings: vec![root],
                    },
                    Stage::Distinct,
                ]);
                QueryOutput::Rows {
                    columns: vec![root],
                }
            }
        };
        let plan = draft
            .finish(
                self.authority()?,
                pipeline,
                if matches!(terminal, ReadTerminal::Exists) {
                    Operation::Exists
                } else {
                    Operation::Rows
                },
                output,
            )
            .map_err(diagnostic)?;
        if matches!(terminal, ReadTerminal::Fetch) {
            let descriptor = self.registry.descriptor_id(name).ok_or_else(|| {
                invalid(
                    "manager_type_unknown",
                    "The manager descriptor is absent from its registry",
                )
            })?;
            let stack = crate::match_request::plan_fetch_stack::build_unique_plan_fetch_stack(
                &self.registry,
                plan.local(),
                &[(root, descriptor)],
            )
            .map_err(diagnostic)?;
            Ok(ManagerRead::Fetch(stack))
        } else {
            Ok(ManagerRead::Scalar { plan })
        }
    }

    fn player_bindings(
        &self,
        draft: &mut PlanDraft,
        type_id: &TypeId,
        players: &[DynamicRolePlayerInput],
        include_subtypes: bool,
        patterns: &mut Vec<Pattern>,
    ) -> Result<Vec<(RoleId, BindingId)>> {
        let mut links = Vec::new();
        for player in players {
            let candidate = self.type_id(&player.player_type_name)?;
            let installed = self
                .registry
                .installed_schema_authority()
                .expect("authority checked");
            let model = installed
                .resolved()
                .types()
                .get(&candidate)
                .ok_or_else(|| {
                    invalid(
                        "manager_type_unknown",
                        "Role-player type is absent from schema authority",
                    )
                })?;
            let mut candidates = vec![candidate.clone()];
            if include_subtypes {
                candidates.extend(model.subtypes().iter().cloned());
            }
            let key_attribute = player
                .key
                .as_ref()
                .map(|(name, _)| self.attribute(&candidate, name))
                .transpose()?;
            let identity = match (&player.iid, &player.key, &key_attribute) {
                (Some(iid), _, _) => ReferenceIdentity::Iid(iid),
                (None, Some((_, value)), Some(attribute)) => ReferenceIdentity::Key {
                    attribute,
                    value: canonical(value)?,
                },
                _ => {
                    return Err(invalid(
                        "manager_player_identity_missing",
                        "A role player requires an IID or key",
                    ));
                }
            };
            let binding = reference_binding(
                draft,
                &Reference {
                    candidates: &candidates,
                    identity,
                },
                patterns,
            )
            .map_err(diagnostic)?;
            links.push((self.role(type_id, &player.role_name)?, binding));
        }
        Ok(links)
    }

    pub(crate) fn insert(
        &self,
        name: &str,
        attributes: &DynamicAttributeMap,
        players: &[DynamicRolePlayerInput],
        put: bool,
        exact_players: bool,
    ) -> Result<BuiltPlan> {
        let type_id = self.type_id(name)?;
        let mut draft = PlanDraft::new(self.transport);
        let root = draft.bind("root").map_err(diagnostic)?;
        let mut patterns = Vec::new();
        let links =
            self.player_bindings(&mut draft, &type_id, players, !exact_players, &mut patterns)?;
        let fields = self
            .fields(&type_id, attributes)?
            .into_iter()
            .map(|(field, value)| Ok((field, draft.value(value).map_err(diagnostic)?)))
            .collect::<Result<_>>()?;
        let thing = InsertThing {
            binding: root,
            type_id: type_id.clone(),
            fields,
            players: links,
        };
        let mut pipeline = Vec::new();
        if !patterns.is_empty() {
            pipeline.push(Stage::Match { patterns });
        }
        pipeline.push(if put {
            Stage::Put {
                things: vec![thing],
                include_subtypes: true,
            }
        } else {
            Stage::Insert {
                things: vec![thing],
            }
        });
        pipeline.push(Stage::Select {
            bindings: vec![root],
        });
        pipeline.push(Stage::Distinct);
        draft
            .finish(
                self.authority()?,
                pipeline,
                Operation::Write {
                    returns: WriteReturns::Rows,
                },
                QueryOutput::Rows {
                    columns: vec![root],
                },
            )
            .map_err(diagnostic)
    }

    pub(crate) fn insert_entities(
        &self,
        name: &str,
        items: &[DynamicAttributeMap],
    ) -> Result<Option<Vec<BuiltPlan>>> {
        let Some(first) = items.first() else {
            return Ok(None);
        };
        if items.iter().any(|item| {
            item.len() != first.len()
                || item
                    .iter()
                    .zip(first)
                    .any(|((name, _), (expected, _))| name != expected)
        }) {
            return Ok(None);
        }
        let type_id = self.type_id(name)?;
        let fields = self.fields(&type_id, first)?;
        let mut draft = PlanDraft::new(ValueTransport::Given);
        let root = draft.bind("root").map_err(diagnostic)?;
        let ordinal = draft.bind("ordinal").map_err(diagnostic)?;
        let ordinal_input = draft.value(CanonicalValue::Long(0)).map_err(diagnostic)?;
        let fields = fields
            .into_iter()
            .map(|(field, value)| Ok((field, draft.value(value).map_err(diagnostic)?)))
            .collect::<Result<_>>()?;
        let rows = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let mut values = Vec::with_capacity(item.len() + 1);
                values.push(Some(CanonicalValue::Long(i64::try_from(index).map_err(
                    |_| {
                        invalid(
                            "manager_batch_limit",
                            "Manager batch ordinal exceeds its scalar domain",
                        )
                    },
                )?)));
                for (_, value) in item {
                    values.push(Some(canonical(value)?));
                }
                Ok(type_bridge_contract::query_plan::InputRow::new(values))
            })
            .collect::<Result<_>>()?;
        let chunks = super::input::chunks(
            rows, 0, super::input::canonical_upper_bound,
            |row| usize::try_from(crate::projected_batch_executor::given_rows_bytes(row)?).map_err(|_| type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic::internal_failure()),
            |_, _| false,
        ).map_err(|error| OrmError::ExecutionDiagnostic(Box::new(error)))?;
        let mut chunks = chunks.into_iter();
        let first = chunks.next().expect("nonempty manager batch");
        let template = draft
            .finish_rows(
                self.authority()?,
                vec![
                    Stage::Match {
                        patterns: vec![Pattern::Let {
                            binding: ordinal,
                            expr: Expr::Operand(ordinal_input),
                        }],
                    },
                    Stage::Insert {
                        things: vec![InsertThing {
                            binding: root,
                            type_id,
                            fields,
                            players: vec![],
                        }],
                    },
                ],
                Operation::Write {
                    returns: WriteReturns::Rows,
                },
                QueryOutput::Rows {
                    columns: vec![ordinal, root],
                },
                Some(first),
            )
            .map_err(diagnostic)?;
        let mut plans = Vec::with_capacity(chunks.len() + 1);
        for rows in chunks {
            plans.push(BuiltPlan {
                plan: template
                    .local()
                    .with_given_rows(rows, self.authority()?.declared())
                    .map_err(diagnostic)?,
                verified: template.verified,
            });
        }
        plans.insert(0, template);
        Ok(Some(plans))
    }

    pub(crate) fn entity_put(
        &self,
        name: &str,
        attributes: &DynamicAttributeMap,
        exact: bool,
    ) -> Result<BuiltPlan> {
        let type_id = self.type_id(name)?;
        let fields = self.fields(&type_id, attributes)?;
        let descriptors = self.attributes(&type_id)?;
        let mut keys = Vec::new();
        let mut attached = Vec::new();
        let mut cleared = std::collections::BTreeSet::new();
        for (field, value) in fields {
            if descriptors.owned_attributes().iter().any(|attribute| {
                attribute.is_key() && attribute.attr_name == field.attribute().label().as_str()
            }) {
                keys.push((field, value));
            } else {
                cleared.insert(field.clone());
                attached.push((field, value));
            }
        }
        if keys.is_empty() {
            return self.insert(name, attributes, &[], !exact, exact);
        }
        if exact {
            for attribute in descriptors
                .owned_attributes()
                .iter()
                .filter(|attribute| !attribute.is_key())
            {
                cleared.insert(self.ownership(&type_id, &attribute.attr_name)?);
            }
        }
        super::put::keyed_entity_put(
            self.authority()?,
            self.transport,
            &super::put::KeyedPut {
                include_subtypes: !exact,
                return_document: false,
                type_id: &type_id,
                keys: &keys,
                fields: &[],
                cleared: &cleared.into_iter().collect::<Vec<_>>(),
                attached: &attached,
            },
        )
        .map_err(diagnostic)
    }

    pub(crate) fn key_expressions(
        &self,
        name: &str,
        attributes: &DynamicAttributeMap,
    ) -> Result<Vec<DynamicExpr>> {
        let type_id = self.type_id(name)?;
        Ok(self
            .attributes(&type_id)?
            .owned_attributes()
            .iter()
            .filter(|attribute| attribute.is_key())
            .filter_map(|attribute| {
                attributes
                    .iter()
                    .find(|(name, _)| *name == attribute.attr_name || *name == attribute.field_name)
                    .map(|(_, value)| DynamicExpr::Compare {
                        attr_name: attribute.attr_name.clone(),
                        operator: DynamicComparisonOp::Eq,
                        value: value.clone(),
                    })
            })
            .collect())
    }

    pub(crate) fn delete_identity(
        &self,
        name: &str,
        iid: Option<&str>,
        attributes: &DynamicAttributeMap,
        players: &[DynamicRolePlayerInput],
    ) -> Result<BuiltPlan> {
        if let Some(iid) = iid {
            return self.delete(name, iid, false, false);
        }
        let type_id = self.type_id(name)?;
        let keys = self.key_expressions(name, attributes)?;
        if keys.is_empty() && players.is_empty() {
            return Err(invalid(
                "manager_identity_missing",
                "Deletion requires an IID, key attributes, or role players",
            ));
        }
        let mut draft = PlanDraft::new(self.transport);
        let root = draft.bind("root").map_err(diagnostic)?;
        let mut patterns = vec![
            QueryPattern::Isa {
                binding: root,
                type_id: type_id.clone(),
                include_subtypes: true,
            }
            .into(),
        ];
        for key in &keys {
            patterns.extend(
                self.expression(&mut draft, root, &type_id, key)?
                    .into_iter()
                    .map(Pattern::Read),
            );
        }
        for (role, player) in
            self.player_bindings(&mut draft, &type_id, players, true, &mut patterns)?
        {
            patterns.push(
                self.links(root, &type_id, vec![AssertionRolePlayer::new(role, player)])
                    .into(),
            );
        }
        draft
            .finish(
                self.authority()?,
                vec![
                    Stage::Match { patterns },
                    Stage::Delete {
                        items: vec![DeleteItem::Thing { binding: root }],
                    },
                ],
                Operation::Write {
                    returns: WriteReturns::Matched,
                },
                QueryOutput::Rows { columns: vec![] },
            )
            .map_err(diagnostic)
    }

    pub(crate) fn read_iids(&self, name: &str, iids: &[String]) -> Result<ManagerRead> {
        let type_id = self.type_id(name)?;
        let mut draft = PlanDraft::new(ValueTransport::Given);
        let root = draft.bind("root").map_err(diagnostic)?;
        let iid = draft.bind("identity").map_err(diagnostic)?;
        let values = iids
            .iter()
            .map(|iid| {
                type_bridge_contract::value::CanonicalString::new(iid.to_ascii_lowercase())
                    .map(CanonicalValue::String)
                    .map_err(diagnostic)
            })
            .collect::<Result<Vec<_>>>()?;
        let operand = draft
            .value(values.first().cloned().ok_or_else(|| {
                invalid(
                    "manager_empty_lookup",
                    "An IID lookup requires at least one IID",
                )
            })?)
            .map_err(diagnostic)?;
        let plan = draft
            .finish_rows(
                self.authority()?,
                vec![
                    Stage::Match {
                        patterns: vec![
                            QueryPattern::Isa {
                                binding: root,
                                type_id,
                                include_subtypes: true,
                            }
                            .into(),
                            Pattern::Let {
                                binding: iid,
                                expr: Expr::Iid { binding: root },
                            },
                            QueryPattern::Value {
                                comparator: ValueComparator::Equal,
                                left: QueryOperand::Binding { binding: iid },
                                right: operand,
                            }
                            .into(),
                        ],
                    },
                    Stage::Select {
                        bindings: vec![root],
                    },
                    Stage::Distinct,
                ],
                Operation::Rows,
                QueryOutput::Rows {
                    columns: vec![root],
                },
                Some(
                    values
                        .into_iter()
                        .map(|value| {
                            type_bridge_contract::query_plan::InputRow::new(vec![Some(value)])
                        })
                        .collect(),
                ),
            )
            .map_err(diagnostic)?;
        let descriptor = self.registry.descriptor_id(name).ok_or_else(|| {
            invalid(
                "manager_type_unknown",
                "The manager descriptor is absent from its registry",
            )
        })?;
        Ok(ManagerRead::Fetch(
            crate::match_request::plan_fetch_stack::build_unique_plan_fetch_stack(
                &self.registry,
                plan.local(),
                &[(root, descriptor)],
            )
            .map_err(diagnostic)?,
        ))
    }

    pub(crate) fn aggregate(
        &self,
        name: &str,
        expressions: &[DynamicExpr],
        groups: &[String],
        aggregates: &[DynamicAggregate],
    ) -> Result<ManagerRead> {
        let type_id = self.type_id(name)?;
        let mut draft = PlanDraft::new(self.transport);
        let root = draft.bind("root").map_err(diagnostic)?;
        let mut patterns = vec![Pattern::Read(QueryPattern::Isa {
            binding: root,
            type_id: type_id.clone(),
            include_subtypes: true,
        })];
        for expression in expressions {
            patterns.extend(
                self.expression(&mut draft, root, &type_id, expression)?
                    .into_iter()
                    .map(Pattern::Read),
            );
        }
        let mut group_bindings = Vec::new();
        let mut columns = Vec::new();
        let mut keys = Vec::new();
        for (index, group) in groups.iter().enumerate() {
            let (binding, has) = self.field_pattern(&mut draft, root, &type_id, group)?;
            patterns.push(has.into());
            group_bindings.push(binding);
            columns.push(binding);
            keys.push(format!("group{index}"));
        }
        let mut assignments = Vec::new();
        for aggregate in aggregates {
            let reducer = match aggregate.function.as_str() {
                "count" => Reducer::Count,
                "sum" => Reducer::Sum,
                "min" => Reducer::Min,
                "max" => Reducer::Max,
                "mean" => Reducer::Mean,
                "median" => Reducer::Median,
                "std" => Reducer::Std,
                _ => {
                    return Err(invalid(
                        "manager_reducer_unknown",
                        "The manager aggregate has an unknown reduction",
                    ));
                }
            };
            let input = if let Some(attribute) = &aggregate.attr_name {
                let (binding, has) = self.field_pattern(&mut draft, root, &type_id, attribute)?;
                patterns.push(has.into());
                Some(binding)
            } else if reducer == Reducer::Count {
                None
            } else {
                return Err(invalid(
                    "manager_reducer_input_missing",
                    "This manager reduction requires a field",
                ));
            };
            let assigned = draft.bind("reduced").map_err(diagnostic)?;
            assignments.push(ReduceAssignment::new(assigned, reducer, input));
            columns.push(assigned);
            keys.push(aggregate.result_key.clone());
        }
        if group_bindings.is_empty()
            && assignments
                .iter()
                .any(|term| !matches!(term.reducer(), Reducer::Count | Reducer::Sum))
        {
            let anchor = draft.bind("anchor").map_err(diagnostic)?;
            patterns.push(Pattern::Let {
                binding: anchor,
                expr: Expr::Operand(QueryOperand::Literal {
                    value: CanonicalValue::Long(1),
                }),
            });
            group_bindings.push(anchor);
        }
        group_bindings.sort();
        let plan = draft
            .finish(
                self.authority()?,
                vec![
                    Stage::Match { patterns },
                    Stage::Reduce {
                        assignments,
                        groups: group_bindings,
                    },
                ],
                Operation::Rows,
                QueryOutput::Rows { columns },
            )
            .map_err(diagnostic)?;
        Ok(ManagerRead::Aggregate { plan, keys })
    }

    pub(crate) fn update(
        &self,
        name: &str,
        iid: Option<&str>,
        attributes: &DynamicAttributeMap,
        players: &[DynamicRolePlayerInput],
        exact: bool,
    ) -> Result<Option<BuiltPlan>> {
        let type_id = self.type_id(name)?;
        let mut draft = PlanDraft::new(self.transport);
        let root = draft.bind("root").map_err(diagnostic)?;
        let mut retained = vec![root];
        let mut patterns: Vec<Pattern> = vec![
            QueryPattern::Isa {
                binding: root,
                type_id: type_id.clone(),
                include_subtypes: !exact,
            }
            .into(),
        ];
        if let Some(iid) = iid {
            patterns.push(Pattern::Iid {
                binding: root,
                iid: iid.into(),
            });
        } else if type_id.kind() == TypeKind::Entity || players.is_empty() {
            let mut keys = 0;
            for attribute in self
                .attributes(&type_id)?
                .owned_attributes()
                .iter()
                .filter(|attribute| attribute.is_key())
            {
                if let Some((_, value)) = attributes
                    .iter()
                    .find(|(name, _)| *name == attribute.attr_name || *name == attribute.field_name)
                {
                    let (field, has) =
                        self.field_pattern(&mut draft, root, &type_id, &attribute.attr_name)?;
                    patterns.push(has.into());
                    patterns.push(
                        QueryPattern::Value {
                            comparator: ValueComparator::Equal,
                            left: QueryOperand::Binding { binding: field },
                            right: draft.value(canonical(value)?).map_err(diagnostic)?,
                        }
                        .into(),
                    );
                    keys += 1;
                }
            }
            if keys == 0 {
                return Err(invalid(
                    "manager_identity_missing",
                    "Mutation requires an IID or key attributes",
                ));
            }
        }
        let fields = self.fields(&type_id, attributes)?;
        let keys: std::collections::BTreeSet<_> = self
            .attributes(&type_id)?
            .owned_attributes()
            .iter()
            .filter(|field| field.is_key())
            .map(|field| field.attr_name.clone())
            .collect();
        let mut attached = Vec::new();
        let mut cleared = std::collections::BTreeSet::new();
        for (field, value) in fields {
            if !keys.contains(field.attribute().label().as_str()) {
                cleared.insert(field.clone());
                let binding = draft.bind("replacement").map_err(diagnostic)?;
                patterns.push(Pattern::Let {
                    binding,
                    expr: Expr::Operand(draft.value(value).map_err(diagnostic)?),
                });
                retained.push(binding);
                attached.push(AttachItem::Has {
                    optional: false,
                    owner: root,
                    field,
                    operand: QueryOperand::Binding { binding },
                });
            }
        }
        if exact {
            for attribute in self
                .attributes(&type_id)?
                .owned_attributes()
                .iter()
                .filter(|attribute| !attribute.is_key())
            {
                cleared.insert(self.ownership(&type_id, &attribute.attr_name)?);
            }
        }
        let mut deleted: Vec<_> = cleared
            .into_iter()
            .map(|field| DeleteItem::Has { owner: root, field })
            .collect();
        let player_start = retained.len();
        if !players.is_empty() {
            let links =
                self.player_bindings(&mut draft, &type_id, players, !exact, &mut patterns)?;
            let mut player_iids = Vec::new();
            for (role, binding) in &links {
                // A single-player role cannot contain converged aliases.
                if links
                    .iter()
                    .filter(|(candidate, _)| candidate == role)
                    .take(2)
                    .count()
                    > 1
                {
                    let iid_binding = draft.bind("player_iid").map_err(diagnostic)?;
                    patterns.push(Pattern::Let {
                        binding: iid_binding,
                        expr: Expr::Iid { binding: *binding },
                    });
                    for (previous_role, previous_iid) in &player_iids {
                        if *previous_role == role {
                            patterns.push(
                                QueryPattern::Value {
                                    comparator: ValueComparator::NotEqual,
                                    left: QueryOperand::Binding {
                                        binding: *previous_iid,
                                    },
                                    right: QueryOperand::Binding {
                                        binding: iid_binding,
                                    },
                                }
                                .into(),
                            );
                        }
                    }
                    player_iids.push((role, iid_binding));
                }
                retained.push(*binding);
                if exact {
                    attached.push(AttachItem::Links {
                        optional: false,
                        relation: root,
                        role: role.clone(),
                        player: *binding,
                    });
                } else if iid.is_none() {
                    patterns.push(
                        self.links(
                            root,
                            &type_id,
                            vec![AssertionRolePlayer::new(role.clone(), *binding)],
                        )
                        .into(),
                    );
                }
            }
            if exact {
                deleted.extend(self.document_roles(&type_id)?.into_iter().map(|role| {
                    DeleteItem::Players {
                        relation: root,
                        role: role.role,
                    }
                }));
            }
        }
        if deleted.is_empty()
            && attached.is_empty()
            && players.is_empty()
            && (iid.is_none() || type_id.kind() != TypeKind::Relation)
        {
            return Ok(None);
        }
        let writes = !deleted.is_empty() || !attached.is_empty();
        let mut pipeline = vec![Stage::Match { patterns }];
        for item in deleted {
            pipeline.push(Stage::Delete { items: vec![item] });
            pipeline.push(Stage::Select {
                bindings: retained.clone(),
            });
            pipeline.push(Stage::Distinct);
        }
        if !attached.is_empty() {
            pipeline.push(Stage::Attach { items: attached });
        }
        drop(retained.drain(1..player_start));
        pipeline.push(Stage::Select { bindings: retained });
        pipeline.push(Stage::Distinct);
        draft
            .finish(
                self.authority()?,
                pipeline,
                if writes {
                    Operation::Write {
                        returns: WriteReturns::Matched,
                    }
                } else {
                    Operation::Count
                },
                QueryOutput::Rows { columns: vec![] },
            )
            .map(Some)
            .map_err(diagnostic)
    }

    pub(crate) fn delete(
        &self,
        name: &str,
        iid: &str,
        exact: bool,
        returning: bool,
    ) -> Result<BuiltPlan> {
        let type_id = self.type_id(name)?;
        let mut draft = PlanDraft::new(self.transport);
        let root = draft.bind("root").map_err(diagnostic)?;
        let mut pipeline = vec![Stage::Match {
            patterns: vec![
                QueryPattern::Isa {
                    binding: root,
                    type_id: type_id.clone(),
                    include_subtypes: !exact,
                }
                .into(),
                Pattern::Iid {
                    binding: root,
                    iid: iid.into(),
                },
            ],
        }];
        if returning {
            pipeline.push(Stage::Fetch(ThingDocument::new(
                root,
                self.document_roles(&type_id)?,
            )));
        }
        pipeline.push(Stage::Delete {
            items: vec![DeleteItem::Thing { binding: root }],
        });
        draft
            .finish(
                self.authority()?,
                pipeline,
                Operation::Write {
                    returns: if returning {
                        WriteReturns::Rows
                    } else {
                        WriteReturns::Matched
                    },
                },
                QueryOutput::Rows { columns: vec![] },
            )
            .map_err(diagnostic)
    }

    pub(crate) fn delete_many(
        &self,
        name: &str,
        iids: &[String],
        exact: bool,
        returning: bool,
    ) -> Result<BuiltPlan> {
        self.identity_batch(name, iids, exact, returning, true)
    }

    pub(crate) fn capture_many(
        &self,
        name: &str,
        iids: &[String],
        exact: bool,
    ) -> Result<BuiltPlan> {
        self.identity_batch(name, iids, exact, true, false)
    }

    fn identity_batch(
        &self,
        name: &str,
        iids: &[String],
        exact: bool,
        returning: bool,
        delete: bool,
    ) -> Result<BuiltPlan> {
        let type_id = self.type_id(name)?;
        let exact = exact
            || self
                .registry
                .installed_schema_authority()
                .expect("authority checked")
                .resolved()
                .types()[&type_id]
                .subtypes()
                .is_empty();
        let mut draft = PlanDraft::new(self.transport);
        let root = draft.bind("root").map_err(diagnostic)?;
        let iid = draft.bind("identity").map_err(diagnostic)?;
        let values = iids
            .iter()
            .map(|iid| {
                if !type_bridge_contract::id::is_canonical_thing_iid(iid) {
                    return Err(invalid(
                        "query_plan_invalid_iid",
                        "Deletion requires canonical IIDs",
                    ));
                }
                type_bridge_contract::value::CanonicalString::new(iid.to_ascii_lowercase())
                    .map(CanonicalValue::String)
                    .map_err(diagnostic)
            })
            .collect::<Result<Vec<_>>>()?;
        let input = draft
            .value(values.first().cloned().ok_or_else(|| {
                invalid(
                    "manager_empty_delete",
                    "A deletion plan requires at least one IID",
                )
            })?)
            .map_err(diagnostic)?;
        let mut patterns = vec![
            QueryPattern::Isa {
                binding: root,
                type_id: type_id.clone(),
                include_subtypes: !exact,
            }
            .into(),
        ];
        if self.transport == ValueTransport::Given {
            patterns.extend([
                Pattern::Let {
                    binding: iid,
                    expr: Expr::Iid { binding: root },
                },
                QueryPattern::Value {
                    comparator: ValueComparator::Equal,
                    left: QueryOperand::Binding { binding: iid },
                    right: input,
                }
                .into(),
            ]);
        } else if let [value] = values.as_slice() {
            let CanonicalValue::String(value) = value else {
                unreachable!("validated IID");
            };
            patterns.push(Pattern::Iid {
                binding: root,
                iid: value.as_str().to_owned(),
            });
        } else {
            patterns.push(
                QueryPattern::Or {
                    branches: values
                        .iter()
                        .map(|value| {
                            vec![QueryPattern::Iid {
                                binding: root,
                                iid: QueryOperand::Literal {
                                    value: value.clone(),
                                },
                            }]
                        })
                        .collect(),
                }
                .into(),
            );
        }
        let mut pipeline = vec![Stage::Match { patterns }];
        if returning {
            pipeline.push(Stage::Fetch(ThingDocument::new(
                root,
                self.document_roles(&type_id)?
                    .into_iter()
                    .map(|role| DocumentRole::collected(role.role))
                    .collect(),
            )));
        }
        if delete {
            pipeline.push(Stage::Delete {
                items: vec![DeleteItem::Thing { binding: root }],
            });
        }
        draft
            .finish_rows(
                self.authority()?,
                pipeline,
                if delete {
                    Operation::Write {
                        returns: if returning {
                            WriteReturns::Rows
                        } else {
                            WriteReturns::Matched
                        },
                    }
                } else {
                    Operation::Rows
                },
                QueryOutput::Rows { columns: vec![] },
                (self.transport == ValueTransport::Given).then(|| {
                    values
                        .into_iter()
                        .map(|value| {
                            type_bridge_contract::query_plan::InputRow::new(vec![Some(value)])
                        })
                        .collect()
                }),
            )
            .map_err(diagnostic)
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ReadTerminal {
    Fetch,
    Identity,
    Count,
    Exists,
}

pub(crate) enum ManagerRead {
    Fetch(PlanFetchStack),
    Scalar { plan: BuiltPlan },
    Aggregate { plan: BuiltPlan, keys: Vec<String> },
}

impl ManagerRead {
    pub(crate) fn plans(&self) -> &[type_bridge_contract::query_plan::v3::QueryPlanV3] {
        match self {
            Self::Fetch(stack) => stack.plans(),
            Self::Scalar { plan, .. } | Self::Aggregate { plan, .. } => {
                std::slice::from_ref(plan.local())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaf_manager_reads_use_one_shared_plan_and_returning_variants_have_identical_bytes() {
        use type_bridge_contract::fingerprint::SemanticProfileId;
        use type_bridge_contract::projection::{
            BindingTarget, ProjectionConfig, ProjectionHandler,
        };
        use type_bridge_contract::schema::DocumentId;
        use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
        let declared = normalize_documents(
            &SchemaDocumentSet::parse([(
                DocumentId::new("manager-parity.yaml").unwrap(),
                "format: typebridge.schema/v2\nentities:\n  person: {}\n",
            )])
            .unwrap(),
        )
        .unwrap();
        let resolved = resolve(
            &declared,
            &SemanticProfileId::new("typedb-3.13.6/v1").unwrap(),
        )
        .unwrap();
        let installed = crate::InstalledRuntimeProjection::try_new(
            project(
                &resolved,
                BindingTarget::Rust,
                &ProjectionConfig::rust(),
                &[ProjectionHandler::rust_v1()],
                &[],
            )
            .unwrap(),
        )
        .unwrap();
        let plans = ManagerPlans::new(installed.manager_registry().unwrap());
        let read = plans
            .read(
                "person",
                false,
                &[],
                &[],
                None,
                None,
                &[],
                ReadTerminal::Fetch,
            )
            .unwrap();
        assert_eq!(read.plans().len(), 1);
        let generic = plans
            .delete_many("person", &["0x10".into()], false, true)
            .unwrap();
        let exact = plans
            .delete_many("person", &["0x10".into()], true, true)
            .unwrap();
        assert_eq!(
            generic.local().canonical_bytes().unwrap(),
            exact.local().canonical_bytes().unwrap()
        );
    }

    #[test]
    fn registry_without_authority_refuses_reads_and_mutations() {
        let manager = ManagerPlans::new(Arc::new(DescriptorRegistry::new()));
        let read = manager
            .read(
                "missing",
                false,
                &[],
                &[],
                None,
                None,
                &[],
                ReadTerminal::Fetch,
            )
            .err()
            .expect("missing authority refuses before reading descriptors");
        let insert = manager
            .insert("missing", &vec![], &[], false, false)
            .expect_err("missing authority refuses insert");
        let delete = manager
            .delete("missing", "0x1e00000000000000000001", true, false)
            .expect_err("missing authority refuses delete");
        for error in [read, insert, delete] {
            let OrmError::Match(error) = error else {
                panic!("refusal must be typed");
            };
            assert_eq!(error.code().as_str(), "manager_plan_authority_unavailable");
        }
    }
}
