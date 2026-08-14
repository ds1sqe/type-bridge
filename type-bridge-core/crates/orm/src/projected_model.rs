//! Immutable binding-neutral values for exact generated-model execution.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use type_bridge_contract::codec::to_canonical_json;
use type_bridge_contract::id::{RoleId, TypeId, TypeKind, is_canonical_thing_iid};
use type_bridge_contract::limits::{MAX_CANONICAL_BYTES, MAX_CANONICAL_COLLECTION_LEN};
use type_bridge_contract::projection::{
    BindingProjectionFingerprint, BindingTarget, ModelProjection, ProjectedModelForm,
    ProjectedModelUse, ProjectedMultiplicity, ReadRoleProjection,
};
use type_bridge_contract::schema::{AnnotationKindId, OwnsFactId};
use type_bridge_contract::schema_fingerprint::SemanticSchemaFingerprint;
use type_bridge_contract::sdk_diagnostic::{
    SdkDiagnosticCategory, SdkDiagnosticCode, SdkDiagnosticDetailValue, SdkDiagnosticMessage,
    SdkDiagnosticName, SdkDiagnosticPathSegment, SdkExecutionDiagnostic,
};
use type_bridge_contract::value::CanonicalValue;

use crate::runtime_projection::{InstalledRuntimeProjection, attribute_value_from_canonical};
use crate::session::database::DatabaseExecutionIdentity;
use crate::value::AttributeValue;

/// Maximum members across one projected model value, including nested references.
pub const MAX_PROJECTED_MODEL_MEMBERS: usize = MAX_CANONICAL_COLLECTION_LEN;

/// Maximum canonical payload bytes across one projected model value.
pub const MAX_PROJECTED_MODEL_BYTES: usize = MAX_CANONICAL_BYTES;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectionBrand {
    semantic: SemanticSchemaFingerprint,
    target: BindingTarget,
    projection: BindingProjectionFingerprint,
}

#[derive(Eq, PartialEq)]
struct ProjectedDatabaseOrigin(DatabaseExecutionIdentity);

impl std::fmt::Debug for ProjectedDatabaseOrigin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProjectedDatabaseOrigin([REDACTED])")
    }
}

/// Opaque database-origin proof retained behind one exact generated facade.
///
/// Bindings cannot construct or inspect this value. Its debug form is
/// permanently redacted and its private, non-serializable payload is shared so
/// bindings can retain it cheaply without exposing an identity token. It
/// exists only to preserve same-database reference fencing across complete or
/// reference hydration and a later relation write.
#[doc(hidden)]
#[derive(Clone)]
pub struct ProjectedReferenceOrigin(Arc<ProjectedDatabaseOrigin>);

impl std::fmt::Debug for ProjectedReferenceOrigin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ProjectedReferenceOrigin([REDACTED])")
    }
}

impl ProjectionBrand {
    pub(crate) fn from_installed(installed: &InstalledRuntimeProjection) -> Self {
        Self {
            semantic: installed.projection().semantic_fingerprint().clone(),
            target: installed.projection().target(),
            projection: installed.projection().projection_fingerprint().clone(),
        }
    }

    pub(crate) fn validate(
        &self,
        installed: &InstalledRuntimeProjection,
        path: Vec<SdkDiagnosticPathSegment>,
    ) -> Result<(), SdkExecutionDiagnostic> {
        let expected_semantic = installed.projection().semantic_fingerprint();
        if &self.semantic != expected_semantic {
            return Err(generated_token_package_fingerprint_mismatch(
                path,
                expected_semantic.as_fingerprint(),
                self.semantic.as_fingerprint(),
            ));
        }
        if self.target != installed.projection().target() {
            return Err(generated_token_package_mismatch(path));
        }
        let expected_projection = installed.projection().projection_fingerprint();
        if &self.projection != expected_projection {
            return Err(generated_token_package_fingerprint_mismatch(
                path,
                expected_projection.as_fingerprint(),
                self.projection.as_fingerprint(),
            ));
        }
        Ok(())
    }

    pub(crate) const fn semantic(&self) -> &SemanticSchemaFingerprint {
        &self.semantic
    }

    pub(crate) const fn target(&self) -> BindingTarget {
        self.target
    }

    pub(crate) const fn projection(&self) -> &BindingProjectionFingerprint {
        &self.projection
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ProjectedSize {
    members: usize,
    bytes: usize,
}

/// Allocation-free cached resource measure for one validated projected value.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectedResourceMeasure(ProjectedSize);

impl ProjectedResourceMeasure {
    /// Return the exact recursively charged member count.
    #[must_use]
    pub const fn members(self) -> usize {
        self.0.members
    }

    /// Return the exact recursively charged canonical byte count.
    #[must_use]
    pub const fn bytes(self) -> usize {
        self.0.bytes
    }
}

/// Binding-neutral resource accounting for one projected create payload.
///
/// This hidden SPI keeps generated bindings on the same total-member and byte
/// budget as [`ProjectedCreate::try_new`] without exposing representation or
/// target-language details.
#[doc(hidden)]
#[derive(Debug)]
pub struct ProjectedCreateBudget {
    type_id: TypeId,
    budget: ProjectedBudget,
}

#[derive(Clone, Copy, Debug, Default)]
struct ProjectedBudget(ProjectedSize);

#[derive(Clone, Copy)]
enum ValidationOrigin {
    Input,
    Hydration,
}

impl ProjectedBudget {
    fn add(
        &mut self,
        size: ProjectedSize,
        path: Vec<SdkDiagnosticPathSegment>,
    ) -> Result<(), SdkExecutionDiagnostic> {
        self.add_with_path(size, || path)
    }

    fn add_with_path(
        &mut self,
        size: ProjectedSize,
        path: impl FnOnce() -> Vec<SdkDiagnosticPathSegment>,
    ) -> Result<(), SdkExecutionDiagnostic> {
        let members = match self.0.members.checked_add(size.members) {
            Some(members) if members <= MAX_PROJECTED_MODEL_MEMBERS => members,
            Some(members) => return Err(member_limit(members, path())),
            None => return Err(member_limit(usize::MAX, path())),
        };
        let bytes = match self.0.bytes.checked_add(size.bytes) {
            Some(bytes) if bytes <= MAX_PROJECTED_MODEL_BYTES => bytes,
            Some(bytes) => return Err(byte_limit(bytes, path())),
            None => return Err(byte_limit(usize::MAX, path())),
        };
        self.0 = ProjectedSize { members, bytes };
        Ok(())
    }

    const fn finish(self) -> ProjectedSize {
        self.0
    }
}

/// One canonical scalar proven against an exact projected attribute type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedAttributeValue {
    brand: ProjectionBrand,
    attribute_type: TypeId,
    value: CanonicalValue,
    size: ProjectedSize,
}

impl ProjectedAttributeValue {
    /// Validate and brand one canonical scalar using installed projection authority.
    pub fn try_new(
        installed: &InstalledRuntimeProjection,
        attribute_type: TypeId,
        value: CanonicalValue,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        installed.validate_canonical_attribute_value(&attribute_type, &value)?;
        let bytes = type_id_bytes(&attribute_type)
            .checked_add(canonical_value_bytes(&value)?)
            .ok_or_else(|| byte_limit(usize::MAX, type_path(&attribute_type)))?;
        if bytes > MAX_PROJECTED_MODEL_BYTES {
            return Err(byte_limit(bytes, type_path(&attribute_type)));
        }
        Ok(Self {
            brand: ProjectionBrand::from_installed(installed),
            attribute_type,
            value,
            size: ProjectedSize { members: 1, bytes },
        })
    }

    /// Validate and brand one canonical match-input scalar using installed
    /// projection authority.
    ///
    /// This is the binding-neutral inverse of [`Self::to_attribute_value`].
    /// Language facades use it to admit generated attribute wrappers as
    /// schema-function scalar arguments without duplicating any of the nine
    /// canonical scalar conversions.
    #[doc(hidden)]
    pub fn try_from_attribute_value(
        installed: &InstalledRuntimeProjection,
        attribute_type: TypeId,
        value: &AttributeValue,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        let canonical =
            crate::runtime_projection::canonical_attribute_value(value).map_err(|_| {
                SdkExecutionDiagnostic::invalid_input(
                    SdkDiagnosticCode::new("wrong_scalar_domain")
                        .expect("static projected-value code is canonical"),
                    SdkDiagnosticMessage::new("projected scalar has the wrong canonical domain")
                        .expect("static projected-value message is canonical"),
                )
            })?;
        Self::try_new(installed, attribute_type, canonical)
    }

    /// Validate and brand one provider-hydrated scalar.
    ///
    /// Provider data that violates generated scalar authority is an integrity
    /// failure, while resource-limit and existing integrity failures retain
    /// their original categories.
    #[doc(hidden)]
    pub fn try_from_hydrated_attribute_value(
        installed: &InstalledRuntimeProjection,
        attribute_type: TypeId,
        value: &AttributeValue,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        let canonical =
            crate::runtime_projection::canonical_attribute_value(value).map_err(|code| {
                integrity(
                    code,
                    "The provider scalar is outside its canonical domain",
                    type_path(&attribute_type),
                )
            })?;
        Self::try_from_hydrated_canonical_value(installed, attribute_type, canonical)
    }

    /// Validate and brand one exact canonical provider-hydrated scalar.
    ///
    /// This seam retains private canonical evidence, including the selected
    /// effective offset for either side of a named-zone daylight-saving
    /// overlap, while classifying invalid provider evidence as integrity.
    #[doc(hidden)]
    pub fn try_from_hydrated_canonical_value(
        installed: &InstalledRuntimeProjection,
        attribute_type: TypeId,
        value: CanonicalValue,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        Self::try_new(installed, attribute_type, value)
            .map_err(|diagnostic| diagnostic_for_origin(diagnostic, ValidationOrigin::Hydration))
    }

    /// Return the exact projected attribute identity.
    #[must_use]
    pub const fn attribute_type(&self) -> &TypeId {
        &self.attribute_type
    }

    /// Return the validated canonical scalar.
    #[must_use]
    pub const fn value(&self) -> &CanonicalValue {
        &self.value
    }

    /// Convert this branded canonical scalar into the canonical match-input value.
    ///
    /// Named time-zone values retain their exact resolved offset and authored
    /// zone spelling, and doubles retain their exact finite IEEE-754 value.
    #[doc(hidden)]
    #[must_use]
    pub fn to_attribute_value(&self) -> AttributeValue {
        attribute_value_from_canonical(&self.value)
    }

    /// Return the cached binding-neutral resource measure without allocation.
    #[doc(hidden)]
    #[must_use]
    pub const fn resource_measure(&self) -> ProjectedResourceMeasure {
        ProjectedResourceMeasure(self.size)
    }

    /// Return the semantic-schema brand carried by this value.
    #[must_use]
    pub const fn semantic_fingerprint(&self) -> &SemanticSchemaFingerprint {
        &self.brand.semantic
    }

    /// Return the exact binding target carried by this value.
    #[must_use]
    pub const fn binding_target(&self) -> BindingTarget {
        self.brand.target
    }

    /// Return the exact binding-projection brand carried by this value.
    #[must_use]
    pub const fn projection_fingerprint(&self) -> &BindingProjectionFingerprint {
        &self.brand.projection
    }

    /// Verify that this value belongs to the supplied installed projection.
    pub fn validate_for(
        &self,
        installed: &InstalledRuntimeProjection,
    ) -> Result<(), SdkExecutionDiagnostic> {
        self.brand
            .validate(installed, type_path(&self.attribute_type))?;
        installed.validate_canonical_attribute_value(&self.attribute_type, &self.value)
    }
}

/// A nonrecursive IID-or-key reference to one exact projected thing type.
#[derive(Clone, Debug)]
pub struct ProjectedReference {
    brand: ProjectionBrand,
    database_origin: Option<ProjectedReferenceOrigin>,
    type_id: TypeId,
    iid: Option<String>,
    keys: BTreeMap<OwnsFactId, ProjectedAttributeValue>,
    size: ProjectedSize,
}

impl PartialEq for ProjectedReference {
    fn eq(&self, other: &Self) -> bool {
        self.brand == other.brand
            && self.type_id == other.type_id
            && self.iid == other.iid
            && self.keys == other.keys
            && self.size == other.size
    }
}

impl Eq for ProjectedReference {}

impl ProjectedReference {
    /// Validate and brand one exact projected reference.
    pub fn try_new(
        installed: &InstalledRuntimeProjection,
        type_id: TypeId,
        iid: Option<String>,
        keys: Vec<(OwnsFactId, ProjectedAttributeValue)>,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        Self::try_new_with_origin(installed, type_id, iid, keys, None)
    }

    /// Validate and brand one provider-hydrated, provider-free reference.
    ///
    /// This constructor is intentionally hydration-specific: it converts
    /// invalid provider identity or key evidence into an integrity diagnostic
    /// without exposing a generic diagnostic-category remapping API.
    #[doc(hidden)]
    pub fn try_new_for_hydration(
        installed: &InstalledRuntimeProjection,
        type_id: TypeId,
        iid: Option<String>,
        keys: Vec<(OwnsFactId, ProjectedAttributeValue)>,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        Self::try_new_for_hydration_with_origin_carrier(installed, type_id, iid, keys, None)
    }

