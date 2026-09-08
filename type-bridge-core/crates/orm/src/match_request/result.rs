//! Canonical match-result and provider-evidence algebra.
//!
//! Provider evidence retains complete positive assignments and satisfied role
//! edges. It is not a public raw-solution stream. Future executors pass this
//! evidence to the Rust result validator, and only that validator may wrap a
//! result in [`ValidatedMatchResult`].

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::value::AttributeValue;

use super::error::{MatchError, MatchErrorCategory, MatchErrorPathSegment};
use super::ids::{
    BindingId, DescriptorId, FieldId, RequestToken, ResultShapeId, RoleEdgeId, RoleId,
};
use super::model::{ThingKind, Window};
use super::validation::ValidatedMatchRequest;

/// The stable provider identity of one TypeDB concept.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConceptId(String);

impl ConceptId {
    /// Construct an unvalidated provider concept identity.
    ///
    /// Provider-result validation checks syntax, presence, and uniqueness.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Return the provider identity spelling.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ConceptId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Typed values returned for one descriptor-qualified model field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HydratedAttribute {
    field: FieldId,
    values: Vec<AttributeValue>,
}

impl HydratedAttribute {
    /// Construct unvalidated field evidence inside the ORM/provider boundary.
    #[allow(dead_code)] // Consumed by the typed provider executor in #176.
    pub(crate) fn new(field: FieldId, values: Vec<AttributeValue>) -> Self {
        Self { field, values }
    }

    /// Return the descriptor-qualified field identity.
    pub fn field(&self) -> &FieldId {
        &self.field
    }

    /// Return field values in provider-declared order.
    ///
    /// The result validator checks cardinality and whether order is meaningful
    /// for the field's active descriptor.
    pub fn values(&self) -> &[AttributeValue] {
        &self.values
    }
}

/// Hydrated role-player evidence without recursively expanding its own roles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HydratedRolePlayer {
    concept_id: ConceptId,
    declared_descriptor: DescriptorId,
    concrete_descriptor: DescriptorId,
    kind: ThingKind,
    attributes: Vec<HydratedAttribute>,
}

impl HydratedRolePlayer {
    /// Construct unvalidated role-player evidence inside the provider boundary.
    #[allow(dead_code)] // Consumed by the typed provider executor in #176.
    pub(crate) fn new(
        concept_id: ConceptId,
        declared_descriptor: DescriptorId,
        concrete_descriptor: DescriptorId,
        kind: ThingKind,
        attributes: Vec<HydratedAttribute>,
    ) -> Self {
        Self {
            concept_id,
            declared_descriptor,
            concrete_descriptor,
            kind,
            attributes,
        }
    }

    /// Return the player's TypeDB concept identity.
    pub fn concept_id(&self) -> &ConceptId {
        &self.concept_id
    }

    /// Return the descriptor requested by the binding/role contract.
    pub fn declared_descriptor(&self) -> &DescriptorId {
        &self.declared_descriptor
    }

    /// Return the player's concrete provider descriptor.
    pub fn concrete_descriptor(&self) -> &DescriptorId {
        &self.concrete_descriptor
    }

    /// Return whether the player is an entity or relation.
    pub const fn kind(&self) -> ThingKind {
        self.kind
    }

    /// Return the player's hydrated attributes.
    pub fn attributes(&self) -> &[HydratedAttribute] {
        &self.attributes
    }
}

/// Hydrated players returned for one descriptor-qualified relation role.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HydratedRole {
    role: RoleId,
    players: Vec<HydratedRolePlayer>,
}

impl HydratedRole {
    /// Construct unvalidated role evidence inside the provider boundary.
    #[allow(dead_code)] // Consumed by the typed provider executor in #176.
    pub(crate) fn new(role: RoleId, players: Vec<HydratedRolePlayer>) -> Self {
        Self { role, players }
    }

    /// Return the descriptor-qualified role identity.
    pub fn role(&self) -> &RoleId {
        &self.role
    }

    /// Return players in provider evidence order, including multiplicity.
    pub fn players(&self) -> &[HydratedRolePlayer] {
        &self.players
    }
}

