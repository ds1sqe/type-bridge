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
        player: AssertionRolePlayer,
        include_subtypes: bool,
    ) -> Result<QueryPattern> {
        let schema = self
            .registry
            .installed_schema_authority()
            .expect("authority checked")
            .resolved();
        let model = &schema.types()[type_id];
        let mut branches = Vec::new();
        for candidate in
            std::iter::once(type_id).chain(model.subtypes().iter().filter(|_| include_subtypes))
        {
            let roles = schema.types()[candidate]
                .relates()
                .iter()
                .filter(|(role, effective)| {
                    *role == player.role() || effective.replaced_roles().contains(player.role())
                })
                .map(|(role, _)| role);
            let before = branches.len();
            for role in roles {
                branches.push(vec![QueryPattern::Links {
                    relation,
                    relation_id: candidate.clone(),
                    players: vec![AssertionRolePlayer::new(role.clone(), player.player())],
                }]);
            }
            if branches.len() == before {
                return Err(OrmError::ExecutionDiagnostic(Box::new(
                    type_bridge_contract::sdk_diagnostic::SdkExecutionDiagnostic::internal_failure(
                    ),
                )));
            }
        }
        if branches.len() == 1 {
            Ok(branches.pop().unwrap().pop().unwrap())
        } else {
            Ok(QueryPattern::Or { branches })
        }
    }

    fn expression(
        &self,
        draft: &mut PlanDraft,
        thing: BindingId,
        type_id: &TypeId,
        expression: &DynamicExpr,
        include_subtypes: bool,
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
                    patterns.extend(self.expression(
                        draft,
                        thing,
                        type_id,
                        child,
                        include_subtypes,
                    )?);
                }
                patterns
            }
            DynamicExpr::Or { exprs } => vec![QueryPattern::Or {
                branches: exprs
                    .iter()
                    .map(|child| self.expression(draft, thing, type_id, child, include_subtypes))
                    .collect::<Result<_>>()?,
            }],
            DynamicExpr::Not { expr } => vec![QueryPattern::Not {
                patterns: self.expression(draft, thing, type_id, expr, include_subtypes)?,
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
                            AssertionRolePlayer::new(role.clone(), player),
                            include_subtypes,
                        )?,
                    ];
                    branch.extend(self.expression(draft, player, candidate, expr, false)?);
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
                    self.expression(&mut draft, root, &type_id, expression, !exact)?
                        .into_iter()
                        .map(Pattern::Read),
                );
            }
        }
        for (role, player) in
            self.player_bindings(&mut draft, &type_id, role_filters, !exact, &mut patterns)?
        {
            patterns.push(
                self.links(
                    root,
                    &type_id,
                    AssertionRolePlayer::new(role, player),
                    !exact,
                )?
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
                                    AssertionRolePlayer::new(role.clone(), player),
                                    !exact,
                                )?,
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
                self.expression(&mut draft, root, &type_id, key, true)?
                    .into_iter()
                    .map(Pattern::Read),
            );
        }
        for (role, player) in
            self.player_bindings(&mut draft, &type_id, players, true, &mut patterns)?
        {
            patterns.push(
                self.links(root, &type_id, AssertionRolePlayer::new(role, player), true)?
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
                self.expression(&mut draft, root, &type_id, expression, true)?
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
                            AssertionRolePlayer::new(role.clone(), *binding),
                            true,
                        )?
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
        if !matches!(pipeline.as_slice(), [.., Stage::Select { bindings }, Stage::Distinct] if bindings == &retained)
        {
            pipeline.push(Stage::Select { bindings: retained });
            pipeline.push(Stage::Distinct);
        }
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

    fn returning_roles(&self, type_id: &TypeId, exact: bool) -> Result<Vec<DocumentRole>> {
        let resolved = self
            .registry
            .installed_schema_authority()
            .expect("authority checked")
            .resolved();
        let model = resolved.types().get(type_id).ok_or_else(|| {
            invalid(
                "manager_type_unknown",
                "The manager type is absent from schema authority",
            )
        })?;
        let mut roles = model.relates().clone();
        if !exact {
            for child in model.subtypes() {
                roles.extend(resolved.types()[child].relates().clone());
            }
        }
        Ok(roles
            .into_iter()
            .map(|(role, projection)| {
                if projection.collection_mode().is_unordered() {
                    DocumentRole::collected(role)
                } else {
                    DocumentRole::new(role, false)
                }
            })
            .collect())
    }

    pub(crate) fn write_many(
        &self,
        name: &str,
        mut kind: super::bulk_write::WriteKind,
        items: &[(
            Option<&str>,
            &DynamicAttributeMap,
            &[DynamicRolePlayerInput],
        )],
    ) -> Result<super::bulk_write::WriteProgram> {
        use super::bulk_write::{WriteRow, WriteShape};
        let type_id = self.type_id(name)?;
        let keys = self
            .attributes(&type_id)?
            .owned_attributes()
            .iter()
            .filter(|field| field.is_key())
            .map(|field| self.ownership(&type_id, &field.attr_name))
            .collect::<Result<Vec<_>>>()?;
        let fields = items
            .iter()
            .map(|(iid, attributes, _)| {
                if matches!(
                    kind,
                    super::bulk_write::WriteKind::Delete
                        | super::bulk_write::WriteKind::DeleteReturning
                ) {
                    if iid.is_some() {
                        return Ok(Vec::new());
                    }
                    return self
                        .attributes(&type_id)?
                        .owned_attributes()
                        .iter()
                        .filter(|field| field.is_key())
                        .filter_map(|field| {
                            attributes
                                .iter()
                                .find(|(name, _)| {
                                    name == &field.attr_name || name == &field.field_name
                                })
                                .map(|(_, value)| (field, value))
                        })
                        .map(|(field, value)| {
                            Ok((
                                self.ownership(&type_id, &field.attr_name)?,
                                canonical(value)?,
                            ))
                        })
                        .collect();
                }
                let mut fields = self.fields(&type_id, attributes)?;
                if kind == super::bulk_write::WriteKind::Update {
                    fields.retain(|(field, _)| !keys.contains(field));
                }
                Ok(fields)
            })
            .collect::<Result<Vec<_>>>()?;
        let supplied_keys = fields
            .iter()
            .map(|fields| {
                fields
                    .iter()
                    .filter(|(field, _)| keys.contains(field))
                    .map(|(field, _)| field.clone())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        if kind == super::bulk_write::WriteKind::PartialPut
            && supplied_keys.iter().all(Vec::is_empty)
        {
            kind = super::bulk_write::WriteKind::MatchPut;
        }
        let candidates = items
            .iter()
            .map(|(_, _, players)| {
                players
                    .iter()
                    .map(|player| {
                        self.type_id(&player.player_type_name).map(|id| {
                            let mut candidates = vec![id.clone()];
                            if matches!(
                                kind,
                                super::bulk_write::WriteKind::MatchPut
                                    | super::bulk_write::WriteKind::Insert
                                    | super::bulk_write::WriteKind::Delete
                                    | super::bulk_write::WriteKind::DeleteReturning
                            ) {
                                candidates.extend(
                                    self.registry
                                        .installed_schema_authority()
                                        .expect("checked authority")
                                        .resolved()
                                        .types()[&id]
                                        .subtypes()
                                        .iter()
                                        .cloned(),
                                );
                            }
                            candidates
                        })
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .collect::<Result<Vec<_>>>()?;
        let key_attributes = items
            .iter()
            .zip(&candidates)
            .map(|((_, _, players), candidates)| {
                players
                    .iter()
                    .zip(candidates)
                    .map(|(player, types)| {
                        player
                            .key
                            .as_ref()
                            .map(|(name, _)| self.attribute(&types[0], name))
                            .transpose()
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .collect::<Result<Vec<_>>>()?;
        let references = items
            .iter()
            .zip(&candidates)
            .zip(&key_attributes)
            .map(|(((_, _, players), candidates), attributes)| {
                players
                    .iter()
                    .zip(candidates)
                    .zip(attributes)
                    .map(|((player, candidates), attribute)| {
                        let identity = match (&player.iid, &player.key, attribute) {
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
                        Ok((
                            self.role(&type_id, &player.role_name)?,
                            Reference {
                                candidates,
                                identity,
                            },
                        ))
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .collect::<Result<Vec<_>>>()?;
        let rows = items
            .iter()
            .zip(&fields)
            .zip(&references)
            .map(|(((iid, _, _), fields), players)| WriteRow {
                iid: *iid,
                fields,
                players,
            })
            .collect::<Vec<_>>();
        let descriptor = self.attributes(&type_id)?;
        let keys = descriptor
            .owned_attributes()
            .iter()
            .filter(|field| field.is_key())
            .map(|field| self.ownership(&type_id, &field.attr_name))
            .collect::<Result<Vec<_>>>()?;
        let keys = if kind == super::bulk_write::WriteKind::PartialPut
            && supplied_keys.windows(2).all(|pair| pair[0] == pair[1])
        {
            supplied_keys.first().cloned().unwrap_or(keys)
        } else {
            keys
        };
        let cleared_fields = descriptor
            .owned_attributes()
            .iter()
            .filter(|field| !field.is_key() && !field.is_ordered)
            .map(|field| self.ownership(&type_id, &field.attr_name))
            .collect::<Result<Vec<_>>>()?;
        let document_roles = if kind == super::bulk_write::WriteKind::DeleteReturning {
            self.returning_roles(&type_id, false)?
        } else {
            self.document_roles(&type_id)?
        };
        let cleared_roles = document_roles
            .iter()
            .map(|role| role.role.clone())
            .collect::<Vec<_>>();
        super::bulk_write::write_many(
            self.authority()?,
            kind,
            WriteShape {
                type_id: &type_id,
                keys: &keys,
                cleared_fields: &cleared_fields,
                cleared_roles: &cleared_roles,
                document_roles,
                include_subtypes: matches!(
                    kind,
                    super::bulk_write::WriteKind::PartialPut
                        | super::bulk_write::WriteKind::PartialUpdate
                ),
                return_documents: !matches!(
                    kind,
                    super::bulk_write::WriteKind::PartialPut
                        | super::bulk_write::WriteKind::MatchPut
                        | super::bulk_write::WriteKind::PartialUpdate
                        | super::bulk_write::WriteKind::Delete
                ),
            },
            &rows,
        )
        .map_err(diagnostic)
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
                self.returning_roles(&type_id, exact)?,
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
    fn inherited_key_order_and_write_ownership_keep_their_canonical_identities() {
        use crate::match_request::{
            MissingOrder, RowCardinality, SessionHandle, SortDirection, Window,
        };
        use sha2::{Digest, Sha256};
        use type_bridge_contract::fingerprint::SemanticProfileId;
        use type_bridge_contract::projection::{
            BindingTarget, ProjectionConfig, ProjectionHandler,
        };
        use type_bridge_contract::schema::DocumentId;
        use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
        let declared = normalize_documents(&SchemaDocumentSet::parse([(
            DocumentId::new("inherited-order.yaml").unwrap(),
            "format: typebridge.schema/v2\nattributes:\n  name: { value: string }\nentities:\n  person:\n    owns:\n      name: { key: true }\n  employee: { sub: person }\n",
        )]).unwrap()).unwrap();
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
        let registry = installed.manager_registry().unwrap();
        let session = SessionHandle::new(Arc::clone(&registry));
        let employee = session.exact("employee").unwrap();
        let direct = employee.field("name").unwrap();
        let inherited = employee.field_owned_by("person", "name").unwrap();
        assert_eq!(inherited.field_id(), direct.field_id());
        let query = session
            .query(session.positional([employee.one()]).unwrap())
            .unwrap();
        let mut bytes = Vec::new();
        for field in [direct, inherited] {
            let request = query
                .validate_fetch_rows(
                    &[field.order(SortDirection::Ascending, MissingOrder::Reject)],
                    Window {
                        offset: 0,
                        limit: 2,
                    },
                    RowCardinality::BoundedMany,
                )
                .unwrap();
            assert_eq!(request.stable_order().terms().len(), 1);
            let stack =
                crate::match_request::plan_adapter::adapt_match_request(&registry, &request)
                    .unwrap();
            let Stage::Match { patterns } = &stack.plans[0].pipeline()[0] else {
                panic!("match prefix")
            };
            assert_eq!(patterns.iter().filter(|pattern| matches!(pattern,
                Pattern::Read(QueryPattern::Has { attribute_id, .. }) if attribute_id.label().as_str() == "name"
            )).count(), 1);
            let sort = stack.plans[0]
                .pipeline()
                .iter()
                .find_map(|stage| match stage {
                    Stage::Sort { terms } => Some(terms),
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                sort.len(),
                2,
                "one public field and the existing IID tie breaker"
            );
            bytes.push(type_bridge_contract::codec::to_canonical_json(&stack.plans).unwrap());
        }
        assert_eq!(bytes[0], bytes[1]);
        assert_eq!(
            format!("{:x}", Sha256::digest(&bytes[0])),
            "c4c5795900f57813cbd0ae963ab8a8d52e167038d98d30f1be899d6862a63726"
        );
        let plans = ManagerPlans::new(registry);
        let insert = plans
            .insert(
                "employee",
                &vec![("name".into(), crate::AttributeValue::String("Ada".into()))],
                &[],
                false,
                true,
            )
            .unwrap();
        let Stage::Insert { things } = &insert.local().pipeline()[0] else {
            panic!("insert stage")
        };
        assert_eq!(things[0].fields[0].0.owner().label().as_str(), "person");
    }

    #[test]
    fn clear_only_updates_keep_one_projection_after_each_delete() {
        use sha2::{Digest, Sha256};
        use type_bridge_contract::fingerprint::SemanticProfileId;
        use type_bridge_contract::projection::{
            BindingTarget, ProjectionConfig, ProjectionHandler,
        };
        use type_bridge_contract::schema::DocumentId;
        use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
        let declared = normalize_documents(
            &SchemaDocumentSet::parse([(
                DocumentId::new("clear-update.yaml").unwrap(),
                "format: typebridge.schema/v2\nattributes:\n  note: { value: string }\n  tag: { value: string }\nentities:\n  person:\n    owns:\n      note: { card: { min: 0, max: 1 } }\n      tag: { card: { min: 0, max: 1 } }\nrelations:\n  membership:\n    owns:\n      note: { card: { min: 0, max: 1 } }\n      tag: { card: { min: 0, max: 1 } }\n    relates:\n      member: { card: { min: 1, max: 1 } }\nplays:\n  person:\n    membership: [member]\n",
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
        let mut digests = Vec::new();
        for name in ["person", "membership"] {
            for attributes in [
                vec![],
                vec![("note".into(), crate::AttributeValue::String("after".into()))],
            ] {
                let plan = plans
                    .update(name, Some("0x10"), &attributes, &[], true)
                    .unwrap()
                    .unwrap();
                let stages = plan.local().pipeline();
                let [
                    Stage::Match { .. },
                    Stage::Delete { .. },
                    Stage::Select { bindings: first },
                    Stage::Distinct,
                    Stage::Delete { .. },
                    Stage::Select { bindings: second },
                    Stage::Distinct,
                    tail @ ..,
                ] = stages
                else {
                    panic!("each delete must retain its deduplication pair: {stages:?}");
                };
                assert_eq!(first, second);
                if attributes.is_empty() {
                    assert_eq!(first.len(), 1);
                    assert!(tail.is_empty(), "clear-only already selects the root");
                } else {
                    assert_eq!(first.len(), 2);
                    assert!(
                        matches!(tail, [Stage::Attach { .. }, Stage::Select { bindings }, Stage::Distinct] if bindings == &first[..1])
                    );
                }
                digests.push(format!(
                    "{:x}",
                    Sha256::digest(plan.local().canonical_bytes().unwrap())
                ));
            }
        }
        assert_eq!(
            digests,
            [
                "9036347ec757cdef5445d7bf9aa25b2a0e7eea7d8ad3c600dbe0a33cea4d54c5",
                "5c8876b7a256fb61abb6c0404f852a8e79f298155195d640d20ed3742299d671",
                "bd9b9adc58609d815addadb097c424efca90383c0db698cc6e6202f3e5ce9eec",
                "53b0015b36d771158d4ef3ed76f7e91fcdf5ac3fa3ec700197a4e9ab56879578",
            ]
        );
    }

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

    fn read_fixture() -> ManagerPlans {
        use type_bridge_contract::fingerprint::SemanticProfileId;
        use type_bridge_contract::projection::{
            BindingTarget, ProjectionConfig, ProjectionHandler,
        };
        use type_bridge_contract::schema::DocumentId;
        use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};
        let declared = normalize_documents(
            &SchemaDocumentSet::parse([(
                DocumentId::new("manager-fetch.yaml").unwrap(),
                r#"format: typebridge.schema/v2
attributes:
  note: { value: string }
  identifier: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
  employee:
    sub: person
    owns:
      note: { card: { min: 0, max: 1 } }
relations:
  membership:
    owns:
      note: { card: { min: 0, max: 1 } }
    relates:
      member: { card: { min: 1, max: 1 } }
  employment:
    sub: membership
    relates:
      worker: { as: member, card: 1 }
plays:
  person:
    membership: [member]
    employment: [worker]
"#,
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
        ManagerPlans::new(installed.manager_registry().unwrap())
    }

    #[test]
    fn unique_manager_fetches_keep_witnesses_and_partition_baselines() {
        let plans = read_fixture();
        let role_lookup = plans
            .read(
                "membership",
                true,
                &[DynamicExpr::RolePlayer {
                    role_name: "member".into(),
                    expr: Box::new(DynamicExpr::Iid { iid: "0x10".into() }),
                }],
                &[],
                Some(1),
                None,
                &[],
                ReadTerminal::Fetch,
            )
            .unwrap();
        assert_eq!(
            role_lookup.plans().len(),
            1,
            "non-selected role witnesses keep one complete fetch"
        );
        assert!(role_lookup.plans()[0].fetch_document_shape().is_some());
        let heterogeneous = plans
            .read(
                "person",
                false,
                &[],
                &[],
                Some(1),
                None,
                &[],
                ReadTerminal::Fetch,
            )
            .unwrap();
        assert_eq!(
            heterogeneous.plans().len(),
            4,
            "selection, mapping and two hydration partitions"
        );
        let ManagerRead::Fetch(stack) = heterogeneous else {
            panic!("model fetch")
        };
        assert_eq!(
            stack.selection,
            Some(0),
            "split hydration keeps its independent selection proof"
        );
    }

    #[cfg(feature = "typedb")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires a live TypeDB (TYPE_BRIDGE_TRANSACTION_LIVE_ADDRESS / TYPEDB_HTTP_PORT)"]
    async fn specialized_role_manager_callers_preserve_exact_and_inclusive_mutations() {
        use crate::_manager::{DynamicEntityManager, DynamicRelationManager};
        use crate::{AttributeValue, ConnectOptions, Database, TxType};
        let address = std::env::var("TYPE_BRIDGE_TRANSACTION_LIVE_ADDRESS").unwrap();
        let name = crate::live_database::name("manager_roles");
        let db = Database::connect_with_options(
            &address,
            &name,
            "admin",
            "password",
            ConnectOptions {
                http_port: std::env::var("TYPEDB_HTTP_PORT").unwrap().parse().unwrap(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        db.create_database().await.unwrap();
        db.execute_raw("define attribute identifier, value string; attribute note, value string; entity person, owns identifier @key, plays membership:member, plays employment:worker; entity employee sub person, owns note @card(0..1); relation membership, relates member @card(1), owns note @card(0..1); relation employment sub membership, relates worker as member @card(1);", TxType::Schema).await.unwrap();
        let plans = read_fixture();
        let registry = &plans.registry;
        let people = DynamicEntityManager::from_registry(
            &db,
            Arc::clone(registry),
            registry.entity("person").unwrap(),
        );
        let iid = people
            .insert(&vec![(
                "identifier".into(),
                AttributeValue::String("P".into()),
            )])
            .await
            .unwrap();
        let player = |role: &str| DynamicRolePlayerInput {
            role_name: role.into(),
            player_type_name: "person".into(),
            iid: Some(iid.clone()),
            key: None,
        };
        let base = DynamicRelationManager::from_registry(
            &db,
            Arc::clone(registry),
            registry.relation("membership").unwrap(),
        );
        let child = DynamicRelationManager::from_registry(
            &db,
            Arc::clone(registry),
            registry.relation("employment").unwrap(),
        );
        let membership = base.insert(&vec![], &[player("member")]).await.unwrap();
        let employment = child.insert(&vec![], &[player("worker")]).await.unwrap();
        let predicate = DynamicExpr::RolePlayer {
            role_name: "member".into(),
            expr: Box::new(DynamicExpr::Iid { iid: iid.clone() }),
        };
        let expected = std::collections::BTreeSet::from([membership.clone(), employment.clone()]);
        let identities = |rows: Vec<crate::_dynamic::DynamicRelationRow>| {
            rows.into_iter()
                .map(|row| row.iid.unwrap())
                .collect::<std::collections::BTreeSet<_>>()
        };
        assert_eq!(
            identities(
                base.get_exact_with_query(std::slice::from_ref(&predicate), &[], Some(1), None)
                    .await
                    .unwrap()
            ),
            std::collections::BTreeSet::from([membership.clone()])
        );
        assert_eq!(
            identities(
                base.get_with_query(std::slice::from_ref(&predicate), &[], None, None)
                    .await
                    .unwrap()
            ),
            expected
        );
        assert_eq!(
            base.count_exact_with_query(std::slice::from_ref(&predicate))
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            base.count_with_query(std::slice::from_ref(&predicate))
                .await
                .unwrap(),
            2
        );
        let sort = DynamicSort::RolePlayerAttribute {
            role_name: "member".into(),
            attr_name: "identifier".into(),
            direction: SortDir::Asc,
        };
        assert_eq!(
            identities(base.get_with_query(&[], &[sort], None, None).await.unwrap()),
            expected
        );
        assert_eq!(
            identities(
                base.get_with_role_filters(&[], &[player("member")])
                    .await
                    .unwrap()
            ),
            expected
        );
        let note = |value: &str| vec![("note".into(), AttributeValue::String(value.into()))];
        base.update_exact(&membership, &note("exact"), &[player("member")])
            .await
            .unwrap();
        assert!(
            base.get_by_iid_exact(&membership).await.unwrap()[0]
                .attributes
                .contains(&note("exact")[0])
        );
        assert!(
            !child.get_by_iid_exact(&employment).await.unwrap()[0]
                .attributes
                .contains(&note("exact")[0])
        );
        base.update(None, &note("inclusive"), &[player("member")])
            .await
            .unwrap();
        for (manager, iid) in [(&base, &membership), (&child, &employment)] {
            assert!(
                manager.get_by_iid_exact(iid).await.unwrap()[0]
                    .attributes
                    .contains(&note("inclusive")[0])
            );
        }
        base.delete_by_iid_exact(&membership).await.unwrap();
        assert_eq!(base.count_exact().await.unwrap(), 0);
        assert_eq!(child.count_exact().await.unwrap(), 1);
        base.insert(&vec![], &[player("member")]).await.unwrap();
        base.delete_identity(None, &vec![], &[player("member")])
            .await
            .unwrap();
        assert_eq!(base.count_exact().await.unwrap(), 0);
        assert_eq!(child.count_exact().await.unwrap(), 0);
        db.delete_database().await.unwrap();
        db.close().unwrap();
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
