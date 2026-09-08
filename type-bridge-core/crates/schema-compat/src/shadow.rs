//! Honest, comparison-only shadowing of the frozen V1 schema parser.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_contract::id::TypeKind;
use type_bridge_contract::schema::{
    AnnotationKindId, DeclaredIdentityFingerprint, DocumentId, FunctionReturnMode, InterfaceKind,
    SchemaAnnotationValue, SchemaDiagnostics, SchemaFact, SemanticProfile,
    SemanticSchemaFingerprint, TypeReference,
};
use type_bridge_contract::value::{CanonicalValue, Cardinality as V2Cardinality, ValueTypeTag};
use type_bridge_core_lib::_parser as parser;
use type_bridge_core_lib::_schema::{
    Cardinality as V1Cardinality, FunctionType as V1Function, OwnedAttribute, PlayedRole, RoleSpec,
    SchemaError, StructType as V1Struct, TypeSchema,
};
use type_bridge_schema::{ResolvedSchema, resolve};

use crate::typeql_to_declared;

const SHADOW_DOCUMENT: &str = "v1-shadow.typeql";

/// One semantic dimension understood by the comparison report.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ShadowDimension {
    /// Presence of a schema type label.
    TypeExistence,
    /// Entity, relation, or attribute kind.
    TypeKind,
    /// Nearest declared parent label.
    DirectParent,
    /// Effective type abstractness.
    TypeAbstract,
    /// Effective attribute independence.
    AttributeIndependent,
    /// Effective attribute value domain.
    ValueType,
    /// Effective ownership interfaces and their annotations.
    EffectiveOwns,
    /// Effective relation-role interfaces and their annotations.
    EffectiveRelates,
    /// Effective role-playing interfaces and their annotations.
    EffectivePlays,
    /// Documentation and metadata annotations.
    DocumentationAndMetadata,
    /// Ordered function signatures.
    FunctionSignatures,
    /// Function bodies and function annotations.
    FunctionBodiesAndAnnotations,
    /// Ordered struct fields.
    StructFields,
    /// Source text, comments, and source spans.
    SourceCommentsAndSpans,
    /// The declared distinction between omitted and explicit defaults.
    OmittedVersusExplicitIdentity,
    /// Independent annotation fact identity and removal semantics.
    IndependentAnnotationIdentityAndRemoval,
    /// Annotations on `sub` facts.
    SubAnnotations,
    /// Extension and capability negotiation semantics.
    ExtensionsAndCapabilities,
    /// Resolver origin paths, descriptors, and dependency SCCs.
    ResolverGraphsAndOrigins,
    /// Cardinalities representable by V2 `u64` but not V1 `u32`.
    CardinalityOutsideV1U32,
}

/// Coverage recorded alongside every compared result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShadowCoverage {
    compared: BTreeSet<ShadowDimension>,
    unimplemented: BTreeSet<ShadowDimension>,
    not_representable: BTreeSet<ShadowDimension>,
    blind_spots: BTreeSet<ShadowDimension>,
}

/// Why one shadow dimension is or is not part of the verdict.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShadowCoverageState {
    /// Both lanes expose the dimension and the report compares it.
    Compared,
    /// Both lanes can expose the dimension, but the shadow projection does not yet compare it.
    Unimplemented,
    /// The frozen V1 model discards or cannot encode the dimension.
    NotRepresentable,
}

impl ShadowCoverage {
    /// Return dimensions actually compared by this implementation.
    #[must_use]
    pub const fn compared(&self) -> &BTreeSet<ShadowDimension> {
        &self.compared
    }

    /// Compatibility spelling for [`Self::compared`].
    #[must_use]
    pub const fn covered(&self) -> &BTreeSet<ShadowDimension> {
        self.compared()
    }

    /// Return representable dimensions not yet compared by this implementation.
    #[must_use]
    pub const fn unimplemented(&self) -> &BTreeSet<ShadowDimension> {
        &self.unimplemented
    }

    /// Return dimensions that the frozen V1 model cannot faithfully represent.
    #[must_use]
    pub const fn not_representable(&self) -> &BTreeSet<ShadowDimension> {
        &self.not_representable
    }

    /// Return the union of unimplemented and unrepresentable dimensions.
    #[must_use]
    pub const fn blind_spots(&self) -> &BTreeSet<ShadowDimension> {
        &self.blind_spots
    }

    /// Return the explicit coverage state for one known dimension.
    #[must_use]
    pub fn state(&self, dimension: ShadowDimension) -> ShadowCoverageState {
        if self.compared.contains(&dimension) {
            ShadowCoverageState::Compared
        } else if self.unimplemented.contains(&dimension) {
            ShadowCoverageState::Unimplemented
        } else {
            debug_assert!(self.not_representable.contains(&dimension));
            ShadowCoverageState::NotRepresentable
        }
    }