/// Complete typed hydration evidence for one selected or matched thing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HydratedThing {
    concept_id: ConceptId,
    declared_descriptor: DescriptorId,
    concrete_descriptor: DescriptorId,
    kind: ThingKind,
    attributes: Vec<HydratedAttribute>,
    roles: Vec<HydratedRole>,
}

impl HydratedThing {
    /// Construct unvalidated thing evidence inside the provider boundary.
    #[allow(dead_code)] // Consumed by the typed provider executor in #176.
    pub(crate) fn new(
        concept_id: ConceptId,
        declared_descriptor: DescriptorId,
        concrete_descriptor: DescriptorId,
        kind: ThingKind,
        attributes: Vec<HydratedAttribute>,
        roles: Vec<HydratedRole>,
    ) -> Self {
        Self {
            concept_id,
            declared_descriptor,
            concrete_descriptor,
            kind,
            attributes,
            roles,
        }
    }

    /// Return the TypeDB concept identity used for result distinctness.
    pub fn concept_id(&self) -> &ConceptId {
        &self.concept_id
    }

    /// Return the descriptor requested by the binding.
    pub fn declared_descriptor(&self) -> &DescriptorId {
        &self.declared_descriptor
    }

    /// Return the concrete descriptor reported by the provider.
    pub fn concrete_descriptor(&self) -> &DescriptorId {
        &self.concrete_descriptor
    }

    /// Return whether this thing is an entity or relation.
    pub const fn kind(&self) -> ThingKind {
        self.kind
    }

    /// Return all declared hydrated field values.
    pub fn attributes(&self) -> &[HydratedAttribute] {
        &self.attributes
    }

    /// Return relation-role evidence; entities must have an empty slice.
    pub fn roles(&self) -> &[HydratedRole] {
        &self.roles
    }
}

/// One positive binding assignment in a provider match solution.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BoundConceptEvidence {
    binding: BindingId,
    thing: HydratedThing,
}

impl BoundConceptEvidence {
    /// Construct an unvalidated binding assignment inside the provider boundary.
    #[allow(dead_code)] // Consumed by the typed provider executor in #176.
    pub(crate) fn new(binding: BindingId, thing: HydratedThing) -> Self {
        Self { binding, thing }
    }

    /// Return the canonical plan-local binding identity.
    pub(crate) const fn binding(&self) -> BindingId {
        self.binding
    }

    /// Return the concept/hydration evidence assigned to the binding.
    pub(crate) fn thing(&self) -> &HydratedThing {
        &self.thing
    }
}

/// Complete positive evidence for one provider match solution.
///
/// Repeated values in a sequence of solutions preserve matching-solution and
/// collection multiplicity. The result validator verifies that every required
/// positive binding and active role edge is present exactly as required.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProviderSolutionEvidence {
    bindings: Vec<BoundConceptEvidence>,
    satisfied_role_edges: Vec<RoleEdgeId>,
}

/// Invocation-bound provider output awaiting canonical result validation.
///
/// The envelope is deliberately non-serializable and crate-internal. A future
/// executor may construct it, but only the result validator may turn it into a
/// [`ValidatedMatchResult`].
#[derive(Debug)]
pub(crate) struct ProviderResultEvidence {
    request_token: RequestToken,
    shape_id: ResultShapeId,
    payload: ProviderResultPayload,
}

impl ProviderResultEvidence {
    /// Construct row-solution evidence for a `FetchRows` request.
    pub(crate) fn rows(
        request_token: RequestToken,
        shape_id: ResultShapeId,
        solutions: Vec<ProviderSolutionEvidence>,
    ) -> Self {
        Self {
            request_token,
            shape_id,
            payload: ProviderResultPayload::Rows {
                solutions,
                apply_window: false,
            },
        }
    }

    /// Construct an unwindowed selected-solution prefix from the streaming
    /// executor. Canonical validation verifies order/distinctness first and
    /// applies the validated offset/limit atomically.
    pub(crate) fn rows_unwindowed(
        request_token: RequestToken,
        shape_id: ResultShapeId,
        solutions: Vec<ProviderSolutionEvidence>,
    ) -> Self {
        Self {
            request_token,
            shape_id,
            payload: ProviderResultPayload::Rows {
                solutions,
                apply_window: true,
            },
        }
    }