    /// Validate a provider-hydrated reference while restoring its opaque origin.
    #[doc(hidden)]
    pub fn try_new_for_hydration_with_origin_carrier(
        installed: &InstalledRuntimeProjection,
        type_id: TypeId,
        iid: Option<String>,
        keys: Vec<(OwnsFactId, ProjectedAttributeValue)>,
        origin: Option<ProjectedReferenceOrigin>,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        Self::try_new_with_origin(installed, type_id, iid, keys, origin)
            .map_err(|diagnostic| diagnostic_for_origin(diagnostic, ValidationOrigin::Hydration))
    }

    /// Validate a reference while restoring an opaque hydrated origin.
    #[doc(hidden)]
    pub fn try_new_with_origin_carrier(
        installed: &InstalledRuntimeProjection,
        type_id: TypeId,
        iid: Option<String>,
        keys: Vec<(OwnsFactId, ProjectedAttributeValue)>,
        origin: Option<ProjectedReferenceOrigin>,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        Self::try_new_with_origin(installed, type_id, iid, keys, origin)
    }

    pub(crate) fn try_new_for_database(
        installed: &InstalledRuntimeProjection,
        type_id: TypeId,
        iid: Option<String>,
        keys: Vec<(OwnsFactId, ProjectedAttributeValue)>,
        database_identity: DatabaseExecutionIdentity,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        Self::try_new_with_origin(
            installed,
            type_id,
            iid,
            keys,
            Some(ProjectedReferenceOrigin(Arc::new(ProjectedDatabaseOrigin(
                database_identity,
            )))),
        )
    }

    fn try_new_with_origin(
        installed: &InstalledRuntimeProjection,
        type_id: TypeId,
        iid: Option<String>,
        keys: Vec<(OwnsFactId, ProjectedAttributeValue)>,
        database_origin: Option<ProjectedReferenceOrigin>,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        let model = require_projected_thing_model(installed, &type_id)?;
        if model.reference_read().target_name().is_none() {
            return Err(invalid_input(
                "reference_facet_not_projected",
                "The selected model has no generated reference facet",
                type_path(&type_id),
            ));
        }
        let mut budget = ProjectedBudget::default();
        budget.add(
            ProjectedSize {
                members: 1 + usize::from(iid.is_some()),
                bytes: type_id_bytes(&type_id) + iid.as_ref().map_or(0, String::len),
            },
            type_path(&type_id),
        )?;
        if iid
            .as_deref()
            .is_some_and(|value| !is_canonical_thing_iid(value))
        {
            return Err(invalid_input(
                "noncanonical_iid",
                "The reference IID is not canonical TypeDB identity text",
                argument_path(&type_id, "iid"),
            ));
        }

        let reference_keys = model
            .reference_read()
            .key_fields()
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut validated = BTreeMap::new();
        for (index, (field_id, value)) in keys.into_iter().enumerate() {
            let path = indexed_field_path(&type_id, "keys", index, &field_id);
            budget.add(
                ProjectedSize {
                    members: 1,
                    bytes: owns_fact_bytes(&field_id),
                },
                path.clone(),
            )?;
            budget.add(value.size, path.clone())?;
            value.brand.validate(installed, path.clone())?;
            if field_id.owner() != &type_id || !reference_keys.contains(&field_id) {
                return Err(invalid_input(
                    "reference_key_not_projected",
                    "The ownership token is not an exact projected reference key",
                    path,
                ));
            }
            let expected_attribute = attribute_type_for_field(&field_id);
            if value.attribute_type != expected_attribute {
                return Err(invalid_input(
                    "field_value_attribute_mismatch",
                    "The branded scalar belongs to a different attribute type",
                    indexed_field_path(&type_id, "keys", index, &field_id),
                ));
            }
            installed.validate_canonical_field_value(&type_id, &field_id, &value.value)?;
            if validated.insert(field_id.clone(), value).is_some() {
                return Err(invalid_input(
                    "duplicate_reference_key",
                    "The same projected reference key appears more than once",
                    indexed_field_path(&type_id, "keys", index, &field_id),
                ));
            }
        }
        if iid.is_none() && validated.is_empty() {
            return Err(invalid_input(
                "missing_reference_identity",
                "A projected reference requires a canonical IID or one projected key",
                type_path(&type_id),
            ));
        }
        if iid.is_none() && validated.len() != 1 {
            return Err(invalid_input(
                "ambiguous_reference_keys",
                "A key-backed projected reference requires exactly one key",
                argument_path(&type_id, "keys"),
            ));
        }
        Ok(Self {
            brand: ProjectionBrand::from_installed(installed),
            database_origin,
            type_id,
            iid,
            keys: validated,
            size: budget.finish(),
        })
    }

    /// Return the exact referenced thing type.
    #[must_use]
    pub const fn type_id(&self) -> &TypeId {
        &self.type_id
    }

    /// Return the canonical IID when this reference is IID-backed.
    #[must_use]
    pub fn iid(&self) -> Option<&str> {
        self.iid.as_deref()
    }

    /// Return deterministic exact projected key evidence.
    #[must_use]
    pub const fn keys(&self) -> &BTreeMap<OwnsFactId, ProjectedAttributeValue> {
        &self.keys
    }

    /// Return the cached binding-neutral resource measure without allocation.
    #[doc(hidden)]
    #[must_use]
    pub const fn resource_measure(&self) -> ProjectedResourceMeasure {
        ProjectedResourceMeasure(self.size)
    }

    /// Return the semantic-schema brand carried by this reference.
    #[must_use]
    pub const fn semantic_fingerprint(&self) -> &SemanticSchemaFingerprint {
        &self.brand.semantic
    }

    /// Return the exact binding target carried by this reference.
    #[must_use]
    pub const fn binding_target(&self) -> BindingTarget {
        self.brand.target
    }

    /// Return the exact binding-projection brand carried by this reference.
    #[must_use]
    pub const fn projection_fingerprint(&self) -> &BindingProjectionFingerprint {
        &self.brand.projection
    }

    pub(crate) fn database_identity(&self) -> Option<&DatabaseExecutionIdentity> {
        self.database_origin
            .as_ref()
            .map(|origin| &origin.0.as_ref().0)
    }

    /// Clone the opaque hydrated database origin, when present.
    #[doc(hidden)]
    #[must_use]
    pub fn origin_carrier(&self) -> Option<ProjectedReferenceOrigin> {
        self.database_origin.clone()
    }

    /// Verify that this reference belongs to the supplied installed projection.
    pub fn validate_for(
        &self,
        installed: &InstalledRuntimeProjection,
    ) -> Result<(), SdkExecutionDiagnostic> {
        self.brand.validate(installed, type_path(&self.type_id))
    }
}

/// One immutable nonrecursive hydrated role player.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedRolePlayer {
    reference: ProjectedReference,
    evidence: ProjectedRolePlayerEvidence,
    size: ProjectedSize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ProjectedRolePlayerEvidence {
    Legacy,
    Reference,
    Complete(BTreeMap<OwnsFactId, Vec<ProjectedAttributeValue>>),
}

impl ProjectedRolePlayer {
    /// Validate legacy hydrated nonrecursive role-player evidence.
    ///
    /// This compatibility constructor does not assert whether a historical
    /// binding hydrated a complete or reference facade. New common producers
    /// use the exact form-specific constructors below.
    pub fn try_new(
        installed: &InstalledRuntimeProjection,
        reference: ProjectedReference,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        reference.validate_for(installed)?;
        require_concrete_thing_model(installed, reference.type_id(), ValidationOrigin::Hydration)?;
        if reference.iid().is_none() {
            return Err(integrity(
                "hydrated_player_iid_missing",
                "A hydrated role player requires a canonical provider IID",
                type_path(reference.type_id()),
            ));
        }
        let size = reference.size;
        Ok(Self {
            reference,
            evidence: ProjectedRolePlayerEvidence::Legacy,
            size,
        })
    }

    /// Validate one exact reference-form player for an exact read role.
    #[doc(hidden)]
    pub fn try_new_reference_for_hydration(
        installed: &InstalledRuntimeProjection,
        read_role: &ReadRoleProjection,
        reference: ProjectedReference,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        reference.validate_for(installed)?;
        let model = require_concrete_thing_model(
            installed,
            reference.type_id(),
            ValidationOrigin::Hydration,
        )?;
        if reference.iid().is_none() {
            return Err(integrity(
                "hydrated_player_iid_missing",
                "A hydrated role player requires a canonical provider IID",
                type_path(reference.type_id()),
            ));
        }
        let selected = select_read_role_player_form(installed, read_role, reference.type_id())?;
        if selected != ProjectedModelForm::Reference {
            return Err(integrity(
                "hydrated_role_player_form_mismatch",
                "The hydrated role player form does not match its exact read-role projection",
                read_role_player_path(read_role, reference.type_id()),
            ));
        }
        validate_exact_hydrated_reference_keys(model, &reference)?;
        let size = reference.size;
        Ok(Self {
            reference,
            evidence: ProjectedRolePlayerEvidence::Reference,
            size,
        })
    }

    /// Validate one complete nonrecursive entity role player for an exact read role.
    ///
    /// The concrete player type selects its nearest projected role-player form.
    /// Complete relation players are rejected because relation materialization
    /// would recursively require their roles. Reference-form players continue
    /// to use [`Self::try_new_reference_for_hydration`].
    #[doc(hidden)]
    pub fn try_new_complete_for_hydration(
        installed: &InstalledRuntimeProjection,
        read_role: &ReadRoleProjection,
        reference: ProjectedReference,
        fields: Vec<(OwnsFactId, Vec<ProjectedAttributeValue>)>,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        reference.validate_for(installed)?;
        let model = require_concrete_thing_model(
            installed,
            reference.type_id(),
            ValidationOrigin::Hydration,
        )?;
        if reference.iid().is_none() {
            return Err(integrity(
                "hydrated_player_iid_missing",
                "A hydrated role player requires a canonical provider IID",
                type_path(reference.type_id()),
            ));
        }
        if reference.type_id().kind() != TypeKind::Entity {
            return Err(integrity(
                "complete_relation_role_player_unsupported",
                "A complete nonrecursive role player must be an entity",
                read_role_player_path(read_role, reference.type_id()),
            ));
        }
        let selected = select_read_role_player_form(installed, read_role, reference.type_id())?;
        if selected != ProjectedModelForm::Complete {
            return Err(integrity(
                "hydrated_role_player_form_mismatch",
                "The hydrated role player form does not match its exact read-role projection",
                read_role_player_path(read_role, reference.type_id()),
            ));
        }

        let mut budget = ProjectedBudget::default();
        budget.add(
            ProjectedSize {
                members: 2,
                bytes: type_id_bytes(reference.type_id()) + reference.iid().map_or(0, str::len),
            },
            read_role_player_path(read_role, reference.type_id()),
        )?;
        let supplied = collect_unique_fields(
            reference.type_id(),
            fields,
            &mut budget,
            ValidationOrigin::Hydration,
        )?;
        let fields =
            validate_read_fields(installed, reference.type_id(), model, supplied, &mut budget)?;
        validate_exact_hydrated_reference_keys(model, &reference)?;
        for field_id in model.reference_read().key_fields() {
            let Some(expected) = reference.keys().get(field_id) else {
                return Err(integrity(
                    "hydrated_reference_key_mismatch",
                    "The complete role player does not retain its exact reference key",
                    field_path(reference.type_id(), field_id),
                ));
            };
            let Some([actual]) = fields.get(field_id).map(Vec::as_slice) else {
                return Err(integrity(
                    "hydrated_reference_key_mismatch",
                    "The complete role player does not retain its exact reference key",
                    field_path(reference.type_id(), field_id),
                ));
            };
            if actual != expected {
                return Err(integrity(
                    "hydrated_reference_key_mismatch",
                    "The complete role player does not retain its exact reference key",
                    field_path(reference.type_id(), field_id),
                ));
            }
        }
        Ok(Self {
            reference,
            evidence: ProjectedRolePlayerEvidence::Complete(fields),
            size: budget.finish(),
        })
    }

    /// Return the exact role-player type.
    #[must_use]
    pub const fn type_id(&self) -> &TypeId {
        self.reference.type_id()
    }

    /// Return the canonical mandatory role-player IID.
    #[must_use]
    pub fn iid(&self) -> &str {
        self.reference
            .iid()
            .expect("projected role-player construction requires an IID")
    }

    /// Return deterministic projected key evidence carried with the player.
    #[must_use]
    pub const fn keys(&self) -> &BTreeMap<OwnsFactId, ProjectedAttributeValue> {
        self.reference.keys()
    }

    /// Return the exact common-producer form, if this is not legacy evidence.
    #[must_use]
    pub const fn exact_form(&self) -> Option<ProjectedModelForm> {
        match &self.evidence {
            ProjectedRolePlayerEvidence::Legacy => None,
            ProjectedRolePlayerEvidence::Reference => Some(ProjectedModelForm::Reference),
            ProjectedRolePlayerEvidence::Complete(_) => Some(ProjectedModelForm::Complete),
        }
    }