    /// Report whether all known dimensions are compared.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.blind_spots.is_empty()
    }
}

/// Stable outcome for one independently executed parser or resolver lane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShadowLaneOutcome {
    /// The lane accepted its input.
    Accepted(ShadowLaneSummary),
    /// The lane rejected its input.
    Rejected(ShadowLaneRejection),
    /// The lane could not run because its prerequisite lane rejected.
    NotRun(ShadowLaneNotRun),
}

impl ShadowLaneOutcome {
    /// Report whether this lane accepted its input.
    #[must_use]
    pub const fn is_accepted(&self) -> bool {
        matches!(self, Self::Accepted(_))
    }
}

/// Deterministic summary for one accepted lane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShadowLaneSummary {
    type_count: usize,
}

impl ShadowLaneSummary {
    fn new(type_count: usize) -> Self {
        Self { type_count }
    }

    /// Return the number of entity, relation, and attribute types accepted.
    #[must_use]
    pub const fn type_count(&self) -> usize {
        self.type_count
    }
}

/// Stable rejection payload for one lane.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShadowLaneRejection {
    code: String,
    message: String,
}

impl ShadowLaneRejection {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    /// Return the stable rejection code.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Return the diagnostic message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Explanation for a lane skipped after a prerequisite rejection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShadowLaneNotRun {
    code: &'static str,
}

impl ShadowLaneNotRun {
    fn new(code: &'static str) -> Self {
        Self { code }
    }

    /// Return the stable prerequisite-failure code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }
}

/// Comparison verdict for successfully projected effective schemas.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShadowVerdict {
    /// Every covered basic-type dimension matched.
    Matched,
    /// At least one covered basic-type dimension differed.
    Mismatched,
}

/// One deterministic field-level difference between effective projections.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ShadowFinding {
    dimension: ShadowDimension,
    type_label: String,
    v1_value: Option<String>,
    v2_value: Option<String>,
}

impl ShadowFinding {
    fn new(
        dimension: ShadowDimension,
        type_label: impl Into<String>,
        v1_value: Option<String>,
        v2_value: Option<String>,
    ) -> Self {
        Self {
            dimension,
            type_label: type_label.into(),
            v1_value,
            v2_value,
        }
    }

    /// Return the mismatched semantic dimension.
    #[must_use]
    pub const fn dimension(&self) -> ShadowDimension {
        self.dimension
    }

    /// Return the affected type label.
    #[must_use]
    pub fn type_label(&self) -> &str {
        &self.type_label
    }

    /// Return the canonical V1 value, or `None` when absent.
    #[must_use]
    pub fn v1_value(&self) -> Option<&str> {
        self.v1_value.as_deref()
    }

    /// Return the canonical V2 value, or `None` when absent.
    #[must_use]
    pub fn v2_value(&self) -> Option<&str> {
        self.v2_value.as_deref()
    }
}

/// Successful comparison of the two effective projections.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShadowCompared {
    verdict: ShadowVerdict,
    coverage: ShadowCoverage,
    findings: Vec<ShadowFinding>,
}

impl ShadowCompared {
    /// Return the verdict over covered dimensions.
    #[must_use]
    pub const fn verdict(&self) -> ShadowVerdict {
        self.verdict
    }

    /// Return explicit covered and uncovered dimensions.
    #[must_use]
    pub const fn coverage(&self) -> &ShadowCoverage {
        &self.coverage
    }

    /// Return findings in deterministic dimension, label, and value order.
    #[must_use]
    pub fn findings(&self) -> &[ShadowFinding] {
        &self.findings
    }

    /// Report whether this comparison alone is sufficient for a V1 cutover.
    ///
    /// A partial match is intentionally never cutover evidence.
    #[must_use]
    pub fn is_cutover_evidence(&self) -> bool {
        self.verdict == ShadowVerdict::Matched && self.coverage.is_complete()
    }
}

/// A lane whose absence prevents an honest effective-schema comparison.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ShadowUnavailableLane {
    /// The validating and inheritance-resolving V1 lane rejected.
    V1Effective,
    /// The V2 TypeQL-to-declared lane rejected.
    V2Declared,
    /// The V2 pure resolver lane rejected or could not run.
    V2Effective,
}

/// Result when no effective-schema comparison can be made.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShadowNotCompared {
    unavailable_lanes: BTreeSet<ShadowUnavailableLane>,
}

impl ShadowNotCompared {
    /// Return every unavailable prerequisite lane.
    #[must_use]
    pub const fn unavailable_lanes(&self) -> &BTreeSet<ShadowUnavailableLane> {
        &self.unavailable_lanes
    }
}

