//! Versioned schema identities, facts, provenance, and declared fingerprints.
//!
//! This module contains no parser, filesystem, provider, binding, query, or
//! migration dependencies. Trusted values are created only through validating
//! constructors; canonical decoding will use explicit validated wire types.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Serialize, Serializer};

use crate::capability::CapabilitySet;
use crate::codec::{FormatVersion, ensure_format_version, to_canonical_json};
use crate::diagnostic::{Diagnostic, DiagnosticCategory};
use crate::fingerprint::{CanonicalizationVersion, Fingerprint, FingerprintDomain};
use crate::id::{AttributeId, FunctionId, Label, RoleId, StructId, TypeId, TypeKind};
use crate::limits::{MAX_CANONICAL_COLLECTION_LEN, MAX_CANONICAL_STRING_BYTES};
use crate::value::{CanonicalValue, Cardinality, ValueTypeTag};

pub use crate::managed_scope::{
    ManagedScopeBinding, ManagedScopeId, ManagedScopeProfileBinding,
    ManagedScopeProfileFingerprint, ManagedScopeProfileId, SemanticProfileFingerprint,
};
pub use crate::schema_delta::{
    ManagedFactSelection, ManagedSchemaState, PatchFormatVersion, SchemaDelta, SchemaOperation,
    SchemaOperationKind, decode_schema_delta, encode_schema_delta,
};
pub use crate::schema_fingerprint::{
    ManagedDeclaredIdentityFingerprint, ManagedSemanticSchemaFingerprint,
    SchemaDocumentSetFingerprint, SemanticSchemaFingerprint,
};
pub use crate::semantic_profile::{InterfaceKind, SemanticProfile};

/// Maximum UTF-8 length of a normalized schema document identifier.
pub const MAX_DOCUMENT_ID_BYTES: usize = 4096;

/// A normalized, relative, forward-slash schema source identifier.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct DocumentId(String);

impl DocumentId {
    /// Validate a schema document identifier.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        let valid_segments = value
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..");
        if value.is_empty()
            || value.len() > MAX_DOCUMENT_ID_BYTES
            || value.starts_with('/')
            || value.contains('\\')
            || value.contains('\0')
            || !valid_segments
        {
            return Err(schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_schema_document_id",
                "schema document identifiers must be normalized relative paths",
            )
            .with_detail("document", value));
        }
        Ok(Self(value))
    }

    /// Return the normalized identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DocumentId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A byte-exact source span with one-based line and column positions.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct SourceSpan {
    document: DocumentId,
    byte_start: u64,
    byte_end: u64,
    line: u32,
    column: u32,
    end_line: u32,
    end_column: u32,
}

impl SourceSpan {
    /// Construct a validated source span.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        document: DocumentId,
        byte_start: u64,
        byte_end: u64,
        line: u32,
        column: u32,
        end_line: u32,
        end_column: u32,
    ) -> Result<Self, Diagnostic> {
        if byte_start > byte_end
            || line == 0
            || column == 0
            || end_line == 0
            || end_column == 0
            || (end_line, end_column) < (line, column)
        {
            return Err(schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_schema_source_span",
                "schema source spans must be ordered and one-based",
            ));
        }
        Ok(Self {
            document,
            byte_start,
            byte_end,
            line,
            column,
            end_line,
            end_column,
        })
    }

    /// Return the source document identifier.
    pub fn document(&self) -> &DocumentId {
        &self.document
    }

    /// Return the inclusive byte start.
    pub const fn byte_start(&self) -> u64 {
        self.byte_start
    }

    /// Return the exclusive byte end.
    pub const fn byte_end(&self) -> u64 {
        self.byte_end
    }

    /// Return the one-based start line.
    pub const fn line(&self) -> u32 {
        self.line
    }

    /// Return the one-based start column.
    pub const fn column(&self) -> u32 {
        self.column
    }

    /// Return the one-based end line.
    pub const fn end_line(&self) -> u32 {
        self.end_line
    }

    /// Return the one-based end column.
    pub const fn end_column(&self) -> u32 {
        self.end_column
    }
}

/// A secondary source label attached to a schema diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiagnosticLabel {
    span: SourceSpan,
    message: String,
}

impl DiagnosticLabel {
    /// Construct a related diagnostic label.
    pub fn new(span: SourceSpan, message: impl Into<String>) -> Self {
        Self {
            span,
            message: message.into(),
        }
    }

    /// Return the related source span.
    pub const fn span(&self) -> &SourceSpan {
        &self.span
    }

    /// Return the related-label message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// A stable contract diagnostic enriched with schema source locations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SchemaDiagnostic {
    diagnostic: Diagnostic,
    primary: Option<SourceSpan>,
    related: Vec<DiagnosticLabel>,
}

impl SchemaDiagnostic {
    /// Construct a source-aware schema diagnostic.
    pub fn new(diagnostic: Diagnostic, primary: Option<SourceSpan>) -> Self {
        Self {
            diagnostic,
            primary,
            related: Vec::new(),
        }
    }

    /// Attach a related source label.
    pub fn with_related(mut self, label: DiagnosticLabel) -> Self {
        self.related.push(label);
        self
    }

    /// Return the stable diagnostic payload.
    pub const fn diagnostic(&self) -> &Diagnostic {
        &self.diagnostic
    }

    /// Return the primary source span, if known.
    pub fn primary(&self) -> Option<&SourceSpan> {
        self.primary.as_ref()
    }

    /// Return related source labels.
    pub fn related(&self) -> &[DiagnosticLabel] {
        &self.related
    }
}

/// One or more schema diagnostics produced by a fail-closed operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaDiagnostics(Vec<SchemaDiagnostic>);

impl SchemaDiagnostics {
    /// Construct a non-empty diagnostic collection.
    pub fn one(diagnostic: SchemaDiagnostic) -> Self {
        Self(vec![diagnostic])
    }

    /// Construct a diagnostic collection from accumulated errors.
    pub fn from_vec(diagnostics: Vec<SchemaDiagnostic>) -> Self {
        Self(diagnostics)
    }

    /// Return all diagnostics in stable order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &SchemaDiagnostic> {
        self.0.iter()
    }

    /// Return the number of diagnostics.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Return whether the collection is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Consume the collection.
    pub fn into_vec(self) -> Vec<SchemaDiagnostic> {
        self.0
    }
}

impl fmt::Display for SchemaDiagnostics {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(first) = self.0.first() {
            write!(formatter, "{}", first.diagnostic())?;
            if self.0.len() > 1 {
                write!(formatter, " (and {} more)", self.0.len() - 1)?;
            }
            Ok(())
        } else {
            formatter.write_str("schema validation failed")
        }
    }
}

impl Error for SchemaDiagnostics {}

/// Identity of a direct subtype edge.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct SubFactId {
    subtype: TypeId,
    supertype: TypeId,
}