    /// Construct distinct-root page evidence.
    pub(crate) fn page(
        request_token: RequestToken,
        shape_id: ResultShapeId,
        root: BindingId,
        solutions: Vec<ProviderSolutionEvidence>,
        window: Window,
        total: Option<u64>,
    ) -> Self {
        let mut selected_roots = Vec::new();
        for solution in &solutions {
            let Some(concept) = solution
                .bindings
                .iter()
                .find(|binding| binding.binding == root)
            else {
                continue;
            };
            if selected_roots.last() != Some(concept.thing.concept_id()) {
                selected_roots.push(concept.thing.concept_id().clone());
            }
        }
        Self::page_selected(
            request_token,
            shape_id,
            root,
            selected_roots,
            solutions,
            window,
            total,
        )
    }

    /// Construct page evidence with the exact ordered distinct-root selection
    /// retained separately from the re-match/hydration solutions.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn page_selected(
        request_token: RequestToken,
        shape_id: ResultShapeId,
        root: BindingId,
        selected_roots: Vec<ConceptId>,
        solutions: Vec<ProviderSolutionEvidence>,
        window: Window,
        total: Option<u64>,
    ) -> Self {
        Self {
            request_token,
            shape_id,
            payload: ProviderResultPayload::Page {
                root,
                selected_roots,
                solutions,
                window,
                total,
            },
        }
    }

    /// Construct distinct-root count evidence.
    pub(crate) fn count(
        request_token: RequestToken,
        shape_id: ResultShapeId,
        root: BindingId,
        value: u64,
    ) -> Self {
        Self {
            request_token,
            shape_id,
            payload: ProviderResultPayload::Count { root, value },
        }
    }

    /// Construct distinct-root existence evidence.
    pub(crate) fn exists(
        request_token: RequestToken,
        shape_id: ResultShapeId,
        root: BindingId,
        value: bool,
    ) -> Self {
        Self {
            request_token,
            shape_id,
            payload: ProviderResultPayload::Exists { root, value },
        }
    }

    /// Construct typed reduction evidence.
    pub(crate) fn reduction(
        request_token: RequestToken,
        shape_id: ResultShapeId,
        root: BindingId,
        group: Option<BindingId>,
        rows: Vec<ReductionRow>,
    ) -> Self {
        Self {
            request_token,
            shape_id,
            payload: ProviderResultPayload::Reduction { root, group, rows },
        }
    }

    /// Construct typed field-grouped reduction evidence.
    pub(crate) fn field_reduction(
        request_token: RequestToken,
        shape_id: ResultShapeId,
        root: BindingId,
        group: super::ids::BoundFieldId,
        rows: Vec<ReductionRow>,
    ) -> Self {
        Self {
            request_token,
            shape_id,
            payload: ProviderResultPayload::FieldReduction { root, group, rows },
        }
    }

    /// Construct typed tuple-field-grouped reduction evidence.
    pub(crate) fn field_tuple_reduction(
        request_token: RequestToken,
        shape_id: ResultShapeId,
        root: BindingId,
        groups: Vec<super::ids::BoundFieldId>,
        rows: Vec<ReductionRow>,
    ) -> Self {
        Self {
            request_token,
            shape_id,
            payload: ProviderResultPayload::FieldTupleReduction { root, groups, rows },
        }
    }

    pub(super) const fn request_token(&self) -> RequestToken {
        self.request_token
    }

    pub(super) fn shape_id(&self) -> &ResultShapeId {
        &self.shape_id
    }

    pub(super) fn into_payload(self) -> ProviderResultPayload {
        self.payload
    }
}