    /// Return complete nonrecursive fields, or an empty map for a reference player.
    #[must_use]
    pub const fn fields(&self) -> &BTreeMap<OwnsFactId, Vec<ProjectedAttributeValue>> {
        match &self.evidence {
            ProjectedRolePlayerEvidence::Complete(fields) => fields,
            ProjectedRolePlayerEvidence::Legacy | ProjectedRolePlayerEvidence::Reference => {
                const EMPTY: &BTreeMap<OwnsFactId, Vec<ProjectedAttributeValue>> = &BTreeMap::new();
                EMPTY
            }
        }
    }

    /// Return this role player's nonrecursive reference value.
    #[must_use]
    pub const fn reference(&self) -> &ProjectedReference {
        &self.reference
    }

    /// Return the cached binding-neutral resource measure without allocation.
    #[doc(hidden)]
    #[must_use]
    pub const fn resource_measure(&self) -> ProjectedResourceMeasure {
        ProjectedResourceMeasure(self.size)
    }

    pub(crate) fn form_for_read_role(
        installed: &InstalledRuntimeProjection,
        read_role: &ReadRoleProjection,
        concrete_type: &TypeId,
    ) -> Result<ProjectedModelForm, SdkExecutionDiagnostic> {
        select_read_role_player_form(installed, read_role, concrete_type)
    }

    /// Return the semantic-schema brand carried by this role player.
    #[must_use]
    pub const fn semantic_fingerprint(&self) -> &SemanticSchemaFingerprint {
        self.reference.semantic_fingerprint()
    }

    /// Return the exact binding target carried by this role player.
    #[must_use]
    pub const fn binding_target(&self) -> BindingTarget {
        self.reference.binding_target()
    }

    /// Return the exact binding-projection brand carried by this role player.
    #[must_use]
    pub const fn projection_fingerprint(&self) -> &BindingProjectionFingerprint {
        self.reference.projection_fingerprint()
    }

    /// Verify that this role player belongs to the supplied installed projection.
    pub fn validate_for(
        &self,
        installed: &InstalledRuntimeProjection,
    ) -> Result<(), SdkExecutionDiagnostic> {
        self.reference.validate_for(installed)
    }
}

fn validate_exact_hydrated_reference_keys(
    model: &ModelProjection,
    reference: &ProjectedReference,
) -> Result<(), SdkExecutionDiagnostic> {
    for field_id in model.reference_read().key_fields() {
        if !reference.keys().contains_key(field_id) {
            return Err(integrity(
                "hydrated_reference_key_mismatch",
                "The exact hydrated role player is missing a projected reference key",
                field_path(reference.type_id(), field_id),
            ));
        }
    }
    if reference.keys().len() != model.reference_read().key_fields().len() {
        return Err(integrity(
            "hydrated_reference_key_mismatch",
            "The exact hydrated role player carries a different projected reference key set",
            type_path(reference.type_id()),
        ));
    }
    Ok(())
}

impl ProjectedCreateBudget {
    /// Start the exact structural budget for one concrete generated create model.
    pub fn try_new(
        installed: &InstalledRuntimeProjection,
        type_id: &TypeId,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        let model = require_creatable_thing_model(installed, type_id)?;
        let mut budget = ProjectedBudget::default();
        budget.add(
            ProjectedSize {
                members: 1,
                bytes: type_id_bytes(type_id),
            },
            type_path(type_id),
        )?;
        for field in model.create().fields() {
            let field_id = field.token();
            budget.add(
                ProjectedSize {
                    members: 1,
                    bytes: owns_fact_bytes(field_id),
                },
                field_path(type_id, field_id),
            )?;
        }
        for role_id in model.create().roles().keys() {
            budget.add(
                ProjectedSize {
                    members: 1,
                    bytes: role_id_bytes(role_id),
                },
                role_path(type_id, role_id),
            )?;
        }
        Ok(Self {
            type_id: type_id.clone(),
            budget,
        })
    }

    /// Charge one already-validated projected field value atomically.
    pub fn try_add_field_value(
        &mut self,
        installed: &InstalledRuntimeProjection,
        field_id: &OwnsFactId,
        index: usize,
        value: &ProjectedAttributeValue,
    ) -> Result<(), SdkExecutionDiagnostic> {
        self.try_add_field_values(installed, field_id, index, std::iter::once(value))
    }

    /// Validate and atomically charge a streamed chunk of exact field values.
    pub fn try_add_field_values<'a>(
        &mut self,
        installed: &InstalledRuntimeProjection,
        field_id: &OwnsFactId,
        start_index: usize,
        values: impl ExactSizeIterator<Item = &'a ProjectedAttributeValue>,
    ) -> Result<(), SdkExecutionDiagnostic> {
        let model = require_creatable_thing_model(installed, &self.type_id)?;
        let field = model
            .create()
            .fields()
            .iter()
            .find(|field| field.token() == field_id)
            .ok_or_else(|| {
                invalid_input(
                    "field_not_creatable",
                    "The ownership token is outside the selected create facet",
                    field_path(&self.type_id, field_id),
                )
            })?;
        let total = start_index.saturating_add(values.len());
        enforce_incremental_max_cardinality(
            field_path(&self.type_id, field_id),
            field.multiplicity(),
            total,
            "field_cardinality_violation",
            "The projected ownership violates its exact cardinality",
        )?;
        let expected_attribute = attribute_type_for_field(field_id);
        let mut candidate = self.budget;
        for (offset, value) in values.enumerate() {
            let index = start_index.saturating_add(offset);
            let path = || indexed_value_path(&self.type_id, field_id, index);
            value.brand.validate(installed, path())?;
            if value.attribute_type != expected_attribute {
                return Err(invalid_input(
                    "field_value_attribute_mismatch",
                    "The branded scalar belongs to a different attribute type",
                    path(),
                ));
            }
            installed.validate_canonical_field_value(&self.type_id, field_id, &value.value)?;
            candidate.add_with_path(value.size, || {
                indexed_value_path(&self.type_id, field_id, index)
            })?;
        }
        self.budget = candidate;
        Ok(())
    }

    /// Charge one already-validated projected role reference atomically.
    pub fn try_add_role_reference(
        &mut self,
        installed: &InstalledRuntimeProjection,
        role_id: &RoleId,
        index: usize,
        reference: &ProjectedReference,
    ) -> Result<(), SdkExecutionDiagnostic> {
        self.try_add_role_references(installed, role_id, index, std::iter::once(reference))
    }

    /// Validate and atomically charge a streamed chunk of exact role references.
    pub fn try_add_role_references<'a>(
        &mut self,
        installed: &InstalledRuntimeProjection,
        role_id: &RoleId,
        start_index: usize,
        references: impl ExactSizeIterator<Item = &'a ProjectedReference>,
    ) -> Result<(), SdkExecutionDiagnostic> {
        let model = require_creatable_thing_model(installed, &self.type_id)?;
        let role = model.create().roles().get(role_id).ok_or_else(|| {
            invalid_input(
                "role_not_creatable",
                "The role token is outside the selected create facet",
                role_path(&self.type_id, role_id),
            )
        })?;
        let token = model.query_tokens().roles().get(role_id).ok_or_else(|| {
            integrity(
                "projected_role_token_missing",
                "The create facet refers to an absent projected role token",
                role_path(&self.type_id, role_id),
            )
        })?;
        let total = start_index.saturating_add(references.len());
        enforce_incremental_max_cardinality(
            role_path(&self.type_id, role_id),
            role.multiplicity(),
            total,
            "role_cardinality_violation",
            "The projected role violates its exact cardinality",
        )?;
        let mut candidate = self.budget;
        for (offset, reference) in references.enumerate() {
            let index = start_index.saturating_add(offset);
            let path = || indexed_player_path(&self.type_id, role_id, index, reference.type_id());
            reference.brand.validate(installed, path())?;
            if !role_domain_intersects(
                installed,
                role.players(),
                token.accepted_players(),
                reference.type_id(),
            ) {
                return Err(invalid_input(
                    "role_player_not_accepted",
                    "The referenced thing type is outside the projected role-player domain",
                    path(),
                ));
            }
            candidate.add_with_path(reference.size, || {
                indexed_player_path(&self.type_id, role_id, index, reference.type_id())
            })?;
        }
        self.budget = candidate;
        Ok(())
    }
}

/// An immutable exact create payload for one projected entity or relation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedCreate {
    brand: ProjectionBrand,
    type_id: TypeId,
    fields: BTreeMap<OwnsFactId, Vec<ProjectedAttributeValue>>,
    roles: BTreeMap<RoleId, Vec<ProjectedReference>>,
    size: ProjectedSize,
}

impl ProjectedCreate {
    /// Validate and brand an exact generated-model create payload.
    pub fn try_new(
        installed: &InstalledRuntimeProjection,
        type_id: TypeId,
        fields: Vec<(OwnsFactId, Vec<ProjectedAttributeValue>)>,
        roles: Vec<(RoleId, Vec<ProjectedReference>)>,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        let model = require_creatable_thing_model(installed, &type_id)?;
        let mut budget = ProjectedBudget::default();
        budget.add(
            ProjectedSize {
                members: 1,
                bytes: type_id_bytes(&type_id),
            },
            type_path(&type_id),
        )?;
        let supplied_fields =
            collect_unique_fields(&type_id, fields, &mut budget, ValidationOrigin::Input)?;
        let supplied_roles =
            collect_unique_roles(&type_id, roles, &mut budget, ValidationOrigin::Input)?;
        let fields =
            validate_create_fields(installed, &type_id, model, supplied_fields, &mut budget)?;
        let roles = validate_create_roles(installed, &type_id, model, supplied_roles, &mut budget)?;
        let size = budget.finish();
        Ok(Self {
            brand: ProjectionBrand::from_installed(installed),
            type_id,
            fields,
            roles,
            size,
        })
    }

    /// Return the exact projected model type.
    #[must_use]
    pub const fn type_id(&self) -> &TypeId {
        &self.type_id
    }

    /// Return deterministic exact ownership inputs, including normalized empty optionals.
    #[must_use]
    pub const fn fields(&self) -> &BTreeMap<OwnsFactId, Vec<ProjectedAttributeValue>> {
        &self.fields
    }

    /// Return deterministic exact role inputs, including normalized empty optionals.
    #[must_use]
    pub const fn roles(&self) -> &BTreeMap<RoleId, Vec<ProjectedReference>> {
        &self.roles
    }

    /// Return the cached binding-neutral resource measure without allocation.
    #[doc(hidden)]
    #[must_use]
    pub const fn resource_measure(&self) -> ProjectedResourceMeasure {
        ProjectedResourceMeasure(self.size)
    }

    /// Return the semantic-schema brand carried by this create value.
    #[must_use]
    pub const fn semantic_fingerprint(&self) -> &SemanticSchemaFingerprint {
        &self.brand.semantic
    }

    /// Return the exact binding target carried by this create value.
    #[must_use]
    pub const fn binding_target(&self) -> BindingTarget {
        self.brand.target
    }

    /// Return the exact binding-projection brand carried by this create value.
    #[must_use]
    pub const fn projection_fingerprint(&self) -> &BindingProjectionFingerprint {
        &self.brand.projection
    }

    /// Verify that this create value belongs to the supplied installed projection.
    pub fn validate_for(
        &self,
        installed: &InstalledRuntimeProjection,
    ) -> Result<(), SdkExecutionDiagnostic> {
        self.brand.validate(installed, type_path(&self.type_id))
    }
}

/// An immutable completely hydrated exact projected thing.
#[derive(Clone, Debug)]
pub struct ProjectedThing {
    brand: ProjectionBrand,
    database_origin: Option<ProjectedReferenceOrigin>,
    type_id: TypeId,
    iid: String,
    fields: BTreeMap<OwnsFactId, Vec<ProjectedAttributeValue>>,
    roles: BTreeMap<RoleId, Vec<ProjectedRolePlayer>>,
    size: ProjectedSize,
}

impl PartialEq for ProjectedThing {
    fn eq(&self, other: &Self) -> bool {
        self.brand == other.brand
            && self.type_id == other.type_id
            && self.iid == other.iid
            && self.fields == other.fields
            && self.roles == other.roles
            && self.size == other.size
    }
}

impl Eq for ProjectedThing {}

impl ProjectedThing {
    /// Validate and brand one complete projected provider result.
    pub fn try_new(
        installed: &InstalledRuntimeProjection,
        type_id: TypeId,
        iid: String,
        fields: Vec<(OwnsFactId, Vec<ProjectedAttributeValue>)>,
        roles: Vec<(RoleId, Vec<ProjectedRolePlayer>)>,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        Self::try_new_with_origin(installed, type_id, iid, fields, roles, None)
    }