impl SubFactId {
    /// Construct a subtype-edge identity.
    pub fn new(subtype: TypeId, supertype: TypeId) -> Result<Self, Diagnostic> {
        if subtype.kind() == TypeKind::Struct
            || supertype.kind() == TypeKind::Struct
            || subtype.kind() != supertype.kind()
            || subtype == supertype
        {
            return Err(schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_sub_fact",
                "subtype edges require distinct types of the same non-struct kind",
            ));
        }
        Ok(Self { subtype, supertype })
    }

    /// Return the subtype.
    pub const fn subtype(&self) -> &TypeId {
        &self.subtype
    }

    /// Return the direct supertype.
    pub const fn supertype(&self) -> &TypeId {
        &self.supertype
    }
}

/// Identity of an attribute value declaration.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ValueFactId(AttributeId);

impl ValueFactId {
    /// Construct an attribute value-fact identity.
    pub const fn new(attribute: AttributeId) -> Self {
        Self(attribute)
    }

    /// Return the attribute identity.
    pub const fn attribute(&self) -> &AttributeId {
        &self.0
    }
}

/// Identity of a direct ownership declaration.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct OwnsFactId {
    owner: TypeId,
    attribute: AttributeId,
}

impl OwnsFactId {
    /// Construct an ownership identity.
    pub fn new(owner: TypeId, attribute: AttributeId) -> Result<Self, Diagnostic> {
        if !matches!(owner.kind(), TypeKind::Entity | TypeKind::Relation) {
            return Err(schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_owns_owner",
                "only entity and relation types can own attributes",
            ));
        }
        Ok(Self { owner, attribute })
    }

    /// Return the owning type.
    pub const fn owner(&self) -> &TypeId {
        &self.owner
    }

    /// Return the owned attribute.
    pub const fn attribute(&self) -> &AttributeId {
        &self.attribute
    }
}

/// Identity of a direct related-role declaration.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct RelatesFactId {
    relation: TypeId,
    role: RoleId,
}

impl RelatesFactId {
    /// Construct a related-role identity.
    pub fn new(relation: TypeId, role: RoleId) -> Result<Self, Diagnostic> {
        if relation.kind() != TypeKind::Relation || relation.label() != role.declaring_relation() {
            return Err(schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_relates_identity",
                "a related role must be declared by its relation type",
            ));
        }
        Ok(Self { relation, role })
    }

    /// Return the declaring relation.
    pub const fn relation(&self) -> &TypeId {
        &self.relation
    }

    /// Return the declared role.
    pub const fn role(&self) -> &RoleId {
        &self.role
    }
}

/// Identity of a direct role-playing declaration.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct PlaysFactId {
    player: TypeId,
    role: RoleId,
}

impl PlaysFactId {
    /// Construct a role-playing identity.
    pub fn new(player: TypeId, role: RoleId) -> Result<Self, Diagnostic> {
        if !matches!(player.kind(), TypeKind::Entity | TypeKind::Relation) {
            return Err(schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_plays_player",
                "only entity and relation types can play roles",
            ));
        }
        Ok(Self { player, role })
    }

    /// Return the player type.
    pub const fn player(&self) -> &TypeId {
        &self.player
    }

    /// Return the played role.
    pub const fn role(&self) -> &RoleId {
        &self.role
    }
}

/// A structural fact that may carry an independent annotation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum AnnotationSubjectId {
    /// A type declaration.
    Type(TypeId),
    /// A subtype edge.
    Sub(SubFactId),
    /// An attribute value declaration.
    Value(ValueFactId),
    /// An ownership declaration.
    Owns(OwnsFactId),
    /// A related-role declaration.
    Relates(RelatesFactId),
    /// A role-playing declaration.
    Plays(PlaysFactId),
    /// A function declaration.
    Function(FunctionId),
}

/// Stable identity of a schema annotation kind.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(tag = "kind", content = "key", rename_all = "snake_case")]
pub enum AnnotationKindId {
    /// `@abstract`.
    Abstract,
    /// `@independent`.
    Independent,
    /// `@key`.
    Key,
    /// `@unique`.
    Unique,
    /// `@card`.
    Card,
    /// `@regex`.
    Regex,
    /// `@range`.
    Range,
    /// `@values`.
    Values,
    /// `@doc`.
    Doc,
    /// One independently identified `@meta` key.
    Meta(Label),
}

impl AnnotationKindId {
    /// Construct an independently identified metadata kind.
    pub fn meta(key: impl Into<String>) -> Result<Self, Diagnostic> {
        Label::new(key).map(Self::Meta)
    }
}

/// Identity of an independent annotation fact.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct AnnotationFactId {
    subject: AnnotationSubjectId,
    kind: AnnotationKindId,
}

impl AnnotationFactId {
    /// Construct an annotation identity.
    pub const fn new(subject: AnnotationSubjectId, kind: AnnotationKindId) -> Self {
        Self { subject, kind }
    }

    /// Return the annotated subject.
    pub const fn subject(&self) -> &AnnotationSubjectId {
        &self.subject
    }

    /// Return the annotation kind.
    pub const fn kind(&self) -> &AnnotationKindId {
        &self.kind
    }
}

/// A validated regular-expression payload retained without a regex engine.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct RegexPattern(String);

impl RegexPattern {
    /// Validate a regex source payload.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_CANONICAL_STRING_BYTES {
            return Err(schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_regex_annotation",
                "regex annotation text must be non-empty and bounded",
            ));
        }
        Ok(Self(value))
    }

    /// Return the regex source.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A validated non-empty documentation payload.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct DocText(String);

impl DocText {
    /// Validate documentation text.
    pub fn new(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_CANONICAL_STRING_BYTES {
            return Err(schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_doc_annotation",
                "documentation text must be non-empty and bounded",
            ));
        }
        Ok(Self(value))
    }

    /// Return the documentation text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A non-empty exact-domain canonical value set.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct CanonicalValueSet(BTreeSet<CanonicalValue>);

/// Precise validation failure for a raw `@values` member sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicalValueSetViolation {
    /// No members were supplied.
    Empty,
    /// The raw member sequence exceeded the canonical collection ceiling.
    MemberLimitExceeded {
        /// Maximum accepted raw member count.
        maximum: usize,
        /// Index of the first rejected raw member.
        first_excess_index: usize,
    },
    /// One member used a different exact scalar domain.
    MixedDomain {
        /// Domain established by the first member.
        expected: ValueTypeTag,
        /// Domain of the conflicting member.
        actual: ValueTypeTag,
        /// Index of the conflicting member.
        member_index: usize,
    },
    /// One exact canonical value occurred more than once.
    Duplicate {
        /// Index of the first occurrence.
        first_index: usize,
        /// Index of the duplicate occurrence.
        duplicate_index: usize,
    },
}

