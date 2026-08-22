//! Target-independent canonical generated-model records and ordered archives.

use serde::{Deserialize, Serialize};

use crate::codec::{from_canonical_json_with_limits, to_canonical_json_with_limits};
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintDomain, SemanticProfileId,
};
use crate::id::{AttributeId, Label, RoleId, TypeId, TypeKind, is_canonical_thing_iid};
use crate::limits::{
    CodecLimits, MAX_CANONICAL_COLLECTION_LEN, MAX_CANONICAL_DEPTH, MAX_CANONICAL_STRING_BYTES,
};
use crate::schema::DeclaredIdentityFingerprint;
use crate::value::CanonicalValue;

/// Exact discriminator for one canonical projected record.
pub const PROJECTED_RECORD_V1: &str = "typebridge.projected-record/v1";
/// Record fingerprint domain.
pub const PROJECTED_RECORD_FINGERPRINT_DOMAIN: &str = "typebridge.projected-record";
/// Record fingerprint canonicalization.
pub const PROJECTED_RECORD_FINGERPRINT_CANONICALIZATION: &str = "typebridge.projected-record/v1";
/// Exact discriminator for one ordered projected archive.
pub const PROJECTED_ARCHIVE_V1: &str = "typebridge.projected-archive/v1";
/// Archive fingerprint domain.
pub const PROJECTED_ARCHIVE_FINGERPRINT_DOMAIN: &str = "typebridge.projected-archive";
/// Archive fingerprint canonicalization.
pub const PROJECTED_ARCHIVE_FINGERPRINT_CANONICALIZATION: &str = "typebridge.projected-archive/v1";

/// Maximum canonical bytes in one record.
pub const MAX_PROJECTED_RECORD_BYTES: usize = 16 * 1024 * 1024;
/// Maximum canonical bytes in one ordered archive.
pub const MAX_PROJECTED_ARCHIVE_BYTES: usize = 32 * 1024 * 1024;
/// Maximum records in one ordered archive.
pub const MAX_PROJECTED_ARCHIVE_RECORDS: usize = 4_096;
/// Maximum decoded members charged to one record or archive.
pub const MAX_PROJECTED_DECODED_WEIGHT: usize = 65_536;

const RECORD_LIMITS: CodecLimits = CodecLimits {
    max_bytes: MAX_PROJECTED_RECORD_BYTES,
    max_depth: MAX_CANONICAL_DEPTH,
    max_collection_len: MAX_CANONICAL_COLLECTION_LEN,
    max_string_bytes: MAX_CANONICAL_STRING_BYTES,
};

const ARCHIVE_LIMITS: CodecLimits = CodecLimits {
    max_bytes: MAX_PROJECTED_ARCHIVE_BYTES,
    max_depth: MAX_CANONICAL_DEPTH,
    max_collection_len: MAX_CANONICAL_COLLECTION_LEN,
    max_string_bytes: MAX_CANONICAL_STRING_BYTES,
};

/// Whether a present multi-value member retains order or canonicalizes as a set-like bag.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectedCollectionMode {
    /// Preserve the exact supplied order.
    Ordered,
    /// Sort by Rust-owned canonical representation order.
    Unordered,
}

/// Exact absence/presence for one projected field or role.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProjectedMemberValues<T> {
    /// The optional member is absent.
    Absent,
    /// The member is present, including a distinct present-empty collection.
    Present {
        /// Collection order meaning.
        collection: ProjectedCollectionMode,
        /// Complete member values.
        values: Vec<T>,
    },
}

impl<T: Ord> ProjectedMemberValues<T> {
    fn normalize(&mut self) -> Result<usize, Diagnostic> {
        let Self::Present { collection, values } = self else {
            return Ok(1);
        };
        ensure_count(values.len(), "projected_record_value_limit")?;
        if *collection == ProjectedCollectionMode::Unordered {
            values.sort_unstable();
        }
        Ok(1 + values.len())
    }
}

/// Canonical identity of one ownership field without a target-language name.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ProjectedFieldId {
    owner: TypeId,
    attribute: AttributeId,
}

impl ProjectedFieldId {
    /// Construct a canonical entity/relation ownership identity.
    pub fn new(owner: TypeId, attribute: AttributeId) -> Result<Self, Diagnostic> {
        if !matches!(owner.kind(), TypeKind::Entity | TypeKind::Relation) {
            return Err(invalid(
                "projected_record_invalid_field_owner",
                "projected field owners must be entity or relation types",
            ));
        }
        Ok(Self { owner, attribute })
    }