/// Operation-specific provider evidence carried by [`ProviderResultEvidence`].
#[derive(Debug)]
pub(super) enum ProviderResultPayload {
    /// Complete positive assignments for fetched rows.
    Rows {
        /// Provider solutions in claimed stable operation order.
        solutions: Vec<ProviderSolutionEvidence>,
        /// Whether validation must apply the request window after full-prefix
        /// order and selected-identity validation.
        apply_window: bool,
    },
    /// Complete positive assignments grouped by distinct page root.
    Page {
        /// Claimed page-root binding.
        root: BindingId,
        /// Exact ordered distinct-root selection from stage two.
        selected_roots: Vec<ConceptId>,
        /// Provider solutions in claimed stable root/collection order.
        solutions: Vec<ProviderSolutionEvidence>,
        /// Claimed page window.
        window: Window,
        /// Same-snapshot total when requested.
        total: Option<u64>,
    },
    /// Claimed lossless distinct-root count.
    Count {
        /// Claimed count root.
        root: BindingId,
        /// Distinct-root count.
        value: u64,
    },
    /// Claimed typed reduction rows.
    Reduction {
        /// Claimed reduced root.
        root: BindingId,
        /// Claimed group binding echo.
        group: Option<BindingId>,
        /// Claimed reduction rows.
        rows: Vec<ReductionRow>,
    },
    /// Claimed typed field-grouped reduction rows.
    FieldReduction {
        /// Claimed reduced root.
        root: BindingId,
        /// Claimed descriptor-qualified group field echo.
        group: super::ids::BoundFieldId,
        /// Claimed reduction rows.
        rows: Vec<ReductionRow>,
    },
    /// Claimed typed tuple-field-grouped reduction rows.
    FieldTupleReduction {
        /// Claimed reduced root.
        root: BindingId,
        /// Claimed ordered descriptor-qualified group fields echo.
        groups: Vec<super::ids::BoundFieldId>,
        /// Claimed reduction rows.
        rows: Vec<ReductionRow>,
    },
    /// Claimed distinct-root existence result.
    Exists {
        /// Claimed existence root.
        root: BindingId,
        /// Whether any root exists.
        value: bool,
    },
}

impl ProviderSolutionEvidence {
    /// Construct one unvalidated provider solution inside the executor boundary.
    #[allow(dead_code)] // Consumed by the typed provider executor in #176.
    pub(crate) fn new(
        bindings: Vec<BoundConceptEvidence>,
        satisfied_role_edges: Vec<RoleEdgeId>,
    ) -> Self {
        Self {
            bindings,
            satisfied_role_edges,
        }
    }

    /// Return all positive plan-binding assignments.
    pub(crate) fn bindings(&self) -> &[BoundConceptEvidence] {
        &self.bindings
    }

    /// Return canonical role edges satisfied by this exact assignment.
    pub(crate) fn satisfied_role_edges(&self) -> &[RoleEdgeId] {
        &self.satisfied_role_edges
    }
}

/// One value in a positional or named match-result slot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SlotValue {
    /// One singular selected concept.
    One(HydratedThing),
    /// One collected selection, preserving validator-defined order/multiplicity.
    Many(Vec<HydratedThing>),
}

/// One canonical positional or named match result row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchRow {
    slots: Vec<SlotValue>,
}

impl MatchRow {
    /// Construct a row inside canonical provider-result validation.
    pub(crate) fn new(slots: Vec<SlotValue>) -> Self {
        Self { slots }
    }

    /// Return slot values in exact validated output order.
    pub fn slots(&self) -> &[SlotValue] {
        &self.slots
    }
}

/// One typed reduced scalar produced by a reduce operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum ReducedValue {
    /// A lossless count; zero on an empty ungrouped stream.
    Count(u64),
    /// An integer-domain result; absent only on an empty ungrouped stream.
    Long(Option<i64>),
    /// A double-domain result; absent only on an empty ungrouped stream.
    Double(Option<f64>),
}

/// Validated group evidence carried by one reduction row.
///
/// The untagged representation preserves the existing serialized shape for
/// binding-grouped rows while permitting a canonical scalar field value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReductionGroupValue {
    /// One grouped thing identity and its hydration evidence.
    Thing(HydratedThing),
    /// One descriptor-validated owned field value.
    Field(AttributeValue),
    /// One ordered tuple of descriptor-validated owned field values.
    Fields(Vec<AttributeValue>),
}

/// One reduction result row: optional group evidence plus reducer outputs
/// in exact requested reducer order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReductionRow {
    group: Option<ReductionGroupValue>,
    values: Vec<ReducedValue>,
}

impl ReductionRow {
    /// Construct one reduction row inside the provider boundary.
    pub(crate) fn new(group: Option<HydratedThing>, values: Vec<ReducedValue>) -> Self {
        Self {
            group: group.map(ReductionGroupValue::Thing),
            values,
        }
    }