impl CanonicalValueSetViolation {
    /// Convert to the stable compatibility diagnostic returned by `new`.
    pub fn into_diagnostic(self) -> Diagnostic {
        match self {
            Self::Empty => schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "empty_values_annotation",
                "values annotations must contain at least one value",
            ),
            Self::MemberLimitExceeded {
                maximum,
                first_excess_index,
            } => schema_diagnostic(
                DiagnosticCategory::ResourceLimit,
                "values_annotation_member_limit_exceeded",
                "values annotation exceeds the raw member ceiling",
            )
            .with_detail(
                "maximum_members",
                i64::try_from(maximum).expect("collection limit fits i64"),
            )
            .with_detail(
                "first_excess_index",
                i64::try_from(first_excess_index).expect("collection index fits i64"),
            ),
            Self::MixedDomain {
                expected,
                actual,
                member_index,
            } => schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "mixed_values_annotation_domain",
                "values annotations require one exact scalar domain",
            )
            .with_detail("expected_value_type", expected.as_str())
            .with_detail("actual_value_type", actual.as_str())
            .with_detail(
                "member_index",
                i64::try_from(member_index).expect("collection index fits i64"),
            ),
            Self::Duplicate {
                first_index,
                duplicate_index,
            } => schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "duplicate_values_annotation_value",
                "values annotations cannot contain duplicates",
            )
            .with_detail(
                "first_index",
                i64::try_from(first_index).expect("collection index fits i64"),
            )
            .with_detail(
                "duplicate_index",
                i64::try_from(duplicate_index).expect("collection index fits i64"),
            ),
        }
    }
}

impl CanonicalValueSet {
    /// Validate a set, rejecting empty, mixed-domain, and duplicate input.
    pub fn new(values: impl IntoIterator<Item = CanonicalValue>) -> Result<Self, Diagnostic> {
        Self::new_detailed(values).map_err(CanonicalValueSetViolation::into_diagnostic)
    }

    /// Validate a raw sequence while retaining member indices for source diagnostics.
    pub fn new_detailed(
        values: impl IntoIterator<Item = CanonicalValue>,
    ) -> Result<Self, CanonicalValueSetViolation> {
        let mut positions = BTreeMap::new();
        let mut value_type = None;
        for (member_index, value) in values.into_iter().enumerate() {
            if member_index >= MAX_CANONICAL_COLLECTION_LEN {
                return Err(CanonicalValueSetViolation::MemberLimitExceeded {
                    maximum: MAX_CANONICAL_COLLECTION_LEN,
                    first_excess_index: member_index,
                });
            }
            if let Some(expected) = value_type {
                if expected != value.value_type() {
                    return Err(CanonicalValueSetViolation::MixedDomain {
                        expected,
                        actual: value.value_type(),
                        member_index,
                    });
                }
            } else {
                value_type = Some(value.value_type());
            }
            if let Some(first_index) = positions.get(&value) {
                return Err(CanonicalValueSetViolation::Duplicate {
                    first_index: *first_index,
                    duplicate_index: member_index,
                });
            }
            positions.insert(value, member_index);
        }
        if positions.is_empty() {
            return Err(CanonicalValueSetViolation::Empty);
        }
        Ok(Self(positions.into_keys().collect()))
    }

    /// Return values in canonical order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &CanonicalValue> {
        self.0.iter()
    }
}

/// An exact-domain, non-empty canonical value range.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct CanonicalValueRange {
    lower: Option<CanonicalValue>,
    upper: Option<CanonicalValue>,
}

/// Precise validation failure for canonical range bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalValueRangeViolation {
    /// Neither bound was supplied.
    Empty,
    /// The two bounds used different exact scalar domains.
    MixedDomain {
        /// Lower-bound domain.
        lower: ValueTypeTag,
        /// Upper-bound domain.
        upper: ValueTypeTag,
    },
    /// The scalar domain has no provider range ordering.
    UnsupportedDomain {
        /// Unsupported scalar domain.
        value_type: ValueTypeTag,
    },
    /// The lower bound was equal to or greater than the upper bound.
    InvalidBounds {
        /// Semantic ordering of lower relative to upper.
        ordering: Ordering,
    },
}

impl CanonicalValueRangeViolation {
    /// Convert to the stable compatibility diagnostic returned by `new`.
    pub fn into_diagnostic(self) -> Diagnostic {
        match self {
            Self::Empty => schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "empty_range_annotation",
                "range annotations require at least one bound",
            ),
            Self::MixedDomain { lower, upper } => schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "mixed_range_annotation_domain",
                "range bounds require one exact scalar domain",
            )
            .with_detail("lower_value_type", lower.as_str())
            .with_detail("upper_value_type", upper.as_str()),
            Self::UnsupportedDomain { value_type } => schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "unsupported_range_annotation_domain",
                "range annotations require an ordered scalar domain",
            )
            .with_detail("value_type", value_type.as_str()),
            Self::InvalidBounds { ordering } => schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_range_annotation_bounds",
                "range lower bounds must be strictly less than upper bounds",
            )
            .with_detail(
                "ordering",
                match ordering {
                    Ordering::Less => "less",
                    Ordering::Equal => "equal",
                    Ordering::Greater => "greater",
                },
            ),
        }
    }
}

impl CanonicalValueRange {
    /// Validate a range with at least one bound and one exact scalar domain.
    pub fn new(
        lower: Option<CanonicalValue>,
        upper: Option<CanonicalValue>,
    ) -> Result<Self, Diagnostic> {
        Self::new_detailed(lower, upper).map_err(CanonicalValueRangeViolation::into_diagnostic)
    }

    /// Validate bounds while retaining the exact failure for source diagnostics.
    pub fn new_detailed(
        lower: Option<CanonicalValue>,
        upper: Option<CanonicalValue>,
    ) -> Result<Self, CanonicalValueRangeViolation> {
        if lower.is_none() && upper.is_none() {
            return Err(CanonicalValueRangeViolation::Empty);
        }
        if let (Some(lower), Some(upper)) = (&lower, &upper)
            && lower.value_type() != upper.value_type()
        {
            return Err(CanonicalValueRangeViolation::MixedDomain {
                lower: lower.value_type(),
                upper: upper.value_type(),
            });
        }
        let value_type = lower
            .as_ref()
            .or(upper.as_ref())
            .expect("non-empty range has one bound")
            .value_type();
        if matches!(value_type, ValueTypeTag::Duration) {
            return Err(CanonicalValueRangeViolation::UnsupportedDomain { value_type });
        }
        if let (Some(lower), Some(upper)) = (&lower, &upper) {
            let ordering = lower
                .semantic_cmp_same_domain(upper)
                .expect("every supported exact domain has semantic ordering");
            if ordering != Ordering::Less {
                return Err(CanonicalValueRangeViolation::InvalidBounds { ordering });
            }
        }
        Ok(Self { lower, upper })
    }

    /// Return the lower bound.
    pub const fn lower(&self) -> Option<&CanonicalValue> {
        self.lower.as_ref()
    }

    /// Return the upper bound.
    pub const fn upper(&self) -> Option<&CanonicalValue> {
        self.upper.as_ref()
    }
}

