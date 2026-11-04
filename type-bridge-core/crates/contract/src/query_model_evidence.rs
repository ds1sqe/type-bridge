//! Canonical model hydration evidence, shared by direct and remote plans.

use crate::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use crate::id::{AttributeId, RoleId, TypeId, TypeKind, is_canonical_thing_iid};
use crate::migration_assertion::BindingId;
use crate::query_plan::ModelValue;
use crate::query_plan::{ModelReductionKey, ModelReductionTerm};
use crate::query_remote::{RemoteFieldValue, RemoteValue};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::BTreeSet;

/// Dense zero-based node identity in one hydration graph.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct HydrationNodeId(u32);

impl HydrationNodeId {
    /// Construct one node identity.
    #[must_use]
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Return the dense ordinal.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// One reference to a canonical graph node under a declared model type.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationReference {
    pub(crate) declared: TypeId,
    pub(crate) node: HydrationNodeId,
}

impl HydrationReference {
    /// Construct one declared occurrence of a graph node.
    #[must_use]
    pub const fn new(declared: TypeId, node: HydrationNodeId) -> Self {
        Self { declared, node }
    }

    /// Return the declared type under which the occurrence was selected.
    #[must_use]
    pub const fn declared(&self) -> &TypeId {
        &self.declared
    }

    /// Return the referenced dense node.
    #[must_use]
    pub const fn node(&self) -> HydrationNodeId {
        self.node
    }
}

/// Complete values for one provider attribute in a hydrated concrete model.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationAttributeEvidence {
    pub(crate) attribute: AttributeId,
    pub(crate) values: Vec<ModelValue>,
}

impl HydrationAttributeEvidence {
    /// Construct one descriptor-qualified attribute evidence entry.
    #[must_use]
    pub const fn new(attribute: AttributeId, values: Vec<ModelValue>) -> Self {
        Self { attribute, values }
    }

    /// Return the provider attribute descriptor.
    #[must_use]
    pub const fn attribute(&self) -> &AttributeId {
        &self.attribute
    }

    /// Return canonical or released-only compatibility values in field order.
    #[must_use]
    pub fn values(&self) -> &[ModelValue] {
        &self.values
    }
}

/// Complete players for one descriptor-qualified hydrated relation role.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationRoleEvidence {
    pub(crate) players: Vec<HydrationReference>,
    pub(crate) role: RoleId,
}

impl HydrationRoleEvidence {
    /// Construct one ordered role-player evidence entry.
    #[must_use]
    pub const fn new(role: RoleId, players: Vec<HydrationReference>) -> Self {
        Self { players, role }
    }

    /// Return the exact descriptor-qualified role.
    #[must_use]
    pub const fn role(&self) -> &RoleId {
        &self.role
    }

    /// Return player occurrences in provider/materializer order.
    #[must_use]
    pub fn players(&self) -> &[HydrationReference] {
        &self.players
    }
}

/// Materializable concept kind of one hydration node.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HydrationNodeKind {
    /// Entity model.
    Entity,
    /// Relation model.
    Relation,
}

/// One canonical provider identity and its complete concrete model evidence.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationNode {
    pub(crate) attributes: Vec<HydrationAttributeEvidence>,
    pub(crate) concrete: TypeId,
    pub(crate) id: HydrationNodeId,
    pub(crate) iid: String,
    pub(crate) kind: HydrationNodeKind,
    pub(crate) roles: Vec<HydrationRoleEvidence>,
}

impl HydrationNode {
    /// Construct one node. Whole-graph ordering, uniqueness, and references
    /// are checked by [`HydrationGraph::new`].
    #[must_use]
    pub const fn new(
        id: HydrationNodeId,
        iid: String,
        concrete: TypeId,
        kind: HydrationNodeKind,
        attributes: Vec<HydrationAttributeEvidence>,
        roles: Vec<HydrationRoleEvidence>,
    ) -> Self {
        Self {
            attributes,
            concrete,
            id,
            iid,
            kind,
            roles,
        }
    }

    /// Return this node's dense identity.
    #[must_use]
    pub const fn id(&self) -> HydrationNodeId {
        self.id
    }

    /// Return the exact provider IID.
    #[must_use]
    pub fn iid(&self) -> &str {
        &self.iid
    }

    /// Return the exact concrete descriptor.
    #[must_use]
    pub const fn concrete(&self) -> &TypeId {
        &self.concrete
    }

    /// Return entity or relation materialization kind.
    #[must_use]
    pub const fn kind(&self) -> HydrationNodeKind {
        self.kind
    }

    /// Return complete ordered attribute evidence.
    #[must_use]
    pub fn attributes(&self) -> &[HydrationAttributeEvidence] {
        &self.attributes
    }