    /// Validate a complete provider result while restoring its opaque origin.
    #[doc(hidden)]
    pub fn try_new_with_origin_carrier(
        installed: &InstalledRuntimeProjection,
        type_id: TypeId,
        iid: String,
        fields: Vec<(OwnsFactId, Vec<ProjectedAttributeValue>)>,
        roles: Vec<(RoleId, Vec<ProjectedRolePlayer>)>,
        origin: Option<ProjectedReferenceOrigin>,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        Self::try_new_with_origin(installed, type_id, iid, fields, roles, origin)
    }

    /// Validate one provider-hydrated exact model identity and IID.
    #[doc(hidden)]
    pub fn validate_hydrated_iid(
        installed: &InstalledRuntimeProjection,
        type_id: &TypeId,
        iid: &str,
    ) -> Result<(), SdkExecutionDiagnostic> {
        validate_hydrated_thing_identity(installed, type_id, iid).map(|_| ())
    }

    pub(crate) fn try_new_for_database(
        installed: &InstalledRuntimeProjection,
        type_id: TypeId,
        iid: String,
        fields: Vec<(OwnsFactId, Vec<ProjectedAttributeValue>)>,
        roles: Vec<(RoleId, Vec<ProjectedRolePlayer>)>,
        database_identity: DatabaseExecutionIdentity,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        Self::try_new_with_origin(
            installed,
            type_id,
            iid,
            fields,
            roles,
            Some(ProjectedReferenceOrigin(Arc::new(ProjectedDatabaseOrigin(
                database_identity,
            )))),
        )
    }

    fn try_new_with_origin(
        installed: &InstalledRuntimeProjection,
        type_id: TypeId,
        iid: String,
        fields: Vec<(OwnsFactId, Vec<ProjectedAttributeValue>)>,
        roles: Vec<(RoleId, Vec<ProjectedRolePlayer>)>,
        database_origin: Option<ProjectedReferenceOrigin>,
    ) -> Result<Self, SdkExecutionDiagnostic> {
        let model = validate_hydrated_thing_identity(installed, &type_id, &iid)?;
        let mut budget = ProjectedBudget::default();
        budget.add(
            ProjectedSize {
                members: 2,
                bytes: type_id_bytes(&type_id) + iid.len(),
            },
            type_path(&type_id),
        )?;
        let supplied_fields =
            collect_unique_fields(&type_id, fields, &mut budget, ValidationOrigin::Hydration)?;
        let supplied_roles =
            collect_unique_role_players(&type_id, roles, &mut budget, ValidationOrigin::Hydration)?;
        let fields =
            validate_read_fields(installed, &type_id, model, supplied_fields, &mut budget)?;
        let roles = validate_read_roles(
            installed,
            &type_id,
            model,
            supplied_roles,
            database_origin.as_ref(),
            &mut budget,
        )?;
        let size = budget.finish();
        Ok(Self {
            brand: ProjectionBrand::from_installed(installed),
            database_origin,
            type_id,
            iid,
            fields,
            roles,
            size,
        })
    }

    /// Return the exact concrete projected model type.
    #[must_use]
    pub const fn type_id(&self) -> &TypeId {
        &self.type_id
    }

    /// Return the canonical mandatory thing IID.
    #[must_use]
    pub fn iid(&self) -> &str {
        &self.iid
    }

    /// Return deterministic complete ownership evidence.
    #[must_use]
    pub const fn fields(&self) -> &BTreeMap<OwnsFactId, Vec<ProjectedAttributeValue>> {
        &self.fields
    }

    /// Return deterministic nonrecursive role-player evidence.
    #[must_use]
    pub const fn roles(&self) -> &BTreeMap<RoleId, Vec<ProjectedRolePlayer>> {
        &self.roles
    }

    /// Return the cached binding-neutral resource measure without allocation.
    #[doc(hidden)]
    #[must_use]
    pub const fn resource_measure(&self) -> ProjectedResourceMeasure {
        ProjectedResourceMeasure(self.size)
    }

    /// Derive an exact nonrecursive reference while preserving database origin.
    ///
    /// The reference carries this thing's canonical IID and one value for
    /// every projected reference key. Provider-hydrated things retain their
    /// private database execution identity; provider-free things remain
    /// unbound.
    pub fn try_to_reference(
        &self,
        installed: &InstalledRuntimeProjection,
    ) -> Result<ProjectedReference, SdkExecutionDiagnostic> {
        self.validate_for(installed)?;
        let model =
            require_concrete_thing_model(installed, &self.type_id, ValidationOrigin::Hydration)?;
        let mut keys = Vec::new();
        for field_id in model.reference_read().key_fields() {
            let Some([value]) = self.fields.get(field_id).map(Vec::as_slice) else {
                return Err(integrity(
                    "hydrated_reference_key_invalid",
                    "The hydrated thing does not carry one exact projected reference key",
                    field_path(&self.type_id, field_id),
                ));
            };
            keys.push((field_id.clone(), value.clone()));
        }
        ProjectedReference::try_new_with_origin(
            installed,
            self.type_id.clone(),
            Some(self.iid.clone()),
            keys,
            self.database_origin.clone(),
        )
        .map_err(|diagnostic| diagnostic_for_origin(diagnostic, ValidationOrigin::Hydration))
    }

    /// Clone the opaque hydrated database origin, when present.
    #[doc(hidden)]
    #[must_use]
    pub fn origin_carrier(&self) -> Option<ProjectedReferenceOrigin> {
        self.database_origin.clone()
    }

    /// Return the semantic-schema brand carried by this thing.
    #[must_use]
    pub const fn semantic_fingerprint(&self) -> &SemanticSchemaFingerprint {
        &self.brand.semantic
    }

    /// Return the exact binding target carried by this thing.
    #[must_use]
    pub const fn binding_target(&self) -> BindingTarget {
        self.brand.target
    }

    /// Return the exact binding-projection brand carried by this thing.
    #[must_use]
    pub const fn projection_fingerprint(&self) -> &BindingProjectionFingerprint {
        &self.brand.projection
    }

    /// Verify that this thing belongs to the supplied installed projection.
    pub fn validate_for(
        &self,
        installed: &InstalledRuntimeProjection,
    ) -> Result<(), SdkExecutionDiagnostic> {
        self.brand.validate(installed, type_path(&self.type_id))
    }
}

fn collect_unique_fields(
    type_id: &TypeId,
    fields: Vec<(OwnsFactId, Vec<ProjectedAttributeValue>)>,
    budget: &mut ProjectedBudget,
    origin: ValidationOrigin,
) -> Result<BTreeMap<OwnsFactId, Vec<ProjectedAttributeValue>>, SdkExecutionDiagnostic> {
    let mut supplied = BTreeMap::new();
    for (index, (field_id, values)) in fields.into_iter().enumerate() {
        let path = indexed_field_path(type_id, "fields", index, &field_id);
        budget.add(
            ProjectedSize {
                members: 1,
                bytes: owns_fact_bytes(&field_id),
            },
            path.clone(),
        )?;
        if supplied.insert(field_id.clone(), values).is_some() {
            return Err(origin_error(
                origin,
                "duplicate_field_token",
                "The same projected ownership token appears more than once",
                path,
            ));
        }
    }
    Ok(supplied)
}

fn collect_unique_roles<T>(
    type_id: &TypeId,
    roles: Vec<(RoleId, Vec<T>)>,
    budget: &mut ProjectedBudget,
    origin: ValidationOrigin,
) -> Result<BTreeMap<RoleId, Vec<T>>, SdkExecutionDiagnostic> {
    let mut supplied = BTreeMap::new();
    for (index, (role_id, players)) in roles.into_iter().enumerate() {
        let path = indexed_role_path(type_id, "roles", index, &role_id);
        budget.add(
            ProjectedSize {
                members: 1,
                bytes: role_id_bytes(&role_id),
            },
            path.clone(),
        )?;
        if supplied.insert(role_id.clone(), players).is_some() {
            return Err(origin_error(
                origin,
                "duplicate_role_token",
                "The same projected role token appears more than once",
                path,
            ));
        }
    }
    Ok(supplied)
}

fn collect_unique_role_players(
    type_id: &TypeId,
    roles: Vec<(RoleId, Vec<ProjectedRolePlayer>)>,
    budget: &mut ProjectedBudget,
    origin: ValidationOrigin,
) -> Result<BTreeMap<RoleId, Vec<ProjectedRolePlayer>>, SdkExecutionDiagnostic> {
    collect_unique_roles(type_id, roles, budget, origin)
}

fn validate_create_fields(
    installed: &InstalledRuntimeProjection,
    type_id: &TypeId,
    model: &ModelProjection,
    mut supplied: BTreeMap<OwnsFactId, Vec<ProjectedAttributeValue>>,
    budget: &mut ProjectedBudget,
) -> Result<BTreeMap<OwnsFactId, Vec<ProjectedAttributeValue>>, SdkExecutionDiagnostic> {
    let allowed = model
        .create()
        .fields()
        .iter()
        .map(|field| field.token())
        .collect::<BTreeSet<_>>();
    if let Some(field_id) = supplied.keys().find(|field_id| !allowed.contains(field_id)) {
        return Err(invalid_input(
            "field_not_creatable",
            "The ownership token is outside the selected create facet",
            field_path(type_id, field_id),
        ));
    }
    let mut output = BTreeMap::new();
    for field in model.create().fields() {
        let field_id = field.token();
        let token = model.query_tokens().fields().get(field_id).ok_or_else(|| {
            integrity(
                "projected_field_token_missing",
                "The create facet refers to an absent projected ownership token",
                field_path(type_id, field_id),
            )
        })?;
        let present = supplied.remove(field_id);
        if present.is_none() {
            budget.add(
                ProjectedSize {
                    members: 1,
                    bytes: owns_fact_bytes(field_id),
                },
                field_path(type_id, field_id),
            )?;
        }
        let values = present.unwrap_or_default();
        validate_field_values(
            installed,
            type_id,
            field_id,
            ProjectedCollectionRule::new(
                field.multiplicity(),
                token
                    .annotations()
                    .keys()
                    .any(|annotation| annotation.kind() == &AnnotationKindId::Distinct),
            ),
            &values,
            budget,
            ValidationOrigin::Input,
        )?;
        output.insert(field_id.clone(), values);
    }
    Ok(output)
}

fn validate_read_fields(
    installed: &InstalledRuntimeProjection,
    type_id: &TypeId,
    model: &ModelProjection,
    mut supplied: BTreeMap<OwnsFactId, Vec<ProjectedAttributeValue>>,
    budget: &mut ProjectedBudget,
) -> Result<BTreeMap<OwnsFactId, Vec<ProjectedAttributeValue>>, SdkExecutionDiagnostic> {
    let allowed = model
        .complete_read()
        .fields()
        .iter()
        .map(|field| field.token())
        .collect::<BTreeSet<_>>();
    if let Some(field_id) = supplied.keys().find(|field_id| !allowed.contains(field_id)) {
        return Err(integrity(
            "field_not_readable",
            "The ownership token is outside the selected complete-read facet",
            field_path(type_id, field_id),
        ));
    }
    let mut output = BTreeMap::new();
    for field in model.complete_read().fields() {
        let field_id = field.token();
        let token = model.query_tokens().fields().get(field_id).ok_or_else(|| {
            integrity(
                "projected_field_token_missing",
                "The complete-read facet refers to an absent projected ownership token",
                field_path(type_id, field_id),
            )
        })?;
        let present = supplied.remove(field_id);
        if present.is_none() {
            budget.add(
                ProjectedSize {
                    members: 1,
                    bytes: owns_fact_bytes(field_id),
                },
                field_path(type_id, field_id),
            )?;
        }
        let values = present.unwrap_or_default();
        validate_field_values(
            installed,
            type_id,
            field_id,
            ProjectedCollectionRule::new(
                field.multiplicity(),
                token
                    .annotations()
                    .keys()
                    .any(|annotation| annotation.kind() == &AnnotationKindId::Distinct),
            ),
            &values,
            budget,
            ValidationOrigin::Hydration,
        )?;
        output.insert(field_id.clone(), values);
    }
    Ok(output)
}

#[derive(Clone, Copy)]
struct ProjectedCollectionRule {
    multiplicity: ProjectedMultiplicity,
    distinct: bool,
}

impl ProjectedCollectionRule {
    const fn new(multiplicity: ProjectedMultiplicity, distinct: bool) -> Self {
        Self {
            multiplicity,
            distinct,
        }
    }
}

fn validate_field_values(
    installed: &InstalledRuntimeProjection,
    type_id: &TypeId,
    field_id: &OwnsFactId,
    rule: ProjectedCollectionRule,
    values: &[ProjectedAttributeValue],
    budget: &mut ProjectedBudget,
    origin: ValidationOrigin,
) -> Result<(), SdkExecutionDiagnostic> {
    enforce_cardinality(type_id, field_id, rule.multiplicity, values.len(), origin)?;
    let path = field_path(type_id, field_id);
    budget.add(
        combined_size(values.iter().map(|value| value.size), path.clone())?,
        path,
    )?;
    let expected_attribute = attribute_type_for_field(field_id);
    for (index, value) in values.iter().enumerate() {
        let path = indexed_value_path(type_id, field_id, index);
        value.brand.validate(installed, path.clone())?;
        if value.attribute_type != expected_attribute {
            return Err(origin_error(
                origin,
                "field_value_attribute_mismatch",
                "The branded scalar belongs to a different attribute type",
                path,
            ));
        }
        installed
            .validate_canonical_field_value(type_id, field_id, &value.value)
            .map_err(|diagnostic| diagnostic_for_origin(diagnostic, origin))?;
    }
    if rule.distinct
        && let Some((first_index, duplicate_index)) = first_canonical_scalar_duplicate(
            values
                .iter()
                .enumerate()
                .map(|(index, value)| (index, &value.value)),
        )
    {
        return Err(ordered_distinct_duplicate(
            origin,
            indexed_value_path(type_id, field_id, duplicate_index),
            first_index,
            duplicate_index,
        ));
    }
    Ok(())
}