    /// Construct one field-grouped reduction row inside the provider boundary.
    pub(crate) fn new_field(group: AttributeValue, values: Vec<ReducedValue>) -> Self {
        Self {
            group: Some(ReductionGroupValue::Field(group)),
            values,
        }
    }

    /// Construct one tuple-field-grouped reduction row inside the provider boundary.
    pub(crate) fn new_fields(group: Vec<AttributeValue>, values: Vec<ReducedValue>) -> Self {
        Self {
            group: Some(ReductionGroupValue::Fields(group)),
            values,
        }
    }

    /// Return the witnessed group evidence for grouped reductions.
    pub fn group(&self) -> Option<&HydratedThing> {
        match self.group.as_ref() {
            Some(ReductionGroupValue::Thing(group)) => Some(group),
            Some(ReductionGroupValue::Field(_) | ReductionGroupValue::Fields(_)) | None => None,
        }
    }

    /// Return the witnessed descriptor-qualified field value for a
    /// field-grouped reduction.
    pub fn field_group(&self) -> Option<&AttributeValue> {
        match self.group.as_ref() {
            Some(ReductionGroupValue::Field(group)) => Some(group),
            Some(ReductionGroupValue::Thing(_) | ReductionGroupValue::Fields(_)) | None => None,
        }
    }

    /// Return the witnessed ordered descriptor-qualified field values for a
    /// tuple-field-grouped reduction.
    pub fn field_groups(&self) -> Option<&[AttributeValue]> {
        match self.group.as_ref() {
            Some(ReductionGroupValue::Fields(groups)) => Some(groups),
            Some(ReductionGroupValue::Thing(_) | ReductionGroupValue::Field(_)) | None => None,
        }
    }

    pub(crate) const fn has_group_evidence(&self) -> bool {
        self.group.is_some()
    }

    /// Return reducer outputs in exact requested reducer order.
    pub fn values(&self) -> &[ReducedValue] {
        &self.values
    }
}

/// Canonical result variants for all match operations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MatchResult {
    /// Distinct selected identity tuples from `FetchRows`.
    Rows {
        /// Validated rows in stable operation order.
        rows: Vec<MatchRow>,
    },
    /// A distinct-root page and optional same-snapshot total.
    Page {
        /// Singular root binding used for identity grouping.
        root: BindingId,
        /// Validated rows for the selected distinct roots.
        entries: Vec<MatchRow>,
        /// Exact requested page window.
        window: Window,
        /// Same-snapshot distinct-root total when requested.
        total: Option<u64>,
    },
    /// A lossless distinct-root count.
    Count {
        /// Root binding whose distinct identities were counted.
        root: BindingId,
        /// Distinct-root count.
        value: u64,
    },
    /// Typed reduction rows from `ReduceBy`.
    Reduction {
        /// Root binding whose matched stream was reduced.
        root: BindingId,
        /// Group binding when the reduction was grouped.
        group: Option<BindingId>,
        /// Validated reduction rows; exactly one ungrouped row, or one row
        /// per witnessed distinct group identity.
        rows: Vec<ReductionRow>,
    },
    /// Typed reduction rows grouped by an owned field value.
    FieldReduction {
        /// Root binding whose matched stream was reduced.
        root: BindingId,
        /// Descriptor-qualified owned field used as the group key.
        group: super::ids::BoundFieldId,
        /// Validated reduction rows, one per witnessed distinct field value.
        rows: Vec<ReductionRow>,
    },
    /// Typed reduction rows grouped by an ordered tuple of owned field values.
    FieldTupleReduction {
        /// Root binding whose matched stream was reduced.
        root: BindingId,
        /// Ordered descriptor-qualified owned fields used as the group key.
        groups: Vec<super::ids::BoundFieldId>,
        /// Validated reduction rows, one per witnessed distinct value tuple.
        rows: Vec<ReductionRow>,
    },
    /// Whether any distinct matching root exists.
    Exists {
        /// Root binding tested for existence.
        root: BindingId,
        /// Existence result.
        value: bool,
    },
}