/// A closed, kind-safe schema annotation payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SchemaAnnotationValue {
    /// A marker annotation with no payload.
    Presence,
    /// A cardinality payload.
    Cardinality(Cardinality),
    /// A regex source payload.
    Regex(RegexPattern),
    /// An exact canonical value range.
    Range(CanonicalValueRange),
    /// A non-empty canonical value set.
    Values(CanonicalValueSet),
    /// Documentation text.
    Doc(DocText),
    /// A typed metadata value.
    Meta(CanonicalValue),
}

/// Existence of an entity, relation, or attribute type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeFact {
    id: TypeId,
}

impl TypeFact {
    /// Construct a type-existence fact.
    pub fn new(id: TypeId) -> Result<Self, Diagnostic> {
        if id.kind() == TypeKind::Struct {
            return Err(schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_type_fact_kind",
                "struct existence uses StructFact",
            ));
        }
        if value_type_tag(id.label().as_str()).is_some() {
            return Err(schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "reserved_schema_type_label",
                "schema type labels cannot collide with built-in value-type tokens",
            ));
        }
        Ok(Self { id })
    }

    /// Return the type identity.
    pub const fn id(&self) -> &TypeId {
        &self.id
    }
}

/// A direct subtype fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubFact {
    id: SubFactId,
}

impl SubFact {
    /// Construct a subtype fact.
    pub const fn new(id: SubFactId) -> Self {
        Self { id }
    }

    /// Return the fact identity.
    pub const fn id(&self) -> &SubFactId {
        &self.id
    }
}

/// An attribute scalar-domain fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValueFact {
    id: ValueFactId,
    value_type: ValueTypeTag,
}

impl ValueFact {
    /// Construct an attribute scalar-domain fact.
    pub const fn new(id: ValueFactId, value_type: ValueTypeTag) -> Self {
        Self { id, value_type }
    }

    /// Return the fact identity.
    pub const fn id(&self) -> &ValueFactId {
        &self.id
    }

    /// Return the scalar domain.
    pub const fn value_type(&self) -> ValueTypeTag {
        self.value_type
    }
}

/// A direct ownership fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OwnsFact {
    id: OwnsFactId,
}

impl OwnsFact {
    /// Construct an ownership fact.
    pub const fn new(id: OwnsFactId) -> Self {
        Self { id }
    }

    /// Return the fact identity.
    pub const fn id(&self) -> &OwnsFactId {
        &self.id
    }
}

/// A direct related-role fact, optionally specializing a parent role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelatesFact {
    id: RelatesFactId,
    specializes: Option<RoleId>,
}

impl RelatesFact {
    /// Construct a related-role fact.
    pub fn new(id: RelatesFactId, specializes: Option<RoleId>) -> Result<Self, Diagnostic> {
        if specializes.as_ref().is_some_and(|role| role == id.role()) {
            return Err(schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "self_specializing_role",
                "a role cannot specialize itself",
            ));
        }
        Ok(Self { id, specializes })
    }

    /// Return the fact identity.
    pub const fn id(&self) -> &RelatesFactId {
        &self.id
    }

    /// Return the specialized parent role, if any.
    pub const fn specializes(&self) -> Option<&RoleId> {
        self.specializes.as_ref()
    }
}

/// A direct role-playing fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlaysFact {
    id: PlaysFactId,
}

impl PlaysFact {
    /// Construct a role-playing fact.
    pub const fn new(id: PlaysFactId) -> Self {
        Self { id }
    }

    /// Return the fact identity.
    pub const fn id(&self) -> &PlaysFactId {
        &self.id
    }
}

/// An independently identified schema annotation fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnnotationFact {
    id: AnnotationFactId,
    value: SchemaAnnotationValue,
}

impl AnnotationFact {
    /// Construct and validate an annotation fact.
    pub fn new(id: AnnotationFactId, value: SchemaAnnotationValue) -> Result<Self, Diagnostic> {
        validate_annotation(id.subject(), id.kind(), &value)?;
        Ok(Self { id, value })
    }

    /// Return the annotation identity.
    pub const fn id(&self) -> &AnnotationFactId {
        &self.id
    }

    /// Return the validated payload.
    pub const fn value(&self) -> &SchemaAnnotationValue {
        &self.value
    }
}

/// A type reference used by a function signature.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum TypeReference {
    /// One of the closed built-in scalar value types.
    Value(ValueTypeTag),
    /// A schema type or struct label resolved against the declared graph.
    Schema(Label),
}

impl TypeReference {
    /// Parse the unambiguous TypeQL type-position spelling.
    pub fn from_token(value: impl Into<String>) -> Result<Self, Diagnostic> {
        let value = value.into();
        Ok(value_type_tag(&value)
            .map(Self::Value)
            .unwrap_or(Self::Schema(Label::new(value)?)))
    }

    /// Return the referenced schema label, if this is not a built-in value type.
    pub const fn schema_label(&self) -> Option<&Label> {
        match self {
            Self::Value(_) => None,
            Self::Schema(label) => Some(label),
        }
    }
}

/// One ordered function parameter.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct FunctionParameter {
    name: Label,
    type_ref: TypeReference,
}

impl FunctionParameter {
    /// Construct a parameter from validated contract values.
    pub const fn new(name: Label, type_ref: TypeReference) -> Self {
        Self { name, type_ref }
    }

    /// Return the parameter name without a provider variable sigil.
    pub const fn name(&self) -> &Label {
        &self.name
    }

    /// Return the parameter type reference.
    pub const fn type_ref(&self) -> &TypeReference {
        &self.type_ref
    }
}

/// One ordered element in a function return signature.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct FunctionReturnElement {
    type_ref: TypeReference,
    optional: bool,
}

impl FunctionReturnElement {
    /// Construct one return element.
    pub const fn new(type_ref: TypeReference, optional: bool) -> Self {
        Self { type_ref, optional }
    }

    /// Return the element type reference.
    pub const fn type_ref(&self) -> &TypeReference {
        &self.type_ref
    }

    /// Report whether this element may be absent.
    pub const fn optional(&self) -> bool {
        self.optional
    }
}

/// Native function return cardinality and ordered shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "elements", rename_all = "snake_case")]
pub enum FunctionReturnMode {
    /// At most one row containing one element.
    Scalar(FunctionReturnElement),
    /// At most one row containing two or more ordered elements.
    Tuple(Vec<FunctionReturnElement>),
    /// Any number of rows containing one or more ordered elements.
    Stream(Vec<FunctionReturnElement>),
}

impl FunctionReturnMode {
    /// Construct a scalar return.
    pub const fn scalar(element: FunctionReturnElement) -> Self {
        Self::Scalar(element)
    }