fn validate_create_roles(
    installed: &InstalledRuntimeProjection,
    type_id: &TypeId,
    model: &ModelProjection,
    mut supplied: BTreeMap<RoleId, Vec<ProjectedReference>>,
    budget: &mut ProjectedBudget,
) -> Result<BTreeMap<RoleId, Vec<ProjectedReference>>, SdkExecutionDiagnostic> {
    let allowed = model.create().roles().keys().collect::<BTreeSet<_>>();
    if let Some(role_id) = supplied.keys().find(|role_id| !allowed.contains(role_id)) {
        return Err(invalid_input(
            "role_not_creatable",
            "The role token is outside the selected create facet",
            role_path(type_id, role_id),
        ));
    }
    let mut output = BTreeMap::new();
    for (role_id, role) in model.create().roles() {
        let present = supplied.remove(role_id);
        if present.is_none() {
            budget.add(
                ProjectedSize {
                    members: 1,
                    bytes: role_id_bytes(role_id),
                },
                role_path(type_id, role_id),
            )?;
        }
        let references = present.unwrap_or_default();
        enforce_role_cardinality(
            type_id,
            role_id,
            role.multiplicity(),
            references.len(),
            ValidationOrigin::Input,
        )?;
        let token = model.query_tokens().roles().get(role_id).ok_or_else(|| {
            integrity(
                "projected_role_token_missing",
                "The create facet refers to an absent projected role token",
                role_path(type_id, role_id),
            )
        })?;
        let role_value_path = role_path(type_id, role_id);
        budget.add(
            combined_size(
                references.iter().map(|reference| reference.size),
                role_value_path.clone(),
            )?,
            role_value_path,
        )?;
        for (index, reference) in references.iter().enumerate() {
            let path = indexed_player_path(type_id, role_id, index, reference.type_id());
            reference.brand.validate(installed, path.clone())?;
            if !role_domain_intersects(
                installed,
                role.players(),
                token.accepted_players(),
                reference.type_id(),
            ) {
                return Err(invalid_input(
                    "role_player_not_accepted",
                    "The referenced thing type is outside the projected role-player domain",
                    path,
                ));
            }
        }
        if token
            .annotations()
            .keys()
            .any(|annotation| annotation.kind() == &AnnotationKindId::Distinct)
            && let Some((first_index, duplicate_index)) =
                first_projected_reference_duplicate(&references)
        {
            return Err(ordered_distinct_duplicate(
                ValidationOrigin::Input,
                indexed_player_path(
                    type_id,
                    role_id,
                    duplicate_index,
                    references[duplicate_index].type_id(),
                ),
                first_index,
                duplicate_index,
            ));
        }
        output.insert(role_id.clone(), references);
    }
    Ok(output)
}

fn validate_read_roles(
    installed: &InstalledRuntimeProjection,
    type_id: &TypeId,
    model: &ModelProjection,
    mut supplied: BTreeMap<RoleId, Vec<ProjectedRolePlayer>>,
    database_origin: Option<&ProjectedReferenceOrigin>,
    budget: &mut ProjectedBudget,
) -> Result<BTreeMap<RoleId, Vec<ProjectedRolePlayer>>, SdkExecutionDiagnostic> {
    let allowed = model
        .complete_read()
        .roles()
        .keys()
        .collect::<BTreeSet<_>>();
    if let Some(role_id) = supplied.keys().find(|role_id| !allowed.contains(role_id)) {
        return Err(integrity(
            "role_not_readable",
            "The role token is outside the selected complete-read facet",
            role_path(type_id, role_id),
        ));
    }
    let mut output = BTreeMap::new();
    for (role_id, role) in model.complete_read().roles() {
        let present = supplied.remove(role_id);
        if present.is_none() {
            budget.add(
                ProjectedSize {
                    members: 1,
                    bytes: role_id_bytes(role_id),
                },
                role_path(type_id, role_id),
            )?;
        }
        let players = present.unwrap_or_default();
        enforce_role_cardinality(
            type_id,
            role_id,
            role.multiplicity(),
            players.len(),
            ValidationOrigin::Hydration,
        )?;
        let token = model.query_tokens().roles().get(role_id).ok_or_else(|| {
            integrity(
                "projected_role_token_missing",
                "The complete-read facet refers to an absent projected role token",
                role_path(type_id, role_id),
            )
        })?;
        let role_value_path = role_path(type_id, role_id);
        budget.add(
            combined_size(
                players.iter().map(|player| player.size),
                role_value_path.clone(),
            )?,
            role_value_path,
        )?;
        for (index, player) in players.iter().enumerate() {
            let path = indexed_player_path(type_id, role_id, index, player.type_id());
            player.reference.brand.validate(installed, path.clone())?;
            validate_role_player_database_origin(
                database_origin,
                player.reference.database_origin.as_ref(),
                path.clone(),
            )?;
            if !role_accepts_concrete(
                installed,
                role.players(),
                token.accepted_players(),
                player.type_id(),
            ) {
                return Err(integrity(
                    "hydrated_role_player_not_accepted",
                    "The hydrated thing type is outside the projected role-player domain",
                    path,
                ));
            }
            if let Some(actual) = player.exact_form() {
                let selected = select_read_role_player_form(installed, role, player.type_id())?;
                if actual != selected {
                    return Err(integrity(
                        "hydrated_role_player_form_mismatch",
                        "The hydrated role player form does not match its exact read-role projection",
                        path,
                    ));
                }
            }
        }
        if token
            .annotations()
            .keys()
            .any(|annotation| annotation.kind() == &AnnotationKindId::Distinct)
            && let Some((first_index, duplicate_index)) = first_projected_player_duplicate(&players)
        {
            return Err(ordered_distinct_duplicate(
                ValidationOrigin::Hydration,
                indexed_player_path(
                    type_id,
                    role_id,
                    duplicate_index,
                    players[duplicate_index].type_id(),
                ),
                first_index,
                duplicate_index,
            ));
        }
        output.insert(role_id.clone(), players);
    }
    Ok(output)
}

fn validate_role_player_database_origin(
    thing_origin: Option<&ProjectedReferenceOrigin>,
    player_origin: Option<&ProjectedReferenceOrigin>,
    path: Vec<SdkDiagnosticPathSegment>,
) -> Result<(), SdkExecutionDiagnostic> {
    match (thing_origin, player_origin) {
        (Some(expected), Some(actual)) if expected.0 == actual.0 => Ok(()),
        (Some(_), None) => Err(integrity(
            "hydrated_role_player_database_origin_missing",
            "A database-bound hydrated role player is missing its opaque database origin",
            path,
        )),
        (Some(_), Some(_)) => Err(integrity(
            "hydrated_role_player_database_origin_mismatch",
            "A hydrated role player belongs to a different opaque database origin",
            path,
        )),
        (None, Some(_)) => Err(integrity(
            "provider_free_role_player_database_origin_present",
            "A provider-free projected thing cannot contain a database-bound role player",
            path,
        )),
        (None, None) => Ok(()),
    }
}

fn require_projected_thing_model<'a>(
    installed: &'a InstalledRuntimeProjection,
    type_id: &TypeId,
) -> Result<&'a ModelProjection, SdkExecutionDiagnostic> {
    if !matches!(type_id.kind(), TypeKind::Entity | TypeKind::Relation) {
        return Err(invalid_input(
            "wrong_thing_kind",
            "Projected model values require an entity or relation type",
            type_path(type_id),
        ));
    }
    let model = installed
        .projection()
        .models()
        .get(type_id)
        .ok_or_else(|| {
            invalid_input(
                "model_not_projected",
                "The model type is absent from the installed runtime projection",
                type_path(type_id),
            )
        })?;
    Ok(model)
}

fn require_concrete_thing_model<'a>(
    installed: &'a InstalledRuntimeProjection,
    type_id: &TypeId,
    origin: ValidationOrigin,
) -> Result<&'a ModelProjection, SdkExecutionDiagnostic> {
    let model = require_projected_thing_model(installed, type_id)
        .map_err(|diagnostic| diagnostic_for_origin(diagnostic, origin))?;
    if model.declaration().is_abstract() || !model.declaration().is_constructible() {
        return Err(origin_error(
            origin,
            "model_not_constructible",
            "The selected projected model is abstract or not constructible",
            type_path(type_id),
        ));
    }
    Ok(model)
}

fn validate_hydrated_thing_identity<'a>(
    installed: &'a InstalledRuntimeProjection,
    type_id: &TypeId,
    iid: &str,
) -> Result<&'a ModelProjection, SdkExecutionDiagnostic> {
    let model = require_concrete_thing_model(installed, type_id, ValidationOrigin::Hydration)?;
    if !is_canonical_thing_iid(iid) {
        return Err(integrity(
            "noncanonical_hydrated_iid",
            "The hydrated thing IID is not canonical TypeDB identity text",
            argument_path(type_id, "iid"),
        ));
    }
    Ok(model)
}

fn require_creatable_thing_model<'a>(
    installed: &'a InstalledRuntimeProjection,
    type_id: &TypeId,
) -> Result<&'a ModelProjection, SdkExecutionDiagnostic> {
    let model = require_concrete_thing_model(installed, type_id, ValidationOrigin::Input)?;
    if !model.create().enabled() {
        return Err(invalid_input(
            "model_not_creatable",
            "The selected projected model has no generated create facet",
            type_path(type_id),
        ));
    }
    Ok(model)
}

fn role_domain_intersects(
    installed: &InstalledRuntimeProjection,
    projected_players: &BTreeSet<ProjectedModelUse>,
    token_players: &BTreeSet<TypeId>,
    referenced_type: &TypeId,
) -> bool {
    installed.projection().models().values().any(|candidate| {
        matches!(candidate.id().kind(), TypeKind::Entity | TypeKind::Relation)
            && !candidate.declaration().is_abstract()
            && candidate.declaration().is_constructible()
            && is_same_or_subtype(installed, candidate.id(), referenced_type)
            && projected_players
                .iter()
                .any(|allowed| is_same_or_subtype(installed, candidate.id(), allowed.id()))
            && token_players
                .iter()
                .any(|allowed| is_same_or_subtype(installed, candidate.id(), allowed))
    })
}

fn role_accepts_concrete(
    installed: &InstalledRuntimeProjection,
    projected_players: &BTreeSet<ProjectedModelUse>,
    token_players: &BTreeSet<TypeId>,
    concrete_type: &TypeId,
) -> bool {
    installed
        .projection()
        .models()
        .get(concrete_type)
        .is_some_and(|candidate| {
            !candidate.declaration().is_abstract()
                && candidate.declaration().is_constructible()
                && projected_players
                    .iter()
                    .any(|allowed| is_same_or_subtype(installed, concrete_type, allowed.id()))
                && token_players
                    .iter()
                    .any(|allowed| is_same_or_subtype(installed, concrete_type, allowed))
        })
}

fn select_read_role_player_form(
    installed: &InstalledRuntimeProjection,
    read_role: &ReadRoleProjection,
    concrete_type: &TypeId,
) -> Result<ProjectedModelForm, SdkExecutionDiagnostic> {
    require_concrete_thing_model(installed, concrete_type, ValidationOrigin::Hydration)?;
    let mut nearest_distance = None;
    let mut nearest_forms = BTreeSet::new();
    for allowed in read_role.players() {
        let Some(distance) = inheritance_distance(installed, concrete_type, allowed.id()) else {
            continue;
        };
        match nearest_distance {
            None => {
                nearest_distance = Some(distance);
                nearest_forms.insert(allowed.form());
            }
            Some(nearest) if distance < nearest => {
                nearest_distance = Some(distance);
                nearest_forms.clear();
                nearest_forms.insert(allowed.form());
            }
            Some(nearest) if distance == nearest => {
                nearest_forms.insert(allowed.form());
            }
            Some(_) => {}
        }
    }
    let mut forms = nearest_forms.into_iter();
    let Some(form) = forms.next() else {
        return Err(integrity(
            "hydrated_role_player_not_accepted",
            "The hydrated thing type is outside the projected read-role domain",
            read_role_player_path(read_role, concrete_type),
        ));
    };
    if forms.next().is_some() {
        return Err(integrity(
            "hydrated_role_player_form_ambiguous",
            "The concrete hydrated role player selects more than one exact projected form",
            read_role_player_path(read_role, concrete_type),
        ));
    }
    Ok(form)
}