/// A provider result proven to match one exact validated request invocation.
///
/// This wrapper is deliberately neither serializable nor publicly
/// constructible. Its request token is invocation-local and cannot be recovered
/// from diagnostic JSON.
#[derive(Debug)]
pub struct ValidatedMatchResult {
    #[allow(dead_code)] // Read by future language-binding materializers.
    request_token: RequestToken,
    shape_id: ResultShapeId,
    result: MatchResult,
}

impl ValidatedMatchResult {
    /// Wrap a completely validated result.
    ///
    /// Visibility is restricted to the parent `match_request` implementation so
    /// only the result validator can supply the unforgeable construction seal.
    pub(super) fn new(
        _seal: super::result_validation::ValidatedResultSeal,
        request_token: RequestToken,
        shape_id: ResultShapeId,
        result: MatchResult,
    ) -> Self {
        Self {
            request_token,
            shape_id,
            result,
        }
    }

    #[cfg(test)]
    fn new_for_test(
        request_token: RequestToken,
        shape_id: ResultShapeId,
        result: MatchResult,
    ) -> Self {
        Self {
            request_token,
            shape_id,
            result,
        }
    }

    /// Return the exact validated output-shape identity.
    pub fn shape_id(&self) -> &ResultShapeId {
        &self.shape_id
    }

    /// Return the validated canonical result.
    pub fn result(&self) -> &MatchResult {
        &self.result
    }

