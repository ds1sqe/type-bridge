//! Test-only encoding of released result values as the V2 outcomes a live
//! executor returns.
//!
//! Tests describe results with released [`HydratedThing`] values; these
//! helpers build the dense hydration graph and rows that
//! [`validated_match_result_from_v2`](super::result_validation::validated_match_result_from_v2)
//! maps back, so every test result travels the one live validation path.

use std::collections::BTreeMap;

use type_bridge_contract::id::{AttributeId, RoleId as ContractRoleId, TypeId, TypeKind};
use type_bridge_contract::migration_assertion::BindingId as ContractBindingId;
use type_bridge_contract::query_remote_v2::{
    HydratedRowV2, HydrationAttributeEvidenceV2, HydrationGraphV2, HydrationNodeIdV2,
    HydrationNodeKindV2, HydrationNodeV2, HydrationReferenceV2, HydrationRoleEvidenceV2,
    HydrationSlotV2, RemoteOutcomeV2, RemoteReductionGroupV2, RemoteReductionRowV2,
};

use crate::_registry::DescriptorRegistry;
use crate::query_v2_adapter::adapt_value;

use super::ids::{BindingId, DescriptorId};
use super::model::{ThingKind, Window};
use super::recording::{model_reduction_outcome, remote_reduced_value};
use super::result::{HydratedAttribute, HydratedThing, ReductionRow, SlotValue};
use super::validation::ValidatedMatchRequest;

/// One role's players as (player IID, player declared type).
type RoleDraft = (ContractRoleId, Vec<(String, TypeId)>);

/// One graph node before dense ordinals are assigned.
struct NodeDraft {
    concrete: TypeId,
    kind: HydrationNodeKindV2,
    attributes: Vec<HydrationAttributeEvidenceV2>,
    roles: Vec<RoleDraft>,
}

/// Collects released things into one dense, IID-ordered hydration graph.
struct GraphBuilder<'r> {
    registry: &'r DescriptorRegistry,
    nodes: BTreeMap<String, NodeDraft>,
}

impl<'r> GraphBuilder<'r> {
    fn new(registry: &'r DescriptorRegistry) -> Self {
        Self {
            registry,
            nodes: BTreeMap::new(),
        }
    }

    fn add_thing(&mut self, thing: &HydratedThing) {
        let roles = thing
            .roles()
            .iter()
            .map(|role| {
                let relation = self.type_name(&role.role().owner);
                let players = role
                    .players()
                    .iter()
                    .map(|player| {
                        self.add_player(
                            player.concept_id().as_str(),
                            player.concrete_descriptor(),
                            player.kind(),
                            player.attributes(),
                        );
                        (
                            player.concept_id().as_str().to_owned(),
                            self.type_id(player.declared_descriptor(), player.kind()),
                        )
                    })
                    .collect();
                (
                    ContractRoleId::new(relation, role.role().name.clone()).unwrap(),
                    players,
                )
            })
            .collect::<Vec<_>>();
        let draft = self.draft(
            thing.concrete_descriptor(),
            thing.kind(),
            thing.attributes(),
        );
        self.nodes.insert(
            thing.concept_id().as_str().to_owned(),
            NodeDraft { roles, ..draft },
        );
    }

    fn add_player(
        &mut self,
        iid: &str,
        concrete: &DescriptorId,
        kind: ThingKind,
        attributes: &[HydratedAttribute],
    ) {
        if !self.nodes.contains_key(iid) {
            let draft = self.draft(concrete, kind, attributes);
            self.nodes.insert(iid.to_owned(), draft);
        }
    }

    fn draft(
        &self,
        concrete: &DescriptorId,
        kind: ThingKind,
        attributes: &[HydratedAttribute],
    ) -> NodeDraft {
        let mut attributes = attributes
            .iter()
            .map(|attribute| {
                let label = self
                    .registry
                    .provider_attribute_name(attribute.field())
                    .unwrap();
                HydrationAttributeEvidenceV2::new(
                    AttributeId::new(label).unwrap(),
                    attribute
                        .values()
                        .iter()
                        .map(|value| adapt_value(value).unwrap())
                        .collect(),
                )
            })
            .collect::<Vec<_>>();
        attributes.sort_by(|left, right| left.attribute().cmp(right.attribute()));
        NodeDraft {
            concrete: self.type_id(concrete, kind),
            kind: match kind {
                ThingKind::Entity => HydrationNodeKindV2::Entity,
                ThingKind::Relation => HydrationNodeKindV2::Relation,
            },
            attributes,
            roles: Vec::new(),
        }
    }

    fn type_name(&self, descriptor: &DescriptorId) -> String {
        self.registry.descriptor_type_name(descriptor).unwrap()
    }

    fn type_id(&self, descriptor: &DescriptorId, kind: ThingKind) -> TypeId {
        let kind = match kind {
            ThingKind::Entity => TypeKind::Entity,
            ThingKind::Relation => TypeKind::Relation,
        };
        TypeId::new(kind, self.type_name(descriptor)).unwrap()
    }