fn inheritance_distance(
    installed: &InstalledRuntimeProjection,
    candidate: &TypeId,
    ancestor: &TypeId,
) -> Option<usize> {
    let mut current = Some(candidate);
    let mut visited = BTreeSet::new();
    let mut distance = 0_usize;
    while let Some(type_id) = current {
        if type_id == ancestor {
            return Some(distance);
        }
        if !visited.insert(type_id) {
            return None;
        }
        current = installed
            .projection()
            .models()
            .get(type_id)
            .and_then(|model| model.declaration().parent());
        distance = distance.checked_add(1)?;
    }
    None
}

fn is_same_or_subtype(
    installed: &InstalledRuntimeProjection,
    candidate: &TypeId,
    ancestor: &TypeId,
) -> bool {
    inheritance_distance(installed, candidate, ancestor).is_some()
}

fn enforce_cardinality(
    type_id: &TypeId,
    field_id: &OwnsFactId,
    multiplicity: ProjectedMultiplicity,
    count: usize,
    origin: ValidationOrigin,
) -> Result<(), SdkExecutionDiagnostic> {
    enforce_exact_cardinality(
        origin,
        multiplicity,
        count,
        field_path(type_id, field_id),
        "missing_required_field",
        "A required projected ownership has no value",
        "field_cardinality_violation",
        "The projected ownership violates its exact cardinality",
    )
}

fn first_canonical_scalar_duplicate<'value>(
    values: impl IntoIterator<Item = (usize, &'value CanonicalValue)>,
) -> Option<(usize, usize)> {
    let mut values = values.into_iter().collect::<Vec<_>>();
    values.sort_by(|(left_index, left), (right_index, right)| {
        canonical_scalar_cmp(left, right).then_with(|| left_index.cmp(right_index))
    });
    values
        .windows(2)
        .filter_map(|pair| {
            let [(left_index, left), (right_index, right)] = pair else {
                return None;
            };
            if !canonical_scalar_equal(left, right) {
                return None;
            }
            Some((
                (*left_index).min(*right_index),
                (*left_index).max(*right_index),
            ))
        })
        .min_by_key(|(first_index, duplicate_index)| (*duplicate_index, *first_index))
}

fn canonical_scalar_cmp(left: &CanonicalValue, right: &CanonicalValue) -> Ordering {
    left.semantic_cmp_same_domain(right)
        .unwrap_or_else(|| left.cmp(right))
}

fn canonical_scalar_equal(left: &CanonicalValue, right: &CanonicalValue) -> bool {
    left == right || left.semantic_cmp_same_domain(right) == Some(Ordering::Equal)
}

fn first_projected_reference_duplicate(
    references: &[ProjectedReference],
) -> Option<(usize, usize)> {
    let mut duplicate = None;
    let mut iids = BTreeMap::<&str, usize>::new();
    let mut keys = BTreeMap::<(&TypeId, &OwnsFactId), Vec<(usize, &CanonicalValue)>>::new();
    for (index, reference) in references.iter().enumerate() {
        if let Some(iid) = reference.iid() {
            if let Some(first_index) = iids.insert(iid, index) {
                retain_earliest_duplicate(&mut duplicate, (first_index, index));
            }
        } else if let Some((field_id, value)) = reference.keys().iter().next() {
            keys.entry((reference.type_id(), field_id))
                .or_default()
                .push((index, value.value()));
        }
    }
    for values in keys.into_values() {
        if let Some(candidate) = first_canonical_scalar_duplicate(values) {
            retain_earliest_duplicate(&mut duplicate, candidate);
        }
    }
    duplicate
}

fn first_projected_player_duplicate(players: &[ProjectedRolePlayer]) -> Option<(usize, usize)> {
    let mut duplicate = None;
    let mut iids = BTreeMap::<&str, usize>::new();
    for (index, player) in players.iter().enumerate() {
        if let Some(first_index) = iids.insert(player.iid(), index) {
            retain_earliest_duplicate(&mut duplicate, (first_index, index));
        }
    }
    duplicate
}

fn retain_earliest_duplicate(retained: &mut Option<(usize, usize)>, candidate: (usize, usize)) {
    if retained.is_none_or(|current| (candidate.1, candidate.0) < (current.1, current.0)) {
        *retained = Some(candidate);
    }
}

fn ordered_distinct_duplicate(
    origin: ValidationOrigin,
    path: Vec<SdkDiagnosticPathSegment>,
    first_index: usize,
    duplicate_index: usize,
) -> SdkExecutionDiagnostic {
    origin_error(
        origin,
        "ordered_distinct_duplicate",
        "The ordered-distinct projected collection contains a duplicate canonical member",
        path,
    )
    .try_with_detail(
        sdk_name("first_index"),
        SdkDiagnosticDetailValue::Count(u64::try_from(first_index).unwrap_or(u64::MAX)),
    )
    .expect("the static ordered-distinct detail is unique")
    .try_with_detail(
        sdk_name("duplicate_index"),
        SdkDiagnosticDetailValue::Count(u64::try_from(duplicate_index).unwrap_or(u64::MAX)),
    )
    .expect("the static ordered-distinct details fit the contract")
}

fn enforce_role_cardinality(
    type_id: &TypeId,
    role_id: &RoleId,
    multiplicity: ProjectedMultiplicity,
    count: usize,
    origin: ValidationOrigin,
) -> Result<(), SdkExecutionDiagnostic> {
    enforce_exact_cardinality(
        origin,
        multiplicity,
        count,
        role_path(type_id, role_id),
        "missing_required_role",
        "A required projected role has no player",
        "role_cardinality_violation",
        "The projected role violates its exact cardinality",
    )
}