    /// Return this result only when it belongs to the exact validated invocation.
    ///
    /// This is the trusted language-binding access gate. Shape identity alone is
    /// insufficient because two equal query shapes receive different live
    /// invocation tokens.
    #[doc(hidden)]
    pub fn for_request<'result>(
        &'result self,
        validated: &ValidatedMatchRequest,
    ) -> Result<&'result MatchResult, MatchError> {
        if self.request_token != validated.request_token() {
            return Err(MatchError::new(
                MatchErrorCategory::ResultDecode,
                "request_token_mismatch",
                "validated result belongs to a different request invocation",
            )
            .at(MatchErrorPathSegment::Result));
        }
        if self.shape_id != *validated.shape_id() {
            return Err(MatchError::new(
                MatchErrorCategory::ResultDecode,
                "result_shape_mismatch",
                "validated result shape does not match the request invocation",
            )
            .at(MatchErrorPathSegment::Result)
            .with_detail("expected", validated.shape_id().as_str())
            .with_detail("actual", self.shape_id.as_str()));
        }
        Ok(&self.result)
    }

    /// Consume the proof wrapper and return its validated result.
    pub fn into_result(self) -> MatchResult {
        self.result
    }

    /// Return the invocation token to trusted canonical-result code.
    #[allow(dead_code)] // Read by future language-binding materializers.
    pub(crate) const fn request_token(&self) -> &RequestToken {
        &self.request_token
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::_attribute::ValueType;
    use crate::_descriptor::{EntityDescriptor, OwnedAttributeDescriptor};
    use crate::_entity::Annotation;
    use crate::_registry::DescriptorRegistry;
    use crate::match_request::model::{
        FetchShape, FetchSlot, MatchBinding, MatchMode, MatchOperation, MatchPlan, MatchRequest,
        RowCardinality, Window,
    };
    use crate::match_request::validation::validate_match_request;

    fn person(name: &str, concept_id: &str) -> HydratedThing {
        let descriptor = DescriptorId::new("entity:person");
        HydratedThing::new(
            ConceptId::new(concept_id),
            descriptor.clone(),
            descriptor.clone(),
            ThingKind::Entity,
            vec![HydratedAttribute::new(
                FieldId::new(descriptor, "name"),
                vec![AttributeValue::String(name.to_owned())],
            )],
            vec![],
        )
    }

    #[test]
    fn hydrated_thing_keeps_typed_identity_descriptor_and_field_evidence() {
        let person = person("Alice", "0x01");

        assert_eq!(person.concept_id().as_str(), "0x01");
        assert_eq!(person.declared_descriptor().as_str(), "entity:person");
        assert_eq!(person.concrete_descriptor().as_str(), "entity:person");
        assert_eq!(person.kind(), ThingKind::Entity);
        assert!(person.roles().is_empty());
        assert_eq!(person.attributes()[0].field().name, "name");
        assert_eq!(
            person.attributes()[0].values(),
            &[AttributeValue::String("Alice".into())]
        );

        let encoded = serde_json::to_value(&person).unwrap();
        assert_eq!(encoded["concept_id"], "0x01");
        assert_eq!(encoded["declared_descriptor"], "entity:person");
        assert_eq!(encoded["attributes"][0]["field"]["name"], "name");
    }

    #[test]
    fn relation_role_evidence_is_typed_and_non_recursive() {
        let person_descriptor = DescriptorId::new("entity:person");
        let employment_descriptor = DescriptorId::new("relation:employment");
        let player = HydratedRolePlayer::new(
            ConceptId::new("0x01"),
            person_descriptor.clone(),
            person_descriptor,
            ThingKind::Entity,
            vec![],
        );
        let relation = HydratedThing::new(
            ConceptId::new("0x10"),
            employment_descriptor.clone(),
            employment_descriptor.clone(),
            ThingKind::Relation,
            vec![],
            vec![HydratedRole::new(
                RoleId::new(employment_descriptor, "employee"),
                vec![player],
            )],
        );

        assert_eq!(relation.roles()[0].role().name, "employee");
        assert_eq!(
            relation.roles()[0].players()[0].concept_id().as_str(),
            "0x01"
        );
        assert_eq!(relation.roles()[0].players()[0].kind(), ThingKind::Entity);
    }

    #[test]
    fn provider_solution_retains_complete_assignments_and_role_edges() {
        let evidence = ProviderSolutionEvidence::new(
            vec![BoundConceptEvidence::new(
                BindingId::new(0),
                person("Alice", "0x01"),
            )],
            vec![RoleEdgeId::new(3)],
        );

        assert_eq!(evidence.bindings()[0].binding(), BindingId::new(0));
        assert_eq!(evidence.bindings()[0].thing().concept_id().as_str(), "0x01");
        assert_eq!(evidence.satisfied_role_edges(), &[RoleEdgeId::new(3)]);
    }

    #[test]
    fn validated_result_binds_shape_and_invocation_without_serializing_token() {
        let token = crate::match_request::validation::request_token_for_test([9; 16]);
        let validated = ValidatedMatchResult::new_for_test(
            token,
            ResultShapeId::new("shape:person"),
            MatchResult::Rows {
                rows: vec![MatchRow::new(vec![SlotValue::One(person("Alice", "0x01"))])],
            },
        );

        assert_eq!(validated.request_token().as_bytes(), &[9; 16]);
        assert_eq!(validated.shape_id().as_str(), "shape:person");
        match validated.result() {
            MatchResult::Rows { rows } => assert_eq!(rows[0].slots().len(), 1),
            other => panic!("expected rows, got {other:?}"),
        }
        assert!(format!("{validated:?}").contains("RequestToken(..)"));
    }

    #[test]
    fn validated_result_access_rejects_an_equal_shape_from_another_invocation() {
        let registry = DescriptorRegistry::new();
        registry
            .register_entity(EntityDescriptor {
                type_name: "person".into(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![OwnedAttributeDescriptor {
                    field_name: "name".into(),
                    attr_name: "person-name".into(),
                    value_type: ValueType::String,
                    annotations: vec![Annotation::Key],
                    is_optional: false,
                    is_ordered: false,
                    doc: None,
                    meta: Default::default(),
                }],
                doc: None,
                meta: Default::default(),
            })
            .unwrap();
        let binding = BindingId::new(0);
        let request = MatchRequest::v1(
            MatchPlan {
                bindings: vec![MatchBinding {
                    id: binding,
                    descriptor: registry.descriptor_id("person").unwrap(),
                    thing_kind: ThingKind::Entity,
                    match_mode: MatchMode::Exact,
                }],
                predicate: None,
                allowed_cross_joins: BTreeSet::new(),
            },
            MatchOperation::FetchRows {
                output: FetchShape::Positional {
                    slots: vec![FetchSlot::One { binding }],
                },
                order: vec![],
                window: Window {
                    offset: 0,
                    limit: 1,
                },
                cardinality: RowCardinality::BoundedMany,
            },
        );
        let first = validate_match_request(&registry, request.clone()).unwrap();
        let second = validate_match_request(&registry, request).unwrap();
        assert_eq!(first.shape_id(), second.shape_id());

        let result = ValidatedMatchResult::new_for_test(
            first.request_token(),
            first.shape_id().clone(),
            MatchResult::Rows { rows: vec![] },
        );

        assert!(result.for_request(&first).is_ok());
        let error = result.for_request(&second).unwrap_err();
        assert_eq!(error.category(), MatchErrorCategory::ResultDecode);
        assert_eq!(error.code().as_str(), "request_token_mismatch");
    }

    #[test]
    fn count_and_exists_keep_lossless_root_identity() {
        let count = MatchResult::Count {
            root: BindingId::new(4),
            value: u64::MAX,
        };
        let exists = MatchResult::Exists {
            root: BindingId::new(4),
            value: true,
        };

        assert_eq!(serde_json::to_value(count).unwrap()["value"], u64::MAX);
        assert_eq!(serde_json::to_value(exists).unwrap()["value"], true);
    }

    #[test]
    fn field_grouping_extends_reduction_rows_without_changing_thing_group_json() {
        let thing_group = ReductionRow::new(
            Some(person("Alice", "person-1")),
            vec![ReducedValue::Count(2)],
        );
        let encoded = serde_json::to_value(&thing_group).unwrap();
        assert_eq!(encoded["group"]["concept_id"], "person-1");
        assert!(encoded["group"].get("Thing").is_none());
        assert!(encoded["group"].get("Field").is_none());
        let decoded: ReductionRow = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, thing_group);

        let field_group = ReductionRow::new_field(
            AttributeValue::String("Engineering".into()),
            vec![ReducedValue::Count(3)],
        );
        let encoded = serde_json::to_value(&field_group).unwrap();
        assert_eq!(encoded["group"]["String"], "Engineering");
        assert!(encoded["group"].get("Thing").is_none());
        assert!(encoded["group"].get("Field").is_none());
        let decoded: ReductionRow = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded, field_group);
    }

    #[test]
    fn every_result_and_slot_variant_matches_the_checked_golden() {
        let results = vec![
            MatchResult::Rows {
                rows: vec![MatchRow::new(vec![
                    SlotValue::One(person("Alice", "person-1")),
                    SlotValue::Many(vec![person("Bob", "person-2")]),
                ])],
            },
            MatchResult::Page {
                root: BindingId::new(0),
                entries: vec![MatchRow::new(vec![SlotValue::One(person(
                    "Alice", "person-1",
                ))])],
                window: Window {
                    offset: 10,
                    limit: 20,
                },
                total: Some(31),
            },
            MatchResult::Count {
                root: BindingId::new(0),
                value: u64::MAX,
            },
            MatchResult::Reduction {
                root: BindingId::new(0),
                group: Some(BindingId::new(1)),
                rows: vec![ReductionRow::new(
                    Some(person("Acme", "company-1")),
                    vec![ReducedValue::Count(2), ReducedValue::Long(Some(72))],
                )],
            },
            MatchResult::FieldReduction {
                root: BindingId::new(0),
                group: super::super::ids::BoundFieldId::new(
                    BindingId::new(0),
                    FieldId::new(DescriptorId::new("entity:person"), "age"),
                ),
                rows: vec![ReductionRow::new_field(
                    AttributeValue::Long(36),
                    vec![ReducedValue::Count(1)],
                )],
            },
            MatchResult::FieldTupleReduction {
                root: BindingId::new(0),
                groups: vec![
                    super::super::ids::BoundFieldId::new(
                        BindingId::new(0),
                        FieldId::new(DescriptorId::new("entity:person"), "department"),
                    ),
                    super::super::ids::BoundFieldId::new(
                        BindingId::new(0),
                        FieldId::new(DescriptorId::new("entity:person"), "age"),
                    ),
                ],
                rows: vec![ReductionRow::new_fields(
                    vec![
                        AttributeValue::String("Engineering".into()),
                        AttributeValue::Long(36),
                    ],
                    vec![ReducedValue::Count(1)],
                )],
            },
            MatchResult::Exists {
                root: BindingId::new(0),
                value: true,
            },
        ];

        assert_eq!(
            serde_json::to_string(&results).unwrap(),
            include_str!("../../tests/fixtures/match_request/result-variants.json").trim()
        );
    }
}