    /// Construct a non-empty tuple return with at least two elements.
    pub fn tuple(elements: Vec<FunctionReturnElement>) -> Result<Self, Diagnostic> {
        if !(2..=MAX_CANONICAL_COLLECTION_LEN).contains(&elements.len()) {
            return Err(schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_function_tuple_return",
                "tuple function returns require between two and the collection limit elements",
            ));
        }
        Ok(Self::Tuple(elements))
    }

    /// Construct a non-empty stream return.
    pub fn stream(elements: Vec<FunctionReturnElement>) -> Result<Self, Diagnostic> {
        if elements.is_empty() || elements.len() > MAX_CANONICAL_COLLECTION_LEN {
            return Err(schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_function_stream_return",
                "stream function returns require a non-empty bounded element list",
            ));
        }
        Ok(Self::Stream(elements))
    }

    /// Return elements in semantic signature order.
    pub fn elements(&self) -> &[FunctionReturnElement] {
        match self {
            Self::Scalar(element) => std::slice::from_ref(element),
            Self::Tuple(elements) | Self::Stream(elements) => elements,
        }
    }
}

/// A validated ordered function signature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FunctionSignature {
    parameters: Vec<FunctionParameter>,
    returns: FunctionReturnMode,
}

impl FunctionSignature {
    /// Construct a signature with unique, bounded ordered parameters.
    pub fn new(
        parameters: Vec<FunctionParameter>,
        returns: FunctionReturnMode,
    ) -> Result<Self, Diagnostic> {
        if parameters.len() > MAX_CANONICAL_COLLECTION_LEN {
            return Err(schema_diagnostic(
                DiagnosticCategory::ResourceLimit,
                "too_many_function_parameters",
                "function parameter count exceeds the canonical collection limit",
            ));
        }
        let mut names = BTreeSet::new();
        if parameters
            .iter()
            .any(|parameter| !names.insert(parameter.name().clone()))
        {
            return Err(schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "duplicate_function_parameter",
                "function parameter names must be unique",
            ));
        }
        Ok(Self {
            parameters,
            returns,
        })
    }

    /// Return parameters in semantic declaration order.
    pub fn parameters(&self) -> &[FunctionParameter] {
        &self.parameters
    }

    /// Return the native return shape.
    pub const fn returns(&self) -> &FunctionReturnMode {
        &self.returns
    }
}

/// Decoded provider-native function body text retained verbatim.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct FunctionBody(String);

impl FunctionBody {
    /// Construct a non-empty bounded body without trimming or rewriting it.
    pub fn new(text: impl Into<String>) -> Result<Self, Diagnostic> {
        let text = text.into();
        if text.is_empty() || text.len() > MAX_CANONICAL_STRING_BYTES {
            return Err(schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "invalid_function_body",
                "decoded function body must be non-empty and bounded",
            ));
        }
        Ok(Self(text))
    }

    /// Return exact decoded body text, including comments and trailing newline.
    pub fn text(&self) -> &str {
        &self.0
    }
}

/// A function declaration with structured signature and decoded body text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FunctionFact {
    id: FunctionId,
    signature: FunctionSignature,
    body: FunctionBody,
}

impl FunctionFact {
    /// Construct a validated function declaration.
    pub const fn new(id: FunctionId, signature: FunctionSignature, body: FunctionBody) -> Self {
        Self {
            id,
            signature,
            body,
        }
    }

    /// Return the function identity.
    pub const fn id(&self) -> &FunctionId {
        &self.id
    }

    /// Return the structured signature.
    pub const fn signature(&self) -> &FunctionSignature {
        &self.signature
    }

    /// Return exact decoded provider body text.
    pub const fn body(&self) -> &FunctionBody {
        &self.body
    }

    /// Iterate schema labels referenced by the signature.
    pub fn schema_references(&self) -> impl Iterator<Item = &Label> {
        self.signature
            .parameters()
            .iter()
            .filter_map(|parameter| parameter.type_ref().schema_label())
            .chain(
                self.signature
                    .returns()
                    .elements()
                    .iter()
                    .filter_map(|element| element.type_ref().schema_label()),
            )
    }
}

/// One ordered field in a struct declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StructField {
    name: Label,
    value_type: ValueTypeTag,
    optional: bool,
}

impl StructField {
    /// Construct a field from validated contract values.
    pub const fn new(name: Label, value_type: ValueTypeTag, optional: bool) -> Self {
        Self {
            name,
            value_type,
            optional,
        }
    }

    /// Return the field name.
    pub const fn name(&self) -> &Label {
        &self.name
    }

    /// Return the built-in field value type.
    pub const fn value_type(&self) -> ValueTypeTag {
        self.value_type
    }

    /// Report whether the field may be absent.
    pub const fn optional(&self) -> bool {
        self.optional
    }
}

/// A named struct declaration with ordered, non-empty built-in fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StructFact {
    id: StructId,
    fields: Vec<StructField>,
}

impl StructFact {
    /// Construct and validate a field-bearing struct declaration.
    pub fn new(id: StructId, fields: Vec<StructField>) -> Result<Self, Diagnostic> {
        if value_type_tag(id.label().as_str()).is_some() {
            return Err(schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "reserved_schema_type_label",
                "struct labels cannot collide with built-in value-type tokens",
            ));
        }
        if fields.is_empty() {
            return Err(schema_diagnostic(
                DiagnosticCategory::InvalidContract,
                "empty_struct_fields",
                "struct declarations require at least one field",
            ));
        }
        if fields.len() > MAX_CANONICAL_COLLECTION_LEN {
            return Err(schema_diagnostic(
                DiagnosticCategory::ResourceLimit,
                "too_many_struct_fields",
                "struct field count exceeds the canonical collection limit",
            ));
        }

        let mut names = BTreeSet::new();
        for field in &fields {
            if !names.insert(field.name().clone()) {
                return Err(schema_diagnostic(
                    DiagnosticCategory::InvalidContract,
                    "duplicate_struct_field",
                    "struct field names must be unique within the struct",
                ));
            }
        }

        Ok(Self { id, fields })
    }

    /// Return the struct identity.
    pub const fn id(&self) -> &StructId {
        &self.id
    }

    /// Return fields in their declared semantic order.
    pub fn fields(&self) -> &[StructField] {
        &self.fields
    }
}

/// Stable identity of any Phase 2 schema fact.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SchemaFactId {
    /// Type existence.
    Type(TypeId),
    /// Direct subtype edge.
    Sub(SubFactId),
    /// Attribute scalar domain.
    Value(ValueFactId),
    /// Direct ownership.
    Owns(OwnsFactId),
    /// Direct related role.
    Relates(RelatesFactId),
    /// Direct role playing.
    Plays(PlaysFactId),
    /// Independent annotation.
    Annotation(AnnotationFactId),
    /// Function declaration.
    Function(FunctionId),
    /// Struct declaration.
    Struct(StructId),
}

/// A validated, atomic direct schema fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SchemaFact {
    /// Type existence.
    Type(TypeFact),
    /// Direct subtype edge.
    Sub(SubFact),
    /// Attribute scalar domain.
    Value(ValueFact),
    /// Direct ownership.
    Owns(OwnsFact),
    /// Direct related role.
    Relates(RelatesFact),
    /// Direct role playing.
    Plays(PlaysFact),
    /// Independent annotation.
    Annotation(AnnotationFact),
    /// Function declaration.
    Function(FunctionFact),
    /// Struct declaration.
    Struct(StructFact),
}

