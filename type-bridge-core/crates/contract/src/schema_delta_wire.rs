//! Private canonical wire DTOs for trusted schema deltas.

use serde::{Deserialize, Serialize};

use crate::capability::CapabilitySet;
use crate::codec::{FormatVersion, from_canonical_json, to_canonical_json};
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::fingerprint::{
    CanonicalizationVersion, Fingerprint, FingerprintAlgorithm, FingerprintDigest,
    FingerprintDomain, SemanticProfileId,
};
use crate::id::{AttributeId, FunctionId, Label, RoleId, StructId, TypeId, TypeKind};
use crate::managed_scope::{ManagedScopeBinding, ManagedScopeId, ManagedScopeProfileId};
use crate::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, CanonicalValueRange,
    CanonicalValueSet, DeclaredIdentityFingerprint, DocText, FunctionBody, FunctionFact,
    FunctionParameter, FunctionReturnElement, FunctionReturnMode, FunctionSignature, OwnsFact,
    OwnsFactId, PlaysFact, PlaysFactId, RegexPattern, RelatesFact, RelatesFactId,
    SchemaAnnotationValue, SchemaFact, SchemaFactId, StructFact, StructField, SubFact, SubFactId,
    TypeFact, TypeReference, ValueFact, ValueFactId,
};
use crate::schema_delta::{
    ManagedFactSelection, ManagedSchemaState, PatchFormatVersion, SchemaDelta, SchemaOperation,
};
use crate::schema_fingerprint::{
    ManagedDeclaredIdentityFingerprint, ManagedSemanticSchemaFingerprint,
};
use crate::value::{CanonicalValue, Cardinality, ValueTypeTag};