/// Either a real projection comparison or an explicit unavailable result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShadowComparison {
    /// Both effective lanes accepted and were compared.
    Compared(ShadowCompared),
    /// One or more effective lanes were unavailable; rejection parity is not equality.
    NotCompared(ShadowNotCompared),
}

/// Complete comparison-only report for one TypeQL source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct V1ShadowReport {
    profile: SemanticProfileId,
    v1_direct: ShadowLaneOutcome,
    v1_effective: ShadowLaneOutcome,
    v2_declared: ShadowLaneOutcome,
    v2_effective: ShadowLaneOutcome,
    v2_declared_fingerprint: Option<DeclaredIdentityFingerprint>,
    v2_semantic_fingerprint: Option<SemanticSchemaFingerprint>,
    comparison: ShadowComparison,
}

impl V1ShadowReport {
    /// Return the semantic-default profile used by the V2 resolver.
    #[must_use]
    pub const fn profile(&self) -> &SemanticProfileId {
        &self.profile
    }

    /// Return the raw V1 parser lane outcome.
    #[must_use]
    pub const fn v1_direct(&self) -> &ShadowLaneOutcome {
        &self.v1_direct
    }

    /// Return the validating, inheritance-resolving V1 lane outcome.
    #[must_use]
    pub const fn v1_effective(&self) -> &ShadowLaneOutcome {
        &self.v1_effective
    }

    /// Return the V2 declared-fact lane outcome.
    #[must_use]
    pub const fn v2_declared(&self) -> &ShadowLaneOutcome {
        &self.v2_declared
    }

    /// Return the V2 pure-resolver lane outcome.
    #[must_use]
    pub const fn v2_effective(&self) -> &ShadowLaneOutcome {
        &self.v2_effective
    }

    /// Return the V2 direct-identity fingerprint when declaration succeeded.
    #[must_use]
    pub const fn v2_declared_fingerprint(&self) -> Option<&DeclaredIdentityFingerprint> {
        self.v2_declared_fingerprint.as_ref()
    }

    /// Return the V2 semantic fingerprint when resolution succeeded.
    #[must_use]
    pub const fn v2_semantic_fingerprint(&self) -> Option<&SemanticSchemaFingerprint> {
        self.v2_semantic_fingerprint.as_ref()
    }

    /// Return the honest effective-projection comparison state.
    #[must_use]
    pub const fn comparison(&self) -> &ShadowComparison {
        &self.comparison
    }
}

/// Internal setup failure distinct from a parser or resolver lane rejection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct V1ShadowInternalError {
    code: &'static str,
    message: String,
}

impl V1ShadowInternalError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Return the stable internal-error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }

    /// Return the internal-error message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for V1ShadowInternalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl Error for V1ShadowInternalError {}