impl SchemaFact {
    /// Return the stable structural identity.
    pub fn id(&self) -> SchemaFactId {
        match self {
            Self::Type(fact) => SchemaFactId::Type(fact.id().clone()),
            Self::Sub(fact) => SchemaFactId::Sub(fact.id().clone()),
            Self::Value(fact) => SchemaFactId::Value(fact.id().clone()),
            Self::Owns(fact) => SchemaFactId::Owns(fact.id().clone()),
            Self::Relates(fact) => SchemaFactId::Relates(fact.id().clone()),
            Self::Plays(fact) => SchemaFactId::Plays(fact.id().clone()),
            Self::Annotation(fact) => SchemaFactId::Annotation(fact.id().clone()),
            Self::Function(fact) => SchemaFactId::Function(fact.id().clone()),
            Self::Struct(fact) => SchemaFactId::Struct(fact.id().clone()),
        }
    }
}

/// A direct fact paired with the one source span that owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourcedSchemaFact {
    fact: SchemaFact,
    source: SourceSpan,
}

impl SourcedSchemaFact {
    /// Pair a validated fact with its direct source.
    pub const fn new(fact: SchemaFact, source: SourceSpan) -> Self {
        Self { fact, source }
    }

    /// Return the fact.
    pub const fn fact(&self) -> &SchemaFact {
        &self.fact
    }

    /// Return the owning source span.
    pub const fn source(&self) -> &SourceSpan {
        &self.source
    }
}

/// A domain-safe source-document fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct DocumentFingerprint(Fingerprint);

impl DocumentFingerprint {
    /// Fingerprint exact source bytes, including comments and spelling.
    pub fn compute(source: &[u8]) -> Result<Self, Diagnostic> {
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new("typebridge.schema.document")?,
            CanonicalizationVersion::new("typebridge.raw-utf8/v1")?,
            None,
            source,
        )))
    }

    /// Return the generic fingerprint metadata.
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }
}

/// A domain-safe fingerprint of direct fact identity and meaning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct DeclaredIdentityFingerprint(Fingerprint);

impl DeclaredIdentityFingerprint {
    fn compute(canonical_bytes: &[u8]) -> Result<Self, Diagnostic> {
        Ok(Self(Fingerprint::compute(
            FingerprintDomain::new("typebridge.schema.declared-identity")?,
            CanonicalizationVersion::new("typebridge.schema-canonical-json/v1")?,
            None,
            canonical_bytes,
        )))
    }

    /// Return the generic fingerprint metadata.
    pub const fn as_fingerprint(&self) -> &Fingerprint {
        &self.0
    }

    pub(crate) fn from_wire(fingerprint: Fingerprint) -> Result<Self, Diagnostic> {
        if fingerprint.domain().as_str() != "typebridge.schema.declared-identity"
            || fingerprint.canonicalization().as_str() != "typebridge.schema-canonical-json/v1"
            || fingerprint.semantic_profile().is_some()
        {
            return Err(Diagnostic::stable(
                DiagnosticCategory::Integrity,
                "invalid_declared_identity_fingerprint",
                "declared identity fingerprint metadata is invalid",
            ));
        }
        Ok(Self(fingerprint))
    }
}

/// A validated normalized graph of direct schema facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredSchema {
    format: FormatVersion,
    required_capabilities: CapabilitySet,
    facts: BTreeMap<SchemaFactId, SchemaFact>,
    provenance: BTreeMap<SchemaFactId, SourceSpan>,
    fingerprint: DeclaredIdentityFingerprint,
}

impl Serialize for DeclaredSchema {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct TrustedDeclaredSchemaView<'a> {
            declared_identity: &'a DeclaredIdentityFingerprint,
            facts: Vec<&'a SchemaFact>,
            format_version: FormatVersion,
            required_capabilities: &'a CapabilitySet,
        }

        TrustedDeclaredSchemaView {
            declared_identity: &self.fingerprint,
            facts: self.facts.values().collect(),
            format_version: self.format,
            required_capabilities: &self.required_capabilities,
        }
        .serialize(serializer)
    }
}

impl DeclaredSchema {
    /// Validate direct fact ownership, references, and annotation combinations.
    pub fn from_facts(
        format: FormatVersion,
        required_capabilities: CapabilitySet,
        sourced_facts: impl IntoIterator<Item = SourcedSchemaFact>,
    ) -> Result<Self, SchemaDiagnostics> {
        ensure_format_version(format, FormatVersion::V1)
            .map_err(|error| SchemaDiagnostics::one(SchemaDiagnostic::new(error, None)))?;

        let mut facts = BTreeMap::new();
        let mut provenance = BTreeMap::<SchemaFactId, SourceSpan>::new();
        let mut diagnostics = Vec::new();
        for sourced in sourced_facts {
            let id = sourced.fact.id();
            if let Some(previous) = provenance.get(&id) {
                diagnostics.push(
                    SchemaDiagnostic::new(
                        schema_diagnostic(
                            DiagnosticCategory::InvalidContract,
                            "duplicate_schema_fact",
                            "a direct schema fact is declared more than once",
                        ),
                        Some(sourced.source.clone()),
                    )
                    .with_related(DiagnosticLabel::new(
                        previous.clone(),
                        "first declaration is here",
                    )),
                );
                continue;
            }
            provenance.insert(id.clone(), sourced.source);
            facts.insert(id, sourced.fact);
        }

        if diagnostics.is_empty() {
            validate_references(&facts, &provenance, &mut diagnostics);
            validate_annotation_combinations(&facts, &provenance, &mut diagnostics);
            validate_annotation_value_domains(&facts, &provenance, &mut diagnostics);
        }
        if !diagnostics.is_empty() {
            return Err(SchemaDiagnostics::from_vec(diagnostics));
        }

        let canonical =
            canonical_declared_identity_bytes(format, &required_capabilities, &facts)
                .map_err(|error| SchemaDiagnostics::one(SchemaDiagnostic::new(error, None)))?;
        let fingerprint = DeclaredIdentityFingerprint::compute(&canonical)
            .map_err(|error| SchemaDiagnostics::one(SchemaDiagnostic::new(error, None)))?;
        Ok(Self {
            format,
            required_capabilities,
            facts,
            provenance,
            fingerprint,
        })
    }

    /// Return the schema format version.
    pub const fn format(&self) -> FormatVersion {
        self.format
    }

    /// Return the required open capability set.
    pub const fn required_capabilities(&self) -> &CapabilitySet {
        &self.required_capabilities
    }

    /// Return a fact by stable identity.
    pub fn fact(&self, id: &SchemaFactId) -> Option<&SchemaFact> {
        self.facts.get(id)
    }

    /// Iterate facts in stable identity order.
    pub fn facts(&self) -> impl ExactSizeIterator<Item = &SchemaFact> {
        self.facts.values()
    }