fn enforce_incremental_max_cardinality(
    path: Vec<SdkDiagnosticPathSegment>,
    multiplicity: ProjectedMultiplicity,
    count: usize,
    violation_code: &'static str,
    violation_message: &'static str,
) -> Result<(), SdkExecutionDiagnostic> {
    let actual = u64::try_from(count).unwrap_or(u64::MAX);
    let cardinality = multiplicity.cardinality();
    if cardinality.max().is_some_and(|maximum| actual > maximum) {
        return Err(cardinality_diagnostic(
            ValidationOrigin::Input,
            violation_code,
            violation_message,
            path,
            actual,
            cardinality.min(),
            cardinality.max(),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn enforce_exact_cardinality(
    origin: ValidationOrigin,
    multiplicity: ProjectedMultiplicity,
    count: usize,
    path: Vec<SdkDiagnosticPathSegment>,
    missing_code: &'static str,
    missing_message: &'static str,
    violation_code: &'static str,
    violation_message: &'static str,
) -> Result<(), SdkExecutionDiagnostic> {
    let count = u64::try_from(count).unwrap_or(u64::MAX);
    let cardinality = multiplicity.cardinality();
    if count == 0 && cardinality.min() > 0 {
        return Err(cardinality_diagnostic(
            origin,
            missing_code,
            missing_message,
            path,
            count,
            cardinality.min(),
            cardinality.max(),
        ));
    }
    if count < cardinality.min() || cardinality.max().is_some_and(|maximum| count > maximum) {
        return Err(cardinality_diagnostic(
            origin,
            violation_code,
            violation_message,
            path,
            count,
            cardinality.min(),
            cardinality.max(),
        ));
    }
    Ok(())
}

fn cardinality_diagnostic(
    origin: ValidationOrigin,
    code: &'static str,
    message: &'static str,
    path: Vec<SdkDiagnosticPathSegment>,
    actual: u64,
    minimum: u64,
    maximum: Option<u64>,
) -> SdkExecutionDiagnostic {
    let mut diagnostic = origin_error(origin, code, message, path)
        .try_with_detail(
            sdk_name("actual_count"),
            SdkDiagnosticDetailValue::Count(actual),
        )
        .expect("the static cardinality detail is unique")
        .try_with_detail(
            sdk_name("minimum_count"),
            SdkDiagnosticDetailValue::Count(minimum),
        )
        .expect("the static cardinality details fit the contract");
    if let Some(maximum) = maximum {
        diagnostic = diagnostic
            .try_with_detail(
                sdk_name("maximum_count"),
                SdkDiagnosticDetailValue::Count(maximum),
            )
            .expect("the static cardinality details fit the contract");
    }
    diagnostic
}

fn canonical_value_bytes(value: &CanonicalValue) -> Result<usize, SdkExecutionDiagnostic> {
    to_canonical_json(value)
        .map(|bytes| bytes.len())
        .map_err(|_| {
            integrity(
                "canonical_value_measurement_failed",
                "The validated canonical scalar could not be measured",
                Vec::new(),
            )
        })
}

fn combined_size(
    sizes: impl IntoIterator<Item = ProjectedSize>,
    path: Vec<SdkDiagnosticPathSegment>,
) -> Result<ProjectedSize, SdkExecutionDiagnostic> {
    let mut combined = ProjectedSize::default();
    for size in sizes {
        combined.members = combined
            .members
            .checked_add(size.members)
            .ok_or_else(|| member_limit(usize::MAX, path.clone()))?;
        combined.bytes = combined
            .bytes
            .checked_add(size.bytes)
            .ok_or_else(|| byte_limit(usize::MAX, path.clone()))?;
    }
    Ok(combined)
}

fn attribute_type_for_field(field_id: &OwnsFactId) -> TypeId {
    TypeId::new(TypeKind::Attribute, field_id.attribute().label().as_str())
        .expect("an ownership fact always carries a validated attribute label")
}

fn type_id_bytes(type_id: &TypeId) -> usize {
    1_usize.saturating_add(type_id.label().as_str().len())
}

fn owns_fact_bytes(field_id: &OwnsFactId) -> usize {
    type_id_bytes(field_id.owner())
        .saturating_add(1)
        .saturating_add(field_id.attribute().label().as_str().len())
}

fn role_id_bytes(role_id: &RoleId) -> usize {
    role_id
        .declaring_relation()
        .as_str()
        .len()
        .saturating_add(1)
        .saturating_add(role_id.label().as_str().len())
}

fn generated_token_package_fingerprint_mismatch(
    path: Vec<SdkDiagnosticPathSegment>,
    expected: &type_bridge_contract::fingerprint::Fingerprint,
    actual: &type_bridge_contract::fingerprint::Fingerprint,
) -> SdkExecutionDiagnostic {
    generated_token_package_mismatch(path)
        .try_with_detail(
            sdk_name("expected_fingerprint"),
            SdkDiagnosticDetailValue::Fingerprint(expected.clone()),
        )
        .expect("the static brand detail is unique")
        .try_with_detail(
            sdk_name("actual_fingerprint"),
            SdkDiagnosticDetailValue::Fingerprint(actual.clone()),
        )
        .expect("the static brand details fit the contract")
}

fn generated_token_package_mismatch(path: Vec<SdkDiagnosticPathSegment>) -> SdkExecutionDiagnostic {
    path.into_iter().fold(
        SdkExecutionDiagnostic::generated_token_package_mismatch(),
        |diagnostic, segment| {
            diagnostic
                .try_at(segment)
                .expect("a projected-value operation path fits the SDK diagnostic contract")
        },
    )
}

fn member_limit(actual: usize, path: Vec<SdkDiagnosticPathSegment>) -> SdkExecutionDiagnostic {
    resource_limit(
        "projected_model_member_limit_exceeded",
        "The projected model value exceeds its total member ceiling",
        path,
    )
    .try_with_detail(
        sdk_name("actual_members"),
        SdkDiagnosticDetailValue::Count(u64::try_from(actual).unwrap_or(u64::MAX)),
    )
    .expect("the static resource detail is unique")
    .try_with_detail(
        sdk_name("maximum_members"),
        SdkDiagnosticDetailValue::Count(
            u64::try_from(MAX_PROJECTED_MODEL_MEMBERS).expect("member limit fits u64"),
        ),
    )
    .expect("the static resource details fit the contract")
}

fn byte_limit(actual: usize, path: Vec<SdkDiagnosticPathSegment>) -> SdkExecutionDiagnostic {
    resource_limit(
        "projected_model_byte_limit_exceeded",
        "The projected model value exceeds its total byte ceiling",
        path,
    )
    .try_with_detail(
        sdk_name("actual_bytes"),
        SdkDiagnosticDetailValue::ByteCount(u64::try_from(actual).unwrap_or(u64::MAX)),
    )
    .expect("the static resource detail is unique")
    .try_with_detail(
        sdk_name("maximum_bytes"),
        SdkDiagnosticDetailValue::ByteCount(
            u64::try_from(MAX_PROJECTED_MODEL_BYTES).expect("byte limit fits u64"),
        ),
    )
    .expect("the static resource details fit the contract")
}

fn origin_error(
    origin: ValidationOrigin,
    code: &'static str,
    message: &'static str,
    path: Vec<SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    match origin {
        ValidationOrigin::Input => invalid_input(code, message, path),
        ValidationOrigin::Hydration => integrity(code, message, path),
    }
}

fn diagnostic_for_origin(
    diagnostic: SdkExecutionDiagnostic,
    origin: ValidationOrigin,
) -> SdkExecutionDiagnostic {
    if !matches!(origin, ValidationOrigin::Hydration)
        || diagnostic.category() != SdkDiagnosticCategory::InvalidInput
    {
        return diagnostic;
    }
    let mut hydrated =
        SdkExecutionDiagnostic::integrity(diagnostic.code().clone(), diagnostic.message());
    for segment in diagnostic.path() {
        hydrated = hydrated
            .try_at(segment.clone())
            .expect("validated SDK diagnostic paths remain bounded");
    }
    for (key, value) in diagnostic.details() {
        hydrated = hydrated
            .try_with_detail(key.clone(), value.clone())
            .expect("validated SDK diagnostic details remain bounded and unique");
    }
    hydrated
}

fn invalid_input(
    code: &'static str,
    message: &'static str,
    path: Vec<SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    with_path(
        SdkExecutionDiagnostic::invalid_input(sdk_code(code), sdk_message(message)),
        path,
    )
}

fn integrity(
    code: &'static str,
    message: &'static str,
    path: Vec<SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    with_path(
        SdkExecutionDiagnostic::integrity(sdk_code(code), sdk_message(message)),
        path,
    )
}

fn resource_limit(
    code: &'static str,
    message: &'static str,
    path: Vec<SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    with_path(
        SdkExecutionDiagnostic::resource_limit(sdk_code(code), sdk_message(message)),
        path,
    )
}

fn with_path(
    mut diagnostic: SdkExecutionDiagnostic,
    path: Vec<SdkDiagnosticPathSegment>,
) -> SdkExecutionDiagnostic {
    for segment in path {
        diagnostic = diagnostic
            .try_at(segment)
            .expect("projected-model diagnostic paths fit the SDK contract");
    }
    diagnostic
}

fn sdk_code(value: &'static str) -> SdkDiagnosticCode {
    SdkDiagnosticCode::new(value).expect("static projected-model diagnostic code is canonical")
}

fn sdk_message(value: &'static str) -> SdkDiagnosticMessage {
    SdkDiagnosticMessage::new(value).expect("static projected-model diagnostic message is valid")
}

fn sdk_name(value: &'static str) -> SdkDiagnosticName {
    SdkDiagnosticName::new(value).expect("static projected-model diagnostic name is canonical")
}

fn type_path(type_id: &TypeId) -> Vec<SdkDiagnosticPathSegment> {
    vec![SdkDiagnosticPathSegment::Type(type_id.clone())]
}

fn argument_path(type_id: &TypeId, name: &'static str) -> Vec<SdkDiagnosticPathSegment> {
    vec![
        SdkDiagnosticPathSegment::Type(type_id.clone()),
        SdkDiagnosticPathSegment::Argument(sdk_name(name)),
    ]
}

fn field_path(type_id: &TypeId, field_id: &OwnsFactId) -> Vec<SdkDiagnosticPathSegment> {
    vec![
        SdkDiagnosticPathSegment::Type(type_id.clone()),
        SdkDiagnosticPathSegment::Field(field_id.clone()),
    ]
}

fn role_path(type_id: &TypeId, role_id: &RoleId) -> Vec<SdkDiagnosticPathSegment> {
    vec![
        SdkDiagnosticPathSegment::Type(type_id.clone()),
        SdkDiagnosticPathSegment::Role(role_id.clone()),
    ]
}

fn indexed_field_path(
    type_id: &TypeId,
    argument: &'static str,
    index: usize,
    field_id: &OwnsFactId,
) -> Vec<SdkDiagnosticPathSegment> {
    vec![
        SdkDiagnosticPathSegment::Type(type_id.clone()),
        SdkDiagnosticPathSegment::Argument(sdk_name(argument)),
        SdkDiagnosticPathSegment::Index(u64::try_from(index).unwrap_or(u64::MAX)),
        SdkDiagnosticPathSegment::Field(field_id.clone()),
    ]
}

fn indexed_role_path(
    type_id: &TypeId,
    argument: &'static str,
    index: usize,
    role_id: &RoleId,
) -> Vec<SdkDiagnosticPathSegment> {
    vec![
        SdkDiagnosticPathSegment::Type(type_id.clone()),
        SdkDiagnosticPathSegment::Argument(sdk_name(argument)),
        SdkDiagnosticPathSegment::Index(u64::try_from(index).unwrap_or(u64::MAX)),
        SdkDiagnosticPathSegment::Role(role_id.clone()),
    ]
}

fn indexed_value_path(
    type_id: &TypeId,
    field_id: &OwnsFactId,
    index: usize,
) -> Vec<SdkDiagnosticPathSegment> {
    vec![
        SdkDiagnosticPathSegment::Type(type_id.clone()),
        SdkDiagnosticPathSegment::Field(field_id.clone()),
        SdkDiagnosticPathSegment::Index(u64::try_from(index).unwrap_or(u64::MAX)),
    ]
}

fn indexed_player_path(
    type_id: &TypeId,
    role_id: &RoleId,
    index: usize,
    player_type: &TypeId,
) -> Vec<SdkDiagnosticPathSegment> {
    vec![
        SdkDiagnosticPathSegment::Type(type_id.clone()),
        SdkDiagnosticPathSegment::Role(role_id.clone()),
        SdkDiagnosticPathSegment::Index(u64::try_from(index).unwrap_or(u64::MAX)),
        SdkDiagnosticPathSegment::Type(player_type.clone()),
    ]
}

fn read_role_player_path(
    read_role: &ReadRoleProjection,
    player_type: &TypeId,
) -> Vec<SdkDiagnosticPathSegment> {
    vec![
        SdkDiagnosticPathSegment::Role(read_role.role().clone()),
        SdkDiagnosticPathSegment::Type(player_type.clone()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::id::AttributeId;
    use type_bridge_contract::projection::{BindingTarget, ProjectionConfig, ProjectionHandler};
    use type_bridge_contract::schema::DocumentId;
    use type_bridge_contract::value::{CanonicalDouble, CanonicalString};
    use type_bridge_schema::{SchemaDocumentSet, normalize_documents, project, resolve};

    const ORIGIN_SCHEMA: &str = r#"format: typebridge.schema/v2
attributes:
  identifier: { value: string }
entities:
  person:
    owns:
      identifier: { key: true }
relations:
  friendship:
    relates:
      friend: { card: 1 }
plays:
  person:
    friendship: [friend]
"#;

    fn origin_projection() -> InstalledRuntimeProjection {
        let documents = SchemaDocumentSet::parse([(
            DocumentId::new("projected-origin.yaml").unwrap(),
            ORIGIN_SCHEMA,
        )])
        .unwrap();
        let resolved = resolve(
            &normalize_documents(&documents).unwrap(),
            &SemanticProfileId::new("typedb-3.12.1/v1").unwrap(),
        )
        .unwrap();
        let runtime = project(
            &resolved,
            BindingTarget::Rust,
            &ProjectionConfig::rust(),
            &[ProjectionHandler::rust_v1()],
            &[],
        )
        .unwrap();
        InstalledRuntimeProjection::try_new(runtime).unwrap()
    }

    fn origin_complete_player(
        installed: &InstalledRuntimeProjection,
        reference: ProjectedReference,
    ) -> ProjectedRolePlayer {
        let person = TypeId::new(TypeKind::Entity, "person").unwrap();
        let friendship = TypeId::new(TypeKind::Relation, "friendship").unwrap();
        let friend = RoleId::new("friendship", "friend").unwrap();
        let identifier =
            OwnsFactId::new(person.clone(), AttributeId::new("identifier").unwrap()).unwrap();
        let identifier_value = ProjectedAttributeValue::try_new(
            installed,
            TypeId::new(TypeKind::Attribute, "identifier").unwrap(),
            CanonicalValue::String(CanonicalString::new("ada").unwrap()),
        )
        .unwrap();
        let reference = ProjectedReference::try_new_with_origin_carrier(
            installed,
            person,
            reference.iid().map(str::to_owned),
            vec![(identifier.clone(), identifier_value.clone())],
            reference.origin_carrier(),
        )
        .unwrap();
        let read_role = &installed.projection().models()[&friendship]
            .complete_read()
            .roles()[&friend];
        ProjectedRolePlayer::try_new_complete_for_hydration(
            installed,
            read_role,
            reference,
            vec![(identifier, vec![identifier_value])],
        )
        .unwrap()
    }

    #[test]
    fn canonical_distinct_identity_uses_scalar_semantics_not_representation_bits() {
        let negative_zero = CanonicalValue::Double(CanonicalDouble::new(-0.0).unwrap());
        let positive_zero = CanonicalValue::Double(CanonicalDouble::new(0.0).unwrap());

        assert_ne!(negative_zero, positive_zero);
        assert_eq!(
            first_canonical_scalar_duplicate([(0, &negative_zero), (1, &positive_zero)]),
            Some((0, 1))
        );
    }

    #[test]
    fn total_member_budget_rejects_one_over_the_ceiling() {
        let mut budget = ProjectedBudget::default();
        let diagnostic = budget
            .add(
                ProjectedSize {
                    members: MAX_PROJECTED_MODEL_MEMBERS + 1,
                    bytes: 0,
                },
                Vec::new(),
            )
            .unwrap_err();
        assert_eq!(diagnostic.category(), SdkDiagnosticCategory::ResourceLimit);
        assert_eq!(
            diagnostic.code().as_str(),
            "projected_model_member_limit_exceeded"
        );
    }

    #[test]
    fn create_budget_charges_exact_structure_and_failed_adds_are_atomic() {
        let installed = origin_projection();
        let person = TypeId::new(TypeKind::Entity, "person").unwrap();
        let friendship = TypeId::new(TypeKind::Relation, "friendship").unwrap();
        let friend = RoleId::new("friendship", "friend").unwrap();
        let identifier =
            OwnsFactId::new(person.clone(), AttributeId::new("identifier").unwrap()).unwrap();
        let mut value = ProjectedAttributeValue::try_new(
            &installed,
            TypeId::new(TypeKind::Attribute, "identifier").unwrap(),
            CanonicalValue::String(CanonicalString::new("ada").unwrap()),
        )
        .unwrap();

        let mut budget = ProjectedCreateBudget::try_new(&installed, &person).unwrap();
        assert_eq!(budget.budget.finish().members, 2);
        budget
            .try_add_field_value(&installed, &identifier, 0, &value)
            .unwrap();
        assert_eq!(budget.budget.finish().members, 3);
        ProjectedCreate::try_new(
            &installed,
            person.clone(),
            vec![(identifier.clone(), vec![value.clone()])],
            Vec::new(),
        )
        .unwrap();

        let mut exact = ProjectedCreateBudget::try_new(&installed, &person).unwrap();
        value.size = ProjectedSize {
            members: MAX_PROJECTED_MODEL_MEMBERS - 2,
            bytes: 0,
        };
        exact
            .try_add_field_value(&installed, &identifier, 0, &value)
            .unwrap();
        let accepted = exact.budget.finish();
        assert_eq!(accepted.members, MAX_PROJECTED_MODEL_MEMBERS);

        value.size = ProjectedSize {
            members: 1,
            bytes: 0,
        };
        let diagnostic = exact
            .try_add_field_value(&installed, &identifier, 0, &value)
            .unwrap_err();
        assert_eq!(
            diagnostic.code().as_str(),
            "projected_model_member_limit_exceeded"
        );
        assert_eq!(exact.budget.finish(), accepted);

        let mut bytes = ProjectedCreateBudget::try_new(&installed, &person).unwrap();
        let before = bytes.budget.finish();
        value.size = ProjectedSize {
            members: 1,
            bytes: MAX_PROJECTED_MODEL_BYTES + 1,
        };
        let diagnostic = bytes
            .try_add_field_value(&installed, &identifier, 0, &value)
            .unwrap_err();
        assert_eq!(
            diagnostic.code().as_str(),
            "projected_model_byte_limit_exceeded"
        );
        assert_eq!(bytes.budget.finish(), before);

        let mut chunk = ProjectedCreateBudget::try_new(&installed, &person).unwrap();
        let mut first = value.clone();
        first.size = ProjectedSize {
            members: 1,
            bytes: 0,
        };
        let mut second = value.clone();
        second.size = ProjectedSize {
            members: MAX_PROJECTED_MODEL_MEMBERS,
            bytes: 0,
        };
        let before = chunk.budget.finish();
        chunk
            .try_add_field_values(&installed, &identifier, 0, [&first, &second].into_iter())
            .unwrap_err();
        assert_eq!(chunk.budget.finish(), before);

        let mut invalid = ProjectedCreateBudget::try_new(&installed, &person).unwrap();
        let before = invalid.budget.finish();
        let outside = OwnsFactId::new(
            person.clone(),
            AttributeId::new("outside-create-facet").unwrap(),
        )
        .unwrap();
        let diagnostic = invalid
            .try_add_field_value(&installed, &outside, 0, &first)
            .unwrap_err();
        assert_eq!(diagnostic.code().as_str(), "field_not_creatable");
        assert_eq!(invalid.budget.finish(), before);

        let mut wrong_nominal = first.clone();
        wrong_nominal.attribute_type = TypeId::new(TypeKind::Attribute, "wrong-attribute").unwrap();
        let diagnostic = invalid
            .try_add_field_value(&installed, &identifier, 0, &wrong_nominal)
            .unwrap_err();
        assert_eq!(diagnostic.code().as_str(), "field_value_attribute_mismatch");
        assert_eq!(invalid.budget.finish(), before);

        value.size = ProjectedSize {
            members: MAX_PROJECTED_MODEL_MEMBERS - 2,
            bytes: 0,
        };
        ProjectedCreate::try_new(
            &installed,
            person.clone(),
            vec![(identifier.clone(), vec![value.clone()])],
            Vec::new(),
        )
        .unwrap();
        value.size.members += 1;
        let diagnostic = ProjectedCreate::try_new(
            &installed,
            person.clone(),
            vec![(identifier.clone(), vec![value])],
            Vec::new(),
        )
        .unwrap_err();
        assert_eq!(
            diagnostic.code().as_str(),
            "projected_model_member_limit_exceeded"
        );

        let mut reference =
            ProjectedReference::try_new(&installed, person, Some("0x1".into()), Vec::new())
                .unwrap();
        let mut role_budget = ProjectedCreateBudget::try_new(&installed, &friendship).unwrap();
        let before = role_budget.budget.finish();
        let diagnostic = role_budget
            .try_add_role_references(&installed, &friend, 0, [&reference, &reference].into_iter())
            .unwrap_err();
        assert_eq!(diagnostic.code().as_str(), "role_cardinality_violation");
        assert_eq!(role_budget.budget.finish(), before);

        let mut wrong_player = reference.clone();
        wrong_player.type_id = friendship.clone();
        let diagnostic = role_budget
            .try_add_role_reference(&installed, &friend, 0, &wrong_player)
            .unwrap_err();
        assert_eq!(diagnostic.code().as_str(), "role_player_not_accepted");
        assert_eq!(role_budget.budget.finish(), before);

        reference.size = ProjectedSize {
            members: MAX_PROJECTED_MODEL_MEMBERS - 2,
            bytes: 0,
        };
        role_budget
            .try_add_role_reference(&installed, &friend, 0, &reference)
            .unwrap();
        assert_eq!(
            role_budget.budget.finish().members,
            MAX_PROJECTED_MODEL_MEMBERS
        );
        ProjectedCreate::try_new(
            &installed,
            friendship.clone(),
            Vec::new(),
            vec![(friend.clone(), vec![reference.clone()])],
        )
        .unwrap();
        reference.size.members += 1;
        let diagnostic = ProjectedCreate::try_new(
            &installed,
            friendship,
            Vec::new(),
            vec![(friend, vec![reference])],
        )
        .unwrap_err();
        assert_eq!(
            diagnostic.code().as_str(),
            "projected_model_member_limit_exceeded"
        );
    }

    #[test]
    fn hydrated_database_origins_are_coherent_and_redacted() {
        let identity = DatabaseExecutionIdentity::isolated("projected-model-a");
        let matching =
            ProjectedReferenceOrigin(Arc::new(ProjectedDatabaseOrigin(identity.clone())));
        let same = ProjectedReferenceOrigin(Arc::new(ProjectedDatabaseOrigin(identity)));
        let different = ProjectedReferenceOrigin(Arc::new(ProjectedDatabaseOrigin(
            DatabaseExecutionIdentity::isolated("projected-model-a"),
        )));

        validate_role_player_database_origin(Some(&matching), Some(&same), Vec::new()).unwrap();
        validate_role_player_database_origin(None, None, Vec::new()).unwrap();

        let missing =
            validate_role_player_database_origin(Some(&matching), None, Vec::new()).unwrap_err();
        assert_eq!(missing.category(), SdkDiagnosticCategory::Integrity);
        assert_eq!(
            missing.code().as_str(),
            "hydrated_role_player_database_origin_missing"
        );

        let mismatch =
            validate_role_player_database_origin(Some(&matching), Some(&different), Vec::new())
                .unwrap_err();
        assert_eq!(mismatch.category(), SdkDiagnosticCategory::Integrity);
        assert_eq!(
            mismatch.code().as_str(),
            "hydrated_role_player_database_origin_mismatch"
        );

        let stripped =
            validate_role_player_database_origin(None, Some(&matching), Vec::new()).unwrap_err();
        assert_eq!(stripped.category(), SdkDiagnosticCategory::Integrity);
        assert_eq!(
            stripped.code().as_str(),
            "provider_free_role_player_database_origin_present"
        );
        assert_eq!(
            format!("{matching:?}"),
            "ProjectedReferenceOrigin([REDACTED])"
        );
    }

    #[test]
    fn projected_thing_constructors_enforce_role_player_database_origins() {
        let installed = origin_projection();
        let person = TypeId::new(TypeKind::Entity, "person").unwrap();
        let friendship = TypeId::new(TypeKind::Relation, "friendship").unwrap();
        let friend = RoleId::new("friendship", "friend").unwrap();
        let first = DatabaseExecutionIdentity::isolated("first");
        let second = DatabaseExecutionIdentity::isolated("second");

        let unbound = origin_complete_player(
            &installed,
            ProjectedReference::try_new(&installed, person.clone(), Some("0x1".into()), vec![])
                .unwrap(),
        );
        let missing = ProjectedThing::try_new_for_database(
            &installed,
            friendship.clone(),
            "0x10".into(),
            vec![],
            vec![(friend.clone(), vec![unbound])],
            first.clone(),
        )
        .unwrap_err();
        assert_eq!(
            missing.code().as_str(),
            "hydrated_role_player_database_origin_missing"
        );

        let bound_player = |identity: DatabaseExecutionIdentity| {
            origin_complete_player(
                &installed,
                ProjectedReference::try_new_for_database(
                    &installed,
                    person.clone(),
                    Some("0x1".into()),
                    vec![],
                    identity,
                )
                .unwrap(),
            )
        };
        ProjectedThing::try_new_for_database(
            &installed,
            friendship.clone(),
            "0x11".into(),
            vec![],
            vec![(friend.clone(), vec![bound_player(first.clone())])],
            first.clone(),
        )
        .unwrap();

        let mismatch = ProjectedThing::try_new_for_database(
            &installed,
            friendship.clone(),
            "0x12".into(),
            vec![],
            vec![(friend.clone(), vec![bound_player(second)])],
            first.clone(),
        )
        .unwrap_err();
        assert_eq!(
            mismatch.code().as_str(),
            "hydrated_role_player_database_origin_mismatch"
        );

        let stripped = ProjectedThing::try_new(
            &installed,
            friendship,
            "0x13".into(),
            vec![],
            vec![(friend, vec![bound_player(first)])],
        )
        .unwrap_err();
        assert_eq!(
            stripped.code().as_str(),
            "provider_free_role_player_database_origin_present"
        );
    }

    #[test]
    fn hydration_carrier_constructors_retain_nested_and_provider_free_origins() {
        let installed = origin_projection();
        let person = TypeId::new(TypeKind::Entity, "person").unwrap();
        let friendship = TypeId::new(TypeKind::Relation, "friendship").unwrap();
        let friend = RoleId::new("friendship", "friend").unwrap();
        let identity = DatabaseExecutionIdentity::isolated("hydration-carrier");

        let bound_reference = ProjectedReference::try_new_for_database(
            &installed,
            person.clone(),
            Some("0x1".into()),
            vec![],
            identity.clone(),
        )
        .unwrap();
        let retained_origin = bound_reference.origin_carrier().unwrap();
        let cloned_origin = retained_origin.clone();
        assert!(Arc::ptr_eq(&retained_origin.0, &cloned_origin.0));
        assert_eq!(
            format!("{retained_origin:?}"),
            "ProjectedReferenceOrigin([REDACTED])"
        );
        assert!(!format!("{bound_reference:?}").contains("hydration-carrier"));
        let rebuilt_reference = ProjectedReference::try_new_for_hydration_with_origin_carrier(
            &installed,
            person.clone(),
            Some("0x1".into()),
            vec![],
            Some(cloned_origin),
        )
        .unwrap();
        assert_eq!(rebuilt_reference.database_identity(), Some(&identity));

        let noncanonical = ProjectedReference::try_new_with_origin_carrier(
            &installed,
            person.clone(),
            Some("not-an-iid".into()),
            vec![],
            Some(retained_origin.clone()),
        )
        .unwrap_err();
        assert_eq!(noncanonical.category(), SdkDiagnosticCategory::InvalidInput);
        assert_eq!(noncanonical.code().as_str(), "noncanonical_iid");

        let identifier =
            OwnsFactId::new(person.clone(), AttributeId::new("identifier").unwrap()).unwrap();
        let identifier_value = ProjectedAttributeValue::try_new(
            &installed,
            TypeId::new(TypeKind::Attribute, "identifier").unwrap(),
            CanonicalValue::String(CanonicalString::new("ada").unwrap()),
        )
        .unwrap();
        let duplicate_key = ProjectedReference::try_new_with_origin_carrier(
            &installed,
            person.clone(),
            Some("0x1".into()),
            vec![
                (identifier.clone(), identifier_value.clone()),
                (identifier, identifier_value),
            ],
            Some(retained_origin),
        )
        .unwrap_err();
        assert_eq!(
            duplicate_key.category(),
            SdkDiagnosticCategory::InvalidInput
        );
        assert_eq!(duplicate_key.code().as_str(), "duplicate_reference_key");
        let bound_player = origin_complete_player(&installed, rebuilt_reference);

        let source = ProjectedThing::try_new_for_database(
            &installed,
            friendship.clone(),
            "0x10".into(),
            vec![],
            vec![(friend.clone(), vec![bound_player.clone()])],
            identity.clone(),
        )
        .unwrap();
        let rebuilt = ProjectedThing::try_new_with_origin_carrier(
            &installed,
            friendship.clone(),
            "0x10".into(),
            vec![],
            vec![(friend.clone(), vec![bound_player])],
            source.origin_carrier(),
        )
        .unwrap();
        assert_eq!(
            rebuilt
                .database_origin
                .as_ref()
                .map(|origin| &origin.0.as_ref().0),
            Some(&identity)
        );
        assert_eq!(
            rebuilt.roles()[&friend][0].reference().database_identity(),
            Some(&identity)
        );

        let unbound_player = origin_complete_player(
            &installed,
            ProjectedReference::try_new_for_hydration_with_origin_carrier(
                &installed,
                person,
                Some("0x2".into()),
                vec![],
                None,
            )
            .unwrap(),
        );
        let unbound = ProjectedThing::try_new_with_origin_carrier(
            &installed,
            friendship,
            "0x11".into(),
            vec![],
            vec![(friend.clone(), vec![unbound_player])],
            None,
        )
        .unwrap();
        assert!(unbound.database_origin.is_none());
        assert!(
            unbound.roles()[&friend][0]
                .reference()
                .database_identity()
                .is_none()
        );
    }

    #[test]
    fn hydrated_iid_validation_has_the_complete_thing_integrity_contract() {
        let installed = origin_projection();
        let person = TypeId::new(TypeKind::Entity, "person").unwrap();
        ProjectedThing::validate_hydrated_iid(&installed, &person, "0x1").unwrap();

        let diagnostic =
            ProjectedThing::validate_hydrated_iid(&installed, &person, "not-an-iid").unwrap_err();
        assert_eq!(diagnostic.category(), SdkDiagnosticCategory::Integrity);
        assert_eq!(diagnostic.code().as_str(), "noncanonical_hydrated_iid");
        assert_eq!(
            diagnostic.path(),
            [
                SdkDiagnosticPathSegment::Type(person),
                SdkDiagnosticPathSegment::Argument(sdk_name("iid")),
            ]
        );
    }

    #[test]
    fn thing_reference_derivation_copies_keys_and_preserves_optional_database_origin() {
        let installed = origin_projection();
        let person = TypeId::new(TypeKind::Entity, "person").unwrap();
        let identifier =
            OwnsFactId::new(person.clone(), AttributeId::new("identifier").unwrap()).unwrap();
        let value = ProjectedAttributeValue::try_new(
            &installed,
            TypeId::new(TypeKind::Attribute, "identifier").unwrap(),
            CanonicalValue::String(CanonicalString::new("ada").unwrap()),
        )
        .unwrap();
        let fields = || vec![(identifier.clone(), vec![value.clone()])];

        let unbound =
            ProjectedThing::try_new(&installed, person.clone(), "0x1".into(), fields(), vec![])
                .unwrap()
                .try_to_reference(&installed)
                .unwrap();
        assert_eq!(unbound.iid(), Some("0x1"));
        assert_eq!(unbound.keys().get(&identifier), Some(&value));
        assert!(unbound.database_identity().is_none());

        let identity = DatabaseExecutionIdentity::isolated("projected-model-a");
        let bound = ProjectedThing::try_new_for_database(
            &installed,
            person,
            "0x2".into(),
            fields(),
            vec![],
            identity.clone(),
        )
        .unwrap()
        .try_to_reference(&installed)
        .unwrap();
        assert_eq!(bound.iid(), Some("0x2"));
        assert_eq!(bound.keys().get(&identifier), Some(&value));
        assert_eq!(bound.database_identity(), Some(&identity));
    }
}