    /// Return the owning thing type.
    pub const fn owner(&self) -> &TypeId {
        &self.owner
    }

    /// Return the owned attribute identity.
    pub const fn attribute(&self) -> &AttributeId {
        &self.attribute
    }

    fn validate(&self) -> Result<(), Diagnostic> {
        Self::new(self.owner.clone(), self.attribute.clone()).map(|_| ())
    }
}

/// One canonical field and its exact absence/multiplicity/value state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectedField {
    identity: ProjectedFieldId,
    member: ProjectedMemberValues<CanonicalValue>,
}

impl ProjectedField {
    /// Construct one projected field.
    pub fn new(identity: ProjectedFieldId, member: ProjectedMemberValues<CanonicalValue>) -> Self {
        Self { identity, member }
    }

    /// Return the canonical ownership identity.
    pub const fn identity(&self) -> &ProjectedFieldId {
        &self.identity
    }

    /// Return exact absence/multiplicity/value state.
    pub const fn member(&self) -> &ProjectedMemberValues<CanonicalValue> {
        &self.member
    }

    fn normalize(&mut self, owner: &TypeId) -> Result<usize, Diagnostic> {
        self.identity.validate()?;
        if self.identity.owner() != owner {
            return Err(invalid(
                "projected_record_field_owner_mismatch",
                "projected field identity belongs to a different owner type",
            ));
        }
        self.member.normalize()
    }
}

/// One ordered generated struct member.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectedStructMember {
    name: Label,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<CanonicalValue>,
}

impl ProjectedStructMember {
    /// Construct a present struct member.
    pub const fn present(name: Label, value: CanonicalValue) -> Self {
        Self {
            name,
            value: Some(value),
        }
    }

    /// Construct an absent optional struct member.
    pub const fn absent(name: Label) -> Self {
        Self { name, value: None }
    }

    /// Return the declared member name.
    pub const fn name(&self) -> &Label {
        &self.name
    }

    /// Return the value, where `None` is exact optional absence.
    pub const fn value(&self) -> Option<&CanonicalValue> {
        self.value.as_ref()
    }
}

/// One exact projected reference key.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ProjectedReferenceKey {
    identity: ProjectedFieldId,
    value: CanonicalValue,
}

impl ProjectedReferenceKey {
    /// Construct one canonical reference key.
    pub const fn new(identity: ProjectedFieldId, value: CanonicalValue) -> Self {
        Self { identity, value }
    }

    /// Return the ownership identity.
    pub const fn identity(&self) -> &ProjectedFieldId {
        &self.identity
    }

    /// Return the exact canonical scalar.
    pub const fn value(&self) -> &CanonicalValue {
        &self.value
    }
}

/// A nonrecursive target-independent IID-or-key reference.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ProjectedRecordReference {
    r#type: TypeId,
    #[serde(skip_serializing_if = "Option::is_none")]
    iid: Option<String>,
    keys: Vec<ProjectedReferenceKey>,
}