    /// Return the direct source owner of a fact.
    pub fn source(&self, id: &SchemaFactId) -> Option<&SourceSpan> {
        self.provenance.get(id)
    }

    /// Return canonical identity bytes with presentation provenance excluded.
    pub fn canonical_identity_bytes(&self) -> Result<Vec<u8>, Diagnostic> {
        canonical_declared_identity_bytes(self.format, &self.required_capabilities, &self.facts)
    }

    /// Return the declared identity fingerprint.
    pub const fn declared_identity_fingerprint(&self) -> &DeclaredIdentityFingerprint {
        &self.fingerprint
    }
}

/// Encode only a constructor-validated declared schema as canonical JSON.
pub fn encode_declared_schema(schema: &DeclaredSchema) -> Result<Vec<u8>, Diagnostic> {
    crate::declared_schema_wire::encode_declared_schema(schema)
}

/// Decode canonical bytes through private wire DTOs and every fact/schema constructor.
pub fn decode_declared_schema(bytes: &[u8]) -> Result<DeclaredSchema, Diagnostic> {
    crate::declared_schema_wire::decode_declared_schema(bytes)
}

#[derive(Serialize)]
struct DeclaredIdentityView<'a> {
    format_version: FormatVersion,
    required_capabilities: &'a CapabilitySet,
    facts: Vec<&'a SchemaFact>,
}

fn canonical_declared_identity_bytes(
    format: FormatVersion,
    required_capabilities: &CapabilitySet,
    facts: &BTreeMap<SchemaFactId, SchemaFact>,
) -> Result<Vec<u8>, Diagnostic> {
    to_canonical_json(&DeclaredIdentityView {
        format_version: format,
        required_capabilities,
        facts: facts.values().collect(),
    })
}

fn validate_annotation(
    subject: &AnnotationSubjectId,
    kind: &AnnotationKindId,
    value: &SchemaAnnotationValue,
) -> Result<(), Diagnostic> {
    let payload_matches = matches!(
        (kind, value),
        (
            AnnotationKindId::Abstract
                | AnnotationKindId::Independent
                | AnnotationKindId::Key
                | AnnotationKindId::Unique,
            SchemaAnnotationValue::Presence
        ) | (
            AnnotationKindId::Card,
            SchemaAnnotationValue::Cardinality(_)
        ) | (AnnotationKindId::Regex, SchemaAnnotationValue::Regex(_))
            | (AnnotationKindId::Range, SchemaAnnotationValue::Range(_))
            | (AnnotationKindId::Values, SchemaAnnotationValue::Values(_))
            | (AnnotationKindId::Doc, SchemaAnnotationValue::Doc(_))
            | (
                AnnotationKindId::Meta(_),
                SchemaAnnotationValue::Meta(CanonicalValue::String(_))
            )
    );
    if !payload_matches {
        return Err(schema_diagnostic(
            DiagnosticCategory::InvalidContract,
            "invalid_annotation_payload",
            "annotation kind and payload do not agree",
        ));
    }
    let subject_matches = match kind {
        AnnotationKindId::Abstract => match subject {
            AnnotationSubjectId::Type(id) => matches!(
                id.kind(),
                TypeKind::Entity | TypeKind::Relation | TypeKind::Attribute
            ),
            AnnotationSubjectId::Relates(_) => true,
            AnnotationSubjectId::Sub(_)
            | AnnotationSubjectId::Value(_)
            | AnnotationSubjectId::Owns(_)
            | AnnotationSubjectId::Plays(_)
            | AnnotationSubjectId::Function(_) => false,
        },
        AnnotationKindId::Independent => matches!(
            subject,
            AnnotationSubjectId::Type(id) if id.kind() == TypeKind::Attribute
        ),
        AnnotationKindId::Key | AnnotationKindId::Unique => {
            matches!(subject, AnnotationSubjectId::Owns(_))
        }
        AnnotationKindId::Card => matches!(
            subject,
            AnnotationSubjectId::Owns(_)
                | AnnotationSubjectId::Relates(_)
                | AnnotationSubjectId::Plays(_)
        ),
        AnnotationKindId::Regex | AnnotationKindId::Range | AnnotationKindId::Values => {
            matches!(
                subject,
                AnnotationSubjectId::Value(_) | AnnotationSubjectId::Owns(_)
            )
        }
        AnnotationKindId::Doc | AnnotationKindId::Meta(_) => matches!(
            subject,
            AnnotationSubjectId::Type(_)
                | AnnotationSubjectId::Sub(_)
                | AnnotationSubjectId::Owns(_)
                | AnnotationSubjectId::Relates(_)
                | AnnotationSubjectId::Plays(_)
                | AnnotationSubjectId::Function(_)
        ),
    };
    if !subject_matches {
        return Err(schema_diagnostic(
            DiagnosticCategory::InvalidContract,
            "invalid_annotation_subject",
            "annotation kind does not apply to this schema subject",
        ));
    }
    Ok(())
}

fn validate_annotation_value_domains(
    facts: &BTreeMap<SchemaFactId, SchemaFact>,
    provenance: &BTreeMap<SchemaFactId, SourceSpan>,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) {
    for (fact_id, fact) in facts {
        let SchemaFact::Annotation(annotation) = fact else {
            continue;
        };

        let kind = annotation.id().kind();
        if !matches!(
            kind,
            AnnotationKindId::Key
                | AnnotationKindId::Unique
                | AnnotationKindId::Regex
                | AnnotationKindId::Range
                | AnnotationKindId::Values
        ) {
            continue;
        }

        let Some((value_type, value_fact_id)) =
            annotation_subject_value_type(annotation.id().subject(), facts)
        else {
            diagnostics.push(SchemaDiagnostic::new(
                schema_diagnostic(
                    DiagnosticCategory::InvalidContract,
                    "unknown_annotation_value_domain",
                    "annotation subject has no resolvable attribute value domain",
                ),
                provenance.get(fact_id).cloned(),
            ));
            continue;
        };

        let valid = match (kind, annotation.value()) {
            (AnnotationKindId::Key | AnnotationKindId::Unique, _) => {
                value_type != ValueTypeTag::Double
            }
            (AnnotationKindId::Regex, SchemaAnnotationValue::Regex(_)) => {
                value_type == ValueTypeTag::String
            }
            (AnnotationKindId::Range, SchemaAnnotationValue::Range(range)) => {
                value_type != ValueTypeTag::Duration
                    && range
                        .lower()
                        .into_iter()
                        .chain(range.upper())
                        .all(|bound| bound.value_type() == value_type)
            }
            (AnnotationKindId::Values, SchemaAnnotationValue::Values(values)) => {
                values.iter().all(|value| value.value_type() == value_type)
            }
            _ => false,
        };

        if !valid {
            let mut diagnostic = SchemaDiagnostic::new(
                schema_diagnostic(
                    DiagnosticCategory::InvalidContract,
                    "invalid_annotation_value_domain",
                    "annotation payload is incompatible with the attribute value domain",
                ),
                provenance.get(fact_id).cloned(),
            );
            if let Some(value_source) = provenance.get(&value_fact_id) {
                diagnostic = diagnostic.with_related(DiagnosticLabel::new(
                    value_source.clone(),
                    "attribute value domain is declared here",
                ));
            }
            diagnostics.push(diagnostic);
        }
    }
}