pub(crate) fn decode_schema_delta(bytes: &[u8]) -> Result<SchemaDelta, Diagnostic> {
    let wire = from_canonical_json::<SchemaDeltaWire>(bytes)?;
    let trusted = wire.rebuild()?;
    if to_canonical_json(&trusted)? != bytes {
        return Err(wire_diagnostic(
            DiagnosticCategory::InvalidContract,
            "non_canonical_schema_delta",
            "schema delta bytes normalize after trusted reconstruction",
        ));
    }
    Ok(trusted)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SchemaDeltaWire {
    format: u16,
    operations: Vec<SchemaOperationWire>,
    required_capabilities: CapabilitySet,
    source: ManagedSchemaStateWire,
    target: ManagedSchemaStateWire,
}

impl SchemaDeltaWire {
    fn rebuild(self) -> Result<SchemaDelta, Diagnostic> {
        let format = PatchFormatVersion::from_wire(self.format)?;
        let source = self.source.rebuild()?;
        let target = self.target.rebuild()?;
        let operations = self
            .operations
            .into_iter()
            .map(SchemaOperationWire::rebuild)
            .collect::<Result<Vec<_>, _>>()?;
        let trusted = SchemaDelta::new(format, source, target, operations)?;
        if self.required_capabilities != *trusted.required_capabilities() {
            return Err(wire_diagnostic(
                DiagnosticCategory::InvalidContract,
                "schema_delta_capability_mismatch",
                "schema delta required capabilities are not the derived transition set",
            ));
        }
        Ok(trusted)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManagedSchemaStateWire {
    declared_identity: FingerprintWire,
    format: FormatVersion,
    managed_declared_identity: FingerprintWire,
    managed_semantic_schema: FingerprintWire,
    required_capabilities: CapabilitySet,
    scope: ManagedScopeBindingWire,
    selection: Vec<SchemaFactIdWire>,
}

impl ManagedSchemaStateWire {
    fn rebuild(self) -> Result<ManagedSchemaState, Diagnostic> {
        ManagedSchemaState::new(
            self.format,
            self.required_capabilities,
            self.scope.rebuild()?,
            ManagedFactSelection::new(
                self.selection
                    .into_iter()
                    .map(SchemaFactIdWire::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
            )?,
            DeclaredIdentityFingerprint::from_wire(self.declared_identity.rebuild()?)?,
            ManagedDeclaredIdentityFingerprint::from_wire(
                self.managed_declared_identity.rebuild()?,
            )?,
            ManagedSemanticSchemaFingerprint::from_wire(self.managed_semantic_schema.rebuild()?)?,
        )
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManagedScopeBindingWire {
    id: ManagedScopeId,
    profile: ManagedScopeProfileBindingWire,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManagedScopeProfileBindingWire {
    fingerprint: FingerprintWire,
    id: ManagedScopeProfileId,
}

impl ManagedScopeBindingWire {
    fn rebuild(self) -> Result<ManagedScopeBinding, Diagnostic> {
        let expected = ManagedScopeBinding::exclusive(self.id)?;
        let fingerprint = self.profile.fingerprint.rebuild()?;
        if self.profile.id != *expected.profile().id()
            || fingerprint != *expected.profile().fingerprint().as_fingerprint()
        {
            return Err(wire_diagnostic(
                DiagnosticCategory::Integrity,
                "invalid_managed_scope_binding",
                "managed scope profile binding does not match the frozen registry",
            ));
        }
        Ok(expected)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FingerprintWire {
    domain: FingerprintDomain,
    algorithm: FingerprintAlgorithm,
    canonicalization: CanonicalizationVersion,
    #[serde(skip_serializing_if = "Option::is_none")]
    semantic_profile: Option<SemanticProfileId>,
    digest: FingerprintDigest,
}

impl FingerprintWire {
    pub(crate) fn rebuild(self) -> Result<Fingerprint, Diagnostic> {
        serde_json::from_value(serde_json::to_value(self).map_err(|_| {
            wire_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_schema_delta_fingerprint",
                "schema delta fingerprint cannot be represented as JSON",
            )
        })?)
        .map_err(|_| {
            wire_diagnostic(
                DiagnosticCategory::Integrity,
                "invalid_schema_delta_fingerprint",
                "schema delta fingerprint metadata is malformed",
            )
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SchemaOperationWire {
    Define {
        facts: Vec<SchemaFactWire>,
    },
    Redefine {
        expected: SchemaFactWire,
        replacement: Box<SchemaFactWire>,
    },
    Undefine {
        fact: SchemaFactWire,
    },
}

impl SchemaOperationWire {
    fn rebuild(self) -> Result<SchemaOperation, Diagnostic> {
        match self {
            Self::Define { facts } => SchemaOperation::define(
                facts
                    .into_iter()
                    .map(SchemaFactWire::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Self::Redefine {
                expected,
                replacement,
            } => SchemaOperation::redefine(expected.rebuild()?, (*replacement).rebuild()?),
            Self::Undefine { fact } => Ok(SchemaOperation::undefine(fact.rebuild()?)),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum SchemaFactWire {
    Type(TypeFactWire),
    Sub(SubFactWire),
    Value(ValueFactWire),
    Owns(OwnsFactWire),
    Relates(RelatesFactWire),
    Plays(PlaysFactWire),
    Annotation(AnnotationFactWire),
    Function(FunctionFactWire),
    Struct(StructFactWire),
}

impl SchemaFactWire {
    pub(crate) fn rebuild(self) -> Result<SchemaFact, Diagnostic> {
        Ok(match self {
            Self::Type(wire) => SchemaFact::Type(TypeFact::new(wire.id.rebuild()?)?),
            Self::Sub(wire) => SchemaFact::Sub(SubFact::new(wire.id.rebuild()?)),
            Self::Value(wire) => {
                SchemaFact::Value(ValueFact::new(ValueFactId::new(wire.id), wire.value_type))
            }
            Self::Owns(wire) => SchemaFact::Owns(OwnsFact::new(wire.id.rebuild()?)),
            Self::Relates(wire) => SchemaFact::Relates(RelatesFact::new(
                wire.id.rebuild()?,
                wire.specializes.map(RoleIdWire::rebuild).transpose()?,
            )?),
            Self::Plays(wire) => SchemaFact::Plays(PlaysFact::new(wire.id.rebuild()?)),
            Self::Annotation(wire) => SchemaFact::Annotation(AnnotationFact::new(
                wire.id.rebuild()?,
                wire.value.rebuild()?,
            )?),
            Self::Function(wire) => SchemaFact::Function(FunctionFact::new(
                wire.id,
                wire.signature.rebuild()?,
                FunctionBody::new(wire.body)?,
            )),
            Self::Struct(wire) => SchemaFact::Struct(StructFact::new(
                wire.id,
                wire.fields
                    .into_iter()
                    .map(StructFieldWire::rebuild)
                    .collect(),
            )?),
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TypeFactWire {
    id: TypeIdWire,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SubFactWire {
    id: SubFactIdWire,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ValueFactWire {
    id: AttributeId,
    value_type: ValueTypeTag,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OwnsFactWire {
    id: OwnsFactIdWire,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RelatesFactWire {
    id: RelatesFactIdWire,
    specializes: Option<RoleIdWire>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlaysFactWire {
    id: PlaysFactIdWire,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AnnotationFactWire {
    id: AnnotationFactIdWire,
    value: AnnotationValueWire,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FunctionFactWire {
    id: FunctionId,
    signature: FunctionSignatureWire,
    body: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StructFactWire {
    id: StructId,
    fields: Vec<StructFieldWire>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TypeIdWire {
    kind: TypeKind,
    label: Label,
}

impl TypeIdWire {
    fn rebuild(self) -> Result<TypeId, Diagnostic> {
        TypeId::new(self.kind, self.label.as_str().to_owned())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RoleIdWire {
    declaring_relation: Label,
    label: Label,
}

impl RoleIdWire {
    fn rebuild(self) -> Result<RoleId, Diagnostic> {
        RoleId::new(
            self.declaring_relation.as_str().to_owned(),
            self.label.as_str().to_owned(),
        )
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SubFactIdWire {
    subtype: TypeIdWire,
    supertype: TypeIdWire,
}

impl SubFactIdWire {
    fn rebuild(self) -> Result<SubFactId, Diagnostic> {
        SubFactId::new(self.subtype.rebuild()?, self.supertype.rebuild()?)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnsFactIdWire {
    owner: TypeIdWire,
    attribute: AttributeId,
}

impl OwnsFactIdWire {
    fn rebuild(self) -> Result<OwnsFactId, Diagnostic> {
        OwnsFactId::new(self.owner.rebuild()?, self.attribute)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RelatesFactIdWire {
    relation: TypeIdWire,
    role: RoleIdWire,
}

impl RelatesFactIdWire {
    fn rebuild(self) -> Result<RelatesFactId, Diagnostic> {
        RelatesFactId::new(self.relation.rebuild()?, self.role.rebuild()?)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PlaysFactIdWire {
    player: TypeIdWire,
    role: RoleIdWire,
}

impl PlaysFactIdWire {
    fn rebuild(self) -> Result<PlaysFactId, Diagnostic> {
        PlaysFactId::new(self.player.rebuild()?, self.role.rebuild()?)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum AnnotationSubjectWire {
    Type(TypeIdWire),
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
            Self::Type(id) => AnnotationSubjectId::Type(id.rebuild()?),
            Self::Sub(id) => AnnotationSubjectId::Sub(id.rebuild()?),
            Self::Value(id) => AnnotationSubjectId::Value(ValueFactId::new(id)),
            Self::Owns(id) => AnnotationSubjectId::Owns(id.rebuild()?),
            Self::Relates(id) => AnnotationSubjectId::Relates(id.rebuild()?),
            Self::Plays(id) => AnnotationSubjectId::Plays(id.rebuild()?),
            Self::Function(id) => AnnotationSubjectId::Function(id),
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(
    tag = "kind",
    content = "key",
    rename_all = "snake_case",
    deny_unknown_fields
)]
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
    fn rebuild(self) -> Result<AnnotationKindId, Diagnostic> {
        Ok(match self {
            Self::Abstract => AnnotationKindId::Abstract,
            Self::Independent => AnnotationKindId::Independent,
            Self::Key => AnnotationKindId::Key,
            Self::Unique => AnnotationKindId::Unique,
            Self::Card => AnnotationKindId::Card,
            Self::Regex => AnnotationKindId::Regex,
            Self::Range => AnnotationKindId::Range,
            Self::Values => AnnotationKindId::Values,
            Self::Doc => AnnotationKindId::Doc,
            Self::Meta(key) => AnnotationKindId::meta(key.as_str().to_owned())?,
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AnnotationFactIdWire {
    subject: AnnotationSubjectWire,
    kind: AnnotationKindWire,
}

impl AnnotationFactIdWire {
    fn rebuild(self) -> Result<AnnotationFactId, Diagnostic> {
        Ok(AnnotationFactId::new(
            self.subject.rebuild()?,
            self.kind.rebuild()?,
        ))
    }
}

#[derive(Deserialize, Serialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum AnnotationValueWire {
    Presence,
    Cardinality(CardinalityWire),
    Regex(String),
    Range(ValueRangeWire),
    Values(Vec<CanonicalValueWire>),
    Doc(String),
    Meta(CanonicalValueWire),
}

impl AnnotationValueWire {
    fn rebuild(self) -> Result<SchemaAnnotationValue, Diagnostic> {
        Ok(match self {
            Self::Presence => SchemaAnnotationValue::Presence,
            Self::Cardinality(value) => SchemaAnnotationValue::Cardinality(value.rebuild()?),
            Self::Regex(value) => SchemaAnnotationValue::Regex(RegexPattern::new(value)?),
            Self::Range(value) => SchemaAnnotationValue::Range(value.rebuild()?),
            Self::Values(values) => SchemaAnnotationValue::Values(CanonicalValueSet::new(
                values
                    .into_iter()
                    .map(CanonicalValueWire::rebuild)
                    .collect::<Result<Vec<_>, _>>()?,
            )?),
            Self::Doc(value) => SchemaAnnotationValue::Doc(DocText::new(value)?),
            Self::Meta(value) => SchemaAnnotationValue::Meta(value.rebuild()?),
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CardinalityWire {
    kind: CardinalityWireKind,
    min: String,
    max: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CardinalityWireKind {
    Cardinality,
}

impl CardinalityWire {
    fn rebuild(self) -> Result<Cardinality, Diagnostic> {
        let min = self.min.parse::<u64>().map_err(|_| {
            wire_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_schema_delta_cardinality",
                "schema delta cardinality minimum is not canonical u64 text",
            )
        })?;
        let max = if self.max == "unbounded" {
            None
        } else {
            Some(self.max.parse::<u64>().map_err(|_| {
                wire_diagnostic(
                    DiagnosticCategory::InvalidContract,
                    "invalid_schema_delta_cardinality",
                    "schema delta cardinality maximum is not canonical u64 text",
                )
            })?)
        };
        Cardinality::new(min, max)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ValueRangeWire {
    lower: Option<CanonicalValueWire>,
    upper: Option<CanonicalValueWire>,
}

impl ValueRangeWire {
    fn rebuild(self) -> Result<CanonicalValueRange, Diagnostic> {
        CanonicalValueRange::new(
            self.lower.map(CanonicalValueWire::rebuild).transpose()?,
            self.upper.map(CanonicalValueWire::rebuild).transpose()?,
        )
    }
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CanonicalValueWire {
    String { value: serde_json::Value },
    Long { value: serde_json::Value },
    Double { bits: String },
    Boolean { value: serde_json::Value },
    Date { value: serde_json::Value },
    Datetime { value: serde_json::Value },
    DatetimeTz { value: serde_json::Value },
    Decimal { value: serde_json::Value },
    Duration { value: serde_json::Value },
}

impl CanonicalValueWire {
    fn rebuild(self) -> Result<CanonicalValue, Diagnostic> {
        serde_json::from_value(serde_json::to_value(self).map_err(|_| {
            wire_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_schema_delta_value",
                "schema delta canonical value cannot be represented as JSON",
            )
        })?)
        .map_err(|_| {
            wire_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_schema_delta_value",
                "schema delta canonical value is malformed",
            )
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum TypeReferenceWire {
    Value(ValueTypeTag),
    Schema(Label),
}

impl TypeReferenceWire {
    fn rebuild(self) -> TypeReference {
        match self {
            Self::Value(value) => TypeReference::Value(value),
            Self::Schema(label) => TypeReference::Schema(label),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FunctionParameterWire {
    name: Label,
    type_ref: TypeReferenceWire,
}

impl FunctionParameterWire {
    fn rebuild(self) -> FunctionParameter {
        FunctionParameter::new(self.name, self.type_ref.rebuild())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FunctionReturnElementWire {
    type_ref: TypeReferenceWire,
    optional: bool,
}

impl FunctionReturnElementWire {
    fn rebuild(self) -> FunctionReturnElement {
        FunctionReturnElement::new(self.type_ref.rebuild(), self.optional)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(
    tag = "kind",
    content = "elements",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum FunctionReturnModeWire {
    Scalar(FunctionReturnElementWire),
    Tuple(Vec<FunctionReturnElementWire>),
    Stream(Vec<FunctionReturnElementWire>),
}

impl FunctionReturnModeWire {
    fn rebuild(self) -> Result<FunctionReturnMode, Diagnostic> {
        match self {
            Self::Scalar(element) => Ok(FunctionReturnMode::scalar(element.rebuild())),
            Self::Tuple(elements) => FunctionReturnMode::tuple(
                elements
                    .into_iter()
                    .map(FunctionReturnElementWire::rebuild)
                    .collect(),
            ),
            Self::Stream(elements) => FunctionReturnMode::stream(
                elements
                    .into_iter()
                    .map(FunctionReturnElementWire::rebuild)
                    .collect(),
            ),
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FunctionSignatureWire {
    parameters: Vec<FunctionParameterWire>,
    returns: FunctionReturnModeWire,
}

impl FunctionSignatureWire {
    fn rebuild(self) -> Result<FunctionSignature, Diagnostic> {
        FunctionSignature::new(
            self.parameters
                .into_iter()
                .map(FunctionParameterWire::rebuild)
                .collect(),
            self.returns.rebuild()?,
        )
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StructFieldWire {
    name: Label,
    value_type: ValueTypeTag,
    optional: bool,
}

impl StructFieldWire {
    fn rebuild(self) -> StructField {
        StructField::new(self.name, self.value_type, self.optional)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum SchemaFactIdWire {
    Type(TypeIdWire),
    Sub(SubFactIdWire),
    Value(AttributeId),
    Owns(OwnsFactIdWire),
    Relates(RelatesFactIdWire),
    Plays(PlaysFactIdWire),
    Annotation(AnnotationFactIdWire),
    Function(FunctionId),
    Struct(StructId),
}

impl SchemaFactIdWire {
    fn rebuild(self) -> Result<SchemaFactId, Diagnostic> {
        Ok(match self {
            Self::Type(id) => SchemaFactId::Type(id.rebuild()?),
            Self::Sub(id) => SchemaFactId::Sub(id.rebuild()?),
            Self::Value(id) => SchemaFactId::Value(ValueFactId::new(id)),
            Self::Owns(id) => SchemaFactId::Owns(id.rebuild()?),
            Self::Relates(id) => SchemaFactId::Relates(id.rebuild()?),
            Self::Plays(id) => SchemaFactId::Plays(id.rebuild()?),
            Self::Annotation(id) => SchemaFactId::Annotation(id.rebuild()?),
            Self::Function(id) => SchemaFactId::Function(id),
            Self::Struct(id) => SchemaFactId::Struct(id),
        })
    }
}

fn wire_diagnostic(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::stable(category, code, message)
}