impl ProjectedRecordReference {
    /// Construct and canonicalize one detached reference.
    pub fn try_new(
        r#type: TypeId,
        iid: Option<String>,
        mut keys: Vec<ProjectedReferenceKey>,
    ) -> Result<Self, Diagnostic> {
        if !matches!(r#type.kind(), TypeKind::Entity | TypeKind::Relation) {
            return Err(invalid(
                "projected_record_invalid_reference_type",
                "projected references require entity or relation types",
            ));
        }
        if iid
            .as_deref()
            .is_some_and(|value| !is_canonical_thing_iid(value))
        {
            return Err(invalid(
                "projected_record_noncanonical_iid",
                "projected record IID is not canonical",
            ));
        }
        ensure_count(keys.len(), "projected_record_key_limit")?;
        for key in &keys {
            key.identity.validate()?;
            if key.identity.owner() != &r#type {
                return Err(invalid(
                    "projected_record_reference_key_owner_mismatch",
                    "projected reference key belongs to a different thing type",
                ));
            }
        }
        keys.sort_unstable();
        if keys
            .windows(2)
            .any(|pair| pair[0].identity == pair[1].identity)
        {
            return Err(invalid(
                "projected_record_duplicate_reference_key",
                "projected reference keys must be unique",
            ));
        }
        if iid.is_none() && keys.len() != 1 {
            return Err(invalid(
                "projected_record_missing_reference_identity",
                "a key-backed projected reference requires exactly one key",
            ));
        }
        Ok(Self { r#type, iid, keys })
    }

    /// Return the concrete referenced type.
    pub const fn type_id(&self) -> &TypeId {
        &self.r#type
    }

    /// Return the canonical IID, when present.
    pub fn iid(&self) -> Option<&str> {
        self.iid.as_deref()
    }

    /// Return canonical key evidence.
    pub fn keys(&self) -> &[ProjectedReferenceKey] {
        &self.keys
    }

    fn normalize(&mut self) -> Result<usize, Diagnostic> {
        let rebuilt = Self::try_new(self.r#type.clone(), self.iid.clone(), self.keys.clone())?;
        *self = rebuilt;
        Ok(1 + usize::from(self.iid.is_some()) + self.keys.len())
    }
}

/// One hydrated role player, either reference-only or complete nonrecursive evidence.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "form", rename_all = "snake_case")]
pub enum ProjectedRecordRolePlayer {
    /// Reference-only role-player evidence.
    Reference {
        /// Exact detached reference.
        reference: ProjectedRecordReference,
    },
    /// Complete nonrecursive entity role-player evidence.
    Complete {
        /// Exact detached reference.
        reference: ProjectedRecordReference,
        /// Complete projected fields for the player.
        fields: Vec<ProjectedField>,
    },
}

impl ProjectedRecordRolePlayer {
    fn normalize(&mut self) -> Result<usize, Diagnostic> {
        match self {
            Self::Reference { reference } => reference.normalize(),
            Self::Complete { reference, fields } => {
                let mut weight = reference.normalize()?;
                normalize_fields(fields, reference.type_id(), &mut weight)?;
                Ok(weight)
            }
        }
    }
}

/// One canonical relation role and its exact absence/multiplicity/player state.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProjectedRole {
    identity: RoleId,
    member: ProjectedMemberValues<ProjectedRecordRolePlayer>,
}

impl ProjectedRole {
    /// Construct one projected role.
    pub fn new(identity: RoleId, member: ProjectedMemberValues<ProjectedRecordRolePlayer>) -> Self {
        Self { identity, member }
    }

    /// Return the canonical role identity.
    pub const fn identity(&self) -> &RoleId {
        &self.identity
    }

    /// Return exact absence/multiplicity/player state.
    pub const fn member(&self) -> &ProjectedMemberValues<ProjectedRecordRolePlayer> {
        &self.member
    }

    fn normalize(&mut self, relation: &TypeId) -> Result<usize, Diagnostic> {
        if relation.kind() != TypeKind::Relation
            || relation.label() != self.identity.declaring_relation()
        {
            return Err(invalid(
                "projected_record_role_owner_mismatch",
                "projected role identity belongs to a different relation type",
            ));
        }
        let ProjectedMemberValues::Present { collection, values } = &mut self.member else {
            return Ok(1);
        };
        ensure_count(values.len(), "projected_record_role_player_limit")?;
        let mut weight = 1;
        for value in values.iter_mut() {
            weight = checked_weight(weight, value.normalize()?)?;
        }
        if *collection == ProjectedCollectionMode::Unordered {
            let mut keyed = values
                .drain(..)
                .map(|value| {
                    let bytes = to_canonical_json_with_limits(&value, RECORD_LIMITS)?;
                    Ok((bytes, value))
                })
                .collect::<Result<Vec<_>, Diagnostic>>()?;
            keyed.sort_unstable_by(|left, right| left.0.cmp(&right.0));
            values.extend(keyed.into_iter().map(|(_, value)| value));
        }
        Ok(weight)
    }
}

/// The closed canonical projected-record content vocabulary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProjectedRecordContent {
    /// One generated attribute/scalar wrapper value.
    AttributeValue {
        /// Exact attribute type.
        r#type: TypeId,
        /// Exact scalar value.
        value: CanonicalValue,
    },
    /// One generated struct value in declared member order.
    StructValue {
        /// Exact struct type.
        r#type: TypeId,
        /// Ordered struct members.
        members: Vec<ProjectedStructMember>,
    },
    /// One generated entity create input.
    EntityCreate {
        /// Exact entity type.
        r#type: TypeId,
        /// Canonically ordered fields.
        fields: Vec<ProjectedField>,
    },
    /// One generated relation create input.
    RelationCreate {
        /// Exact relation type.
        r#type: TypeId,
        /// Canonically ordered fields.
        fields: Vec<ProjectedField>,
        /// Canonically ordered roles.
        roles: Vec<ProjectedRole>,
    },
    /// One detached hydrated entity snapshot.
    EntitySnapshot {
        /// Exact concrete entity type.
        r#type: TypeId,
        /// Canonical provider IID.
        iid: String,
        /// Complete canonically ordered projected fields.
        fields: Vec<ProjectedField>,
    },
    /// One detached hydrated relation snapshot.
    RelationSnapshot {
        /// Exact concrete relation type.
        r#type: TypeId,
        /// Canonical provider IID.
        iid: String,
        /// Complete canonically ordered projected fields.
        fields: Vec<ProjectedField>,
        /// Complete canonically ordered projected roles.
        roles: Vec<ProjectedRole>,
    },
    /// One detached nonrecursive reference.
    Reference {
        /// Exact detached reference.
        reference: ProjectedRecordReference,
    },
}