/// Execute the V1 direct/effective and V2 declared/effective lanes independently.
///
/// This API accepts TypeQL source, never a [`TypeSchema`], so it cannot become a
/// backdoor V1-to-V2 adapter. A matched verdict covers only the dimensions listed
/// by [`ShadowCoverage::covered`].
pub fn v1_shadow_report(
    typeql: &str,
    profile: &SemanticProfileId,
) -> Result<V1ShadowReport, V1ShadowInternalError> {
    let document = DocumentId::new(SHADOW_DOCUMENT).map_err(|diagnostic| {
        V1ShadowInternalError::new("shadow_document_id_invalid", diagnostic.to_string())
    })?;

    let v1_direct_result = parser::parse_typeql(typeql);
    let v1_direct = match &v1_direct_result {
        Ok(schema) => accepted_v1(schema),
        Err(error) => rejected_v1(error),
    };

    let semantic_profile = SemanticProfile::resolve(profile).map_err(|diagnostic| {
        V1ShadowInternalError::new("shadow_semantic_profile_invalid", diagnostic.to_string())
    })?;
    let v1_effective_result = TypeSchema::from_typeql(typeql);
    let v1_effective_projection = v1_effective_result
        .as_ref()
        .ok()
        .map(|schema| project_v1(schema, &semantic_profile));
    let v1_effective = match &v1_effective_result {
        Ok(schema) => accepted_v1(schema),
        Err(error) => rejected_v1(error),
    };

    let v2_declared_result = typeql_to_declared(document, typeql);
    let v2_declared_fingerprint = v2_declared_result
        .as_ref()
        .ok()
        .map(|declared| declared.declared_identity_fingerprint().clone());
    let v2_declared = match &v2_declared_result {
        Ok(declared) => ShadowLaneOutcome::Accepted(ShadowLaneSummary::new(
            declared
                .facts()
                .filter(|fact| matches!(fact, SchemaFact::Type(_)))
                .count(),
        )),
        Err(diagnostics) => rejected_v2(diagnostics),
    };

    let v2_resolved_result = v2_declared_result
        .as_ref()
        .ok()
        .map(|declared| resolve(declared, profile));
    let v2_effective_projection = v2_resolved_result
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(project_v2);
    let v2_semantic_fingerprint = v2_resolved_result
        .as_ref()
        .and_then(|result| result.as_ref().ok())
        .map(|resolved| resolved.semantic_fingerprint().clone());
    let v2_effective = match &v2_resolved_result {
        Some(Ok(resolved)) => {
            ShadowLaneOutcome::Accepted(ShadowLaneSummary::new(resolved.types().len()))
        }
        Some(Err(diagnostics)) => rejected_v2(diagnostics),
        None => ShadowLaneOutcome::NotRun(ShadowLaneNotRun::new("v2_declared_rejected")),
    };

    let comparison = match (&v1_effective_projection, &v2_effective_projection) {
        (Some(v1), Some(v2)) => ShadowComparison::Compared(compare(v1, v2)),
        _ => {
            let mut unavailable_lanes = BTreeSet::new();
            if v1_effective_projection.is_none() {
                unavailable_lanes.insert(ShadowUnavailableLane::V1Effective);
            }
            if v2_declared_result.is_err() {
                unavailable_lanes.insert(ShadowUnavailableLane::V2Declared);
            }
            if v2_effective_projection.is_none() {
                unavailable_lanes.insert(ShadowUnavailableLane::V2Effective);
            }
            ShadowComparison::NotCompared(ShadowNotCompared { unavailable_lanes })
        }
    };

    Ok(V1ShadowReport {
        profile: profile.clone(),
        v1_direct,
        v1_effective,
        v2_declared,
        v2_effective,
        v2_declared_fingerprint,
        v2_semantic_fingerprint,
        comparison,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BasicTypeKind {
    Entity,
    Relation,
    Attribute,
}

impl BasicTypeKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Entity => "entity",
            Self::Relation => "relation",
            Self::Attribute => "attribute",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BasicTypeProjection {
    kind: BasicTypeKind,
    parent: Option<String>,
    is_abstract: bool,
    is_independent: bool,
    value_type: Option<String>,
    owns: BTreeMap<String, String>,
    relates: BTreeMap<String, String>,
    plays: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SchemaProjection {
    types: BTreeMap<String, BasicTypeProjection>,
    functions: BTreeMap<String, String>,
    structs: BTreeMap<String, String>,
    documentation_and_metadata: BTreeMap<String, String>,
}

fn accepted_v1(schema: &TypeSchema) -> ShadowLaneOutcome {
    ShadowLaneOutcome::Accepted(ShadowLaneSummary::new(
        schema.entities.len() + schema.relations.len() + schema.attributes.len(),
    ))
}

fn rejected_v1(error: &SchemaError) -> ShadowLaneOutcome {
    let code = match error {
        SchemaError::ParseError { .. } => "v1_parse_error",
        SchemaError::InheritanceCycle { .. } => "v1_inheritance_cycle",
        SchemaError::UnknownParent { .. } => "v1_unknown_parent",
        SchemaError::DuplicateDefinition { .. } => "v1_duplicate_definition",
        SchemaError::ValidationError { .. } => "v1_validation_error",
    };
    ShadowLaneOutcome::Rejected(ShadowLaneRejection::new(code, error.to_string()))
}

fn rejected_v2(diagnostics: &SchemaDiagnostics) -> ShadowLaneOutcome {
    let code = diagnostics
        .iter()
        .next()
        .map(|entry| entry.diagnostic().code().as_str())
        .unwrap_or("v2_schema_rejected");
    ShadowLaneOutcome::Rejected(ShadowLaneRejection::new(code, diagnostics.to_string()))
}

fn project_v1(schema: &TypeSchema, profile: &SemanticProfile) -> SchemaProjection {
    let mut types = BTreeMap::new();
    let mut documentation_and_metadata = BTreeMap::new();
    for entity in schema.entities.values() {
        types.insert(
            entity.name.clone(),
            BasicTypeProjection {
                kind: BasicTypeKind::Entity,
                parent: entity.parent.clone(),
                is_abstract: entity.is_abstract,
                is_independent: false,
                value_type: None,
                owns: project_v1_owns(&entity.owns, profile),
                relates: BTreeMap::new(),
                plays: project_v1_plays(&entity.plays, profile),
            },
        );
        insert_doc_meta(
            &mut documentation_and_metadata,
            format!("type {}", entity.name),
            entity.doc.as_deref(),
            entity
                .meta
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        );
        project_v1_interface_docs(
            &mut documentation_and_metadata,
            &entity.name,
            &entity.owns,
            &entity.plays,
            &[],
        );
    }
    for relation in schema.relations.values() {
        types.insert(
            relation.name.clone(),
            BasicTypeProjection {
                kind: BasicTypeKind::Relation,
                parent: relation.parent.clone(),
                is_abstract: relation.is_abstract,
                is_independent: false,
                value_type: None,
                owns: project_v1_owns(&relation.owns, profile),
                relates: project_v1_relates(&relation.roles, profile),
                plays: project_v1_plays(&relation.plays, profile),
            },
        );
        insert_doc_meta(
            &mut documentation_and_metadata,
            format!("type {}", relation.name),
            relation.doc.as_deref(),
            relation
                .meta
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        );
        project_v1_interface_docs(
            &mut documentation_and_metadata,
            &relation.name,
            &relation.owns,
            &relation.plays,
            &relation.roles,
        );
    }
    for attribute in schema.attributes.values() {
        types.insert(
            attribute.name.clone(),
            BasicTypeProjection {
                kind: BasicTypeKind::Attribute,
                parent: attribute.parent.clone(),
                is_abstract: attribute.is_abstract,
                is_independent: attribute.is_independent,
                value_type: normalize_v1_value_type(&attribute.value_type),
                owns: BTreeMap::new(),
                relates: BTreeMap::new(),
                plays: BTreeMap::new(),
            },
        );
        insert_doc_meta(
            &mut documentation_and_metadata,
            format!("type {}", attribute.name),
            attribute.doc.as_deref(),
            attribute
                .meta
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        );
    }
    SchemaProjection {
        types,
        functions: schema
            .functions
            .iter()
            .map(|(name, function)| (name.clone(), project_v1_function(function)))
            .collect(),
        structs: schema
            .structs
            .iter()
            .map(|(name, value)| (name.clone(), project_v1_struct(value)))
            .collect(),
        documentation_and_metadata,
    }
}

fn project_v2(schema: &ResolvedSchema) -> SchemaProjection {
    let mut documentation_and_metadata = BTreeMap::new();
    let types = schema
        .types()
        .values()
        .map(|resolved| {
            let id = resolved.id();
            let kind = match id.kind() {
                TypeKind::Entity => BasicTypeKind::Entity,
                TypeKind::Relation => BasicTypeKind::Relation,
                TypeKind::Attribute => BasicTypeKind::Attribute,
                TypeKind::Struct => unreachable!("resolved structs are not schema types"),
            };
            let parent = resolved
                .supertypes()
                .first()
                .map(|parent| parent.label().as_str().to_owned());
            let is_independent = resolved
                .annotations()
                .contains_key(&AnnotationKindId::Independent);
            let value_type = resolved
                .value_type()
                .map(|value| v2_value_type(value.value_type()).to_owned());
            insert_v2_doc_meta(
                &mut documentation_and_metadata,
                format!("type {}", id.label().as_str()),
                resolved.annotations(),
            );
            for owns in resolved.owns().values() {
                insert_v2_doc_meta(
                    &mut documentation_and_metadata,
                    format!(
                        "owns {} {}",
                        id.label().as_str(),
                        owns.id().attribute().label().as_str()
                    ),
                    owns.annotations(),
                );
            }
            for plays in resolved.plays().values() {
                insert_v2_doc_meta(
                    &mut documentation_and_metadata,
                    format!(
                        "plays {} {}:{}",
                        id.label().as_str(),
                        plays.id().role().declaring_relation().as_str(),
                        plays.id().role().label().as_str()
                    ),
                    plays.annotations(),
                );
            }
            for relates in resolved.relates().values() {
                insert_v2_doc_meta(
                    &mut documentation_and_metadata,
                    format!(
                        "relates {} {}",
                        id.label().as_str(),
                        relates.id().role().label().as_str()
                    ),
                    relates.annotations(),
                );
            }
            (
                id.label().as_str().to_owned(),
                BasicTypeProjection {
                    kind,
                    parent,
                    is_abstract: resolved.is_abstract(),
                    is_independent,
                    value_type,
                    owns: resolved
                        .owns()
                        .values()
                        .map(|owns| {
                            (
                                owns.id().attribute().label().as_str().to_owned(),
                                format_owns(owns.cardinality(), owns.is_key(), owns.is_unique()),
                            )
                        })
                        .collect(),
                    relates: resolved
                        .relates()
                        .values()
                        .map(|relates| {
                            let replaced = relates
                                .replaced_roles()
                                .iter()
                                .map(|role| role.label().as_str())
                                .collect::<Vec<_>>()
                                .join(",");
                            (
                                relates.id().role().label().as_str().to_owned(),
                                format_relates(
                                    relates.cardinality(),
                                    relates.is_abstract(),
                                    &replaced,
                                ),
                            )
                        })
                        .collect(),
                    plays: resolved
                        .plays()
                        .values()
                        .map(|plays| {
                            (
                                format!(
                                    "{}:{}",
                                    plays.id().role().declaring_relation().as_str(),
                                    plays.id().role().label().as_str()
                                ),
                                format_cardinality(plays.cardinality()),
                            )
                        })
                        .collect(),
                },
            )
        })
        .collect();
    SchemaProjection {
        types,
        functions: schema
            .functions()
            .values()
            .map(|function| {
                (
                    function.id().label().as_str().to_owned(),
                    project_v2_function(function.declaration()),
                )
            })
            .collect(),
        structs: schema
            .structs()
            .values()
            .map(|value| {
                (
                    value.id().label().as_str().to_owned(),
                    value
                        .fields()
                        .iter()
                        .map(|field| {
                            format!(
                                "{}:{}{}",
                                field.name().as_str(),
                                v2_value_type(field.value_type()),
                                if field.optional() { "?" } else { "" }
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(","),
                )
            })
            .collect(),
        documentation_and_metadata,
    }
}

fn compare(v1: &SchemaProjection, v2: &SchemaProjection) -> ShadowCompared {
    let coverage = coverage();
    let labels = v1
        .types
        .keys()
        .chain(v2.types.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut findings = BTreeSet::new();
    for label in labels {
        match (v1.types.get(&label), v2.types.get(&label)) {
            (Some(left), Some(right)) => {
                if left.kind != right.kind {
                    findings.insert(ShadowFinding::new(
                        ShadowDimension::TypeKind,
                        &label,
                        Some(left.kind.as_str().to_owned()),
                        Some(right.kind.as_str().to_owned()),
                    ));
                }
                insert_difference(
                    &mut findings,
                    ShadowDimension::DirectParent,
                    &label,
                    left.parent.clone(),
                    right.parent.clone(),
                );
                insert_difference(
                    &mut findings,
                    ShadowDimension::TypeAbstract,
                    &label,
                    Some(left.is_abstract.to_string()),
                    Some(right.is_abstract.to_string()),
                );
                insert_difference(
                    &mut findings,
                    ShadowDimension::AttributeIndependent,
                    &label,
                    Some(left.is_independent.to_string()),
                    Some(right.is_independent.to_string()),
                );
                insert_difference(
                    &mut findings,
                    ShadowDimension::ValueType,
                    &label,
                    left.value_type.clone(),
                    right.value_type.clone(),
                );
                compare_named_values(
                    &mut findings,
                    ShadowDimension::EffectiveOwns,
                    &format!("{label} owns "),
                    &left.owns,
                    &right.owns,
                );
                compare_named_values(
                    &mut findings,
                    ShadowDimension::EffectiveRelates,
                    &format!("{label} relates "),
                    &left.relates,
                    &right.relates,
                );
                compare_named_values(
                    &mut findings,
                    ShadowDimension::EffectivePlays,
                    &format!("{label} plays "),
                    &left.plays,
                    &right.plays,
                );
            }
            (Some(left), None) => {
                findings.insert(ShadowFinding::new(
                    ShadowDimension::TypeExistence,
                    &label,
                    Some(left.kind.as_str().to_owned()),
                    None,
                ));
            }
            (None, Some(right)) => {
                findings.insert(ShadowFinding::new(
                    ShadowDimension::TypeExistence,
                    &label,
                    None,
                    Some(right.kind.as_str().to_owned()),
                ));
            }
            (None, None) => {}
        }
    }
    compare_named_values(
        &mut findings,
        ShadowDimension::FunctionSignatures,
        "function ",
        &v1.functions,
        &v2.functions,
    );
    compare_named_values(
        &mut findings,
        ShadowDimension::StructFields,
        "struct ",
        &v1.structs,
        &v2.structs,
    );
    compare_named_values(
        &mut findings,
        ShadowDimension::DocumentationAndMetadata,
        "",
        &v1.documentation_and_metadata,
        &v2.documentation_and_metadata,
    );
    let findings = findings.into_iter().collect::<Vec<_>>();
    ShadowCompared {
        verdict: if findings.is_empty() {
            ShadowVerdict::Matched
        } else {
            ShadowVerdict::Mismatched
        },
        coverage,
        findings,
    }
}

fn compare_named_values(
    findings: &mut BTreeSet<ShadowFinding>,
    dimension: ShadowDimension,
    subject_prefix: &str,
    v1: &BTreeMap<String, String>,
    v2: &BTreeMap<String, String>,
) {
    for key in v1.keys().chain(v2.keys()).collect::<BTreeSet<_>>() {
        insert_difference(
            findings,
            dimension,
            &format!("{subject_prefix}{key}"),
            v1.get(key).cloned(),
            v2.get(key).cloned(),
        );
    }
}

fn insert_difference(
    findings: &mut BTreeSet<ShadowFinding>,
    dimension: ShadowDimension,
    label: &str,
    v1_value: Option<String>,
    v2_value: Option<String>,
) {
    if v1_value != v2_value {
        findings.insert(ShadowFinding::new(dimension, label, v1_value, v2_value));
    }
}

fn coverage() -> ShadowCoverage {
    let compared = BTreeSet::from([
        ShadowDimension::TypeExistence,
        ShadowDimension::TypeKind,
        ShadowDimension::DirectParent,
        ShadowDimension::TypeAbstract,
        ShadowDimension::AttributeIndependent,
        ShadowDimension::ValueType,
        ShadowDimension::EffectiveOwns,
        ShadowDimension::EffectiveRelates,
        ShadowDimension::EffectivePlays,
        ShadowDimension::DocumentationAndMetadata,
        ShadowDimension::FunctionSignatures,
    ]);
    let unimplemented = BTreeSet::new();
    let not_representable = BTreeSet::from([
        ShadowDimension::FunctionBodiesAndAnnotations,
        ShadowDimension::StructFields,
        ShadowDimension::SourceCommentsAndSpans,
        ShadowDimension::OmittedVersusExplicitIdentity,
        ShadowDimension::IndependentAnnotationIdentityAndRemoval,
        ShadowDimension::SubAnnotations,
        ShadowDimension::ExtensionsAndCapabilities,
        ShadowDimension::ResolverGraphsAndOrigins,
        ShadowDimension::CardinalityOutsideV1U32,
    ]);
    let blind_spots = unimplemented.union(&not_representable).copied().collect();
    ShadowCoverage {
        compared,
        unimplemented,
        not_representable,
        blind_spots,
    }
}

fn project_v1_owns(owns: &[OwnedAttribute], profile: &SemanticProfile) -> BTreeMap<String, String> {
    owns.iter()
        .map(|owns| {
            let cardinality = if owns.is_key {
                V2Cardinality::new(1, Some(1)).expect("the effective key cardinality is exact-one")
            } else {
                v1_cardinality(owns.cardinality.as_ref(), profile, InterfaceKind::Owns)
            };
            (
                owns.name.clone(),
                format_owns(cardinality, owns.is_key, owns.is_key || owns.is_unique),
            )
        })
        .collect()
}

fn project_v1_plays(plays: &[PlayedRole], profile: &SemanticProfile) -> BTreeMap<String, String> {
    plays
        .iter()
        .map(|plays| {
            (
                plays.role_ref.clone(),
                format_cardinality(v1_cardinality(
                    plays.cardinality.as_ref(),
                    profile,
                    InterfaceKind::Plays,
                )),
            )
        })
        .collect()
}

fn project_v1_relates(roles: &[RoleSpec], profile: &SemanticProfile) -> BTreeMap<String, String> {
    roles
        .iter()
        .map(|role| {
            (
                role.name.clone(),
                format_relates(
                    v1_cardinality(role.cardinality.as_ref(), profile, InterfaceKind::Relates),
                    role.is_abstract,
                    role.overrides.as_deref().unwrap_or(""),
                ),
            )
        })
        .collect()
}

fn v1_cardinality(
    cardinality: Option<&V1Cardinality>,
    profile: &SemanticProfile,
    interface: InterfaceKind,
) -> V2Cardinality {
    cardinality.map_or_else(
        || profile.default_cardinality(interface),
        |cardinality| {
            V2Cardinality::new(u64::from(cardinality.min), cardinality.max.map(u64::from))
                .expect("validated V1 cardinality must fit the V2 domain")
        },
    )
}

fn format_cardinality(cardinality: V2Cardinality) -> String {
    format!(
        "{}..{}",
        cardinality.min(),
        cardinality
            .max()
            .map_or_else(|| "unbounded".to_owned(), |maximum| maximum.to_string())
    )
}

fn format_owns(cardinality: V2Cardinality, key: bool, unique: bool) -> String {
    format!(
        "card={};key={key};unique={unique}",
        format_cardinality(cardinality)
    )
}

fn format_relates(cardinality: V2Cardinality, is_abstract: bool, replaces: &str) -> String {
    format!(
        "card={};abstract={is_abstract};replaces={replaces}",
        format_cardinality(cardinality)
    )
}

fn project_v1_interface_docs(
    projection: &mut BTreeMap<String, String>,
    owner: &str,
    owns: &[OwnedAttribute],
    plays: &[PlayedRole],
    relates: &[RoleSpec],
) {
    for interface in owns {
        insert_doc_meta(
            projection,
            format!("owns {owner} {}", interface.name),
            interface.doc.as_deref(),
            interface
                .meta
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        );
    }
    for interface in plays {
        insert_doc_meta(
            projection,
            format!("plays {owner} {}", interface.role_ref),
            interface.doc.as_deref(),
            interface
                .meta
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        );
    }
    for interface in relates {
        insert_doc_meta(
            projection,
            format!("relates {owner} {}", interface.name),
            interface.doc.as_deref(),
            interface
                .meta
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str())),
        );
    }
}

fn insert_doc_meta<'a>(
    projection: &mut BTreeMap<String, String>,
    subject: String,
    doc: Option<&str>,
    meta: impl Iterator<Item = (&'a str, &'a str)>,
) {
    let meta = meta
        .map(|(key, value)| format!("{key:?}={value:?}"))
        .collect::<Vec<_>>()
        .join(",");
    if doc.is_some() || !meta.is_empty() {
        projection.insert(subject, format!("doc={doc:?};meta={meta}"));
    }
}

fn insert_v2_doc_meta(
    projection: &mut BTreeMap<String, String>,
    subject: String,
    annotations: &BTreeMap<AnnotationKindId, SchemaAnnotationValue>,
) {
    let doc = annotations.get(&AnnotationKindId::Doc).and_then(|value| {
        if let SchemaAnnotationValue::Doc(doc) = value {
            Some(doc.as_str())
        } else {
            None
        }
    });
    let meta = annotations.iter().filter_map(|(kind, value)| {
        let AnnotationKindId::Meta(key) = kind else {
            return None;
        };
        let value = match value {
            SchemaAnnotationValue::Meta(CanonicalValue::String(value)) => value.as_str().to_owned(),
            SchemaAnnotationValue::Meta(value) => format!("{value:?}"),
            _ => return None,
        };
        Some((key.as_str(), value))
    });
    let meta = meta.collect::<Vec<_>>();
    insert_doc_meta(
        projection,
        subject,
        doc,
        meta.iter().map(|(key, value)| (*key, value.as_str())),
    );
}

fn project_v1_function(function: &V1Function) -> String {
    let parameters = function
        .parameters
        .iter()
        .map(|parameter| {
            format!(
                "{}:{}",
                parameter.name,
                normalize_type_token(&parameter.type_)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let returns = function
        .return_type
        .types
        .iter()
        .map(|value| {
            format!(
                "{}{}",
                normalize_type_token(&value.name),
                if value.optional { "?" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let mode = if function.return_type.is_stream {
        "stream"
    } else if function.return_type.types.len() == 1 {
        "scalar"
    } else {
        "tuple"
    };
    format!("({parameters})->{mode}({returns})")
}

fn project_v2_function(function: &type_bridge_contract::schema::FunctionFact) -> String {
    let signature = function.signature();
    let parameters = signature
        .parameters()
        .iter()
        .map(|parameter| {
            format!(
                "{}:{}",
                parameter.name().as_str(),
                type_reference(parameter.type_ref())
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let (mode, elements) = match signature.returns() {
        FunctionReturnMode::Scalar(element) => ("scalar", std::slice::from_ref(element)),
        FunctionReturnMode::Tuple(elements) => ("tuple", elements.as_slice()),
        FunctionReturnMode::Stream(elements) => ("stream", elements.as_slice()),
    };
    let returns = elements
        .iter()
        .map(|element| {
            format!(
                "{}{}",
                type_reference(element.type_ref()),
                if element.optional() { "?" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("({parameters})->{mode}({returns})")
}

fn type_reference(reference: &TypeReference) -> String {
    match reference {
        TypeReference::Value(value) => v2_value_type(*value).to_owned(),
        TypeReference::Schema(label) => label.as_str().to_owned(),
    }
}

fn project_v1_struct(value: &V1Struct) -> String {
    value
        .fields
        .iter()
        .map(|field| {
            format!(
                "{}:{}{}",
                field.name,
                normalize_type_token(&field.value_type),
                if field.optional { "?" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn normalize_type_token(value: &str) -> String {
    normalize_v1_value_type(value).unwrap_or_else(|| value.trim().to_owned())
}

fn normalize_v1_value_type(value: &str) -> Option<String> {
    let normalized = match value.trim() {
        "" => return None,
        "long" | "integer" => "integer",
        "bool" | "boolean" => "boolean",
        "datetime-tz" | "datetime_tz" => "datetime-tz",
        other => other,
    };
    Some(normalized.to_owned())
}

const fn v2_value_type(value: ValueTypeTag) -> &'static str {
    match value {
        ValueTypeTag::String => "string",
        ValueTypeTag::Long => "integer",
        ValueTypeTag::Double => "double",
        ValueTypeTag::Boolean => "boolean",
        ValueTypeTag::Date => "date",
        ValueTypeTag::DateTime => "datetime",
        ValueTypeTag::DateTimeTz => "datetime-tz",
        ValueTypeTag::Decimal => "decimal",
        ValueTypeTag::Duration => "duration",
    }
}
