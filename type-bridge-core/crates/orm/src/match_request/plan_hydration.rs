//! Shared pure hydration, coalescing, order and predicate validation.

use crate::error::PlanExecutionError;
use crate::match_request::descriptor_plan::adapt_value;
use crate::plan_lowering::failure;
use crate::value::AttributeValue;
use regex::Regex;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory};
use type_bridge_contract::id::{AttributeId, RoleId, TypeId, TypeKind, is_canonical_thing_iid};
use type_bridge_contract::query_model_evidence::{
    HydratedRow, HydrationAttributeEvidence, HydrationGraph, HydrationNode, HydrationNodeId,
    HydrationNodeKind, HydrationReference, HydrationRoleEvidence, HydrationSlot,
};
use type_bridge_contract::query_model_limits::ModelReplyDecodeLimits;
use type_bridge_contract::query_plan::{
    HydrationBinding, HydrationDescriptor, HydrationField, HydrationProjection, HydrationRole,
    ModelComparator, ModelField, ModelPredicate, ModelStableOrder, ModelValue,
};
use type_bridge_contract::value::ValueTypeTag;
use unicase::UniCase;

type ExecResult<T> = Result<T, PlanExecutionError>;

#[derive(Clone, Debug)]
pub(crate) struct Solution {
    pub(crate) bindings: BTreeMap<u16, String>,
}

pub(crate) struct HydrationAuthority<'plan> {
    pub(crate) projection: &'plan HydrationProjection,
    pub(crate) bindings: BTreeMap<u16, &'plan HydrationBinding>,
}

impl<'plan> HydrationAuthority<'plan> {
    pub(crate) fn new(projection: &'plan HydrationProjection) -> ExecResult<Self> {
        let bindings = projection
            .bindings()
            .iter()
            .map(|binding| (binding.binding().get(), binding))
            .collect::<BTreeMap<_, _>>();
        let descriptors = projection
            .descriptors()
            .iter()
            .map(|descriptor| (descriptor.descriptor().clone(), descriptor))
            .collect::<BTreeMap<_, _>>();
        if bindings.len() != projection.bindings().len()
            || descriptors.len() != projection.descriptors().len()
        {
            return Err(validation_error(
                "query_model_hydration_authority",
                "validated hydration authority contains duplicate identities",
            ));
        }
        Ok(Self {
            projection,
            bindings,
        })
    }