    fn finish(self) -> (HydrationGraphV2, GraphIds<'r>) {
        let ids = self
            .nodes
            .keys()
            .enumerate()
            .map(|(index, iid)| {
                (
                    iid.clone(),
                    HydrationNodeIdV2::new(u32::try_from(index).unwrap()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let nodes = self
            .nodes
            .into_iter()
            .map(|(iid, draft)| {
                let mut roles = draft
                    .roles
                    .into_iter()
                    .map(|(role, players)| {
                        HydrationRoleEvidenceV2::new(
                            role,
                            players
                                .into_iter()
                                .map(|(iid, declared)| {
                                    HydrationReferenceV2::new(declared, ids[&iid])
                                })
                                .collect(),
                        )
                    })
                    .collect::<Vec<_>>();
                roles.sort_by(|left, right| left.role().cmp(right.role()));
                HydrationNodeV2::new(
                    ids[&iid],
                    iid,
                    draft.concrete,
                    draft.kind,
                    draft.attributes,
                    roles,
                )
            })
            .collect();
        (
            HydrationGraphV2::new(nodes).unwrap(),
            GraphIds {
                registry: self.registry,
                ids,
            },
        )
    }
}

/// Dense node ordinals of a finished graph.
struct GraphIds<'r> {
    registry: &'r DescriptorRegistry,
    ids: BTreeMap<String, HydrationNodeIdV2>,
}

impl GraphIds<'_> {
    fn reference(&self, thing: &HydratedThing) -> HydrationReferenceV2 {
        let builder = GraphBuilder::new(self.registry);
        HydrationReferenceV2::new(
            builder.type_id(thing.declared_descriptor(), thing.kind()),
            self.ids[thing.concept_id().as_str()],
        )
    }

    fn row(&self, slots: &[SlotValue]) -> HydratedRowV2 {
        HydratedRowV2::new(
            slots
                .iter()
                .map(|slot| match slot {
                    SlotValue::One(thing) => HydrationSlotV2::Singular {
                        value: self.reference(thing),
                    },
                    SlotValue::Many(things) => HydrationSlotV2::Collection {
                        values: things.iter().map(|thing| self.reference(thing)).collect(),
                    },
                })
                .collect(),
        )
    }
}

fn graph_of<'r>(
    registry: &'r DescriptorRegistry,
    rows: &[Vec<SlotValue>],
) -> (HydrationGraphV2, GraphIds<'r>) {
    let mut builder = GraphBuilder::new(registry);
    for slot in rows.iter().flatten() {
        match slot {
            SlotValue::One(thing) => builder.add_thing(thing),
            SlotValue::Many(things) => things.iter().for_each(|thing| builder.add_thing(thing)),
        }
    }
    builder.finish()
}

/// Encode released rows as the live `HydratedRows` outcome.
pub(crate) fn hydrated_rows(
    registry: &DescriptorRegistry,
    rows: Vec<Vec<SlotValue>>,
) -> RemoteOutcomeV2 {
    let (graph, ids) = graph_of(registry, &rows);
    RemoteOutcomeV2::HydratedRows {
        rows: rows.iter().map(|row| ids.row(row)).collect(),
        graph,
    }
}

/// Encode released page entries as the live `HydratedPage` outcome.
pub(crate) fn hydrated_page(
    registry: &DescriptorRegistry,
    root: BindingId,
    window: Window,
    total: Option<u64>,
    entries: Vec<Vec<SlotValue>>,
) -> RemoteOutcomeV2 {
    let (graph, ids) = graph_of(registry, &entries);
    RemoteOutcomeV2::HydratedPage {
        entries: entries.iter().map(|entry| ids.row(entry)).collect(),
        graph,
        limit: window.limit,
        offset: window.offset,
        root: contract_binding(root),
        total,
    }
}

/// Encode a distinct-root count as the live `DistinctCount` outcome.
pub(crate) fn distinct_count(root: BindingId, value: u64) -> RemoteOutcomeV2 {
    RemoteOutcomeV2::DistinctCount {
        root: contract_binding(root),
        value,
    }
}

/// Encode a distinct-root verdict as the live `DistinctExists` outcome.
pub(crate) fn distinct_exists(root: BindingId, value: bool) -> RemoteOutcomeV2 {
    RemoteOutcomeV2::DistinctExists {
        root: contract_binding(root),
        value,
    }
}

/// Encode released reduction rows as the live `ModelReduction` outcome,
/// echoing the adapted reduction contract of `validated`.
pub(crate) fn model_reduction(
    registry: &DescriptorRegistry,
    validated: &ValidatedMatchRequest,
    rows: Vec<ReductionRow>,
) -> RemoteOutcomeV2 {
    let mut builder = GraphBuilder::new(registry);
    for group in rows.iter().filter_map(ReductionRow::group) {
        builder.add_thing(group);
    }
    let (graph, ids) = builder.finish();
    let rows = rows
        .iter()
        .map(|row| {
            let group = if let Some(thing) = row.group() {
                Some(RemoteReductionGroupV2::Thing {
                    value: ids.reference(thing),
                })
            } else if let Some(value) = row.field_group() {
                Some(RemoteReductionGroupV2::Field {
                    value: adapt_value(value).unwrap(),
                })
            } else {
                row.field_groups()
                    .map(|values| RemoteReductionGroupV2::Fields {
                        values: values
                            .iter()
                            .map(|value| adapt_value(value).unwrap())
                            .collect(),
                    })
            };
            RemoteReductionRowV2::new(
                group,
                row.values().iter().map(remote_reduced_value).collect(),
            )
        })
        .collect();
    model_reduction_outcome(registry, validated, graph, rows).unwrap()
}

fn contract_binding(binding: BindingId) -> ContractBindingId {
    ContractBindingId::new(binding.get()).unwrap()
}
