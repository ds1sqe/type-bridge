//! Bounded canonical wire decoder for trusted runtime projections.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::codec::from_canonical_json;
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::fingerprint::Fingerprint;
use crate::id::{AttributeId, FunctionId, Label, RoleId, StructId, TypeId};
use crate::projection::{
    BindingTarget, CodeResourceDigest, CompleteReadProjection, CreateFieldProjection,
    CreateProjection, CreateRoleProjection, DeclarationProjection, DeclaredRoleProjection,
    EmissionPlan, FieldTokenProjection, FunctionParameterProjection, FunctionProjection,
    FunctionReturnElementProjection, FunctionReturnProjection, ModelProjection, PlayingProjection,
    ProjectedAnnotation, ProjectedContainer, ProjectedModelForm, ProjectedModelUse,
    ProjectedMultiplicity, ProjectedTypeRef, ProjectionConfig, ProjectionHandler,
    QueryTokenProjection, ReadFieldProjection, ReadRoleProjection, ReferenceConstructionPolicy,
    ReferenceReadProjection, RoleTokenProjection, RuntimeProjection, StructFieldProjection,
    StructProjection, TargetIdentifier,
};
use crate::schema::{
    AnnotationFactId, AnnotationKindId, AnnotationSubjectId, CanonicalValueRange,
    CanonicalValueSet, DocText, OwnsFactId, PlaysFactId, RegexPattern, RelatesFactId,
    SchemaAnnotationValue, SubFactId, ValueFactId,
};
use crate::schema_fingerprint::SemanticSchemaFingerprint;
use crate::value::{CanonicalValue, Cardinality, ValueTypeTag};

/// Decode exact canonical projection bytes, reconstruct every trusted value, and verify detached fingerprints.
pub fn decode_runtime_projection_verified(
    projection: &[u8],
    expected_semantic: &[u8],
    expected_binding: &[u8],
) -> Result<RuntimeProjection, Diagnostic> {
    let wire = from_canonical_json::<RuntimeProjectionWire>(projection)?;
    let expected_semantic = from_canonical_json::<Fingerprint>(expected_semantic)?;
    let expected_binding = from_canonical_json::<Fingerprint>(expected_binding)?;
    if wire.semantic_fingerprint != expected_semantic {
        return Err(integrity(
            "runtime_projection_semantic_fingerprint_mismatch",
            "runtime projection and detached semantic fingerprints differ",
        ));
    }
    let embedded_binding = wire.projection_fingerprint.clone();
    let rebuilt = wire.rebuild()?;
    let actual_binding = rebuilt.projection_fingerprint().as_fingerprint();
    if actual_binding != &embedded_binding || actual_binding != &expected_binding {
        return Err(integrity(
            "runtime_projection_fingerprint_mismatch",
            "runtime projection content does not match its embedded and detached binding fingerprints",
        ));
    }
    Ok(rebuilt)
}

fn invalid(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::stable(DiagnosticCategory::InvalidContract, code, message)
}

fn integrity(code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::stable(DiagnosticCategory::Integrity, code, message)
}

fn target_name(target: BindingTarget, value: String) -> Result<TargetIdentifier, Diagnostic> {
    match target {
        BindingTarget::Python => TargetIdentifier::python(value),
        BindingTarget::TypeScript => TargetIdentifier::typescript(value),
        BindingTarget::Rust => TargetIdentifier::rust(value),
    }
}