impl ProjectedRecordContent {
    /// Return the closed record-kind spelling.
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::AttributeValue { .. } => "attribute_value",
            Self::StructValue { .. } => "struct_value",
            Self::EntityCreate { .. } => "entity_create",
            Self::RelationCreate { .. } => "relation_create",
            Self::EntitySnapshot { .. } => "entity_snapshot",
            Self::RelationSnapshot { .. } => "relation_snapshot",
            Self::Reference { .. } => "reference",
        }
    }

    fn normalize(&mut self) -> Result<usize, Diagnostic> {
        let mut weight = 1;
        match self {
            Self::AttributeValue { r#type, .. } => require_kind(r#type, TypeKind::Attribute),
            Self::StructValue { r#type, members } => {
                require_kind(r#type, TypeKind::Struct)?;
                ensure_count(members.len(), "projected_record_member_limit")?;
                let mut names = std::collections::BTreeSet::new();
                for member in members.iter() {
                    if !names.insert(member.name.clone()) {
                        return Err(invalid(
                            "projected_record_duplicate_struct_member",
                            "projected struct members must be unique",
                        ));
                    }
                }
                weight = checked_weight(weight, members.len())?;
                Ok(())
            }
            Self::EntityCreate { r#type, fields } => {
                require_kind(r#type, TypeKind::Entity)?;
                normalize_fields(fields, r#type, &mut weight)
            }
            Self::RelationCreate {
                r#type,
                fields,
                roles,
            } => {
                require_kind(r#type, TypeKind::Relation)?;
                normalize_fields(fields, r#type, &mut weight)?;
                normalize_roles(roles, r#type, &mut weight)
            }
            Self::EntitySnapshot {
                r#type,
                iid,
                fields,
            } => {
                require_kind(r#type, TypeKind::Entity)?;
                require_iid(iid)?;
                normalize_fields(fields, r#type, &mut weight)
            }
            Self::RelationSnapshot {
                r#type,
                iid,
                fields,
                roles,
            } => {
                require_kind(r#type, TypeKind::Relation)?;
                require_iid(iid)?;
                normalize_fields(fields, r#type, &mut weight)?;
                normalize_roles(roles, r#type, &mut weight)
            }
            Self::Reference { reference } => {
                weight = checked_weight(weight, reference.normalize()?)?;
                Ok(())
            }
        }?;
        Ok(weight)
    }
}

/// One validated target-independent projected record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedRecord {
    semantic_profile: SemanticProfileId,
    declared_schema_identity: DeclaredIdentityFingerprint,
    content: ProjectedRecordContent,
    fingerprint: Fingerprint,
    weight: usize,
}

impl ProjectedRecord {
    /// Validate, canonicalize, and fingerprint one projected record.
    pub fn try_new(
        semantic_profile: SemanticProfileId,
        declared_schema_identity: DeclaredIdentityFingerprint,
        mut content: ProjectedRecordContent,
    ) -> Result<Self, Diagnostic> {
        let weight = content.normalize()?;
        let unsigned = RecordUnsigned {
            format: PROJECTED_RECORD_V1,
            semantic_profile: &semantic_profile,
            declared_schema_identity: declared_schema_identity.as_fingerprint(),
            record: &content,
        };
        let canonical = to_canonical_json_with_limits(&unsigned, RECORD_LIMITS)?;
        let fingerprint = Fingerprint::compute(
            FingerprintDomain::new(PROJECTED_RECORD_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(PROJECTED_RECORD_FINGERPRINT_CANONICALIZATION)?,
            Some(semantic_profile.clone()),
            &canonical,
        );
        Ok(Self {
            semantic_profile,
            declared_schema_identity,
            content,
            fingerprint,
            weight,
        })
    }

    /// Return the semantic profile bound into the canonical bytes.
    pub const fn semantic_profile(&self) -> &SemanticProfileId {
        &self.semantic_profile
    }

    /// Return the target-independent declared-schema identity.
    pub const fn declared_schema_identity(&self) -> &DeclaredIdentityFingerprint {
        &self.declared_schema_identity
    }

    /// Return closed canonical record content.
    pub const fn content(&self) -> &ProjectedRecordContent {
        &self.content
    }

    /// Return the domain-separated record fingerprint.
    pub const fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    /// Encode exact canonical record bytes.
    pub fn encode(&self) -> Result<Vec<u8>, Diagnostic> {
        to_canonical_json_with_limits(&self.to_wire(), RECORD_LIMITS)
    }

    /// Strictly decode, normalize, and verify one canonical record.
    pub fn decode(bytes: &[u8]) -> Result<Self, Diagnostic> {
        let wire: RecordWire = from_canonical_json_with_limits(bytes, RECORD_LIMITS)?;
        wire.rebuild()
    }

    fn to_wire(&self) -> RecordWireRef<'_> {
        RecordWireRef {
            format: PROJECTED_RECORD_V1,
            semantic_profile: &self.semantic_profile,
            declared_schema_identity: self.declared_schema_identity.as_fingerprint(),
            record: &self.content,
            fingerprint: &self.fingerprint,
        }
    }
}

#[derive(Serialize)]
struct RecordUnsigned<'a> {
    format: &'static str,
    semantic_profile: &'a SemanticProfileId,
    declared_schema_identity: &'a Fingerprint,
    record: &'a ProjectedRecordContent,
}

#[derive(Serialize)]
struct RecordWireRef<'a> {
    format: &'static str,
    semantic_profile: &'a SemanticProfileId,
    declared_schema_identity: &'a Fingerprint,
    record: &'a ProjectedRecordContent,
    fingerprint: &'a Fingerprint,
}

#[derive(Deserialize, Serialize)]
struct RecordWire {
    format: String,
    semantic_profile: SemanticProfileId,
    declared_schema_identity: Fingerprint,
    record: ProjectedRecordContent,
    fingerprint: Fingerprint,
}

impl RecordWire {
    fn rebuild(self) -> Result<ProjectedRecord, Diagnostic> {
        if self.format != PROJECTED_RECORD_V1 {
            return Err(invalid(
                "unsupported_projected_record_format",
                "projected record format is not supported",
            ));
        }
        let declared = DeclaredIdentityFingerprint::from_wire(self.declared_schema_identity)?;
        let rebuilt = ProjectedRecord::try_new(self.semantic_profile, declared, self.record)?;
        if rebuilt.fingerprint != self.fingerprint {
            return Err(integrity(
                "projected_record_fingerprint_mismatch",
                "projected record content differs from its fingerprint",
            ));
        }
        Ok(rebuilt)
    }
}

/// One validated eager ordered archive of complete projected records.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectedArchive {
    semantic_profile: SemanticProfileId,
    declared_schema_identity: DeclaredIdentityFingerprint,
    records: Vec<ProjectedRecord>,
    fingerprint: Fingerprint,
}

impl ProjectedArchive {
    /// Validate and fingerprint one complete ordered archive.
    pub fn try_new(records: Vec<ProjectedRecord>) -> Result<Self, Diagnostic> {
        if records.is_empty() || records.len() > MAX_PROJECTED_ARCHIVE_RECORDS {
            return Err(resource(
                "projected_archive_record_limit",
                "projected archive record count is outside the supported range",
            ));
        }
        let semantic_profile = records[0].semantic_profile.clone();
        let declared_schema_identity = records[0].declared_schema_identity.clone();
        let mut weight = 0;
        for record in &records {
            if record.semantic_profile != semantic_profile
                || record.declared_schema_identity != declared_schema_identity
            {
                return Err(invalid(
                    "projected_archive_schema_mismatch",
                    "every projected archive record must use one schema and semantic profile",
                ));
            }
            weight = checked_weight(weight, record.weight)?;
        }
        let unsigned = ArchiveUnsigned {
            format: PROJECTED_ARCHIVE_V1,
            semantic_profile: &semantic_profile,
            declared_schema_identity: declared_schema_identity.as_fingerprint(),
            records: records.iter().map(ProjectedRecord::to_wire).collect(),
        };
        let canonical = to_canonical_json_with_limits(&unsigned, ARCHIVE_LIMITS)?;
        let fingerprint = Fingerprint::compute(
            FingerprintDomain::new(PROJECTED_ARCHIVE_FINGERPRINT_DOMAIN)?,
            CanonicalizationVersion::new(PROJECTED_ARCHIVE_FINGERPRINT_CANONICALIZATION)?,
            Some(semantic_profile.clone()),
            &canonical,
        );
        Ok(Self {
            semantic_profile,
            declared_schema_identity,
            records,
            fingerprint,
        })
    }

    /// Return records in exact caller order.
    pub fn records(&self) -> &[ProjectedRecord] {
        &self.records
    }

    /// Return the domain-separated archive fingerprint.
    pub const fn fingerprint(&self) -> &Fingerprint {
        &self.fingerprint
    }

    /// Encode exact canonical archive bytes.
    pub fn encode(&self) -> Result<Vec<u8>, Diagnostic> {
        let wire = ArchiveWireRef {
            format: PROJECTED_ARCHIVE_V1,
            semantic_profile: &self.semantic_profile,
            declared_schema_identity: self.declared_schema_identity.as_fingerprint(),
            records: self.records.iter().map(ProjectedRecord::to_wire).collect(),
            fingerprint: &self.fingerprint,
        };
        to_canonical_json_with_limits(&wire, ARCHIVE_LIMITS)
    }

    /// Strictly decode and validate a complete archive before publication.
    pub fn decode(bytes: &[u8]) -> Result<Self, Diagnostic> {
        let wire: ArchiveWire = from_canonical_json_with_limits(bytes, ARCHIVE_LIMITS)?;
        if wire.format != PROJECTED_ARCHIVE_V1 {
            return Err(invalid(
                "unsupported_projected_archive_format",
                "projected archive format is not supported",
            ));
        }
        let declared = DeclaredIdentityFingerprint::from_wire(wire.declared_schema_identity)?;
        let records = wire
            .records
            .into_iter()
            .map(RecordWire::rebuild)
            .collect::<Result<Vec<_>, Diagnostic>>()?;
        let rebuilt = Self::try_new(records)?;
        if rebuilt.semantic_profile != wire.semantic_profile
            || rebuilt.declared_schema_identity != declared
            || rebuilt.fingerprint != wire.fingerprint
        {
            return Err(integrity(
                "projected_archive_fingerprint_mismatch",
                "projected archive content differs from its envelope or fingerprint",
            ));
        }
        Ok(rebuilt)
    }
}

#[derive(Serialize)]
struct ArchiveUnsigned<'a> {
    format: &'static str,
    semantic_profile: &'a SemanticProfileId,
    declared_schema_identity: &'a Fingerprint,
    records: Vec<RecordWireRef<'a>>,
}

#[derive(Serialize)]
struct ArchiveWireRef<'a> {
    format: &'static str,
    semantic_profile: &'a SemanticProfileId,
    declared_schema_identity: &'a Fingerprint,
    records: Vec<RecordWireRef<'a>>,
    fingerprint: &'a Fingerprint,
}

#[derive(Deserialize, Serialize)]
struct ArchiveWire {
    format: String,
    semantic_profile: SemanticProfileId,
    declared_schema_identity: Fingerprint,
    records: Vec<RecordWire>,
    fingerprint: Fingerprint,
}

fn normalize_fields(
    fields: &mut [ProjectedField],
    owner: &TypeId,
    weight: &mut usize,
) -> Result<(), Diagnostic> {
    ensure_count(fields.len(), "projected_record_member_limit")?;
    for field in fields.iter_mut() {
        *weight = checked_weight(*weight, field.normalize(owner)?)?;
    }
    fields.sort_unstable_by(|left, right| left.identity.cmp(&right.identity));
    if fields
        .windows(2)
        .any(|pair| pair[0].identity == pair[1].identity)
    {
        return Err(invalid(
            "projected_record_duplicate_field",
            "projected record fields must be unique",
        ));
    }
    Ok(())
}

fn normalize_roles(
    roles: &mut [ProjectedRole],
    relation: &TypeId,
    weight: &mut usize,
) -> Result<(), Diagnostic> {
    ensure_count(roles.len(), "projected_record_member_limit")?;
    for role in roles.iter_mut() {
        *weight = checked_weight(*weight, role.normalize(relation)?)?;
    }
    roles.sort_unstable_by(|left, right| left.identity.cmp(&right.identity));
    if roles
        .windows(2)
        .any(|pair| pair[0].identity == pair[1].identity)
    {
        return Err(invalid(
            "projected_record_duplicate_role",
            "projected record roles must be unique",
        ));
    }
    Ok(())
}

fn require_kind(type_id: &TypeId, expected: TypeKind) -> Result<(), Diagnostic> {
    if type_id.kind() == expected {
        Ok(())
    } else {
        Err(invalid(
            "projected_record_type_kind_mismatch",
            "projected record type kind does not match its record kind",
        ))
    }
}

fn require_iid(iid: &str) -> Result<(), Diagnostic> {
    if is_canonical_thing_iid(iid) {
        Ok(())
    } else {
        Err(invalid(
            "projected_record_noncanonical_iid",
            "projected record IID is not canonical",
        ))
    }
}

fn ensure_count(actual: usize, code: &'static str) -> Result<(), Diagnostic> {
    if actual <= MAX_CANONICAL_COLLECTION_LEN {
        Ok(())
    } else {
        Err(resource(
            code,
            "projected record collection exceeds its member ceiling",
        ))
    }
}

fn checked_weight(current: usize, addition: usize) -> Result<usize, Diagnostic> {
    match current.checked_add(addition) {
        Some(total) if total <= MAX_PROJECTED_DECODED_WEIGHT => Ok(total),
        _ => Err(resource(
            "projected_record_decoded_weight_limit",
            "projected record decoded object weight exceeds its ceiling",
        )),
    }
}

fn invalid(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::stable(DiagnosticCategory::InvalidContract, code, message)
}

fn integrity(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::stable(DiagnosticCategory::Integrity, code, message)
}

fn resource(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::stable(DiagnosticCategory::ResourceLimit, code, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::{CanonicalizationVersion, FingerprintDomain};
    use crate::value::CanonicalString;

    fn authority() -> (SemanticProfileId, DeclaredIdentityFingerprint) {
        let profile = SemanticProfileId::new("typedb-3.12.1/v1").unwrap();
        let fingerprint = Fingerprint::compute(
            FingerprintDomain::new("typebridge.schema.declared-identity").unwrap(),
            CanonicalizationVersion::new("typebridge.schema-canonical-json/v1").unwrap(),
            None,
            b"workforce-v5",
        );
        (
            profile,
            DeclaredIdentityFingerprint::from_wire(fingerprint).unwrap(),
        )
    }

    fn scalar(value: &str) -> ProjectedRecord {
        let (profile, declared) = authority();
        ProjectedRecord::try_new(
            profile,
            declared,
            ProjectedRecordContent::AttributeValue {
                r#type: TypeId::new(TypeKind::Attribute, "display-name").unwrap(),
                value: CanonicalValue::String(CanonicalString::new(value).unwrap()),
            },
        )
        .unwrap()
    }

    #[test]
    fn scalar_record_and_ordered_archive_round_trip_with_domain_separation() {
        let first = scalar("Ada");
        let second = scalar("Grace");
        let bytes = first.encode().unwrap();
        assert_eq!(
            String::from_utf8(bytes.clone()).unwrap(),
            r#"{"declared_schema_identity":{"algorithm":"sha256","canonicalization":"typebridge.schema-canonical-json/v1","digest":"ad8f474ebb093bdf6d357787f63f9eb149f49910cad05f59a2cedb306a5e759d","domain":"typebridge.schema.declared-identity"},"fingerprint":{"algorithm":"sha256","canonicalization":"typebridge.projected-record/v1","digest":"797cf35f8fe741ba8792caea4b1dcf75aeee0f3f7cc59243ccb0b96cd0e431c0","domain":"typebridge.projected-record","semantic_profile":"typedb-3.12.1/v1"},"format":"typebridge.projected-record/v1","record":{"kind":"attribute_value","type":{"kind":"attribute","label":"display-name"},"value":{"kind":"string","value":"Ada"}},"semantic_profile":"typedb-3.12.1/v1"}"#
        );
        assert_eq!(ProjectedRecord::decode(&bytes).unwrap(), first);
        assert_eq!(
            first.fingerprint().domain().as_str(),
            PROJECTED_RECORD_FINGERPRINT_DOMAIN
        );

        let archive = ProjectedArchive::try_new(vec![first.clone(), second.clone()]).unwrap();
        let archive_bytes = archive.encode().unwrap();
        assert_eq!(
            String::from_utf8(archive_bytes.clone()).unwrap(),
            r#"{"declared_schema_identity":{"algorithm":"sha256","canonicalization":"typebridge.schema-canonical-json/v1","digest":"ad8f474ebb093bdf6d357787f63f9eb149f49910cad05f59a2cedb306a5e759d","domain":"typebridge.schema.declared-identity"},"fingerprint":{"algorithm":"sha256","canonicalization":"typebridge.projected-archive/v1","digest":"69a519248eec9fe9df6f09fa2ac0411ad7c344d68f7a6211f68c808df6597c51","domain":"typebridge.projected-archive","semantic_profile":"typedb-3.12.1/v1"},"format":"typebridge.projected-archive/v1","records":[{"declared_schema_identity":{"algorithm":"sha256","canonicalization":"typebridge.schema-canonical-json/v1","digest":"ad8f474ebb093bdf6d357787f63f9eb149f49910cad05f59a2cedb306a5e759d","domain":"typebridge.schema.declared-identity"},"fingerprint":{"algorithm":"sha256","canonicalization":"typebridge.projected-record/v1","digest":"797cf35f8fe741ba8792caea4b1dcf75aeee0f3f7cc59243ccb0b96cd0e431c0","domain":"typebridge.projected-record","semantic_profile":"typedb-3.12.1/v1"},"format":"typebridge.projected-record/v1","record":{"kind":"attribute_value","type":{"kind":"attribute","label":"display-name"},"value":{"kind":"string","value":"Ada"}},"semantic_profile":"typedb-3.12.1/v1"},{"declared_schema_identity":{"algorithm":"sha256","canonicalization":"typebridge.schema-canonical-json/v1","digest":"ad8f474ebb093bdf6d357787f63f9eb149f49910cad05f59a2cedb306a5e759d","domain":"typebridge.schema.declared-identity"},"fingerprint":{"algorithm":"sha256","canonicalization":"typebridge.projected-record/v1","digest":"58a8047fbbd29c01f05736d0234de5ff7fe2b5af64f25f7cdcc07547f3a23cb7","domain":"typebridge.projected-record","semantic_profile":"typedb-3.12.1/v1"},"format":"typebridge.projected-record/v1","record":{"kind":"attribute_value","type":{"kind":"attribute","label":"display-name"},"value":{"kind":"string","value":"Grace"}},"semantic_profile":"typedb-3.12.1/v1"}],"semantic_profile":"typedb-3.12.1/v1"}"#
        );
        assert_eq!(ProjectedArchive::decode(&archive_bytes).unwrap(), archive);
        assert_eq!(
            archive.fingerprint().domain().as_str(),
            PROJECTED_ARCHIVE_FINGERPRINT_DOMAIN
        );
        assert_ne!(archive.fingerprint().digest(), first.fingerprint().digest());

        let reversed = ProjectedArchive::try_new(vec![second, first]).unwrap();
        assert_ne!(archive.fingerprint(), reversed.fingerprint());
    }

    #[test]
    fn reference_normalizes_keys_and_never_contains_origin_authority() {
        let person = TypeId::new(TypeKind::Entity, "person").unwrap();
        let key =
            ProjectedFieldId::new(person.clone(), AttributeId::new("person-id").unwrap()).unwrap();
        let reference = ProjectedRecordReference::try_new(
            person,
            Some("0x1".into()),
            vec![ProjectedReferenceKey::new(
                key,
                CanonicalValue::String(CanonicalString::new("ada").unwrap()),
            )],
        )
        .unwrap();
        let (profile, declared) = authority();
        let record = ProjectedRecord::try_new(
            profile,
            declared,
            ProjectedRecordContent::Reference { reference },
        )
        .unwrap();
        let bytes = record.encode().unwrap();
        assert!(
            !bytes
                .windows("origin".len())
                .any(|bytes| bytes == b"origin")
        );
        assert_eq!(ProjectedRecord::decode(&bytes).unwrap(), record);
    }

    #[test]
    fn decode_rejects_wrong_format_fingerprint_and_duplicate_keys() {
        let bytes = scalar("Ada").encode().unwrap();
        let text = String::from_utf8(bytes).unwrap();
        for hostile in [
            text.replacen(PROJECTED_RECORD_V1, "typebridge.projected-record/v2", 1),
            text.replacen("Ada", "Eve", 1),
            text.replacen("{", "{\"format\":\"typebridge.projected-record/v1\",", 1),
        ] {
            assert!(ProjectedRecord::decode(hostile.as_bytes()).is_err());
        }
    }
}