    /// Return complete ordered relation-role evidence.
    #[must_use]
    pub fn roles(&self) -> &[HydrationRoleEvidence] {
        &self.roles
    }
}

/// One dense, provider-identity-deduplicated hydration graph.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HydrationGraph {
    pub(crate) nodes: Vec<HydrationNode>,
}

impl HydrationGraph {
    /// Validate and construct one complete hydration graph.
    pub fn new(nodes: Vec<HydrationNode>) -> Result<Self, Diagnostic> {
        let graph = Self { nodes };
        graph.validate()?;
        Ok(graph)
    }

    /// Return dense nodes in canonical identity order.
    #[must_use]
    pub fn nodes(&self) -> &[HydrationNode] {
        &self.nodes
    }

    // Structure only. How many nodes a graph may hold is the caller's
    // budget, checked where a budget is known
    // (`validate_hydration_graph_budget`), not a property of the container.
    pub(crate) fn validate(&self) -> Result<(), Diagnostic> {
        let mut iids = BTreeSet::new();
        let mut previous_iid = None;
        for (index, node) in self.nodes.iter().enumerate() {
            if usize::try_from(node.id.get()).ok() != Some(index) {
                return Err(remote_evidence_mismatch(
                    "hydration graph node IDs must be dense zero-based ordinals",
                ));
            }
            if !is_canonical_thing_iid(&node.iid) || !iids.insert(node.iid.as_str()) {
                return Err(remote_evidence_mismatch(
                    "hydration graph provider IIDs must be canonical and unique",
                ));
            }
            if previous_iid.is_some_and(|previous| previous >= node.iid.as_str()) {
                return Err(remote_evidence_mismatch(
                    "hydration graph nodes must be strictly ordered by provider IID",
                ));
            }
            previous_iid = Some(node.iid.as_str());
            let expected_kind = match node.kind {
                HydrationNodeKind::Entity => TypeKind::Entity,
                HydrationNodeKind::Relation => TypeKind::Relation,
            };
            if node.concrete.kind() != expected_kind {
                return Err(remote_evidence_mismatch(
                    "hydration node kind contradicts its concrete descriptor",
                ));
            }
            if node.kind == HydrationNodeKind::Entity && !node.roles.is_empty() {
                return Err(remote_evidence_mismatch(
                    "hydrated entities must not carry relation roles",
                ));
            }
            if node.attributes.windows(2).any(|pair| {
                pair[0].attribute().label().as_str() >= pair[1].attribute().label().as_str()
            }) {
                return Err(remote_evidence_mismatch(
                    "hydration attributes must be strictly descriptor-ordered",
                ));
            }
            if node
                .roles
                .windows(2)
                .any(|pair| pair[0].role >= pair[1].role)
            {
                return Err(remote_evidence_mismatch(
                    "hydration roles must be strictly descriptor-ordered",
                ));
            }
        }
        for node in &self.nodes {
            for role in &node.roles {
                for player in &role.players {
                    let player_index = usize::try_from(player.node.get()).map_err(|_| {
                        remote_evidence_mismatch(
                            "hydration role player references an unknown graph node",
                        )
                    })?;
                    let Some(player_node) = self.nodes.get(player_index) else {
                        return Err(remote_evidence_mismatch(
                            "hydration role player references an unknown graph node",
                        ));
                    };
                    if player.declared.kind() != player_node.concrete.kind() {
                        return Err(remote_evidence_mismatch(
                            "hydration role-player declared and concrete kinds disagree",
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

/// One hydrated model-result slot.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HydrationSlot {
    /// One singular model occurrence.
    Singular {
        /// Exact graph reference.
        value: HydrationReference,
    },
    /// One ordered model collection preserving multiplicity.
    Collection {
        /// Ordered graph references.
        values: Vec<HydrationReference>,
    },
}

/// One ordered hydrated output row.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HydratedRow {
    pub(crate) slots: Vec<HydrationSlot>,
}

impl HydratedRow {
    /// Construct one ordered row.
    #[must_use]
    pub const fn new(slots: Vec<HydrationSlot>) -> Self {
        Self { slots }
    }

    /// Return output slots in public order.
    #[must_use]
    pub fn slots(&self) -> &[HydrationSlot] {
        &self.slots
    }
}

pub(crate) fn remote_evidence_mismatch(message: &'static str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCategory::Integrity,
        DiagnosticCode::new("query_remote_v2_evidence_mismatch")
            .expect("fixed evidence diagnostic"),
        message,
    )
}

/// One typed group key in a model-compatible reduction row.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelReductionGroup {
    /// One attached thing identity and its complete graph projection.
    Thing {
        /// Declared-to-concrete graph reference.
        value: HydrationReference,
    },
    /// One scalar field value.
    Field {
        /// Exact compatibility scalar.
        value: ModelValue,
    },
    /// One ordered Cartesian field-value tuple.
    Fields {
        /// Exact compatibility scalars in request field order.
        values: Vec<ModelValue>,
    },
}

impl ModelReductionGroup {
    /// Construct a thing group key.
    #[must_use]
    pub const fn thing(value: HydrationReference) -> Self {
        Self::Thing { value }
    }

    /// Construct a field group key.
    #[must_use]
    pub const fn field(value: ModelValue) -> Self {
        Self::Field { value }
    }

    /// Construct a tuple-field group key.
    #[must_use]
    pub const fn fields(values: Vec<ModelValue>) -> Self {
        Self::Fields { values }
    }
}

/// One exact typed reducer cell.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelReducedValue {
    /// Lossless distinct-root count.
    Count {
        /// Exact unsigned count.
        value: u64,
    },
    /// Optional long-domain reducer result.
    Long {
        /// `None` for reducers undefined on the observed stream.
        value: Option<i64>,
    },
    /// Optional finite double-domain reducer result with exact IEEE bits.
    DoubleBits {
        /// `None` for reducers undefined on the observed stream.
        value: Option<u64>,
    },
}

/// One ordered typed reduction row.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelReductionRow {
    pub(crate) group: Option<ModelReductionGroup>,
    pub(crate) values: Vec<ModelReducedValue>,
}

impl ModelReductionRow {
    /// Construct one reduction row.
    #[must_use]
    pub const fn new(group: Option<ModelReductionGroup>, values: Vec<ModelReducedValue>) -> Self {
        Self { group, values }
    }

    /// Return the optional typed group key.
    #[must_use]
    pub const fn group(&self) -> Option<&ModelReductionGroup> {
        self.group.as_ref()
    }

    /// Return reducer cells in exact request order.
    #[must_use]
    pub fn values(&self) -> &[ModelReducedValue] {
        &self.values
    }
}

/// The typed terminal evidence of one model query.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelOutcome {
    /// Evidence-validated low-level positional rows.
    Rows {
        /// Positional row values.
        rows: Vec<Vec<RemoteValue>>,
    },
    /// Evidence-validated low-level fetched documents.
    Documents {
        /// Positional document fields.
        documents: Vec<Vec<RemoteFieldValue>>,
    },
    /// Low-level answer count.
    Count {
        /// Exact unsigned count.
        value: u64,
    },
    /// Low-level existence verdict.
    Exists {
        /// Exact existence verdict.
        value: bool,
    },
    /// Model-oriented rows sharing one validated hydration graph.
    HydratedRows {
        /// Dense graph used by every row.
        graph: HydrationGraph,
        /// Ordered result rows.
        rows: Vec<HydratedRow>,
    },
    /// One distinct-root model page and optional same-snapshot total.
    HydratedPage {
        /// Ordered page entries.
        entries: Vec<HydratedRow>,
        /// Dense graph used by every entry.
        graph: HydrationGraph,
        /// Exact requested page limit.
        limit: u64,
        /// Exact requested page offset.
        offset: u64,
        /// Exact distinct root binding.
        #[serde(deserialize_with = "deserialize_binding_id")]
        root: BindingId,
        /// Optional same-snapshot total.
        total: Option<u64>,
    },
    /// Lossless distinct-root count.
    DistinctCount {
        /// Exact counted root binding.
        #[serde(deserialize_with = "deserialize_binding_id")]
        root: BindingId,
        /// Exact unsigned count.
        value: u64,
    },
    /// Distinct-root existence verdict.
    DistinctExists {
        /// Exact tested root binding.
        #[serde(deserialize_with = "deserialize_binding_id")]
        root: BindingId,
        /// Exact existence verdict.
        value: bool,
    },
    /// Model-oriented typed reduction rows and optional group hydration.
    ModelReduction {
        /// Dense graph rooted only by thing-group keys.
        graph: HydrationGraph,
        /// Exact reduced root binding echoed from the model terminal.
        #[serde(deserialize_with = "deserialize_binding_id")]
        root: BindingId,
        /// Exact grouping contract echoed from the model terminal.
        group: Option<ModelReductionKey>,
        /// Exact reducer contract echoed from the model terminal.
        reducers: Vec<ModelReductionTerm>,
        /// Deterministically ordered typed reduction rows.
        rows: Vec<ModelReductionRow>,
    },
}

#[derive(Deserialize)]
#[serde(transparent)]
struct BindingIdWire(u16);

fn deserialize_binding_id<'de, D>(deserializer: D) -> Result<BindingId, D::Error>
where
    D: Deserializer<'de>,
{
    let wire = BindingIdWire::deserialize(deserializer)?;
    BindingId::new(wire.0).map_err(serde::de::Error::custom)
}