fn collect_unique<K, V>(
    values: impl IntoIterator<Item = V>,
    key: impl Fn(&V) -> K,
    code: &'static str,
) -> Result<BTreeMap<K, V>, Diagnostic>
where
    K: Ord,
{
    let mut output = BTreeMap::new();
    for value in values {
        if output.insert(key(&value), value).is_some() {
            return Err(invalid(
                code,
                "canonical projection wire contains a duplicate identity",
            ));
        }
    }
    Ok(output)
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum BindingTargetWire {
    Python,
    #[serde(rename = "typescript")]
    TypeScript,
    Rust,
}

impl From<BindingTargetWire> for BindingTarget {
    fn from(value: BindingTargetWire) -> Self {
        match value {
            BindingTargetWire::Python => Self::Python,
            BindingTargetWire::TypeScript => Self::TypeScript,
            BindingTargetWire::Rust => Self::Rust,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "binding")]
enum ProjectionConfigWire {
    #[serde(rename = "python")]
    Python {
        naming_policy: PythonNamingPolicyWire,
    },
    #[serde(rename = "typescript")]
    TypeScript {
        naming_policy: TypeScriptNamingPolicyWire,
    },
    #[serde(rename = "rust")]
    Rust {
        naming_policy: RustNamingPolicyWire,
        create_policy: RustCreatePolicyWire,
    },
}

#[derive(Deserialize, Serialize)]
enum PythonNamingPolicyWire {
    #[serde(rename = "typebridge.python/v1")]
    TypeBridgeV1,
}

#[derive(Deserialize, Serialize)]
enum TypeScriptNamingPolicyWire {
    #[serde(rename = "typebridge.typescript/v1")]
    TypeBridgeV1,
}

#[derive(Deserialize, Serialize)]
enum RustNamingPolicyWire {
    #[serde(rename = "typebridge.rust/v1")]
    TypeBridgeV1,
}

#[derive(Deserialize, Serialize)]
enum RustCreatePolicyWire {
    #[serde(rename = "typebridge.rust.validated-create-input/v1")]
    ValidatedInputV1,
}

impl ProjectionConfigWire {
    fn rebuild(self) -> ProjectionConfig {
        match self {
            Self::Python { .. } => ProjectionConfig::python(),
            Self::TypeScript { .. } => ProjectionConfig::typescript(),
            Self::Rust { .. } => ProjectionConfig::rust(),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProjectionHandlerWire {
    id: String,
    version: u16,
}

impl ProjectionHandlerWire {
    fn rebuild(self) -> Result<ProjectionHandler, Diagnostic> {
        ProjectionHandler::new(self.id, self.version)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CodeResourceWire {
    id: String,
    content_fingerprint: Fingerprint,
}

impl CodeResourceWire {
    fn rebuild(self) -> Result<CodeResourceDigest, Diagnostic> {
        CodeResourceDigest::from_wire(self.id, self.content_fingerprint)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnsFactIdWire {
    owner: TypeId,
    attribute: AttributeId,
}

impl OwnsFactIdWire {
    fn rebuild(self) -> Result<OwnsFactId, Diagnostic> {
        OwnsFactId::new(self.owner, self.attribute)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RelatesFactIdWire {
    relation: TypeId,
    role: RoleId,
}

impl RelatesFactIdWire {
    fn rebuild(self) -> Result<RelatesFactId, Diagnostic> {
        RelatesFactId::new(self.relation, self.role)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct PlaysFactIdWire {
    player: TypeId,
    role: RoleId,
}

impl PlaysFactIdWire {
    fn rebuild(self) -> Result<PlaysFactId, Diagnostic> {
        PlaysFactId::new(self.player, self.role)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SubFactIdWire {
    subtype: TypeId,
    supertype: TypeId,
}

impl SubFactIdWire {
    fn rebuild(self) -> Result<SubFactId, Diagnostic> {
        SubFactId::new(self.subtype, self.supertype)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum AnnotationSubjectWire {
    Type(TypeId),
    Sub(SubFactIdWire),
    Value(AttributeId),
    Owns(OwnsFactIdWire),
    Relates(RelatesFactIdWire),
    Plays(PlaysFactIdWire),
    Function(FunctionId),
}

impl AnnotationSubjectWire {
    fn rebuild(self) -> Result<AnnotationSubjectId, Diagnostic> {
        Ok(match self {
            Self::Type(value) => AnnotationSubjectId::Type(value),
            Self::Sub(value) => AnnotationSubjectId::Sub(value.rebuild()?),
            Self::Value(value) => AnnotationSubjectId::Value(ValueFactId::new(value)),
            Self::Owns(value) => AnnotationSubjectId::Owns(value.rebuild()?),
            Self::Relates(value) => AnnotationSubjectId::Relates(value.rebuild()?),
            Self::Plays(value) => AnnotationSubjectId::Plays(value.rebuild()?),
            Self::Function(value) => AnnotationSubjectId::Function(value),
        })
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", content = "key", rename_all = "snake_case")]
enum AnnotationKindWire {
    Abstract,
    Independent,
    Key,
    Unique,
    Card,
    Regex,
    Range,
    Values,
    Doc,
    Meta(Label),
}

impl AnnotationKindWire {
    fn rebuild(self) -> AnnotationKindId {
        match self {
            Self::Abstract => AnnotationKindId::Abstract,
            Self::Independent => AnnotationKindId::Independent,
            Self::Key => AnnotationKindId::Key,
            Self::Unique => AnnotationKindId::Unique,
            Self::Card => AnnotationKindId::Card,
            Self::Regex => AnnotationKindId::Regex,
            Self::Range => AnnotationKindId::Range,
            Self::Values => AnnotationKindId::Values,
            Self::Doc => AnnotationKindId::Doc,
            Self::Meta(value) => AnnotationKindId::Meta(value),
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AnnotationFactIdWire {
    subject: AnnotationSubjectWire,
    kind: AnnotationKindWire,
}

impl AnnotationFactIdWire {
    fn rebuild(self) -> Result<AnnotationFactId, Diagnostic> {
        Ok(AnnotationFactId::new(
            self.subject.rebuild()?,
            self.kind.rebuild(),
        ))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ValueRangeWire {
    lower: Option<CanonicalValue>,
    upper: Option<CanonicalValue>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum AnnotationValueWire {
    Presence,
    Cardinality(Cardinality),
    Regex(String),
    Range(ValueRangeWire),
    Values(Vec<CanonicalValue>),
    Doc(String),
    Meta(CanonicalValue),
}

impl AnnotationValueWire {
    fn rebuild(self) -> Result<SchemaAnnotationValue, Diagnostic> {
        Ok(match self {
            Self::Presence => SchemaAnnotationValue::Presence,
            Self::Cardinality(value) => SchemaAnnotationValue::Cardinality(value),
            Self::Regex(value) => SchemaAnnotationValue::Regex(RegexPattern::new(value)?),
            Self::Range(value) => {
                SchemaAnnotationValue::Range(CanonicalValueRange::new(value.lower, value.upper)?)
            }
            Self::Values(values) => SchemaAnnotationValue::Values(CanonicalValueSet::new(values)?),
            Self::Doc(value) => SchemaAnnotationValue::Doc(DocText::new(value)?),
            Self::Meta(value) => SchemaAnnotationValue::Meta(value),
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AnnotationWire {
    id: AnnotationFactIdWire,
    value: AnnotationValueWire,
}

impl AnnotationWire {
    fn rebuild(self) -> Result<ProjectedAnnotation, Diagnostic> {
        Ok(ProjectedAnnotation::new(
            self.id.rebuild()?,
            self.value.rebuild()?,
        ))
    }
}

fn annotations(
    values: Vec<AnnotationWire>,
) -> Result<BTreeMap<AnnotationFactId, ProjectedAnnotation>, Diagnostic> {
    let values = values
        .into_iter()
        .map(AnnotationWire::rebuild)
        .collect::<Result<Vec<_>, _>>()?;
    collect_unique(
        values,
        |value| value.id().clone(),
        "duplicate_projected_annotation",
    )
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[derive(Eq, Ord, PartialEq, PartialOrd)]
enum ProjectedModelFormWire {
    Complete,
    Reference,
}

impl From<ProjectedModelFormWire> for ProjectedModelForm {
    fn from(value: ProjectedModelFormWire) -> Self {
        match value {
            ProjectedModelFormWire::Complete => Self::Complete,
            ProjectedModelFormWire::Reference => Self::Reference,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct ProjectedModelUseWire {
    id: TypeId,
    form: ProjectedModelFormWire,
}

impl ProjectedModelUseWire {
    fn rebuild(self) -> ProjectedModelUse {
        ProjectedModelUse::new(self.id, self.form.into())
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum ProjectedTypeRefWire {
    Scalar(ValueTypeTag),
    Model(ProjectedModelUseWire),
    Struct(StructId),
}

impl ProjectedTypeRefWire {
    fn rebuild(self) -> ProjectedTypeRef {
        match self {
            Self::Scalar(value) => ProjectedTypeRef::Scalar(value),
            Self::Model(value) => ProjectedTypeRef::Model(value.rebuild()),
            Self::Struct(value) => ProjectedTypeRef::Struct(value),
        }
    }
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProjectedContainerWire {
    Scalar,
    Sequence,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MultiplicityWire {
    cardinality: Cardinality,
    required: bool,
    container: ProjectedContainerWire,
}

impl MultiplicityWire {
    fn rebuild(self) -> Result<ProjectedMultiplicity, Diagnostic> {
        let rebuilt = ProjectedMultiplicity::from_cardinality(self.cardinality);
        let container = match rebuilt.container() {
            ProjectedContainer::Scalar => ProjectedContainerWire::Scalar,
            ProjectedContainer::Sequence => ProjectedContainerWire::Sequence,
        };
        if rebuilt.required() != self.required || container != self.container {
            return Err(invalid(
                "invalid_projected_multiplicity",
                "projected multiplicity does not match its cardinality",
            ));
        }
        Ok(rebuilt)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FieldTokenWire {
    id: OwnsFactIdWire,
    target_name: String,
    multiplicity: MultiplicityWire,
    key: bool,
    unique: bool,
    annotations: Vec<AnnotationWire>,
}

impl FieldTokenWire {
    fn rebuild(self, target: BindingTarget) -> Result<FieldTokenProjection, Diagnostic> {
        FieldTokenProjection::new(
            self.id.rebuild()?,
            target_name(target, self.target_name)?,
            self.multiplicity.rebuild()?,
            self.key,
            self.unique,
            annotations(self.annotations)?,
        )
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RoleTokenWire {
    owner: TypeId,
    role: RoleId,
    target_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    player_union_target_name: Option<String>,
    accepted_players: BTreeSet<TypeId>,
    specializes: Option<RoleId>,
    multiplicity: MultiplicityWire,
    is_abstract: bool,
    annotations: Vec<AnnotationWire>,
}

impl RoleTokenWire {
    fn rebuild(self, target: BindingTarget) -> Result<RoleTokenProjection, Diagnostic> {
        let player_union_target_name = self
            .player_union_target_name
            .map(|value| target_name(target, value))
            .transpose()?;
        let mut projection = RoleTokenProjection::new(
            self.owner,
            self.role,
            target_name(target, self.target_name)?,
            self.accepted_players,
            self.specializes,
            self.multiplicity.rebuild()?,
            self.is_abstract,
            annotations(self.annotations)?,
        )?;
        if let Some(name) = player_union_target_name {
            projection = projection.with_player_union_target_name(name);
        }
        Ok(projection)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DeclaredRoleWire {
    role: RoleId,
    specializes: Option<RoleId>,
}

impl DeclaredRoleWire {
    fn rebuild(self) -> DeclaredRoleProjection {
        DeclaredRoleProjection::new(self.role, self.specializes)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DeclarationWire {
    parent: Option<TypeId>,
    value_type: Option<ValueTypeTag>,
    is_abstract: bool,
    is_constructible: bool,
    annotations: Vec<AnnotationWire>,
    value_annotations: Vec<AnnotationWire>,
    direct_fields: Vec<OwnsFactIdWire>,
    direct_roles: Vec<DeclaredRoleWire>,
    direct_plays: BTreeSet<PlaysFactIdWire>,
}

impl DeclarationWire {
    fn rebuild(self) -> Result<DeclarationProjection, Diagnostic> {
        let roles = self
            .direct_roles
            .into_iter()
            .map(DeclaredRoleWire::rebuild)
            .collect::<Vec<_>>();
        let roles = collect_unique(
            roles,
            |value| value.role().clone(),
            "duplicate_declared_role",
        )?;
        let direct_fields = self
            .direct_fields
            .into_iter()
            .map(OwnsFactIdWire::rebuild)
            .collect::<Result<_, _>>()?;
        let direct_plays = self
            .direct_plays
            .into_iter()
            .map(PlaysFactIdWire::rebuild)
            .collect::<Result<_, _>>()?;
        DeclarationProjection::new(
            self.parent,
            self.value_type,
            self.is_abstract,
            self.is_constructible,
            annotations(self.annotations)?,
            direct_fields,
            roles,
            direct_plays,
        )?
        .with_value_annotations(annotations(self.value_annotations)?)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateFieldWire {
    token: OwnsFactIdWire,
    value: ProjectedTypeRefWire,
    multiplicity: MultiplicityWire,
}

impl CreateFieldWire {
    fn rebuild(self) -> Result<CreateFieldProjection, Diagnostic> {
        Ok(CreateFieldProjection::new(
            self.token.rebuild()?,
            self.value.rebuild(),
            self.multiplicity.rebuild()?,
        ))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateRoleWire {
    role: RoleId,
    players: BTreeSet<ProjectedModelUseWire>,
    multiplicity: MultiplicityWire,
}

impl CreateRoleWire {
    fn rebuild(self) -> Result<CreateRoleProjection, Diagnostic> {
        CreateRoleProjection::new(
            self.role,
            self.players
                .into_iter()
                .map(ProjectedModelUseWire::rebuild)
                .collect(),
            self.multiplicity.rebuild()?,
        )
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CreateWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_name: Option<String>,
    enabled: bool,
    fields: Vec<CreateFieldWire>,
    roles: Vec<CreateRoleWire>,
}

impl CreateWire {
    fn rebuild(self, target: BindingTarget) -> Result<CreateProjection, Diagnostic> {
        let target_name = self
            .target_name
            .map(|value| target_name(target, value))
            .transpose()?;
        let fields = self
            .fields
            .into_iter()
            .map(CreateFieldWire::rebuild)
            .collect::<Result<_, _>>()?;
        let roles = self
            .roles
            .into_iter()
            .map(CreateRoleWire::rebuild)
            .collect::<Result<Vec<_>, _>>()?;
        let roles = collect_unique(roles, |value| value.role().clone(), "duplicate_create_role")?;
        let mut projection = CreateProjection::new(self.enabled, fields, roles)?;
        if let Some(name) = target_name {
            projection = projection.with_target_name(name);
        }
        Ok(projection)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReadFieldWire {
    token: OwnsFactIdWire,
    value: ProjectedTypeRefWire,
    multiplicity: MultiplicityWire,
}

impl ReadFieldWire {
    fn rebuild(self) -> Result<ReadFieldProjection, Diagnostic> {
        Ok(ReadFieldProjection::new(
            self.token.rebuild()?,
            self.value.rebuild(),
            self.multiplicity.rebuild()?,
        ))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReadRoleWire {
    role: RoleId,
    players: BTreeSet<ProjectedModelUseWire>,
    multiplicity: MultiplicityWire,
}

impl ReadRoleWire {
    fn rebuild(self) -> Result<ReadRoleProjection, Diagnostic> {
        ReadRoleProjection::new(
            self.role,
            self.players
                .into_iter()
                .map(ProjectedModelUseWire::rebuild)
                .collect(),
            self.multiplicity.rebuild()?,
        )
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RoleUpcastWire {
    role: RoleId,
    ancestors: Vec<RoleId>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CompleteReadWire {
    fields: Vec<ReadFieldWire>,
    roles: Vec<ReadRoleWire>,
    nominal_upcasts: Vec<TypeId>,
    role_upcasts: Vec<RoleUpcastWire>,
}

impl CompleteReadWire {
    fn rebuild(self) -> Result<CompleteReadProjection, Diagnostic> {
        if self.role_upcasts.iter().any(|entry| {
            entry.ancestors.len() > crate::limits::CANONICAL_CODEC_LIMITS.max_collection_len
        }) {
            return Err(Diagnostic::stable(
                crate::diagnostic::DiagnosticCategory::ResourceLimit,
                "projection_role_upcast_limit_exceeded",
                "projection role-upcast ancestors exceed the canonical collection limit",
            ));
        }
        let fields = self
            .fields
            .into_iter()
            .map(ReadFieldWire::rebuild)
            .collect::<Result<_, _>>()?;
        let roles = self
            .roles
            .into_iter()
            .map(ReadRoleWire::rebuild)
            .collect::<Result<Vec<_>, _>>()?;
        let roles = collect_unique(roles, |value| value.role().clone(), "duplicate_read_role")?;
        let role_upcasts = collect_unique(
            self.role_upcasts,
            |value| value.role.clone(),
            "duplicate_role_upcast",
        )?
        .into_iter()
        .map(|(role, value)| (role, value.ancestors))
        .collect();
        CompleteReadProjection::new(fields, roles, self.nominal_upcasts)?
            .with_role_upcasts(role_upcasts)
    }
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReferenceConstructionPolicyWire {
    IidOnly,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReferenceReadWire {
    target_name: Option<String>,
    key_fields: Vec<OwnsFactIdWire>,
    construction_policy: ReferenceConstructionPolicyWire,
}

impl ReferenceReadWire {
    fn rebuild(self, target: BindingTarget) -> Result<ReferenceReadProjection, Diagnostic> {
        let target_name = self
            .target_name
            .map(|value| target_name(target, value))
            .transpose()?;
        let key_fields = self
            .key_fields
            .into_iter()
            .map(OwnsFactIdWire::rebuild)
            .collect::<Result<_, _>>()?;
        let projection = ReferenceReadProjection::new(target_name, key_fields)?;
        if projection.construction_policy() != ReferenceConstructionPolicy::IidOnly {
            return Err(invalid(
                "invalid_reference_construction_policy",
                "reference construction policy is unsupported",
            ));
        }
        Ok(projection)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct QueryTokensWire {
    type_id: TypeId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target_name: Option<String>,
    fields: Vec<FieldTokenWire>,
    roles: Vec<RoleTokenWire>,
}

impl QueryTokensWire {
    fn rebuild(self, target: BindingTarget) -> Result<QueryTokenProjection, Diagnostic> {
        let target_name = self
            .target_name
            .map(|value| target_name(target, value))
            .transpose()?;
        let fields = self
            .fields
            .into_iter()
            .map(|value| value.rebuild(target))
            .collect::<Result<Vec<_>, _>>()?;
        let fields = collect_unique(fields, |value| value.id().clone(), "duplicate_query_field")?;
        let roles = self
            .roles
            .into_iter()
            .map(|value| value.rebuild(target))
            .collect::<Result<Vec<_>, _>>()?;
        let roles = collect_unique(roles, |value| value.role().clone(), "duplicate_query_role")?;
        let mut projection = QueryTokenProjection::new(self.type_id, fields, roles)?;
        if let Some(name) = target_name {
            projection = projection.with_target_name(name);
        }
        Ok(projection)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ModelWire {
    id: TypeId,
    target_name: String,
    declaration: DeclarationWire,
    create: CreateWire,
    complete_read: CompleteReadWire,
    reference_read: ReferenceReadWire,
    query_tokens: QueryTokensWire,
}

impl ModelWire {
    fn rebuild(self, target: BindingTarget) -> Result<ModelProjection, Diagnostic> {
        ModelProjection::new(
            self.id,
            target_name(target, self.target_name)?,
            self.declaration.rebuild()?,
            self.create.rebuild(target)?,
            self.complete_read.rebuild()?,
            self.reference_read.rebuild(target)?,
            self.query_tokens.rebuild(target)?,
        )
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StructFieldWire {
    name: Label,
    target_name: String,
    value_type: ValueTypeTag,
    optional: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StructWire {
    id: StructId,
    target_name: String,
    fields: Vec<StructFieldWire>,
}

impl StructWire {
    fn rebuild(self, target: BindingTarget) -> Result<StructProjection, Diagnostic> {
        let fields = self
            .fields
            .into_iter()
            .map(|value| {
                Ok(StructFieldProjection::new(
                    value.name,
                    target_name(target, value.target_name)?,
                    value.value_type,
                    value.optional,
                ))
            })
            .collect::<Result<_, Diagnostic>>()?;
        StructProjection::new(self.id, target_name(target, self.target_name)?, fields)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FunctionParameterWire {
    name: Label,
    target_name: String,
    type_ref: ProjectedTypeRefWire,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FunctionReturnElementWire {
    type_ref: ProjectedTypeRefWire,
    optional: bool,
}

impl FunctionReturnElementWire {
    fn rebuild(self) -> FunctionReturnElementProjection {
        FunctionReturnElementProjection::new(self.type_ref.rebuild(), self.optional)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", content = "elements", rename_all = "snake_case")]
enum FunctionReturnWire {
    Scalar(FunctionReturnElementWire),
    Tuple(Vec<FunctionReturnElementWire>),
    Stream(Vec<FunctionReturnElementWire>),
}

impl FunctionReturnWire {
    fn rebuild(self) -> FunctionReturnProjection {
        match self {
            Self::Scalar(value) => FunctionReturnProjection::Scalar(value.rebuild()),
            Self::Tuple(values) => FunctionReturnProjection::Tuple(
                values
                    .into_iter()
                    .map(FunctionReturnElementWire::rebuild)
                    .collect(),
            ),
            Self::Stream(values) => FunctionReturnProjection::Stream(
                values
                    .into_iter()
                    .map(FunctionReturnElementWire::rebuild)
                    .collect(),
            ),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FunctionWire {
    id: FunctionId,
    target_name: String,
    parameters: Vec<FunctionParameterWire>,
    returns: FunctionReturnWire,
    annotations: Vec<AnnotationWire>,
}

impl FunctionWire {
    fn rebuild(self, target: BindingTarget) -> Result<FunctionProjection, Diagnostic> {
        let parameters = self
            .parameters
            .into_iter()
            .map(|value| {
                Ok(FunctionParameterProjection::new(
                    value.name,
                    target_name(target, value.target_name)?,
                    value.type_ref.rebuild(),
                ))
            })
            .collect::<Result<_, Diagnostic>>()?;
        FunctionProjection::new(
            self.id,
            target_name(target, self.target_name)?,
            parameters,
            self.returns.rebuild(),
        )?
        .with_annotations(annotations(self.annotations)?)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PlayingWire {
    id: PlaysFactIdWire,
    role: RoleId,
    target_name: Option<String>,
    multiplicity: MultiplicityWire,
    annotations: Vec<AnnotationWire>,
}

impl PlayingWire {
    fn rebuild(self, target: BindingTarget) -> Result<PlayingProjection, Diagnostic> {
        let mut value = PlayingProjection::new(
            self.id.rebuild()?,
            self.role,
            self.multiplicity.rebuild()?,
            annotations(self.annotations)?,
        )?;
        if let Some(name) = self.target_name {
            value = value.with_target_name(target_name(target, name)?);
        }
        Ok(value)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EmissionWire {
    model_shells: Vec<TypeId>,
    model_link_components: Vec<BTreeSet<TypeId>>,
    structs: Vec<StructId>,
    functions: Vec<FunctionId>,
}

impl EmissionWire {
    fn rebuild(self) -> Result<EmissionPlan, Diagnostic> {
        EmissionPlan::new(
            self.model_shells,
            self.model_link_components,
            self.structs,
            self.functions,
        )
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeProjectionWire {
    target: BindingTargetWire,
    config: ProjectionConfigWire,
    semantic_fingerprint: Fingerprint,
    projection_fingerprint: Fingerprint,
    generator_handlers: Vec<ProjectionHandlerWire>,
    code_resources: Vec<CodeResourceWire>,
    models: Vec<ModelWire>,
    structs: Vec<StructWire>,
    functions: Vec<FunctionWire>,
    playing_facts: Vec<PlayingWire>,
    emission: EmissionWire,
}

impl RuntimeProjectionWire {
    fn rebuild(self) -> Result<RuntimeProjection, Diagnostic> {
        let target = BindingTarget::from(self.target);
        let semantic = SemanticSchemaFingerprint::from_wire(self.semantic_fingerprint)?;
        let handlers = self
            .generator_handlers
            .into_iter()
            .map(ProjectionHandlerWire::rebuild)
            .collect::<Result<Vec<_>, _>>()?;
        let resources = self
            .code_resources
            .into_iter()
            .map(CodeResourceWire::rebuild)
            .collect::<Result<Vec<_>, _>>()?;
        let models = self
            .models
            .into_iter()
            .map(|value| value.rebuild(target))
            .collect::<Result<Vec<_>, _>>()?;
        let models = collect_unique(
            models,
            |value| value.id().clone(),
            "duplicate_runtime_projection_model",
        )?;
        let structs = self
            .structs
            .into_iter()
            .map(|value| value.rebuild(target))
            .collect::<Result<Vec<_>, _>>()?;
        let structs = collect_unique(
            structs,
            |value| value.id().clone(),
            "duplicate_runtime_projection_struct",
        )?;
        let functions = self
            .functions
            .into_iter()
            .map(|value| value.rebuild(target))
            .collect::<Result<Vec<_>, _>>()?;
        let functions = collect_unique(
            functions,
            |value| value.id().clone(),
            "duplicate_runtime_projection_function",
        )?;
        let playing = self
            .playing_facts
            .into_iter()
            .map(|value| value.rebuild(target))
            .collect::<Result<Vec<_>, _>>()?;
        let playing = collect_unique(
            playing,
            |value| value.id().clone(),
            "duplicate_runtime_projection_playing",
        )?;
        RuntimeProjection::try_new(
            target,
            self.config.rebuild(),
            semantic,
            &handlers,
            &resources,
            models,
            structs,
            functions,
            playing,
            self.emission.rebuild()?,
        )
    }
}