    pub(crate) fn binding(&self, binding: u16) -> ExecResult<&'plan HydrationBinding> {
        self.bindings.get(&binding).copied().ok_or_else(|| {
            validation_error(
                "query_model_hydration_binding",
                "provider hydration references an unknown binding",
            )
        })
    }

    pub(crate) fn descriptor_by_label(
        &self,
        label: &str,
    ) -> ExecResult<&'plan HydrationDescriptor> {
        let mut matches = self
            .projection
            .descriptors()
            .iter()
            .filter(|descriptor| descriptor.descriptor().label().as_str() == label);
        let descriptor = matches.next().ok_or_else(|| {
            validation_error(
                "query_model_hydration_descriptor",
                "provider hydration returned an unknown concrete type",
            )
        })?;
        if matches.next().is_some() {
            return Err(validation_error(
                "query_model_hydration_descriptor",
                "provider hydration label is ambiguous across thing kinds",
            ));
        }
        Ok(descriptor)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PlayerDraft {
    pub(crate) declared: TypeId,
    pub(crate) iid: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RoleDraft {
    pub(crate) role: RoleId,
    pub(crate) reference_roles: Vec<RoleId>,
    pub(crate) players: Vec<PlayerDraft>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NodeDraft {
    pub(crate) iid: String,
    pub(crate) concrete: TypeId,
    pub(crate) kind: HydrationNodeKind,
    pub(crate) attributes: Vec<(AttributeId, Vec<ModelValue>)>,
    pub(crate) roles: Vec<RoleDraft>,
    pub(crate) full: bool,
}

pub(crate) struct GraphBuilder {
    pub(crate) nodes: BTreeMap<String, NodeDraft>,
    pub(crate) max_nodes: u64,
    pub(crate) max_attribute_values: u64,
    pub(crate) max_role_players: u64,
    pub(crate) attribute_values: u64,
    pub(crate) role_players: u64,
}

impl GraphBuilder {
    pub(crate) fn new(limits: ModelReplyDecodeLimits) -> Self {
        Self {
            nodes: BTreeMap::new(),
            max_nodes: limits.max_graph_nodes,
            max_attribute_values: limits.max_attribute_values,
            max_role_players: limits.max_role_players,
            attribute_values: 0,
            role_players: 0,
        }
    }

    pub(crate) fn insert(&mut self, node: NodeDraft) -> ExecResult<()> {
        if !is_canonical_thing_iid(&node.iid) {
            return Err(evidence_error(
                "query_model_hydration_iid",
                "hydration returned a malformed provider IID",
            ));
        }
        if let Some(previous) = self.nodes.get(&node.iid) {
            if previous.concrete != node.concrete
                || previous.kind != node.kind
                || previous.attributes != node.attributes
            {
                return Err(evidence_error(
                    "query_model_hydration_conflict",
                    "one provider IID carries contradictory concrete hydration evidence",
                ));
            }
            if previous.full && node.full && previous.roles != node.roles {
                return Err(evidence_error(
                    "query_model_hydration_conflict",
                    "one provider IID carries contradictory relation-role evidence",
                ));
            }
            if !previous.full && node.full {
                self.charge_role_players(&node.roles)?;
                if let Some(previous) = self.nodes.get_mut(&node.iid) {
                    previous.roles = node.roles;
                    previous.full = true;
                } else {
                    return Err(validation_error(
                        "query_model_hydration_state",
                        "hydration merge lost an existing provider identity",
                    ));
                }
            }
            return Ok(());
        }
        let next_nodes = u64::try_from(self.nodes.len())
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        if next_nodes > self.max_nodes {
            return Err(resource_error(
                "query_model_graph_limit",
                "hydration construction exceeded the caller's graph-node budget",
            ));
        }
        let values = node
            .attributes
            .iter()
            .try_fold(0_u64, |total, (_, values)| {
                total
                    .checked_add(u64::try_from(values.len()).unwrap_or(u64::MAX))
                    .ok_or_else(|| {
                        resource_error(
                            "query_model_attribute_limit",
                            "hydration attribute-value counter overflowed",
                        )
                    })
            })?;
        self.attribute_values = self
            .attribute_values
            .checked_add(values)
            .filter(|total| *total <= self.max_attribute_values)
            .ok_or_else(|| {
                resource_error(
                    "query_model_attribute_limit",
                    "hydration construction exceeded the caller's attribute-value budget",
                )
            })?;
        if node.full {
            self.charge_role_players(&node.roles)?;
        }
        self.nodes.insert(node.iid.clone(), node);
        Ok(())
    }

    pub(crate) fn charge_role_players(&mut self, roles: &[RoleDraft]) -> ExecResult<()> {
        let players = roles.iter().try_fold(0_u64, |total, role| {
            total
                .checked_add(u64::try_from(role.players.len()).unwrap_or(u64::MAX))
                .ok_or_else(|| {
                    resource_error(
                        "query_model_role_player_limit",
                        "hydration role-player counter overflowed",
                    )
                })
        })?;
        self.role_players = self
            .role_players
            .checked_add(players)
            .filter(|total| *total <= self.max_role_players)
            .ok_or_else(|| {
                resource_error(
                    "query_model_role_player_limit",
                    "hydration construction exceeded the caller's role-player budget",
                )
            })?;
        Ok(())
    }

    pub(crate) fn node(&self, iid: &str) -> ExecResult<&NodeDraft> {
        self.nodes.get(iid).ok_or_else(|| {
            evidence_error(
                "query_model_hydration_missing",
                "selected provider identity lacks complete hydration evidence",
            )
        })
    }

    pub(crate) fn finish(self, roots: &BTreeSet<String>) -> ExecResult<FinishedGraph> {
        let mut retained = BTreeSet::new();
        let mut pending = roots.iter().cloned().collect::<Vec<_>>();
        while let Some(iid) = pending.pop() {
            if !retained.insert(iid.clone()) {
                continue;
            }
            let node = self.nodes.get(&iid).ok_or_else(|| {
                evidence_error(
                    "query_model_hydration_missing",
                    "output references a provider identity absent from hydration evidence",
                )
            })?;
            if node.full {
                for role in &node.roles {
                    pending.extend(role.players.iter().map(|player| player.iid.clone()));
                }
            }
        }
        let ids = retained
            .iter()
            .enumerate()
            .map(|(index, iid)| {
                let index = u32::try_from(index).map_err(|_| {
                    resource_error(
                        "query_model_graph_limit",
                        "hydration graph ordinal exceeds the contract range",
                    )
                })?;
                Ok((iid.clone(), HydrationNodeId::new(index)))
            })
            .collect::<ExecResult<BTreeMap<_, _>>>()?;
        let mut nodes = Vec::with_capacity(retained.len());
        for iid in retained {
            let draft = &self.nodes[&iid];
            let mut attributes = draft
                .attributes
                .iter()
                .map(|(attribute, values)| {
                    HydrationAttributeEvidence::new(attribute.clone(), values.clone())
                })
                .collect::<Vec<_>>();
            // Registry field aliases determine model-member order, while the
            // remote graph contract canonicalizes evidence by provider
            // descriptor identity. Keep those independent order domains from
            // leaking into one another.
            attributes.sort_by(|left, right| left.attribute().cmp(right.attribute()));
            let mut roles = if draft.full {
                draft
                    .roles
                    .iter()
                    .map(|role| {
                        let players = role
                            .players
                            .iter()
                            .map(|player| {
                                ids.get(&player.iid)
                                    .copied()
                                    .map(|node| {
                                        HydrationReference::new(player.declared.clone(), node)
                                    })
                                    .ok_or_else(|| {
                                        evidence_error(
                                            "query_model_hydration_missing",
                                            "relation role references missing player hydration",
                                        )
                                    })
                            })
                            .collect::<ExecResult<Vec<_>>>()?;
                        Ok(HydrationRoleEvidence::new(role.role.clone(), players))
                    })
                    .collect::<ExecResult<Vec<_>>>()?
            } else {
                Vec::new()
            };
            roles.sort_by(|left, right| left.role().cmp(right.role()));
            nodes.push(HydrationNode::new(
                ids[&iid],
                iid,
                draft.concrete.clone(),
                draft.kind,
                attributes,
                roles,
            ));
        }
        let graph = HydrationGraph::new(nodes).map_err(PlanExecutionError::Validation)?;
        Ok(FinishedGraph { graph, ids })
    }
}

pub(crate) struct FinishedGraph {
    pub(crate) graph: HydrationGraph,
    pub(crate) ids: BTreeMap<String, HydrationNodeId>,
}

impl FinishedGraph {
    pub(crate) fn reference(&self, declared: &TypeId, iid: &str) -> ExecResult<HydrationReference> {
        self.ids
            .get(iid)
            .copied()
            .map(|node| HydrationReference::new(declared.clone(), node))
            .ok_or_else(|| {
                evidence_error(
                    "query_model_hydration_missing",
                    "output references a provider identity pruned from its hydration graph",
                )
            })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WireThingKind {
    Entity,
    Relation,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HydratedThingWire {
    pub(crate) binding: u16,
    pub(crate) concept_id: String,
    pub(crate) concrete_type: String,
    pub(crate) kind: WireThingKind,
    #[serde(default)]
    pub(crate) attributes: Vec<HydratedAttributeWire>,
    #[serde(default)]
    pub(crate) roles: Vec<HydratedRoleWire>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HydratedAttributeWire {
    pub(crate) field: String,
    pub(crate) value_type: String,
    pub(crate) values: Vec<JsonValue>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HydratedRoleWire {
    pub(crate) role: String,
    pub(crate) players: Vec<HydratedPlayerWire>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HydratedPlayerWire {
    pub(crate) concept_id: String,
    pub(crate) declared_type: String,
    pub(crate) concrete_type: String,
    pub(crate) kind: WireThingKind,
    #[serde(default)]
    pub(crate) attributes: Vec<HydratedAttributeWire>,
}

pub(crate) struct DecodedThing {
    pub(crate) node: NodeDraft,
    pub(crate) nested: Vec<NodeDraft>,
}

pub(crate) fn decode_hydrated_document(
    value: JsonValue,
    expected_binding: Option<u16>,
    authority: &HydrationAuthority<'_>,
) -> ExecResult<DecodedThing> {
    if let Ok(wire) = serde_json::from_value::<HydratedThingWire>(value.clone()) {
        if expected_binding.is_some_and(|expected| expected != wire.binding) {
            return Err(evidence_error(
                "query_model_hydration_binding",
                "hydration document contains the wrong binding ordinal",
            ));
        }
        return decode_exact_thing(wire, authority);
    }
    let object = value.as_object().ok_or_else(|| {
        evidence_error(
            "query_model_hydration_document",
            "TypeDB hydration document is not an object",
        )
    })?;
    let binding = expected_binding
        .or_else(|| object.get("binding").and_then(json_u16))
        .ok_or_else(|| {
            evidence_error(
                "query_model_hydration_binding",
                "TypeDB hydration document omits its binding ordinal",
            )
        })?;
    let iid = object
        .get("concept_id")
        .and_then(json_string)
        .ok_or_else(|| {
            evidence_error(
                "query_model_hydration_iid",
                "TypeDB hydration document omits its provider IID",
            )
        })?
        .to_owned();
    let concrete_label = object
        .get("concrete_type")
        .and_then(json_string)
        .ok_or_else(|| {
            evidence_error(
                "query_model_hydration_descriptor",
                "TypeDB hydration document omits its concrete type",
            )
        })?;
    let projected = authority.descriptor_by_label(concrete_label)?;
    validate_binding_concrete(binding, projected.descriptor(), authority)?;
    let attributes = decode_wildcard_attributes(projected, object.get("attributes"))?;
    let (roles, nested) = if projected.descriptor().kind() == TypeKind::Relation {
        decode_wildcard_roles(projected, object.get("roles"), authority)?
    } else {
        if object
            .get("roles")
            .is_some_and(|value| value.as_array().is_none_or(|roles| !roles.is_empty()))
        {
            return Err(evidence_error(
                "query_model_hydration_roles",
                "hydrated entity unexpectedly carries relation roles",
            ));
        }
        (Vec::new(), Vec::new())
    };
    Ok(DecodedThing {
        node: NodeDraft {
            iid,
            concrete: projected.descriptor().clone(),
            kind: node_kind(projected.descriptor().kind())?,
            attributes,
            roles,
            full: true,
        },
        nested,
    })
}

pub(crate) fn decode_exact_thing(
    wire: HydratedThingWire,
    authority: &HydrationAuthority<'_>,
) -> ExecResult<DecodedThing> {
    let projected = authority.descriptor_by_label(&wire.concrete_type)?;
    validate_binding_concrete(wire.binding, projected.descriptor(), authority)?;
    validate_wire_kind(wire.kind, projected.descriptor().kind())?;
    let attributes = decode_exact_attributes(projected, wire.attributes)?;
    let (roles, nested) = if projected.descriptor().kind() == TypeKind::Relation {
        decode_exact_roles(projected, wire.roles, authority)?
    } else if wire.roles.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        return Err(evidence_error(
            "query_model_hydration_roles",
            "hydrated entity unexpectedly carries relation roles",
        ));
    };
    Ok(DecodedThing {
        node: NodeDraft {
            iid: wire.concept_id,
            concrete: projected.descriptor().clone(),
            kind: node_kind(projected.descriptor().kind())?,
            attributes,
            roles,
            full: true,
        },
        nested,
    })
}

pub(crate) fn validate_binding_concrete(
    binding: u16,
    concrete: &TypeId,
    authority: &HydrationAuthority<'_>,
) -> ExecResult<()> {
    if authority
        .binding(binding)?
        .concrete_descriptors()
        .contains(concrete)
    {
        Ok(())
    } else {
        Err(evidence_error(
            "query_model_hydration_descriptor",
            "hydrated concrete type is outside the binding's validated subtype closure",
        ))
    }
}

pub(crate) fn decode_wildcard_attributes(
    descriptor: &HydrationDescriptor,
    value: Option<&JsonValue>,
) -> ExecResult<Vec<(AttributeId, Vec<ModelValue>)>> {
    let object = match value {
        None => None,
        Some(value) => Some(value.as_object().ok_or_else(|| {
            evidence_error(
                "query_model_hydration_attributes",
                "TypeDB wildcard hydration attributes are not an object",
            )
        })?),
    };
    let mut result = Vec::with_capacity(descriptor.fields().len());
    for field in descriptor.fields() {
        let by_attribute = object.and_then(|object| object.get(field.attribute().label().as_str()));
        let by_alias = object.and_then(|object| object.get(field.alias()));
        if by_attribute.is_some()
            && by_alias.is_some()
            && field.alias() != field.attribute().label().as_str()
        {
            return Err(evidence_error(
                "query_model_hydration_attributes",
                "TypeDB wildcard hydration repeats one projected attribute under two names",
            ));
        }
        let raw = by_attribute.or(by_alias);
        result.push((
            field.attribute().clone(),
            decode_attribute_values(field, raw)?,
        ));
    }
    if let Some(object) = object {
        for name in object.keys() {
            let known = descriptor
                .fields()
                .iter()
                .any(|field| field.alias() == name || field.attribute().label().as_str() == name);
            if !known {
                return Err(evidence_error(
                    "query_model_hydration_attributes",
                    "TypeDB wildcard hydration returned an undeclared attribute",
                ));
            }
        }
    }
    Ok(result)
}

pub(crate) fn decode_attribute_values(
    field: &HydrationField,
    raw: Option<&JsonValue>,
) -> ExecResult<Vec<ModelValue>> {
    let mut values = match raw {
        None => Vec::new(),
        Some(raw) if raw.is_null() => Vec::new(),
        Some(raw) => {
            let raw_values = raw
                .as_array()
                .map_or_else(|| vec![raw], |values| values.iter().collect());
            raw_values
                .into_iter()
                .filter(|value| !value.is_null())
                .map(|value| compatibility_value_from_json(value, field.value_type()))
                .collect::<ExecResult<Vec<_>>>()?
        }
    };
    canonicalize_field_values(field, &mut values)?;
    Ok(values)
}

pub(crate) fn decode_exact_attributes(
    descriptor: &HydrationDescriptor,
    attributes: Vec<HydratedAttributeWire>,
) -> ExecResult<Vec<(AttributeId, Vec<ModelValue>)>> {
    let mut by_attribute = BTreeMap::new();
    for attribute in attributes {
        let field = descriptor
            .fields()
            .iter()
            .find(|field| {
                field.alias() == attribute.field
                    || field.attribute().label().as_str() == attribute.field
            })
            .ok_or_else(|| {
                evidence_error(
                    "query_model_hydration_attributes",
                    "typed hydration returned an undeclared attribute",
                )
            })?;
        if attribute.value_type != value_type_label(field.value_type()) {
            return Err(evidence_error(
                "query_model_hydration_value_type",
                "typed hydration attribute declares the wrong scalar domain",
            ));
        }
        let mut values = attribute
            .values
            .iter()
            .map(|value| compatibility_value_from_json(value, field.value_type()))
            .collect::<ExecResult<Vec<_>>>()?;
        canonicalize_field_values(field, &mut values)?;
        if by_attribute
            .insert(field.attribute().clone(), values)
            .is_some()
        {
            return Err(evidence_error(
                "query_model_hydration_attributes",
                "typed hydration returns one attribute more than once",
            ));
        }
    }
    Ok(descriptor
        .fields()
        .iter()
        .map(|field| {
            (
                field.attribute().clone(),
                by_attribute.remove(field.attribute()).unwrap_or_default(),
            )
        })
        .collect())
}

pub(crate) fn canonicalize_field_values(
    field: &HydrationField,
    values: &mut Vec<ModelValue>,
) -> ExecResult<()> {
    if field.ordered() {
        return Ok(());
    }

    // Compatibility values have a representation identity for stable wire
    // fingerprints and a semantic identity for released V1 scalar behavior.
    // Sort by the latter, using the former only to choose one deterministic
    // spelling before collapsing semantic duplicates.
    for index in 1..values.len() {
        let mut current = index;
        while current > 0 {
            let semantic = values[current - 1]
                .semantic_cmp_same_domain(&values[current])
                .ok_or_else(|| {
                    evidence_error(
                        "query_model_hydration_value_order",
                        "unordered hydration values are not comparable in their validated domain",
                    )
                })?;
            let canonical = semantic.then_with(|| values[current - 1].cmp(&values[current]));
            if canonical != Ordering::Greater {
                break;
            }
            values.swap(current - 1, current);
            current -= 1;
        }
    }
    values.dedup_by(|left, right| left.semantic_cmp_same_domain(right) == Some(Ordering::Equal));
    Ok(())
}

pub(crate) fn decode_wildcard_roles(
    descriptor: &HydrationDescriptor,
    value: Option<&JsonValue>,
    authority: &HydrationAuthority<'_>,
) -> ExecResult<(Vec<RoleDraft>, Vec<NodeDraft>)> {
    let documents = match value {
        None => &[][..],
        Some(value) => value.as_array().ok_or_else(|| {
            evidence_error(
                "query_model_hydration_roles",
                "TypeDB nested role hydration is not a list",
            )
        })?,
    };
    let mut players = BTreeMap::<RoleId, Vec<PlayerDraft>>::new();
    let mut nested = Vec::new();
    for document in documents {
        let document = document.as_object().ok_or_else(|| {
            evidence_error(
                "query_model_hydration_roles",
                "TypeDB nested role hydration member is not an object",
            )
        })?;
        let role_label = document.get("role").and_then(json_string).ok_or_else(|| {
            evidence_error(
                "query_model_hydration_roles",
                "TypeDB nested role hydration omits the role label",
            )
        })?;
        let role = projected_role(descriptor, role_label)?;
        let iid = document
            .get("player_concept_id")
            .and_then(json_string)
            .ok_or_else(|| {
                evidence_error(
                    "query_model_hydration_role_player",
                    "TypeDB nested role hydration omits the player IID",
                )
            })?
            .to_owned();
        let concrete_label = document
            .get("player_concrete_type")
            .and_then(json_string)
            .ok_or_else(|| {
                evidence_error(
                    "query_model_hydration_role_player",
                    "TypeDB nested role hydration omits the player concrete type",
                )
            })?;
        let projected = authority.descriptor_by_label(concrete_label)?;
        let declared = declared_player_type(role, projected.descriptor())?;
        let attributes = decode_wildcard_attributes(projected, document.get("attributes"))?;
        nested.push(NodeDraft {
            iid: iid.clone(),
            concrete: projected.descriptor().clone(),
            kind: node_kind(projected.descriptor().kind())?,
            attributes,
            roles: Vec::new(),
            full: false,
        });
        players
            .entry(role.role().clone())
            .or_default()
            .push(PlayerDraft { declared, iid });
    }
    complete_roles(descriptor, players, nested)
}

pub(crate) fn decode_exact_roles(
    descriptor: &HydrationDescriptor,
    roles: Vec<HydratedRoleWire>,
    authority: &HydrationAuthority<'_>,
) -> ExecResult<(Vec<RoleDraft>, Vec<NodeDraft>)> {
    let mut grouped = BTreeMap::<RoleId, Vec<PlayerDraft>>::new();
    let mut nested = Vec::new();
    for wire_role in roles {
        let role = projected_role(descriptor, &wire_role.role)?;
        if grouped.contains_key(role.role()) {
            return Err(evidence_error(
                "query_model_hydration_roles",
                "typed hydration returns one relation role more than once",
            ));
        }
        let mut role_players = Vec::new();
        for player in wire_role.players {
            let projected = authority.descriptor_by_label(&player.concrete_type)?;
            validate_wire_kind(player.kind, projected.descriptor().kind())?;
            let declared = TypeId::new(projected.descriptor().kind(), player.declared_type)
                .map_err(PlanExecutionError::Validation)?;
            let compatible = role.players().iter().any(|authority| {
                authority.declared_descriptor() == &declared
                    && authority
                        .concrete_descriptors()
                        .contains(projected.descriptor())
            });
            if !compatible {
                return Err(evidence_error(
                    "query_model_hydration_role_player",
                    "typed hydration role player violates its declared-to-concrete authority",
                ));
            }
            let attributes = decode_exact_attributes(projected, player.attributes)?;
            nested.push(NodeDraft {
                iid: player.concept_id.clone(),
                concrete: projected.descriptor().clone(),
                kind: node_kind(projected.descriptor().kind())?,
                attributes,
                roles: Vec::new(),
                full: false,
            });
            role_players.push(PlayerDraft {
                declared,
                iid: player.concept_id,
            });
        }
        grouped.insert(role.role().clone(), role_players);
    }
    complete_roles(descriptor, grouped, nested)
}

pub(crate) fn complete_roles(
    descriptor: &HydrationDescriptor,
    mut grouped: BTreeMap<RoleId, Vec<PlayerDraft>>,
    nested: Vec<NodeDraft>,
) -> ExecResult<(Vec<RoleDraft>, Vec<NodeDraft>)> {
    let mut roles = Vec::with_capacity(descriptor.roles().len());
    for projection in descriptor.roles() {
        let mut players = grouped.remove(projection.role()).unwrap_or_default();
        if !projection.ordered() {
            players.sort_by(|left, right| {
                (&left.iid, &left.declared).cmp(&(&right.iid, &right.declared))
            });
        }
        roles.push(RoleDraft {
            role: projection.role().clone(),
            reference_roles: projection.reference_roles().to_vec(),
            players,
        });
    }
    if !grouped.is_empty() {
        return Err(evidence_error(
            "query_model_hydration_roles",
            "hydration returned a role outside the concrete projection",
        ));
    }
    Ok((roles, nested))
}

pub(crate) fn projected_role<'a>(
    descriptor: &'a HydrationDescriptor,
    label: &str,
) -> ExecResult<&'a HydrationRole> {
    let (owner, role_label) = label
        .rsplit_once(':')
        .map_or((None, label), |(owner, role)| (Some(owner), role));
    let mut matches = descriptor.roles().iter().filter(|role| {
        role.role().label().as_str() == role_label
            && owner.is_none_or(|owner| {
                role.role().declaring_relation().as_str() == owner
                    || role.reference_roles().iter().any(|reference| {
                        reference.declaring_relation().as_str() == owner
                            && reference.label().as_str() == role_label
                    })
            })
    });
    let role = matches.next().ok_or_else(|| {
        evidence_error(
            "query_model_hydration_roles",
            "hydration returned an undeclared relation role",
        )
    })?;
    if matches.next().is_some() {
        return Err(evidence_error(
            "query_model_hydration_roles",
            "hydration role label is ambiguous under the concrete relation",
        ));
    }
    Ok(role)
}

pub(crate) fn declared_player_type(role: &HydrationRole, concrete: &TypeId) -> ExecResult<TypeId> {
    role.players()
        .iter()
        .find(|player| player.concrete_descriptors().contains(concrete))
        .map(|player| player.declared_descriptor().clone())
        .ok_or_else(|| {
            evidence_error(
                "query_model_hydration_role_player",
                "hydrated role player is outside the role's concrete player closure",
            )
        })
}

pub(crate) fn compatibility_value_from_json(
    value: &JsonValue,
    expected: ValueTypeTag,
) -> ExecResult<ModelValue> {
    let value = AttributeValue::from_json(value, value_type_label(expected)).ok_or_else(|| {
        evidence_error(
            "query_model_hydration_value_type",
            "hydration attribute value does not match its validated scalar domain",
        )
    })?;
    adapt_value(&value).map_err(PlanExecutionError::Validation)
}

pub(crate) const fn value_type_label(value_type: ValueTypeTag) -> &'static str {
    match value_type {
        ValueTypeTag::String => "string",
        ValueTypeTag::Long => "long",
        ValueTypeTag::Double => "double",
        ValueTypeTag::Boolean => "boolean",
        ValueTypeTag::Date => "date",
        ValueTypeTag::DateTime => "datetime",
        ValueTypeTag::DateTimeTz => "datetime-tz",
        ValueTypeTag::Decimal => "decimal",
        ValueTypeTag::Duration => "duration",
    }
}

pub(crate) fn validate_wire_kind(kind: WireThingKind, expected: TypeKind) -> ExecResult<()> {
    if matches!(
        (kind, expected),
        (WireThingKind::Entity, TypeKind::Entity) | (WireThingKind::Relation, TypeKind::Relation)
    ) {
        Ok(())
    } else {
        Err(evidence_error(
            "query_model_hydration_kind",
            "hydration thing kind contradicts its concrete descriptor",
        ))
    }
}

pub(crate) fn node_kind(kind: TypeKind) -> ExecResult<HydrationNodeKind> {
    match kind {
        TypeKind::Entity => Ok(HydrationNodeKind::Entity),
        TypeKind::Relation => Ok(HydrationNodeKind::Relation),
        TypeKind::Attribute | TypeKind::Struct => Err(validation_error(
            "query_model_hydration_kind",
            "model hydration projection contains a non-thing descriptor",
        )),
    }
}

pub(crate) fn json_string(value: &JsonValue) -> Option<&str> {
    bounded_unwrap_value(value)?.as_str()
}

pub(crate) fn json_u16(value: &JsonValue) -> Option<u16> {
    let value = bounded_unwrap_value(value)?;
    value
        .as_u64()
        .and_then(|value| u16::try_from(value).ok())
        .or_else(|| {
            value.as_f64().and_then(|value| {
                (value.is_finite()
                    && value.fract() == 0.0
                    && value >= 0.0
                    && value <= f64::from(u16::MAX))
                .then_some(value as u16)
            })
        })
}

pub(crate) fn bounded_unwrap_value(mut value: &JsonValue) -> Option<&JsonValue> {
    const MAX_WRAPPER_DEPTH: usize = 16;
    for _ in 0..=MAX_WRAPPER_DEPTH {
        let Some(next) = value.as_object().and_then(|object| object.get("value")) else {
            return Some(value);
        };
        value = next;
    }
    None
}

#[derive(Clone)]
pub(crate) enum SlotDraft {
    Singular { declared: TypeId, iid: String },
    Collection { declared: TypeId, iids: Vec<String> },
}

pub(crate) fn finish_hydrated_rows(
    graph: GraphBuilder,
    drafts: Vec<Vec<SlotDraft>>,
) -> ExecResult<(HydrationGraph, Vec<HydratedRow>)> {
    let roots = drafts
        .iter()
        .flat_map(|row| row.iter())
        .flat_map(|slot| match slot {
            SlotDraft::Singular { iid, .. } => std::slice::from_ref(iid).iter(),
            SlotDraft::Collection { iids, .. } => iids.iter(),
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    let finished = graph.finish(&roots)?;
    let rows = drafts
        .into_iter()
        .map(|row| {
            let slots = row
                .into_iter()
                .map(|slot| match slot {
                    SlotDraft::Singular { declared, iid } => Ok(HydrationSlot::Singular {
                        value: finished.reference(&declared, &iid)?,
                    }),
                    SlotDraft::Collection { declared, iids } => Ok(HydrationSlot::Collection {
                        values: iids
                            .into_iter()
                            .map(|iid| finished.reference(&declared, &iid))
                            .collect::<ExecResult<Vec<_>>>()?,
                    }),
                })
                .collect::<ExecResult<Vec<_>>>()?;
            Ok(HydratedRow::new(slots))
        })
        .collect::<ExecResult<Vec<_>>>()?;
    Ok((finished.graph, rows))
}

pub(crate) fn try_sort_iids(
    values: &mut [String],
    order: &ModelStableOrder,
    graph: &GraphBuilder,
) -> ExecResult<()> {
    for index in 1..values.len() {
        let mut current = index;
        while current > 0
            && compare_iids_by_order(&values[current - 1], &values[current], order, graph)?
                == Ordering::Greater
        {
            values.swap(current - 1, current);
            current -= 1;
        }
    }
    Ok(())
}

pub(crate) fn compare_iids_by_order(
    left: &str,
    right: &str,
    order: &ModelStableOrder,
    graph: &GraphBuilder,
) -> ExecResult<Ordering> {
    for term in order.terms() {
        let left = order_field_value(graph.node(left)?, term.field())?;
        let right = order_field_value(graph.node(right)?, term.field())?;
        let ordering = compare_optional_field_values(left, right, term.missing(), term.field())?;
        let ordering = if term.direction()
            == type_bridge_contract::query_plan::ModelOrderDirection::Descending
        {
            ordering.reverse()
        } else {
            ordering
        };
        if ordering != Ordering::Equal {
            return Ok(ordering);
        }
    }
    Ok(left.cmp(right))
}

pub(crate) fn order_field_value<'a>(
    node: &'a NodeDraft,
    field: &ModelField,
) -> ExecResult<Option<&'a ModelValue>> {
    let values = node
        .attributes
        .iter()
        .find(|(attribute, _)| attribute == field.attribute())
        .map(|(_, values)| values.as_slice())
        .unwrap_or_default();
    if values.len() > 1 {
        return Err(field_evidence_error(
            "query_model_order_non_scalar",
            "stable order field carries more than one hydrated scalar value",
            field,
        ));
    }
    Ok(values.first())
}

pub(crate) fn compare_optional_values(
    left: Option<&ModelValue>,
    right: Option<&ModelValue>,
    missing: type_bridge_contract::query_plan::ModelMissingOrder,
) -> ExecResult<Ordering> {
    match (left, right) {
        (None, None) => Ok(Ordering::Equal),
        (None, Some(_)) => match missing {
            type_bridge_contract::query_plan::ModelMissingOrder::First => Ok(Ordering::Less),
            type_bridge_contract::query_plan::ModelMissingOrder::Last => Ok(Ordering::Greater),
            type_bridge_contract::query_plan::ModelMissingOrder::Reject => Err(evidence_error(
                "query_model_order_missing",
                "stable order rejects missing hydrated values",
            )),
        },
        (Some(_), None) => match missing {
            type_bridge_contract::query_plan::ModelMissingOrder::First => Ok(Ordering::Greater),
            type_bridge_contract::query_plan::ModelMissingOrder::Last => Ok(Ordering::Less),
            type_bridge_contract::query_plan::ModelMissingOrder::Reject => Err(evidence_error(
                "query_model_order_missing",
                "stable order rejects missing hydrated values",
            )),
        },
        (Some(left), Some(right)) => left.semantic_cmp_same_domain(right).ok_or_else(|| {
            evidence_error(
                "query_model_order_value_type",
                "stable order values are not comparable in their validated domain",
            )
        }),
    }
}

pub(crate) fn validate_solution_order(
    solutions: &[Solution],
    order: &ModelStableOrder,
    graph: &GraphBuilder,
) -> ExecResult<()> {
    for adjacent in solutions.windows(2) {
        if compare_solutions_by_order(&adjacent[0], &adjacent[1], order, graph)?
            == Ordering::Greater
        {
            return Err(evidence_error(
                "query_model_unstable_provider_order",
                "provider solutions violate the validator-derived stable total order",
            ));
        }
    }
    Ok(())
}

pub(crate) fn try_sort_solutions(
    values: &mut [Solution],
    order: &ModelStableOrder,
    graph: &GraphBuilder,
) -> ExecResult<()> {
    for value in values.iter() {
        compare_solutions_by_order(value, value, order, graph)?;
    }
    let mut failure = None;
    values.sort_by(
        |left, right| match compare_solutions_by_order(left, right, order, graph) {
            Ok(ordering) => ordering,
            Err(error) => {
                failure.get_or_insert(error);
                Ordering::Equal
            }
        },
    );
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(())
}

pub(crate) fn compare_solutions_by_order(
    left: &Solution,
    right: &Solution,
    order: &ModelStableOrder,
    graph: &GraphBuilder,
) -> ExecResult<Ordering> {
    for term in order.terms() {
        let left = solution_field_values(left, graph, term.field())?;
        let right = solution_field_values(right, graph, term.field())?;
        if left.len() > 1 || right.len() > 1 {
            return Err(field_evidence_error(
                "query_model_order_non_scalar",
                "stable order field carries more than one hydrated scalar value",
                term.field(),
            ));
        }
        let ordering = compare_optional_field_values(
            left.first(),
            right.first(),
            term.missing(),
            term.field(),
        )?;
        let ordering = if term.direction()
            == type_bridge_contract::query_plan::ModelOrderDirection::Descending
        {
            ordering.reverse()
        } else {
            ordering
        };
        if ordering != Ordering::Equal {
            return Ok(ordering);
        }
    }
    for binding in order.identity_tiebreakers() {
        let left = left.bindings.get(&binding.get()).ok_or_else(|| {
            evidence_error(
                "query_model_solution_binding",
                "stable-order identity tie breaker references a missing binding",
            )
        })?;
        let right = right.bindings.get(&binding.get()).ok_or_else(|| {
            evidence_error(
                "query_model_solution_binding",
                "stable-order identity tie breaker references a missing binding",
            )
        })?;
        let ordering = left.cmp(right);
        if ordering != Ordering::Equal {
            return Ok(ordering);
        }
    }
    Ok(Ordering::Equal)
}

pub(crate) fn charge_selected_solution_semantics(
    solutions: &[Solution],
    graph: &GraphBuilder,
    limits: ModelReplyDecodeLimits,
    selected: Option<&BTreeSet<u16>>,
) -> ExecResult<()> {
    let mut hydrated_things = 0_u64;
    let mut attribute_values = 0_u64;
    for solution in solutions {
        for (binding, iid) in &solution.bindings {
            if selected.is_some_and(|selected| !selected.contains(binding)) {
                continue;
            }
            let node = graph.node(iid)?;
            hydrated_things = hydrated_things.checked_add(1).ok_or_else(|| {
                resource_error(
                    "query_model_graph_limit",
                    "hydrated thing multiplicity counter overflowed",
                )
            })?;
            attribute_values = attribute_values
                .checked_add(node_attribute_value_count(node)?)
                .ok_or_else(|| {
                    resource_error(
                        "query_model_attribute_limit",
                        "hydrated attribute-value multiplicity counter overflowed",
                    )
                })?;
            for role in &node.roles {
                for player in &role.players {
                    hydrated_things = hydrated_things.checked_add(1).ok_or_else(|| {
                        resource_error(
                            "query_model_graph_limit",
                            "hydrated thing multiplicity counter overflowed",
                        )
                    })?;
                    attribute_values = attribute_values
                        .checked_add(node_attribute_value_count(graph.node(&player.iid)?)?)
                        .ok_or_else(|| {
                            resource_error(
                                "query_model_attribute_limit",
                                "hydrated attribute-value multiplicity counter overflowed",
                            )
                        })?;
                }
            }
        }
    }
    if hydrated_things > limits.max_graph_nodes {
        return Err(resource_error(
            "query_model_graph_limit",
            "provider hydration exceeded the hydrated-thing ceiling",
        ));
    }
    if attribute_values > limits.max_attribute_values {
        return Err(resource_error(
            "query_model_attribute_limit",
            "provider hydration exceeded the attribute-value ceiling",
        ));
    }
    Ok(())
}

pub(crate) fn node_attribute_value_count(node: &NodeDraft) -> ExecResult<u64> {
    node.attributes
        .iter()
        .try_fold(0_u64, |count, (_, values)| {
            count
                .checked_add(u64::try_from(values.len()).unwrap_or(u64::MAX))
                .ok_or_else(|| {
                    resource_error(
                        "query_model_attribute_limit",
                        "hydrated attribute-value counter overflowed",
                    )
                })
        })
}

pub(crate) type PredicateWitnesses = BTreeMap<(u16, String, AttributeId), Vec<ModelValue>>;

fn predicate_field_values<'a>(
    solution: &Solution,
    graph: &'a GraphBuilder,
    field: &ModelField,
    native: Option<(&'a PredicateWitnesses, &BTreeSet<u16>)>,
) -> ExecResult<&'a [ModelValue]> {
    if let Some((witnesses, selected)) = native
        && !selected.contains(&field.binding().get())
    {
        let iid = solution
            .bindings
            .get(&field.binding().get())
            .ok_or_else(|| {
                evidence_error(
                    "query_model_solution_binding",
                    "predicate witness omits its binding",
                )
            })?;
        return witnesses
            .get(&(
                field.binding().get(),
                iid.clone(),
                field.attribute().clone(),
            ))
            .map(Vec::as_slice)
            .ok_or_else(|| {
                evidence_error(
                    "query_model_predicate_evidence",
                    "predicate witness column is missing",
                )
            });
    }
    solution_field_values(solution, graph, field)
}

pub(crate) fn evaluate_pattern_with_witnesses(
    pattern: &ModelPredicate,
    solution: &Solution,
    graph: &GraphBuilder,
    native: Option<(&PredicateWitnesses, &BTreeSet<u16>)>,
) -> ExecResult<bool> {
    Ok(recheck_pattern(pattern, solution, graph, native)? != Some(false))
}

// None is native-only evidence: negation must not turn an unchecked edge into a refusal.
fn recheck_pattern(
    pattern: &ModelPredicate,
    solution: &Solution,
    graph: &GraphBuilder,
    native: Option<(&PredicateWitnesses, &BTreeSet<u16>)>,
) -> ExecResult<Option<bool>> {
    match pattern {
        ModelPredicate::FieldValue {
            field,
            comparator,
            value,
        } => {
            for candidate in predicate_field_values(solution, graph, field, native)? {
                if compare_predicate_values(*comparator, candidate, value)? {
                    return Ok(Some(true));
                }
            }
            Ok(Some(false))
        }
        ModelPredicate::FieldComparison {
            left,
            comparator,
            right,
        } => {
            let left = predicate_field_values(solution, graph, left, native)?;
            let right = predicate_field_values(solution, graph, right, native)?;
            for left in left {
                for right in right {
                    if compare_predicate_values(*comparator, left, right)? {
                        return Ok(Some(true));
                    }
                }
            }
            Ok(Some(false))
        }
        ModelPredicate::FieldPresence { field, present } => Ok(Some(
            predicate_field_values(solution, graph, field, native)?.is_empty() != *present,
        )),
        ModelPredicate::BindingIid { binding, iid } => {
            Ok(Some(solution.bindings.get(&binding.get()).is_some_and(
                |candidate| candidate.eq_ignore_ascii_case(iid),
            )))
        }
        ModelPredicate::RoleEdge {
            relation,
            role,
            player,
            ..
        } => {
            if native.is_some_and(|(_, selected)| !selected.contains(&relation.get())) {
                return Ok(None);
            }
            let relation_iid = solution.bindings.get(&relation.get()).ok_or_else(|| {
                evidence_error(
                    "query_model_solution_binding",
                    "role edge references a missing relation binding",
                )
            })?;
            let player_iid = solution.bindings.get(&player.get()).ok_or_else(|| {
                evidence_error(
                    "query_model_solution_binding",
                    "role edge references a missing player binding",
                )
            })?;
            let relation = graph.node(relation_iid)?;
            Ok(Some(relation.roles.iter().any(|candidate| {
                (&candidate.role == role || candidate.reference_roles.contains(role))
                    && candidate
                        .players
                        .iter()
                        .any(|candidate| candidate.iid == *player_iid)
            })))
        }
        ModelPredicate::Reachable {
            source,
            target,
            max_depth,
            ..
        } => {
            if *max_depth == 0 {
                Ok(Some(
                    solution.bindings.get(&source.get()) == solution.bindings.get(&target.get()),
                ))
            } else {
                // Positive path witnesses are intentionally existential and
                // projected away by the typed compiler. The provider boundary
                // is trusted only for that path proof; both public endpoints
                // and every materialized node remain independently checked.
                Ok(None)
            }
        }
        ModelPredicate::And { patterns } => {
            let mut unchecked = false;
            for child in patterns {
                match recheck_pattern(child, solution, graph, native)? {
                    Some(false) => return Ok(Some(false)),
                    None => unchecked = true,
                    Some(true) => {}
                }
            }
            Ok((!unchecked).then_some(true))
        }
        ModelPredicate::Or { patterns } => {
            let mut unchecked = false;
            for child in patterns {
                match recheck_pattern(child, solution, graph, native)? {
                    Some(true) => return Ok(Some(true)),
                    None => unchecked = true,
                    Some(false) => {}
                }
            }
            Ok((!unchecked).then_some(false))
        }
        ModelPredicate::Not { pattern } => {
            Ok(recheck_pattern(pattern, solution, graph, native)?.map(|value| !value))
        }
    }
}

pub(crate) fn solution_field_values<'a>(
    solution: &Solution,
    graph: &'a GraphBuilder,
    field: &ModelField,
) -> ExecResult<&'a [ModelValue]> {
    let iid = solution
        .bindings
        .get(&field.binding().get())
        .ok_or_else(|| {
            evidence_error(
                "query_model_solution_binding",
                "field predicate references a missing binding",
            )
        })?;
    Ok(graph
        .node(iid)?
        .attributes
        .iter()
        .find(|(attribute, _)| attribute == field.attribute())
        .map(|(_, values)| values.as_slice())
        .unwrap_or_default())
}

pub(crate) fn compare_predicate_values(
    comparator: ModelComparator,
    left: &ModelValue,
    right: &ModelValue,
) -> ExecResult<bool> {
    match comparator {
        ModelComparator::Equal | ModelComparator::NotEqual => {
            let equal = left
                .semantic_cmp_same_domain(right)
                .map(|ordering| ordering == Ordering::Equal)
                .ok_or_else(|| {
                    evidence_error(
                        "query_model_predicate_equal_type",
                        "predicate values do not share their validated scalar domain",
                    )
                })?;
            Ok(if comparator == ModelComparator::Equal {
                equal
            } else {
                !equal
            })
        }
        ModelComparator::Less
        | ModelComparator::LessOrEqual
        | ModelComparator::Greater
        | ModelComparator::GreaterOrEqual => {
            let ordering = left.semantic_cmp_same_domain(right).ok_or_else(|| {
                evidence_error(
                    "query_model_predicate_order_type",
                    "predicate values are not order-compatible",
                )
            })?;
            Ok(match comparator {
                ModelComparator::Less => ordering == Ordering::Less,
                ModelComparator::LessOrEqual => ordering != Ordering::Greater,
                ModelComparator::Greater => ordering == Ordering::Greater,
                ModelComparator::GreaterOrEqual => ordering != Ordering::Less,
                _ => false,
            })
        }
        ModelComparator::Contains
        | ModelComparator::StartsWith
        | ModelComparator::EndsWith
        | ModelComparator::Regex => {
            let left = compatibility_string(left)?;
            let right = compatibility_string(right)?;
            match comparator {
                ModelComparator::Contains => Ok(UniCase::new(left.as_str())
                    .to_folded_case()
                    .contains(&UniCase::new(right.as_str()).to_folded_case())),
                ModelComparator::StartsWith => Ok(left.starts_with(&right)),
                ModelComparator::EndsWith => Ok(left.ends_with(&right)),
                ModelComparator::Regex => Regex::new(&right)
                    .map(|regex| regex.is_match(&left))
                    .map_err(|_| {
                        validation_error(
                            "query_model_regex",
                            "validated compatibility predicate contains an invalid regular expression",
                        )
                    }),
                _ => Ok(false),
            }
        }
    }
}

pub(crate) fn compatibility_string(value: &ModelValue) -> ExecResult<String> {
    if value.value_type() != ValueTypeTag::String {
        return Err(evidence_error(
            "query_model_predicate_string_type",
            "string predicate operator received a non-string value",
        ));
    }
    if let Some(type_bridge_contract::value::CanonicalValue::String(value)) =
        value.canonical_value()
    {
        return Ok(value.as_str().to_owned());
    }
    value.released_text().ok_or_else(|| {
        evidence_error(
            "query_model_predicate_string_type",
            "string predicate value has no canonical or released representation",
        )
    })
}

pub(crate) fn validation_error(code: &'static str, message: &'static str) -> PlanExecutionError {
    PlanExecutionError::Validation(failure(DiagnosticCategory::InvalidContract, code, message))
}

pub(crate) fn evidence_error(code: &'static str, message: &'static str) -> PlanExecutionError {
    PlanExecutionError::Validation(evidence_failure(code, message))
}

pub(crate) fn field_evidence_error(
    code: &'static str,
    message: &'static str,
    field: &ModelField,
) -> PlanExecutionError {
    let owner_kind = match field.descriptor().kind() {
        type_bridge_contract::id::TypeKind::Entity => "entity",
        type_bridge_contract::id::TypeKind::Relation => "relation",
        type_bridge_contract::id::TypeKind::Attribute => "attribute",
        type_bridge_contract::id::TypeKind::Struct => "struct",
    };
    PlanExecutionError::Validation(
        evidence_failure(code, message)
            .with_detail(
                "field_owner",
                format!("{owner_kind}:{}", field.descriptor().label().as_str()),
            )
            .with_detail("field_name", field.attribute().label().as_str().to_owned()),
    )
}

pub(crate) fn compare_optional_field_values(
    left: Option<&ModelValue>,
    right: Option<&ModelValue>,
    missing: type_bridge_contract::query_plan::ModelMissingOrder,
    field: &ModelField,
) -> ExecResult<Ordering> {
    compare_optional_values(left, right, missing).map_err(|error| match error {
        PlanExecutionError::Validation(diagnostic)
            if diagnostic.code().as_str() == "query_model_order_missing" =>
        {
            field_evidence_error(
                "query_model_order_missing",
                "stable order rejects missing hydrated values",
                field,
            )
        }
        PlanExecutionError::Validation(diagnostic)
            if diagnostic.code().as_str() == "query_model_order_value_type" =>
        {
            field_evidence_error(
                "query_model_order_value_type",
                "stable order values are not comparable in their validated domain",
                field,
            )
        }
        error => error,
    })
}

pub(crate) fn resource_error(code: &'static str, message: &'static str) -> PlanExecutionError {
    PlanExecutionError::Validation(failure(DiagnosticCategory::ResourceLimit, code, message))
}

pub(crate) fn evidence_failure(code: &'static str, message: &'static str) -> Diagnostic {
    failure(DiagnosticCategory::Integrity, code, message)
}

/// Issue graph references from documents already validated against the plan.
pub(crate) fn graph_from_verified_plan_documents(
    registry: &crate::_registry::DescriptorRegistry,
    request: &super::ValidatedMatchRequest,
    plan: &type_bridge_contract::query_plan::QueryPlan,
    documents: &[JsonValue],
    limits: crate::QueryExecutionResourceLimits,
) -> ExecResult<(HydrationGraph, Vec<HydrationReference>)> {
    use super::descriptor_plan::{MatchRequestAdapterAuthority, build_hydration};
    let metadata = MatchRequestAdapterAuthority::from_registry(registry)
        .map_err(PlanExecutionError::Validation)?;
    let projection = build_hydration(
        request.request(),
        registry,
        metadata.context().resolved_schema(),
    )
    .map_err(PlanExecutionError::Validation)?;
    let authority = HydrationAuthority::new(&projection)?;
    let declared = authority.binding(0)?.declared_descriptor().clone();
    let mut graph = GraphBuilder::new(ModelReplyDecodeLimits {
        max_bytes: limits.bytes,
        max_items: limits.items,
        max_graph_nodes: limits.graph_nodes,
        max_attribute_values: limits.attribute_values,
        max_collection_members: limits.collection_members,
        max_role_players: limits.role_players,
    });
    let order = insert_plan_documents(&mut graph, &authority, 0, plan, documents)?;
    let finished = graph.finish(&order.iter().cloned().collect())?;
    let references = order
        .iter()
        .map(|iid| finished.reference(&declared, iid))
        .collect::<ExecResult<Vec<_>>>()?;
    Ok((finished.graph, references))
}

pub(crate) fn validate_plan_documents(
    plan: &type_bridge_contract::query_plan::QueryPlan,
    documents: &[JsonValue],
    limits: crate::QueryExecutionResourceLimits,
) -> ExecResult<()> {
    if documents.len() as u64 > limits.items {
        return Err(resource_error(
            "processed_item_limit",
            "fetch documents exceed the item budget",
        ));
    }
    let bytes = serde_json::to_vec(documents)
        .map_err(|_| {
            evidence_error(
                "query_plan_hydration_document",
                "fetch documents cannot encode",
            )
        })?
        .len();
    if bytes as u64 > limits.bytes {
        return Err(resource_error(
            "response_byte_limit",
            "fetch documents exceed the byte budget",
        ));
    }
    let answer = crate::query_plan_exec::PlanAnswer::Documents(documents.to_vec())
        .to_staged_v3(plan)
        .map_err(PlanExecutionError::Validation)?;
    answer
        .validate_for(plan)
        .map_err(PlanExecutionError::Validation)?;
    if answer.item_count() > limits.items {
        return Err(resource_error(
            "processed_item_limit",
            "fetch documents exceed the item budget",
        ));
    }
    let counts = answer.resource_counts();
    for (actual, maximum, code) in [
        (counts[0], limits.graph_nodes, "query_model_graph_limit"),
        (
            counts[1],
            limits.attribute_values,
            "query_model_attribute_limit",
        ),
        (
            counts[2],
            limits.collection_members,
            "query_model_collection_limit",
        ),
        (
            counts[3],
            limits.role_players,
            "query_model_role_player_limit",
        ),
    ] {
        if actual > maximum {
            return Err(resource_error(
                code,
                "fetch documents exceed the semantic answer budget",
            ));
        }
    }

    Ok(())
}

pub(crate) fn insert_plan_documents(
    graph: &mut GraphBuilder,
    authority: &HydrationAuthority<'_>,
    binding: u16,
    plan: &type_bridge_contract::query_plan::QueryPlan,
    documents: &[JsonValue],
) -> ExecResult<Vec<String>> {
    use type_bridge_contract::query_plan::Stage;
    let document = plan
        .pipeline()
        .iter()
        .find_map(|stage| match stage {
            Stage::Fetch(document) => Some(document),
            _ => None,
        })
        .ok_or_else(|| {
            validation_error(
                "query_plan_hydration_fetch",
                "model materialization requires a thing fetch",
            )
        })?;
    // TypeDB 3 unordered roles are sets of player IIDs: repeated links add no occurrence.
    // Coalesce Cartesian fan-out across other roles, keeping each role index separate.
    type PlayerDocuments = BTreeMap<(usize, String), JsonValue>;
    let mut grouped: BTreeMap<String, (JsonValue, PlayerDocuments)> = BTreeMap::new();
    let mut order = Vec::new();
    for value in documents {
        let iid = value.get("_iid").and_then(json_string).ok_or_else(|| {
            evidence_error("query_plan_hydration_iid", "fetch document omits its IID")
        })?;
        let concrete = value.get("_type").and_then(json_string).ok_or_else(|| {
            evidence_error(
                "query_plan_hydration_type",
                "fetch document omits its concrete type",
            )
        })?;
        let descriptor = authority.descriptor_by_label(concrete)?;
        if descriptor.roles().iter().any(|role| {
            !document.roles.iter().any(|listed| {
                listed.role == *role.role() || role.reference_roles().contains(&listed.role)
            })
        }) {
            return Err(evidence_error(
                "query_plan_hydration_roles",
                "model fetch omits an effective relation role",
            ));
        }
        let root = serde_json::json!({"binding":binding,"concept_id":iid,"concrete_type":concrete,"attributes":value["attributes"]});
        if !grouped.contains_key(iid) {
            order.push(iid.to_owned());
        }
        let (stored, players) = grouped
            .entry(iid.to_owned())
            .or_insert_with(|| (root.clone(), BTreeMap::new()));
        if *stored != root {
            return Err(evidence_error(
                "query_plan_hydration_conflict",
                "one IID has contradictory fetch evidence",
            ));
        }
        for (index, listed) in document.roles.iter().enumerate() {
            let role_name = format!(
                "{}:{}",
                listed.role.declaring_relation(),
                listed.role.label()
            );
            let collected;
            let triples;
            let player_documents = if listed.collect {
                collected = value
                    .get(format!("_role_{index}_players"))
                    .and_then(JsonValue::as_array)
                    .ok_or_else(|| {
                        evidence_error(
                            "query_plan_hydration_roles",
                            "collected role list is missing",
                        )
                    })?;
                collected.as_slice()
            } else {
                let Some(player) = value
                    .get(format!("_role_{index}_iid"))
                    .and_then(json_string)
                else {
                    continue;
                };
                triples = [
                    serde_json::json!({"_iid": player, "_type": value[format!("_role_{index}_type")], "attributes": value[format!("_role_{index}_attributes")]}),
                ];
                triples.as_slice()
            };
            if !player_documents.is_empty() && projected_role(descriptor, &role_name)?.ordered() {
                return Err(evidence_error(
                    "query_plan_hydration_ordered_role",
                    "populated ordered roles require provider fetch ordinals",
                ));
            }
            for player in player_documents {
                let iid = player.get("_iid").and_then(json_string).ok_or_else(|| {
                    evidence_error("query_plan_hydration_iid", "player document omits its IID")
                })?;
                let key = (index, iid.to_owned());
                let player = serde_json::json!({ "role": role_name, "player_concept_id": iid, "player_concrete_type": player["_type"], "attributes": player["attributes"] });
                if let Some(previous) = players.insert(key, player.clone())
                    && previous != player
                {
                    return Err(evidence_error(
                        "query_plan_hydration_conflict",
                        "one player IID has contradictory fetch evidence",
                    ));
                }
            }
        }
    }
    for (_, (mut value, players)) in grouped {
        value["roles"] = JsonValue::Array(players.into_values().collect());
        let decoded = decode_hydrated_document(value, Some(binding), authority)?;
        for nested in decoded.nested {
            graph.insert(nested)?;
        }
        graph.insert(decoded.node)?;
    }
    Ok(order)
}

#[cfg(test)]
mod predicate_recheck_tests {
    use super::*;
    use type_bridge_contract::migration_assertion::BindingId;
    use type_bridge_contract::value::{CanonicalString, CanonicalValue};

    #[test]
    fn native_only_predicates_stay_unchecked_under_negation_and_boolean_trees() {
        let person = TypeId::new(TypeKind::Entity, "person").unwrap();
        let employment = TypeId::new(TypeKind::Relation, "employment").unwrap();
        let field = AttributeId::new("name").unwrap();
        let value = |text| {
            ModelValue::canonical(CanonicalValue::String(CanonicalString::new(text).unwrap()))
        };
        let mut graph = GraphBuilder::new(ModelReplyDecodeLimits::DEFAULT_CEILING);
        graph
            .insert(NodeDraft {
                iid: "0x01".into(),
                concrete: person.clone(),
                kind: HydrationNodeKind::Entity,
                attributes: vec![(field.clone(), vec![value("Ada")])],
                roles: vec![],
                full: true,
            })
            .unwrap();
        let solution = Solution {
            bindings: BTreeMap::from([(0, "0x01".into()), (1, "0x02".into())]),
        };
        let witnesses = PredicateWitnesses::new();
        let selected = BTreeSet::from([0]);
        let native = Some((&witnesses, &selected));
        let role = |label| ModelPredicate::RoleEdge {
            relation: BindingId::new(1).unwrap(),
            player: BindingId::new(0).unwrap(),
            relation_type: employment.clone(),
            role: RoleId::new("employment", label).unwrap(),
            include_relation_subtypes: false,
        };
        let not = |pattern| ModelPredicate::Not {
            pattern: Box::new(pattern),
        };
        let predicate = ModelPredicate::And {
            patterns: vec![role("employee"), not(role("manager"))],
        };
        assert!(evaluate_pattern_with_witnesses(&predicate, &solution, &graph, native).unwrap());
        let reachable = ModelPredicate::Reachable {
            min_depth: 1,
            max_depth: 2,
            relation: employment.clone(),
            role_from: RoleId::new("employment", "employee").unwrap(),
            role_to: RoleId::new("employment", "manager").unwrap(),
            source: BindingId::new(0).unwrap(),
            target: BindingId::new(1).unwrap(),
        };
        assert!(
            evaluate_pattern_with_witnesses(&not(reachable), &solution, &graph, native).unwrap()
        );
        let hydrated = |text| ModelPredicate::FieldValue {
            field: ModelField::new(
                BindingId::new(0).unwrap(),
                person.clone(),
                field.clone(),
                ValueTypeTag::String,
            ),
            comparator: ModelComparator::Equal,
            value: value(text),
        };
        let cases = [
            (hydrated("Ada"), Some(true)),
            (hydrated("Grace"), Some(false)),
            (role("employee"), None),
        ];
        for (left, lhs) in &cases {
            assert_eq!(
                recheck_pattern(&not(left.clone()), &solution, &graph, native).unwrap(),
                lhs.map(|value| !value)
            );
            for (right, rhs) in &cases {
                let and = if *lhs == Some(false) || *rhs == Some(false) {
                    Some(false)
                } else if lhs.is_none() || rhs.is_none() {
                    None
                } else {
                    Some(true)
                };
                let or = if *lhs == Some(true) || *rhs == Some(true) {
                    Some(true)
                } else if lhs.is_none() || rhs.is_none() {
                    None
                } else {
                    Some(false)
                };
                for (pattern, expected) in [
                    (
                        ModelPredicate::And {
                            patterns: vec![left.clone(), right.clone()],
                        },
                        and,
                    ),
                    (
                        ModelPredicate::Or {
                            patterns: vec![left.clone(), right.clone()],
                        },
                        or,
                    ),
                ] {
                    assert_eq!(
                        recheck_pattern(&pattern, &solution, &graph, native).unwrap(),
                        expected
                    );
                    assert_eq!(
                        evaluate_pattern_with_witnesses(&pattern, &solution, &graph, native)
                            .unwrap(),
                        expected != Some(false)
                    );
                }
            }
        }
        assert!(
            !evaluate_pattern_with_witnesses(&hydrated("Grace"), &solution, &graph, native)
                .unwrap()
        );
    }
}