fn annotation_subject_value_type(
    subject: &AnnotationSubjectId,
    facts: &BTreeMap<SchemaFactId, SchemaFact>,
) -> Option<(ValueTypeTag, SchemaFactId)> {
    let mut attribute = match subject {
        AnnotationSubjectId::Value(id) => id.attribute().clone(),
        AnnotationSubjectId::Owns(id) => id.attribute().clone(),
        AnnotationSubjectId::Type(_)
        | AnnotationSubjectId::Sub(_)
        | AnnotationSubjectId::Relates(_)
        | AnnotationSubjectId::Plays(_)
        | AnnotationSubjectId::Function(_) => return None,
    };
    let mut visited = BTreeSet::new();

    loop {
        let attribute_type = TypeId::new(TypeKind::Attribute, attribute.label().as_str()).ok()?;
        if !visited.insert(attribute_type.clone()) {
            return None;
        }

        let value_fact_id = ValueFactId::new(attribute.clone());
        let schema_fact_id = SchemaFactId::Value(value_fact_id);
        if let Some(SchemaFact::Value(value)) = facts.get(&schema_fact_id) {
            return Some((value.value_type(), schema_fact_id));
        }

        let supertype = facts.values().find_map(|fact| {
            let SchemaFact::Sub(sub) = fact else {
                return None;
            };
            (sub.id().subtype() == &attribute_type
                && sub.id().supertype().kind() == TypeKind::Attribute)
                .then(|| sub.id().supertype().clone())
        })?;
        attribute = AttributeId::new(supertype.label().as_str()).ok()?;
    }
}

fn validate_references(
    facts: &BTreeMap<SchemaFactId, SchemaFact>,
    provenance: &BTreeMap<SchemaFactId, SourceSpan>,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) {
    let type_ids = facts
        .keys()
        .filter_map(|id| match id {
            SchemaFactId::Type(id) => Some(id.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let role_ids = facts
        .keys()
        .filter_map(|id| match id {
            SchemaFactId::Relates(id) => Some(id.role().clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let struct_labels = facts
        .keys()
        .filter_map(|id| match id {
            SchemaFactId::Struct(id) => Some(id.label().clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();

    for (id, fact) in facts {
        let valid = match fact {
            SchemaFact::Type(_) | SchemaFact::Struct(_) => true,
            SchemaFact::Function(fact) => fact.schema_references().all(|label| {
                type_ids.iter().any(|id| id.label() == label) || struct_labels.contains(label)
            }),
            SchemaFact::Sub(fact) => {
                type_ids.contains(fact.id().subtype()) && type_ids.contains(fact.id().supertype())
            }
            SchemaFact::Value(fact) => type_ids.contains(&attribute_type_id(fact.id().attribute())),
            SchemaFact::Owns(fact) => {
                type_ids.contains(fact.id().owner())
                    && type_ids.contains(&attribute_type_id(fact.id().attribute()))
            }
            SchemaFact::Relates(fact) => {
                type_ids.contains(fact.id().relation())
                    && fact
                        .specializes()
                        .is_none_or(|role| role_ids.contains(role))
            }
            SchemaFact::Plays(fact) => {
                type_ids.contains(fact.id().player()) && role_ids.contains(fact.id().role())
            }
            SchemaFact::Annotation(fact) => {
                facts.contains_key(&subject_fact_id(fact.id().subject()))
            }
        };
        if !valid {
            diagnostics.push(SchemaDiagnostic::new(
                schema_diagnostic(
                    DiagnosticCategory::InvalidContract,
                    "unknown_schema_fact_reference",
                    "schema fact references a declaration that does not exist",
                ),
                provenance.get(id).cloned(),
            ));
        }
    }
}

fn validate_annotation_combinations(
    facts: &BTreeMap<SchemaFactId, SchemaFact>,
    provenance: &BTreeMap<SchemaFactId, SourceSpan>,
    diagnostics: &mut Vec<SchemaDiagnostic>,
) {
    let mut by_subject = BTreeMap::<AnnotationSubjectId, BTreeSet<AnnotationKindId>>::new();
    for fact in facts.values() {
        if let SchemaFact::Annotation(annotation) = fact {
            by_subject
                .entry(annotation.id().subject().clone())
                .or_default()
                .insert(annotation.id().kind().clone());
        }
    }
    for (subject, kinds) in by_subject {
        if kinds.contains(&AnnotationKindId::Key)
            && (kinds.contains(&AnnotationKindId::Unique)
                || kinds.contains(&AnnotationKindId::Card))
        {
            let key_id =
                SchemaFactId::Annotation(AnnotationFactId::new(subject, AnnotationKindId::Key));
            diagnostics.push(SchemaDiagnostic::new(
                schema_diagnostic(
                    DiagnosticCategory::InvalidContract,
                    "key_annotation_conflict",
                    "key cannot be combined with unique or cardinality",
                ),
                provenance.get(&key_id).cloned(),
            ));
        }
    }
}

fn attribute_type_id(attribute: &AttributeId) -> TypeId {
    TypeId::new(TypeKind::Attribute, attribute.label().as_str())
        .expect("validated attribute labels always form attribute type identities")
}

fn subject_fact_id(subject: &AnnotationSubjectId) -> SchemaFactId {
    match subject {
        AnnotationSubjectId::Type(id) => SchemaFactId::Type(id.clone()),
        AnnotationSubjectId::Sub(id) => SchemaFactId::Sub(id.clone()),
        AnnotationSubjectId::Value(id) => SchemaFactId::Value(id.clone()),
        AnnotationSubjectId::Owns(id) => SchemaFactId::Owns(id.clone()),
        AnnotationSubjectId::Relates(id) => SchemaFactId::Relates(id.clone()),
        AnnotationSubjectId::Plays(id) => SchemaFactId::Plays(id.clone()),
        AnnotationSubjectId::Function(id) => SchemaFactId::Function(id.clone()),
    }
}

fn value_type_tag(value: &str) -> Option<ValueTypeTag> {
    match value {
        "string" => Some(ValueTypeTag::String),
        "integer" => Some(ValueTypeTag::Long),
        "double" => Some(ValueTypeTag::Double),
        "boolean" => Some(ValueTypeTag::Boolean),
        "date" => Some(ValueTypeTag::Date),
        "datetime" => Some(ValueTypeTag::DateTime),
        "datetime-tz" => Some(ValueTypeTag::DateTimeTz),
        "decimal" => Some(ValueTypeTag::Decimal),
        "duration" => Some(ValueTypeTag::Duration),
        _ => None,
    }
}

fn schema_diagnostic(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
) -> Diagnostic {
    Diagnostic::stable(category, code, message)
}
