//! Closed generated-descriptor compatibility input for direct schema facts.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use type_bridge_contract::codec::{FormatVersion, from_canonical_json, to_canonical_json};
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::id::{AttributeId, Label, RoleId, TypeId, TypeKind};
use type_bridge_contract::schema::{
    AnnotationFact, AnnotationFactId, AnnotationKindId, AnnotationSubjectId, CanonicalValueRange,
    CanonicalValueSet, DeclaredSchema, DocText, DocumentId, OwnsFact, OwnsFactId, RegexPattern,
    RelatesFactId, SchemaAnnotationValue, SchemaDiagnostic, SchemaDiagnostics, SchemaFact,
    SourceSpan, SubFact, SubFactId, TypeFact, ValueFact, ValueFactId,
};
use type_bridge_contract::value::{CanonicalString, CanonicalValue, Cardinality, ValueTypeTag};
use type_bridge_core_lib::_schema::TypeSchema;
use type_bridge_schema::FactAssembler;

use crate::released_syntax::ReleasedSyntax;

/// Exact discriminator for the first generated direct-descriptor format.
pub const GENERATED_DECLARED_DESCRIPTOR_V1: &str = "typebridge.generated-descriptors/v1";

/// Generated package path containing the canonical direct declaration snapshot.
pub const GENERATED_DECLARED_DESCRIPTOR_PATH: &str = "declared-schema.json";

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum SnapshotKind {
    Declared,
    Effective,
    Partial,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProvenanceKind {
    Direct,
    Effective,
    Synthesized,
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DescriptorSource {
    provenance: ProvenanceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    document: Option<String>,
    byte_start: u64,
    byte_end: u64,
    line: u32,
    column: u32,
    end_line: u32,
    end_column: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AttributeDescriptor {
    label: String,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    value_type: Option<ValueTypeTag>,
    #[serde(default)]
    is_abstract: bool,
    #[serde(default)]
    is_independent: bool,
    #[serde(default)]
    regex: Option<String>,
    #[serde(default)]
    values: Option<Vec<CanonicalValue>>,
    #[serde(default)]
    range: Option<ValueRangeDescriptor>,
    #[serde(default)]
    doc: Option<String>,
    #[serde(default)]
    meta: BTreeMap<String, String>,
    source: DescriptorSource,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ValueRangeDescriptor {
    #[serde(default)]
    min: Option<CanonicalValue>,
    #[serde(default)]
    max: Option<CanonicalValue>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ObjectDescriptor {
    label: String,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    is_abstract: bool,
    #[serde(default)]
    doc: Option<String>,
    #[serde(default)]
    meta: BTreeMap<String, String>,
    #[serde(default)]
    owns: Vec<OwnsDescriptor>,
    source: DescriptorSource,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RelationDescriptor {
    #[serde(flatten)]
    object: ObjectDescriptor,
    #[serde(default)]
    relates: Vec<RelatesDescriptor>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OwnsDescriptor {
    attribute: String,
    #[serde(default)]
    key: bool,
    #[serde(default)]
    unique: bool,
    #[serde(default)]
    card: Option<Cardinality>,
    #[serde(default)]
    doc: Option<String>,
    #[serde(default)]
    meta: BTreeMap<String, String>,
    source: DescriptorSource,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RelatesDescriptor {
    role: String,
    #[serde(default)]
    specializes: Option<String>,
    #[serde(default)]
    is_abstract: bool,
    #[serde(default)]
    card: Option<Cardinality>,
    #[serde(default)]
    doc: Option<String>,
    #[serde(default)]
    meta: BTreeMap<String, String>,
    source: DescriptorSource,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PlaysDescriptor {
    player: String,
    relation: String,
    role: String,
    #[serde(default)]
    card: Option<Cardinality>,
    #[serde(default)]
    doc: Option<String>,
    #[serde(default)]
    meta: BTreeMap<String, String>,
    source: DescriptorSource,
}

/// Closed, direct-only descriptor snapshot emitted by generated bindings.
///
/// Existing CRUD descriptors are not this type: they flatten inherited owns
/// and effective roles. Producers must emit this snapshot at generation time,
/// before binding-local projection or inheritance merging.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedDeclaredDescriptorSetV1 {
    format: String,
    snapshot_kind: SnapshotKind,
    closed_world: bool,
    #[serde(default)]
    unsupported_constructs: Vec<String>,
    #[serde(default)]
    attributes: Vec<AttributeDescriptor>,
    #[serde(default)]
    entities: Vec<ObjectDescriptor>,
    #[serde(default)]
    relations: Vec<RelationDescriptor>,
    #[serde(default)]
    plays: Vec<PlaysDescriptor>,
}

/// Decode canonical generated-descriptor JSON and adapt direct declarations
/// through the source-language-neutral [`FactAssembler`].
pub fn generated_descriptors_to_declared(
    document: DocumentId,
    canonical_json: &[u8],
) -> Result<DeclaredSchema, SchemaDiagnostics> {
    let descriptors = from_canonical_json::<GeneratedDeclaredDescriptorSetV1>(canonical_json)
        .map_err(|diagnostic| one(diagnostic, None))?;
    descriptors.into_declared(document, canonical_json.len())
}

/// Project TypeQL direct facts into the closed generated-descriptor format.
pub fn typeql_to_generated_descriptors(
    document: DocumentId,
    source: &str,
) -> Result<String, SchemaDiagnostics> {
    if type_bridge_core_lib::_parser::parse_typeql(source).is_ok_and(|schema| {
        schema.attributes.is_empty()
            && schema.entities.is_empty()
            && schema.relations.is_empty()
            && schema.functions.is_empty()
            && schema.structs.is_empty()
    }) {
        return empty_generated_declared_descriptors_json().map_err(|_| {
            error(
                "generated_descriptor_encoding_failed",
                "canonical empty generated-descriptor encoding failed",
                None,
            )
        });
    }

    let evidence_source = source;
    // The descriptor snapshot deliberately excludes functions, and released
    // generator input carries opaque dummy function bodies the strict
    // grammar rejects; strip them with the released parser's own extents.
    let source = type_bridge_core_lib::_parser::strip_function_definitions(source);
    // List capabilities and released-only annotations sit outside the
    // overlap grammar: pin the plain capability, record each construct,
    // and mark the snapshot open-world instead of failing the whole
    // generation.
    let (source, mut unsupported) = strip_unportable_constructs(&source);
    let declared =
        released_descriptors_to_declared(document, &source, evidence_source, &mut unsupported)?;
    let mut descriptors = GeneratedDeclaredDescriptorSetV1::from_declared(&declared)?;
    if !unsupported.is_empty() {
        unsupported.sort_by_key(|(start, _)| *start);
        descriptors.closed_world = false;
        descriptors.unsupported_constructs = unsupported
            .into_iter()
            .map(|(_, spelling)| spelling)
            .collect();
    }
    let bytes = to_canonical_json(&descriptors).map_err(|diagnostic| one(diagnostic, None))?;
    Ok(String::from_utf8(bytes).expect("canonical JSON is valid UTF-8"))
}

/// Adapt released descriptors without pretending that an open-world V1
/// reference or domain-incoherent annotation is a portable V2 fact.
///
/// The released parser stores value annotations independently from its final
/// `value` clause, so inputs such as `value string @regex(...), value integer`
/// were valid generator inputs. The canonical assembler correctly rejects
/// that regex for an integer value domain. Keep generation operational by
/// redacting only the diagnosed annotation from the portable projection and
/// recording its exact source spelling in the descriptor's open-world list.
/// The same generator-only lane omits a capability whose parent, owned
/// attribute, relation, or role is absent from a partial released export.
/// Strict and lossless V2 importers never call this adapter.
fn released_descriptors_to_declared(
    document: DocumentId,
    source: &str,
    evidence_source: &str,
    unsupported: &mut Vec<(usize, String)>,
) -> Result<DeclaredSchema, SchemaDiagnostics> {
    let mut portable = source.to_owned();
    let reference_projection = released_unresolved_ranges(&document, source)?;
    let omitted_declarations = reference_projection
        .omitted_declarations
        .keys()
        .copied()
        .collect::<BTreeSet<_>>();
    let omitted_capabilities = reference_projection
        .omitted
        .keys()
        .copied()
        .collect::<BTreeSet<_>>();
    let mut redacted = BTreeMap::<usize, String>::new();
    for (start, end) in &reference_projection.omitted_declarations {
        let Some(spelling) = evidence_source.get(*start..*end) else {
            return released_descriptor_projection(
                document,
                &portable,
                &omitted_declarations,
                &omitted_capabilities,
                &reference_projection.played_role_declarations,
            );
        };
        redacted.insert(*start, spelling.to_owned());
    }
    for (start, end) in reference_projection.omitted {
        let Some(spelling) = evidence_source.get(start..end) else {
            return released_descriptor_projection(
                document,
                &portable,
                &omitted_declarations,
                &omitted_capabilities,
                &reference_projection.played_role_declarations,
            );
        };
        redacted.insert(start, spelling.to_owned());
    }

    // Every retry must blank at least one previously visible annotation.  The
    // source contains a finite number of `@` bytes, so this budget makes the
    // compatibility repair explicitly bounded even when independent
    // annotation failures are discovered in different assembler phases.
    let mut redaction_budget = source.bytes().filter(|byte| *byte == b'@').count();
    loop {
        match released_descriptor_projection(
            document.clone(),
            &portable,
            &omitted_declarations,
            &omitted_capabilities,
            &reference_projection.played_role_declarations,
        ) {
            Ok(declared) => {
                unsupported.extend(redacted);
                return Ok(declared);
            }
            Err(diagnostics) => {
                let Some(ranges) =
                    incompatible_released_annotations(&diagnostics, evidence_source, &redacted)
                else {
                    return Err(diagnostics);
                };
                if ranges.is_empty() || ranges.len() > redaction_budget {
                    return Err(diagnostics);
                }
                redaction_budget -= ranges.len();
                for range in &ranges {
                    redacted.insert(range.start, evidence_source[range.clone()].to_owned());
                }
                let extents = released_annotation_redaction_extents(&portable, &ranges);
                portable = type_bridge_core_lib::_parser::blank_source_extents(&portable, &extents);
            }
        }
    }
}

/// Include a released annotation's immediately preceding comma in its
/// length-preserving retry redaction.
///
/// The frozen parser accepts both `value string @regex(...)` and
/// `value string, @regex(...)`. [`ReleasedSyntax`] normally removes that
/// optional comma while the annotation is visible. A descriptor retry blanks
/// a domain-incoherent annotation before normalizing again, so retaining the
/// comma would leave `value string, , value integer`. Comments are released
/// trivia and may appear between the separator and annotation; string
/// literals remain hard token boundaries.
fn released_annotation_redaction_extents(
    source: &str,
    annotations: &[core::ops::Range<usize>],
) -> Vec<core::ops::Range<usize>> {
    let mut extents = Vec::with_capacity(annotations.len().saturating_mul(2));
    for annotation in annotations {
        if let Some(separator) = preceding_released_annotation_comma(source, annotation.start) {
            extents.push(separator);
        }
        extents.push(annotation.clone());
    }
    extents.sort_by_key(|extent| extent.start);
    extents.dedup();
    extents
}

fn preceding_released_annotation_comma(
    source: &str,
    annotation_start: usize,
) -> Option<core::ops::Range<usize>> {
    use type_bridge_core_lib::_parser::{SourceRegionKind, scan_source_regions};

    for (range, kind) in scan_source_regions(source).into_iter().rev() {
        if range.start >= annotation_start {
            continue;
        }
        let end = range.end.min(annotation_start);
        if range.start >= end {
            continue;
        }
        match kind {
            SourceRegionKind::LineComment => continue,
            SourceRegionKind::StringLiteral => return None,
            SourceRegionKind::Code => {
                for (offset, character) in source[range.start..end].char_indices().rev() {
                    if character.is_whitespace() {
                        continue;
                    }
                    let position = range.start + offset;
                    return (character == ',').then_some(position..position + 1);
                }
            }
        }
    }
    None
}

/// Build the one-pass unresolved-reference index from a byte-aligned released
/// projection. Function bodies and generator-only constructs are irrelevant
/// to reference closure; blanking them lets the strict AST index consume the
/// same accepted V1 source without changing any capability offset.
fn released_unresolved_ranges(
    document: &DocumentId,
    source: &str,
) -> Result<crate::ReleasedReferenceProjection, SchemaDiagnostics> {
    let source = type_bridge_core_lib::_parser::strip_function_definitions(source);
    let (source, _) = strip_unportable_constructs(&source);
    // The strict TypeQL AST used by the index accepts a narrower comment
    // vocabulary than the frozen generator. Comment blanking is byte-length
    // preserving, so indexed capability offsets still address the original
    // released source exactly.
    let source = blank_released_comments(&source);
    ReleasedSyntax::accepted_with_size_policy(
        &source,
        crate::TypeqlSourceSizePolicy::TrustedGenerator,
    )
    .map(|released| crate::released_unresolved_capability_ranges(document, &released))
    .transpose()
    .map(Option::unwrap_or_default)
}

fn released_descriptor_projection(
    document: DocumentId,
    source: &str,
    omitted_declarations: &BTreeSet<usize>,
    omitted_capabilities: &BTreeSet<usize>,
    played_role_declarations: &BTreeMap<usize, String>,
) -> Result<DeclaredSchema, SchemaDiagnostics> {
    let size_policy = crate::TypeqlSourceSizePolicy::TrustedGenerator;
    let Some(released) = ReleasedSyntax::accepted_with_size_policy(source, size_policy) else {
        return released_typeql_to_declared_stripped_projection_with_references(
            document,
            source,
            size_policy,
        )
        .map(crate::function_references::TypeqlDeclaredSchema::into_declared);
    };
    let mut released_error =
        match crate::released_typeql_to_declared_with_references_omitting_capabilities(
            document.clone(),
            &released,
            omitted_declarations,
            omitted_capabilities,
            played_role_declarations,
            size_policy,
        ) {
            Ok(declared) => return Ok(declared.into_declared()),
            Err(error) => error,
        };

    let without_comments = blank_released_comments(source);
    if let Some(released) =
        ReleasedSyntax::accepted_with_size_policy(&without_comments, size_policy)
    {
        match crate::released_typeql_to_declared_with_references_omitting_capabilities(
            document,
            &released,
            omitted_declarations,
            omitted_capabilities,
            played_role_declarations,
            size_policy,
        ) {
            Ok(declared) => return Ok(declared.into_declared()),
            Err(error) => released_error = error,
        }
    }
    Err(released_error)
}

fn incompatible_released_annotations(
    diagnostics: &SchemaDiagnostics,
    source: &str,
    redacted: &BTreeMap<usize, String>,
) -> Option<Vec<core::ops::Range<usize>>> {
    let mut ranges = BTreeMap::new();
    for diagnostic in diagnostics.iter() {
        let span = diagnostic.primary()?;
        let start = usize::try_from(span.byte_start()).ok()?;
        let end = usize::try_from(span.byte_end()).ok()?;
        if start >= end || redacted.contains_key(&start) {
            return None;
        }
        let spelling = source.get(start..end)?;
        let annotation = released_annotation_name(spelling)?;
        if !is_released_annotation_projection_failure(
            annotation,
            diagnostic.diagnostic().code().as_str(),
        ) {
            return None;
        }
        ranges.insert(start, start..end);
    }
    (!ranges.is_empty()).then(|| ranges.into_values().collect())
}

/// Return the exact annotation keyword only when the diagnostic span starts
/// at a real annotation.  Spans come from the parsed AST; requiring byte zero
/// to be `@` prevents compatibility recovery from ever scanning string or
/// comment contents for marker-like text.
fn released_annotation_name(spelling: &str) -> Option<&str> {
    let rest = spelling.strip_prefix('@')?;
    let end = rest
        .bytes()
        .position(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// Closed list of canonical-contract failures that can be introduced only by
/// projecting an annotation accepted by the frozen V1 parser.  Each arm is
/// additionally pinned to annotation kinds capable of producing that code;
/// strict TypeQL/V2 import never calls this generator-only recovery lane.
fn is_released_annotation_projection_failure(annotation: &str, code: &str) -> bool {
    let is_value = matches!(annotation, "regex" | "range" | "values" | "key" | "unique");
    let is_literal_collection = matches!(annotation, "range" | "values");
    match code {
        "invalid_cardinality" | "invalid_typeql_cardinality" => annotation == "card",
        "invalid_regex_annotation" | "invalid_typeql_regex" => annotation == "regex",
        "invalid_doc_annotation" | "invalid_typeql_doc" => annotation == "doc",
        "invalid_typeql_meta_key" | "invalid_typeql_meta_value" | "malformed_id" => {
            annotation == "meta"
        }
        "empty_values_annotation"
        | "values_annotation_member_limit_exceeded"
        | "mixed_values_annotation_domain"
        | "duplicate_values_annotation_value" => annotation == "values",
        "empty_range_annotation"
        | "mixed_range_annotation_domain"
        | "unsupported_range_annotation_domain"
        | "invalid_range_annotation_bounds" => annotation == "range",
        "invalid_annotation_value_domain" | "unknown_annotation_value_domain" => is_value,
        // A value annotation on a released attribute with no `value` clause
        // has both an unknown subject fact and an unknown value domain.
        "unknown_schema_fact_reference" => {
            matches!(annotation, "regex" | "range" | "values")
        }
        "canonical_string_limit_exceeded" => {
            matches!(annotation, "range" | "values" | "meta")
        }
        "invalid_canonical_scalar"
        | "invalid_typeql_boolean"
        | "invalid_typeql_integer"
        | "invalid_typeql_double"
        | "invalid_typeql_decimal"
        | "invalid_typeql_string"
        | "invalid_typeql_date"
        | "invalid_typeql_datetime"
        | "invalid_typeql_datetime_tz"
        | "invalid_typeql_duration"
        | "invalid_typeql_literal" => is_literal_collection,
        "key_annotation_conflict" => annotation == "key",
        _ => false,
    }
}

/// Project released generator TypeQL through the same compatibility
/// normalization used by descriptor generation.
///
/// The caller remains responsible for retaining the raw source as authority:
/// list markers and released-only annotations are deliberately absent from
/// this portable declared-fact projection.
pub fn released_typeql_to_declared_projection(
    document: DocumentId,
    source: &str,
) -> Result<DeclaredSchema, SchemaDiagnostics> {
    released_typeql_to_declared_projection_with_references(document, source)
        .map(crate::function_references::TypeqlDeclaredSchema::into_declared)
}

/// Project released TypeQL without discarding any schema-semantic construct.
///
/// This preserves the released parser's multi-`define`, alias, comment, and
/// re-opened-label compatibility, but deliberately refuses list capabilities,
/// released-only annotations, and opaque function/struct definitions that the
/// portable fact graph cannot represent. Authority boundaries that require an
/// exact schema contract must use this projection instead of
/// [`released_typeql_to_declared_projection`].
pub fn released_typeql_to_declared_lossless_projection(
    document: DocumentId,
    source: &str,
) -> Result<DeclaredSchema, SchemaDiagnostics> {
    released_typeql_to_declared_lossless_projection_with_references(document, source)
        .map(crate::function_references::TypeqlDeclaredSchema::into_declared)
}

/// Losslessly project released TypeQL while retaining static function-body references.
///
/// This is the reference-preserving form of
/// [`released_typeql_to_declared_lossless_projection`]. It rejects constructs
/// outside the portable fact graph instead of silently omitting them.
pub fn released_typeql_to_declared_lossless_projection_with_references(
    document: DocumentId,
    source: &str,
) -> Result<crate::function_references::TypeqlDeclaredSchema, SchemaDiagnostics> {
    if let Some(released) = ReleasedSyntax::accepted(source) {
        let mut released_error = match crate::released_typeql_to_declared_with_references(
            document.clone(),
            &released,
            crate::TypeqlSourceSizePolicy::Defensive,
        ) {
            Ok(declared) => return Ok(declared),
            Err(error) => error,
        };

        let without_comments = blank_released_comments(source);
        if let Some(released) = ReleasedSyntax::accepted(&without_comments) {
            match crate::released_typeql_to_declared_with_references(
                document,
                &released,
                crate::TypeqlSourceSizePolicy::Defensive,
            ) {
                Ok(declared) => return Ok(declared),
                Err(error) => released_error = error,
            }
        }
        return Err(released_error);
    }

    match crate::typeql_to_declared_with_references(document.clone(), source) {
        Ok(declared) => Ok(declared),
        Err(_) => {
            let without_comments = blank_released_comments(source);
            crate::typeql_to_declared_with_references(document, &without_comments)
        }
    }
}

/// Project released generator TypeQL while retaining references from every
/// function body accepted by the strict importer.
///
/// If the source requires the frozen opaque-function fallback, those stripped
/// bodies necessarily contribute no function-reference entries; the raw
/// source remains the caller's authority for compatibility preservation.
pub fn released_typeql_to_declared_projection_with_references(
    document: DocumentId,
    source: &str,
) -> Result<crate::function_references::TypeqlDeclaredSchema, SchemaDiagnostics> {
    released_typeql_to_declared_presence_projection_with_references(document, source)
        .map(|(declared, _)| declared)
}

/// Project released TypeQL while retaining the byte offsets of constructs
/// absent from the portable V2 fact vocabulary.
///
/// Writer-fence presence uses the offsets only to determine whether a frozen
/// authority fact itself carries released-only semantics. They are not a
/// substitute for the lossless projection used by query and adoption
/// authority.
pub(crate) fn released_typeql_to_declared_presence_projection_with_references(
    document: DocumentId,
    source: &str,
) -> Result<(crate::function_references::TypeqlDeclaredSchema, Vec<usize>), SchemaDiagnostics> {
    let (source, stripped) = strip_unportable_constructs(source);
    released_typeql_to_declared_stripped_projection_with_references(
        document,
        &source,
        crate::TypeqlSourceSizePolicy::Defensive,
    )
    .map(|declared| {
        (
            declared,
            stripped.into_iter().map(|(offset, _)| offset).collect(),
        )
    })
}

fn released_typeql_to_declared_stripped_projection_with_references(
    document: DocumentId,
    source: &str,
    size_policy: crate::TypeqlSourceSizePolicy,
) -> Result<crate::function_references::TypeqlDeclaredSchema, SchemaDiagnostics> {
    if let Some(released) = ReleasedSyntax::accepted_with_size_policy(source, size_policy) {
        let mut released_error = match crate::released_typeql_to_declared_with_references(
            document.clone(),
            &released,
            size_policy,
        ) {
            Ok(declared) => return Ok(declared),
            Err(error) => error,
        };

        let without_comments = blank_released_comments(source);
        if let Some(released) =
            ReleasedSyntax::accepted_with_size_policy(&without_comments, size_policy)
        {
            match crate::released_typeql_to_declared_with_references(
                document.clone(),
                &released,
                size_policy,
            ) {
                Ok(declared) => return Ok(declared),
                Err(error) => released_error = error,
            }
        }

        // Valid live TypeDB functions remain first-class facts. The frozen
        // opaque-body fallback is used only when the strict importer cannot
        // parse the released function spelling.
        let without_definitions =
            type_bridge_core_lib::_parser::strip_function_definitions(&without_comments);
        if let Some(released) =
            ReleasedSyntax::accepted_with_size_policy(&without_definitions, size_policy)
        {
            return crate::released_typeql_to_declared_with_references(
                document,
                &released,
                size_policy,
            );
        }
        return Err(released_error);
    }

    match crate::typeql_to_declared_with_references_with_size_policy(
        document.clone(),
        source,
        size_policy,
    ) {
        Ok(declared) => Ok(declared),
        Err(_) => {
            let without_comments = blank_released_comments(source);
            match crate::typeql_to_declared_with_references_with_size_policy(
                document.clone(),
                &without_comments,
                size_policy,
            ) {
                Ok(declared) => Ok(declared),
                Err(_) => {
                    let without_definitions =
                        type_bridge_core_lib::_parser::strip_function_definitions(
                            &without_comments,
                        );
                    crate::typeql_to_declared_with_references_with_size_policy(
                        document,
                        &without_definitions,
                        size_policy,
                    )
                }
            }
        }
    }
}

/// Blank released comment spellings before entering parsers that accept a
/// narrower comment vocabulary. This preserves every byte offset and line
/// break and never touches markers inside string literals.
fn blank_released_comments(source: &str) -> String {
    use type_bridge_core_lib::_parser::{
        SourceRegionKind, blank_source_extents, scan_source_regions,
    };
    let comments = scan_source_regions(source)
        .into_iter()
        .filter_map(|(range, kind)| (kind == SourceRegionKind::LineComment).then_some(range))
        .collect::<Vec<_>>();
    blank_source_extents(source, &comments)
}

/// Blank `ident[]` list markers, the released-only `@distinct`, `@cascade`,
/// and `@subkey(...)` annotations, and the frozen parser's boundless
/// `@range(..)` spelling outside comments and string literals, recording
/// every construct.
///
/// `@distinct` is only legal on list capabilities, all of which strip
/// here, so any occurrence belongs to a stripped list. `@cascade` and
/// `@subkey` are released ownership annotations with no portable V2
/// identity; stripping them here keeps the legacy generator working
/// while the open-world marker records the incompleteness. Redaction is
/// length-preserving (spans become spaces) so descriptor offsets keep
/// indexing the original document.
fn strip_unportable_constructs(source: &str) -> (String, Vec<(usize, String)>) {
    use type_bridge_core_lib::_parser::{
        SourceRegionKind, blank_source_extents, scan_source_regions,
    };
    let ident_byte = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-';
    let bytes = source.as_bytes();
    let mut extents: Vec<core::ops::Range<usize>> = Vec::new();
    let mut stripped = Vec::new();
    for (range, kind) in scan_source_regions(source) {
        if kind != SourceRegionKind::Code {
            continue;
        }
        let mut index = range.start;
        while index < range.end {
            let byte = bytes[index];
            if byte == b'['
                && index + 1 < range.end
                && bytes[index + 1] == b']'
                && index > range.start
                && ident_byte(bytes[index - 1])
            {
                let mut start = index;
                while start > range.start && ident_byte(bytes[start - 1]) {
                    start -= 1;
                }
                stripped.push((start, format!("{}[]", &source[start..index])));
                extents.push(index..index + 2);
                index += 2;
                continue;
            }
            if byte == b'@'
                && let Some((construct, length)) =
                    match_unportable_annotation(source, index, &ident_byte)
            {
                stripped.push((index, construct));
                extents.push(index..index + length);
                index += length;
                continue;
            }
            // Advance one full character so multi-byte text never lands
            // this scanner inside a codepoint.
            index += source[index..].chars().next().map_or(1, char::len_utf8);
        }
    }
    (blank_source_extents(source, &extents), stripped)
}

/// Match a released-only annotation at `start` (which points at `@`),
/// returning its recorded spelling and byte length.
fn match_unportable_annotation(
    source: &str,
    start: usize,
    ident_byte: &dyn Fn(u8) -> bool,
) -> Option<(String, usize)> {
    let rest = &source[start..];
    let boundary = |after: usize| !ident_byte(*rest.as_bytes().get(after).unwrap_or(&b' '));
    for bare in ["@distinct", "@cascade"] {
        if rest.starts_with(bare) && boundary(bare.len()) {
            return Some((bare.to_owned(), bare.len()));
        }
    }
    let range = "@range";
    if rest.starts_with(range) && boundary(range.len()) {
        let mut cursor = skip_released_trivia(source, start + range.len())?;
        if source.as_bytes().get(cursor) == Some(&b'(') {
            cursor += 1;
            cursor = skip_released_trivia(source, cursor)?;
            if source[cursor..].starts_with("..") {
                cursor += 2;
                cursor = skip_released_trivia(source, cursor)?;
                if source.as_bytes().get(cursor) == Some(&b')') {
                    cursor += 1;
                    return Some((source[start..cursor].to_owned(), cursor - start));
                }
            }
        }
    }
    let subkey = "@subkey";
    if !rest.starts_with(subkey) || !boundary(subkey.len()) {
        return None;
    }

    // Mirror the frozen parser's `ws_comments` +
    // `delimited("(", padded(identifier), ")")` grammar. In particular,
    // comments between the keyword and argument may contain parentheses;
    // searching for the first `)` would redact only part of the annotation.
    let mut cursor = skip_released_trivia(source, start + subkey.len())?;
    if source.as_bytes().get(cursor) != Some(&b'(') {
        return None;
    }
    cursor += 1;
    cursor = skip_released_trivia(source, cursor)?;
    let first = *source.as_bytes().get(cursor)?;
    if !first.is_ascii_alphabetic() && first != b'_' {
        return None;
    }
    cursor += 1;
    while source
        .as_bytes()
        .get(cursor)
        .is_some_and(|byte| ident_byte(*byte))
    {
        cursor += 1;
    }
    cursor = skip_released_trivia(source, cursor)?;
    if source.as_bytes().get(cursor) != Some(&b')') {
        return None;
    }
    cursor += 1;
    let length = cursor - start;
    Some((source[start..cursor].to_owned(), length))
}

/// Skip exactly the whitespace and comment forms accepted by the frozen V1
/// generator parser. Unterminated block comments are not trivia: leaving the
/// annotation intact lets the parser return its released syntax error.
fn skip_released_trivia(source: &str, mut cursor: usize) -> Option<usize> {
    loop {
        let before = cursor;
        while source[cursor..]
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_whitespace())
        {
            cursor += source[cursor..]
                .chars()
                .next()
                .expect("whitespace character exists")
                .len_utf8();
        }
        if source[cursor..].starts_with('#') || source[cursor..].starts_with("//") {
            cursor = source[cursor..]
                .find('\n')
                .map_or(source.len(), |newline| cursor + newline + 1);
            continue;
        }
        if source[cursor..].starts_with("/*") {
            let close = source[cursor + 2..].find("*/")?;
            cursor += 2 + close + 2;
            continue;
        }
        if cursor == before {
            return Some(cursor);
        }
    }
}

/// Project a generation-time TypeQL input into canonical direct-descriptor JSON.
pub fn generated_declared_descriptors_json(source: &str) -> Result<String, String> {
    let document = DocumentId::new("generated/schema.tql")
        .expect("static generated schema document ID is valid");
    typeql_to_generated_descriptors(document, source).map_err(|diagnostics| diagnostics.to_string())
}

/// Render models and attach the direct declaration snapshot from the same input.
pub fn generate_package_with_declared_descriptors(
    input: &str,
    target: type_bridge_core_lib::_bindgen::TargetLanguage,
    options: &type_bridge_core_lib::_bindgen::BindgenOptions,
) -> Result<type_bridge_core_lib::_bindgen::GeneratedPackage, String> {
    let descriptors = generated_declared_descriptors_json(input)
        .map_err(|error| format!("Failed to render declared descriptor snapshot: {error}"))?;
    let document = DocumentId::new("generated/schema.tql")
        .expect("static generated schema document ID is valid");
    let reference_projection = released_unresolved_ranges(&document, input)
        .map_err(|error| format!("Failed to index released schema references: {error}"))?;
    let ranges = reference_projection
        .omitted_from_render
        .into_iter()
        .filter_map(|(start, end)| {
            input.get(start..end).and_then(|spelling| {
                let spelling = spelling.trim_start();
                let rest = spelling.strip_prefix("relates")?;
                rest.as_bytes()
                    .first()
                    .is_some_and(|byte| {
                        !byte.is_ascii_alphanumeric() && !matches!(*byte, b'_' | b'-')
                    })
                    .then(|| released_render_capability_range(input, start, end))
            })
        })
        .collect::<Vec<_>>();
    let render_source = type_bridge_core_lib::_parser::blank_source_extents(input, &ranges);
    let mut render_schema = TypeSchema::from_typeql(&render_source).map_err(|error| {
        format!("Failed to parse released schema for model generation: {error}")
    })?;
    sanitize_released_render_schema(&mut render_schema);
    let mut package = type_bridge_core_lib::_bindgen::BindgenPlan::from_schema(&render_schema)
        .render(target, options);
    attach_declared_descriptors(&mut package, descriptors, target)?;
    Ok(package)
}

/// Extend a capability span left through its separator for a syntactically
/// valid byte-preserving render projection. Comments between the comma and
/// capability are part of the blanked render-only range; descriptor evidence
/// continues to use the exact capability span.
fn released_render_capability_range(
    source: &str,
    capability_start: usize,
    capability_end: usize,
) -> core::ops::Range<usize> {
    use type_bridge_core_lib::_parser::{SourceRegionKind, scan_source_regions};

    let separator = scan_source_regions(&source[..capability_start])
        .into_iter()
        .rev()
        .filter(|(_, kind)| *kind == SourceRegionKind::Code)
        .find_map(|(range, _)| {
            source[range.clone()]
                .char_indices()
                .rev()
                .find_map(|(offset, character)| {
                    (!character.is_whitespace()).then_some((range.start + offset, character))
                })
        })
        .and_then(|(offset, character)| (character == ',').then_some(offset));
    separator.unwrap_or(capability_start)..capability_end
}

/// Remove open-world references that cannot be represented by a standalone
/// generated package.
///
/// Released generation accepts partial schema exports. Their raw parsed form
/// can therefore name a parent, owned attribute, relation role, or role
/// specialization that is absent from this input. The canonical descriptor
/// records those exact omissions; the binding projection must make the same
/// conservative choice instead of emitting missing imports, phantom `plays`
/// strings, undefined Rust fields, or `player_type = "unknown"` metadata.
/// This adapter is intentionally private to the released generator lane.
fn sanitize_released_render_schema(schema: &mut TypeSchema) {
    let attribute_names = schema.attributes.keys().cloned().collect::<BTreeSet<_>>();
    for attribute in schema.attributes.values_mut() {
        if attribute
            .parent
            .as_ref()
            .is_some_and(|parent| !attribute_names.contains(parent))
        {
            attribute.parent = None;
        }
    }

    let entity_names = schema.entities.keys().cloned().collect::<BTreeSet<_>>();
    for entity in schema.entities.values_mut() {
        if entity
            .parent
            .as_ref()
            .is_some_and(|parent| !entity_names.contains(parent))
        {
            entity.parent = None;
        }
        retain_known_ownerships(&attribute_names, &mut entity.owns, &mut entity.owns_order);
    }

    let relation_names = schema.relations.keys().cloned().collect::<BTreeSet<_>>();
    for relation in schema.relations.values_mut() {
        if relation
            .parent
            .as_ref()
            .is_some_and(|parent| !relation_names.contains(parent))
        {
            relation.parent = None;
        }
        retain_known_ownerships(
            &attribute_names,
            &mut relation.owns,
            &mut relation.owns_order,
        );
    }

    // The byte-aligned source projection removed every invalid direct
    // specialization before inheritance was resolved. Do not infer direct
    // provenance from this resolved role vector: inherited specialized roles
    // carry the same `overrides` field, and coincident local/parent names make
    // such inference ambiguous. At this point the resolved relation role sets
    // are authoritative for retaining plays edges.
    let roles_by_relation = released_roles_by_relation(schema);
    for entity in schema.entities.values_mut() {
        entity
            .plays
            .retain(|played| released_role_exists(&roles_by_relation, &played.role_ref));
    }
    for relation in schema.relations.values_mut() {
        relation
            .plays
            .retain(|played| released_role_exists(&roles_by_relation, &played.role_ref));
    }
}

fn retain_known_ownerships(
    attribute_names: &BTreeSet<String>,
    owns: &mut Vec<type_bridge_core_lib::_schema::OwnedAttribute>,
    owns_order: &mut Vec<String>,
) {
    owns.retain(|owned| attribute_names.contains(&owned.name));
    owns_order.retain(|name| attribute_names.contains(name));
}

fn released_role_exists(
    roles_by_relation: &BTreeMap<String, BTreeSet<String>>,
    role_ref: &str,
) -> bool {
    let Some((relation, role)) = role_ref.split_once(':') else {
        return false;
    };
    !role.contains(':')
        && roles_by_relation
            .get(relation)
            .is_some_and(|roles| roles.contains(role))
}

fn released_roles_by_relation(schema: &TypeSchema) -> BTreeMap<String, BTreeSet<String>> {
    schema
        .relations
        .iter()
        .map(|(name, relation)| {
            (
                name.clone(),
                relation
                    .roles
                    .iter()
                    .map(|role| role.name.clone())
                    .collect(),
            )
        })
        .collect()
}

/// Render the canonical empty declared-descriptor snapshot: a closed
/// world with zero declarations, as produced by a complete teardown.
pub fn empty_generated_declared_descriptors_json() -> Result<String, String> {
    let descriptors = GeneratedDeclaredDescriptorSetV1 {
        format: GENERATED_DECLARED_DESCRIPTOR_V1.to_string(),
        snapshot_kind: SnapshotKind::Declared,
        closed_world: true,
        unsupported_constructs: Vec::new(),
        attributes: Vec::new(),
        entities: Vec::new(),
        relations: Vec::new(),
        plays: Vec::new(),
    };
    let bytes = to_canonical_json(&descriptors).map_err(|diagnostic| diagnostic.to_string())?;
    Ok(String::from_utf8(bytes).expect("canonical JSON is valid UTF-8"))
}

/// Attach an already-rendered declared-descriptor snapshot to a generated
/// package exactly like the standard generation path: the snapshot file
/// plus, for Python, the registry constant.
pub fn attach_declared_descriptors(
    package: &mut type_bridge_core_lib::_bindgen::GeneratedPackage,
    descriptors: String,
    target: type_bridge_core_lib::_bindgen::TargetLanguage,
) -> Result<(), String> {
    if target == type_bridge_core_lib::_bindgen::TargetLanguage::Python {
        let registry = package
            .files
            .iter_mut()
            .find(|file| file.path == "registry.py")
            .ok_or_else(|| "Python bindgen package did not contain registry.py".to_string())?;
        let literal = to_canonical_json(&descriptors)
            .map_err(|error| format!("Failed to quote declared descriptor snapshot: {error}"))?;
        let literal = String::from_utf8(literal).expect("canonical JSON is valid UTF-8");
        registry.contents.push_str(&format!(
            "\nGENERATED_DECLARED_DESCRIPTORS_JSON: str = {literal}\n\
             __all__.append(\"GENERATED_DECLARED_DESCRIPTORS_JSON\")\n"
        ));
    }

    package
        .files
        .push(type_bridge_core_lib::_bindgen::GeneratedFile {
            path: GENERATED_DECLARED_DESCRIPTOR_PATH.to_string(),
            contents: descriptors,
        });
    Ok(())
}

impl GeneratedDeclaredDescriptorSetV1 {
    fn from_declared(declared: &DeclaredSchema) -> Result<Self, SchemaDiagnostics> {
        let mut descriptors = Self {
            format: GENERATED_DECLARED_DESCRIPTOR_V1.to_string(),
            snapshot_kind: SnapshotKind::Declared,
            closed_world: true,
            unsupported_constructs: Vec::new(),
            attributes: Vec::new(),
            entities: Vec::new(),
            relations: Vec::new(),
            plays: Vec::new(),
        };

        for fact in declared.facts() {
            let SchemaFact::Type(fact) = fact else {
                if matches!(fact, SchemaFact::Function(_) | SchemaFact::Struct(_)) {
                    return Err(unsupported_fact(
                        declared,
                        fact,
                        "unsupported_generated_descriptor_fact",
                        "generated descriptor snapshots do not encode functions or structs",
                    ));
                }
                continue;
            };
            let id = fact.id();
            let source = direct_source(declared, &SchemaFact::Type(fact.clone()))?;
            match id.kind() {
                TypeKind::Attribute => descriptors.attributes.push(AttributeDescriptor {
                    label: id.label().as_str().to_string(),
                    parent: None,
                    value_type: None,
                    is_abstract: false,
                    is_independent: false,
                    regex: None,
                    values: None,
                    range: None,
                    doc: None,
                    meta: BTreeMap::new(),
                    source,
                }),
                TypeKind::Entity => descriptors.entities.push(ObjectDescriptor {
                    label: id.label().as_str().to_string(),
                    parent: None,
                    is_abstract: false,
                    doc: None,
                    meta: BTreeMap::new(),
                    owns: Vec::new(),
                    source,
                }),
                TypeKind::Relation => descriptors.relations.push(RelationDescriptor {
                    object: ObjectDescriptor {
                        label: id.label().as_str().to_string(),
                        parent: None,
                        is_abstract: false,
                        doc: None,
                        meta: BTreeMap::new(),
                        owns: Vec::new(),
                        source,
                    },
                    relates: Vec::new(),
                }),
                TypeKind::Struct => unreachable!("struct existence does not use TypeFact"),
            }
        }

        for fact in declared.facts() {
            match fact {
                SchemaFact::Sub(fact) => {
                    let subtype = fact.id().subtype();
                    let parent = fact.id().supertype().label().as_str().to_string();
                    let slot = parent_slot(&mut descriptors, subtype);
                    if slot.replace(parent).is_some() {
                        return Err(unsupported_fact(
                            declared,
                            &SchemaFact::Sub(fact.clone()),
                            "unsupported_generated_descriptor_multiple_inheritance",
                            "generated descriptor snapshots require one direct parent per type",
                        ));
                    }
                }
                SchemaFact::Value(fact) => {
                    attribute_mut(&mut descriptors, fact.id().attribute().label().as_str())
                        .value_type = Some(fact.value_type());
                }
                SchemaFact::Owns(fact) => {
                    let id = fact.id();
                    let source = direct_source(declared, &SchemaFact::Owns(fact.clone()))?;
                    object_mut(&mut descriptors, id.owner())
                        .owns
                        .push(OwnsDescriptor {
                            attribute: id.attribute().label().as_str().to_string(),
                            key: false,
                            unique: false,
                            card: None,
                            doc: None,
                            meta: BTreeMap::new(),
                            source,
                        });
                }
                SchemaFact::Relates(fact) => {
                    let id = fact.id();
                    let source = direct_source(declared, &SchemaFact::Relates(fact.clone()))?;
                    relation_mut(&mut descriptors, id.relation().label().as_str())
                        .relates
                        .push(RelatesDescriptor {
                            role: id.role().label().as_str().to_string(),
                            specializes: fact
                                .specializes()
                                .map(|role| role.label().as_str().to_string()),
                            is_abstract: false,
                            card: None,
                            doc: None,
                            meta: BTreeMap::new(),
                            source,
                        });
                }
                SchemaFact::Plays(fact) => {
                    let id = fact.id();
                    let source = direct_source(declared, &SchemaFact::Plays(fact.clone()))?;
                    descriptors.plays.push(PlaysDescriptor {
                        player: id.player().label().as_str().to_string(),
                        relation: id.role().declaring_relation().as_str().to_string(),
                        role: id.role().label().as_str().to_string(),
                        card: None,
                        doc: None,
                        meta: BTreeMap::new(),
                        source,
                    });
                }
                SchemaFact::Type(_)
                | SchemaFact::Annotation(_)
                | SchemaFact::Function(_)
                | SchemaFact::Struct(_) => {}
            }
        }

        for fact in declared.facts() {
            if let SchemaFact::Annotation(annotation) = fact {
                apply_annotation(&mut descriptors, declared, annotation)?;
            }
        }
        Ok(descriptors)
    }

    fn into_declared(
        self,
        document: DocumentId,
        source_len: usize,
    ) -> Result<DeclaredSchema, SchemaDiagnostics> {
        if self.format != GENERATED_DECLARED_DESCRIPTOR_V1 {
            return Err(error(
                "unsupported_generated_descriptor_format",
                "generated descriptor input uses an unsupported format",
                None,
            ));
        }
        if !matches!(self.snapshot_kind, SnapshotKind::Declared) {
            return Err(error(
                "generated_descriptor_snapshot_not_declared",
                "effective or partial descriptor snapshots cannot recover direct facts",
                None,
            ));
        }
        if !self.closed_world {
            return Err(error(
                "generated_descriptor_snapshot_incomplete",
                "generated descriptor input must cover its complete declared model set",
                None,
            ));
        }
        if let Some(construct) = self.unsupported_constructs.first() {
            return Err(SchemaDiagnostics::one(SchemaDiagnostic::new(
                Diagnostic::new(
                    DiagnosticCategory::UnsupportedCapability,
                    DiagnosticCode::new("unsupported_generated_descriptor_construct")
                        .expect("static generated-descriptor diagnostic code is valid"),
                    "generated descriptor input contains a construct outside the overlap grammar",
                )
                .with_detail("construct", construct.clone()),
                None,
            )));
        }

        let mut assembler = FactAssembler::new(FormatVersion::V1);
        let mut type_ids = BTreeMap::<String, TypeId>::new();

        for attribute in &self.attributes {
            insert_type(
                &mut assembler,
                &mut type_ids,
                TypeKind::Attribute,
                &attribute.label,
                source(&document, source_len, &attribute.source)?,
            )?;
        }
        for entity in &self.entities {
            insert_type(
                &mut assembler,
                &mut type_ids,
                TypeKind::Entity,
                &entity.label,
                source(&document, source_len, &entity.source)?,
            )?;
        }
        for relation in &self.relations {
            insert_type(
                &mut assembler,
                &mut type_ids,
                TypeKind::Relation,
                &relation.object.label,
                source(&document, source_len, &relation.object.source)?,
            )?;
        }

        for attribute in &self.attributes {
            insert_attribute(&mut assembler, &type_ids, attribute, &document, source_len)?;
        }
        for entity in &self.entities {
            insert_object(
                &mut assembler,
                &type_ids,
                TypeKind::Entity,
                entity,
                &document,
                source_len,
            )?;
        }
        for relation in &self.relations {
            insert_object(
                &mut assembler,
                &type_ids,
                TypeKind::Relation,
                &relation.object,
                &document,
                source_len,
            )?;
            insert_relates(&mut assembler, &type_ids, relation, &document, source_len)?;
        }
        for plays in &self.plays {
            insert_plays(&mut assembler, &type_ids, plays, &document, source_len)?;
        }

        assembler.finish()
    }
}

fn insert_type(
    assembler: &mut FactAssembler,
    type_ids: &mut BTreeMap<String, TypeId>,
    kind: TypeKind,
    label: &str,
    source: SourceSpan,
) -> Result<(), SchemaDiagnostics> {
    let id = TypeId::new(kind, label).map_err(|diagnostic| contract(diagnostic, &source))?;
    let fact = TypeFact::new(id.clone()).map_err(|diagnostic| contract(diagnostic, &source))?;
    assembler.insert_fact(SchemaFact::Type(fact), source)?;
    type_ids.insert(label.to_owned(), id);
    Ok(())
}

fn insert_attribute(
    assembler: &mut FactAssembler,
    type_ids: &BTreeMap<String, TypeId>,
    descriptor: &AttributeDescriptor,
    document: &DocumentId,
    source_len: usize,
) -> Result<(), SchemaDiagnostics> {
    let source = source(document, source_len, &descriptor.source)?;
    let id = required_type(type_ids, &descriptor.label, TypeKind::Attribute, &source)?;
    insert_parent(assembler, id.clone(), descriptor.parent.as_deref(), &source)?;
    insert_type_annotations(
        assembler,
        id.clone(),
        descriptor.is_abstract,
        descriptor.is_independent,
        descriptor.doc.as_deref(),
        &descriptor.meta,
        &source,
    )?;

    let attribute =
        AttributeId::new(&descriptor.label).map_err(|diagnostic| contract(diagnostic, &source))?;
    let value_id = ValueFactId::new(attribute);
    if let Some(value_type) = descriptor.value_type {
        assembler.insert_fact(
            SchemaFact::Value(ValueFact::new(value_id.clone(), value_type)),
            source.clone(),
        )?;
    }
    if let Some(regex) = &descriptor.regex {
        let regex =
            RegexPattern::new(regex.clone()).map_err(|diagnostic| contract(diagnostic, &source))?;
        insert_annotation(
            assembler,
            AnnotationSubjectId::Value(value_id.clone()),
            AnnotationKindId::Regex,
            SchemaAnnotationValue::Regex(regex),
            &source,
        )?;
    }
    if let Some(values) = &descriptor.values {
        let values = CanonicalValueSet::new(values.clone())
            .map_err(|diagnostic| contract(diagnostic, &source))?;
        insert_annotation(
            assembler,
            AnnotationSubjectId::Value(value_id.clone()),
            AnnotationKindId::Values,
            SchemaAnnotationValue::Values(values),
            &source,
        )?;
    }
    if let Some(range) = &descriptor.range {
        let range = CanonicalValueRange::new(range.min.clone(), range.max.clone())
            .map_err(|diagnostic| contract(diagnostic, &source))?;
        insert_annotation(
            assembler,
            AnnotationSubjectId::Value(value_id),
            AnnotationKindId::Range,
            SchemaAnnotationValue::Range(range),
            &source,
        )?;
    }
    Ok(())
}

fn insert_object(
    assembler: &mut FactAssembler,
    type_ids: &BTreeMap<String, TypeId>,
    kind: TypeKind,
    descriptor: &ObjectDescriptor,
    document: &DocumentId,
    source_len: usize,
) -> Result<(), SchemaDiagnostics> {
    let object_source = source(document, source_len, &descriptor.source)?;
    let id = required_type(type_ids, &descriptor.label, kind, &object_source)?;
    insert_parent(
        assembler,
        id.clone(),
        descriptor.parent.as_deref(),
        &object_source,
    )?;
    insert_type_annotations(
        assembler,
        id.clone(),
        descriptor.is_abstract,
        false,
        descriptor.doc.as_deref(),
        &descriptor.meta,
        &object_source,
    )?;
    for owns in &descriptor.owns {
        let source = source(document, source_len, &owns.source)?;
        let attribute = AttributeId::new(&owns.attribute)
            .map_err(|diagnostic| contract(diagnostic, &source))?;
        let owns_id = OwnsFactId::new(id.clone(), attribute)
            .map_err(|diagnostic| contract(diagnostic, &source))?;
        assembler.insert_fact(
            SchemaFact::Owns(OwnsFact::new(owns_id.clone())),
            source.clone(),
        )?;
        if owns.key {
            insert_presence(
                assembler,
                AnnotationSubjectId::Owns(owns_id.clone()),
                AnnotationKindId::Key,
                &source,
            )?;
        }
        if owns.unique {
            insert_presence(
                assembler,
                AnnotationSubjectId::Owns(owns_id.clone()),
                AnnotationKindId::Unique,
                &source,
            )?;
        }
        if let Some(cardinality) = owns.card {
            insert_annotation(
                assembler,
                AnnotationSubjectId::Owns(owns_id.clone()),
                AnnotationKindId::Card,
                SchemaAnnotationValue::Cardinality(cardinality),
                &source,
            )?;
        }
        insert_doc_meta(
            assembler,
            AnnotationSubjectId::Owns(owns_id),
            owns.doc.as_deref(),
            &owns.meta,
            &source,
        )?;
    }
    Ok(())
}

fn insert_relates(
    assembler: &mut FactAssembler,
    type_ids: &BTreeMap<String, TypeId>,
    descriptor: &RelationDescriptor,
    document: &DocumentId,
    source_len: usize,
) -> Result<(), SchemaDiagnostics> {
    let relation_source = source(document, source_len, &descriptor.object.source)?;
    let relation = required_type(
        type_ids,
        &descriptor.object.label,
        TypeKind::Relation,
        &relation_source,
    )?;
    for relates in &descriptor.relates {
        let source = source(document, source_len, &relates.source)?;
        let role = RoleId::new(&descriptor.object.label, &relates.role)
            .map_err(|diagnostic| contract(diagnostic, &source))?;
        let id = RelatesFactId::new(relation.clone(), role)
            .map_err(|diagnostic| contract(diagnostic, &source))?;
        let specializes = relates
            .specializes
            .as_ref()
            .map(|label| {
                Label::new(label.clone())
                    .map(|label| (label, source.clone()))
                    .map_err(|diagnostic| contract(diagnostic, &source))
            })
            .transpose()?;
        assembler.insert_relates(id.clone(), specializes, source.clone())?;
        if relates.is_abstract {
            insert_presence(
                assembler,
                AnnotationSubjectId::Relates(id.clone()),
                AnnotationKindId::Abstract,
                &source,
            )?;
        }
        if let Some(cardinality) = relates.card {
            insert_annotation(
                assembler,
                AnnotationSubjectId::Relates(id.clone()),
                AnnotationKindId::Card,
                SchemaAnnotationValue::Cardinality(cardinality),
                &source,
            )?;
        }
        insert_doc_meta(
            assembler,
            AnnotationSubjectId::Relates(id),
            relates.doc.as_deref(),
            &relates.meta,
            &source,
        )?;
    }
    Ok(())
}

fn insert_plays(
    assembler: &mut FactAssembler,
    type_ids: &BTreeMap<String, TypeId>,
    descriptor: &PlaysDescriptor,
    document: &DocumentId,
    source_len: usize,
) -> Result<(), SchemaDiagnostics> {
    let source = source(document, source_len, &descriptor.source)?;
    let player = type_ids.get(&descriptor.player).cloned().ok_or_else(|| {
        error(
            "unknown_generated_descriptor_player",
            "generated playing descriptor names an undeclared player",
            Some(source.clone()),
        )
    })?;
    let relation = required_type(type_ids, &descriptor.relation, TypeKind::Relation, &source)?;
    let role = RoleId::new(relation.label().as_str(), &descriptor.role)
        .map_err(|diagnostic| contract(diagnostic, &source))?;
    let id = type_bridge_contract::schema::PlaysFactId::new(player, role)
        .map_err(|diagnostic| contract(diagnostic, &source))?;
    assembler.insert_plays(
        Label::new(&descriptor.player).map_err(|diagnostic| contract(diagnostic, &source))?,
        relation.label().clone(),
        Label::new(&descriptor.role).map_err(|diagnostic| contract(diagnostic, &source))?,
        source.clone(),
    );
    if let Some(cardinality) = descriptor.card {
        insert_annotation(
            assembler,
            AnnotationSubjectId::Plays(id.clone()),
            AnnotationKindId::Card,
            SchemaAnnotationValue::Cardinality(cardinality),
            &source,
        )?;
    }
    insert_doc_meta(
        assembler,
        AnnotationSubjectId::Plays(id),
        descriptor.doc.as_deref(),
        &descriptor.meta,
        &source,
    )
}

fn insert_parent(
    assembler: &mut FactAssembler,
    subtype: TypeId,
    parent: Option<&str>,
    source: &SourceSpan,
) -> Result<(), SchemaDiagnostics> {
    let Some(parent) = parent else {
        return Ok(());
    };
    let supertype =
        TypeId::new(subtype.kind(), parent).map_err(|diagnostic| contract(diagnostic, source))?;
    let id =
        SubFactId::new(subtype, supertype).map_err(|diagnostic| contract(diagnostic, source))?;
    assembler.insert_fact(SchemaFact::Sub(SubFact::new(id)), source.clone())
}

fn insert_type_annotations(
    assembler: &mut FactAssembler,
    id: TypeId,
    is_abstract: bool,
    is_independent: bool,
    doc: Option<&str>,
    meta: &BTreeMap<String, String>,
    source: &SourceSpan,
) -> Result<(), SchemaDiagnostics> {
    if is_abstract {
        insert_presence(
            assembler,
            AnnotationSubjectId::Type(id.clone()),
            AnnotationKindId::Abstract,
            source,
        )?;
    }
    if is_independent {
        insert_presence(
            assembler,
            AnnotationSubjectId::Type(id.clone()),
            AnnotationKindId::Independent,
            source,
        )?;
    }
    insert_doc_meta(assembler, AnnotationSubjectId::Type(id), doc, meta, source)
}

fn insert_presence(
    assembler: &mut FactAssembler,
    subject: AnnotationSubjectId,
    kind: AnnotationKindId,
    source: &SourceSpan,
) -> Result<(), SchemaDiagnostics> {
    insert_annotation(
        assembler,
        subject,
        kind,
        SchemaAnnotationValue::Presence,
        source,
    )
}

fn insert_doc_meta(
    assembler: &mut FactAssembler,
    subject: AnnotationSubjectId,
    doc: Option<&str>,
    meta: &BTreeMap<String, String>,
    source: &SourceSpan,
) -> Result<(), SchemaDiagnostics> {
    if let Some(doc) = doc {
        let doc = DocText::new(doc).map_err(|diagnostic| contract(diagnostic, source))?;
        insert_annotation(
            assembler,
            subject.clone(),
            AnnotationKindId::Doc,
            SchemaAnnotationValue::Doc(doc),
            source,
        )?;
    }
    for (key, value) in meta {
        let kind = AnnotationKindId::meta(key.clone())
            .map_err(|diagnostic| contract(diagnostic, source))?;
        let value = CanonicalString::new(value.clone())
            .map_err(|diagnostic| contract(diagnostic, source))?;
        insert_annotation(
            assembler,
            subject.clone(),
            kind,
            SchemaAnnotationValue::Meta(CanonicalValue::String(value)),
            source,
        )?;
    }
    Ok(())
}

fn insert_annotation(
    assembler: &mut FactAssembler,
    subject: AnnotationSubjectId,
    kind: AnnotationKindId,
    value: SchemaAnnotationValue,
    source: &SourceSpan,
) -> Result<(), SchemaDiagnostics> {
    let fact = AnnotationFact::new(AnnotationFactId::new(subject, kind), value)
        .map_err(|diagnostic| contract(diagnostic, source))?;
    assembler.insert_fact(SchemaFact::Annotation(fact), source.clone())
}

fn required_type(
    type_ids: &BTreeMap<String, TypeId>,
    label: &str,
    kind: TypeKind,
    source: &SourceSpan,
) -> Result<TypeId, SchemaDiagnostics> {
    type_ids
        .get(label)
        .filter(|id| id.kind() == kind)
        .cloned()
        .ok_or_else(|| {
            error(
                "unknown_generated_descriptor_type",
                "generated descriptor references an undeclared or wrong-kind type",
                Some(source.clone()),
            )
        })
}

fn source(
    document: &DocumentId,
    source_len: usize,
    descriptor: &DescriptorSource,
) -> Result<SourceSpan, SchemaDiagnostics> {
    if !matches!(descriptor.provenance, ProvenanceKind::Direct) {
        return Err(error(
            "generated_descriptor_provenance_not_direct",
            "generated descriptor entries must retain direct declaration provenance",
            None,
        ));
    }
    if descriptor.document.is_none()
        && descriptor.byte_end > u64::try_from(source_len).unwrap_or(u64::MAX)
    {
        return Err(error(
            "generated_descriptor_source_out_of_bounds",
            "generated descriptor provenance points outside its canonical document",
            None,
        ));
    }
    let document = descriptor
        .document
        .as_ref()
        .map(|value| DocumentId::new(value.clone()).map_err(|diagnostic| one(diagnostic, None)))
        .transpose()?
        .unwrap_or_else(|| document.clone());
    SourceSpan::new(
        document,
        descriptor.byte_start,
        descriptor.byte_end,
        descriptor.line,
        descriptor.column,
        descriptor.end_line,
        descriptor.end_column,
    )
    .map_err(|diagnostic| one(diagnostic, None))
}

fn direct_source(
    declared: &DeclaredSchema,
    fact: &SchemaFact,
) -> Result<DescriptorSource, SchemaDiagnostics> {
    let source = declared.source(&fact.id()).ok_or_else(|| {
        error(
            "generated_descriptor_missing_direct_source",
            "every emitted generated descriptor fact must retain direct provenance",
            None,
        )
    })?;
    Ok(DescriptorSource {
        provenance: ProvenanceKind::Direct,
        document: Some(source.document().as_str().to_string()),
        byte_start: source.byte_start(),
        byte_end: source.byte_end(),
        line: source.line(),
        column: source.column(),
        end_line: source.end_line(),
        end_column: source.end_column(),
    })
}

fn parent_slot<'a>(
    descriptors: &'a mut GeneratedDeclaredDescriptorSetV1,
    subtype: &TypeId,
) -> &'a mut Option<String> {
    match subtype.kind() {
        TypeKind::Attribute => &mut attribute_mut(descriptors, subtype.label().as_str()).parent,
        TypeKind::Entity | TypeKind::Relation => &mut object_mut(descriptors, subtype).parent,
        TypeKind::Struct => unreachable!("structs do not participate in subtype facts"),
    }
}

fn attribute_mut<'a>(
    descriptors: &'a mut GeneratedDeclaredDescriptorSetV1,
    label: &str,
) -> &'a mut AttributeDescriptor {
    descriptors
        .attributes
        .iter_mut()
        .find(|descriptor| descriptor.label == label)
        .expect("declared attribute references have a matching type fact")
}

fn relation_mut<'a>(
    descriptors: &'a mut GeneratedDeclaredDescriptorSetV1,
    label: &str,
) -> &'a mut RelationDescriptor {
    descriptors
        .relations
        .iter_mut()
        .find(|descriptor| descriptor.object.label == label)
        .expect("declared relation references have a matching type fact")
}

fn object_mut<'a>(
    descriptors: &'a mut GeneratedDeclaredDescriptorSetV1,
    id: &TypeId,
) -> &'a mut ObjectDescriptor {
    match id.kind() {
        TypeKind::Entity => descriptors
            .entities
            .iter_mut()
            .find(|descriptor| descriptor.label == id.label().as_str())
            .expect("declared entity references have a matching type fact"),
        TypeKind::Relation => &mut relation_mut(descriptors, id.label().as_str()).object,
        TypeKind::Attribute | TypeKind::Struct => {
            unreachable!("only entities and relations own attributes")
        }
    }
}

fn apply_annotation(
    descriptors: &mut GeneratedDeclaredDescriptorSetV1,
    declared: &DeclaredSchema,
    annotation: &AnnotationFact,
) -> Result<(), SchemaDiagnostics> {
    let kind = annotation.id().kind();
    let value = annotation.value();
    let handled = match annotation.id().subject() {
        AnnotationSubjectId::Type(id) => match id.kind() {
            TypeKind::Attribute => {
                let descriptor = attribute_mut(descriptors, id.label().as_str());
                match (kind, value) {
                    (AnnotationKindId::Abstract, SchemaAnnotationValue::Presence) => {
                        descriptor.is_abstract = true;
                        true
                    }
                    (AnnotationKindId::Independent, SchemaAnnotationValue::Presence) => {
                        descriptor.is_independent = true;
                        true
                    }
                    _ => apply_doc_meta(kind, value, &mut descriptor.doc, &mut descriptor.meta),
                }
            }
            TypeKind::Entity | TypeKind::Relation => {
                let descriptor = object_mut(descriptors, id);
                match (kind, value) {
                    (AnnotationKindId::Abstract, SchemaAnnotationValue::Presence) => {
                        descriptor.is_abstract = true;
                        true
                    }
                    _ => apply_doc_meta(kind, value, &mut descriptor.doc, &mut descriptor.meta),
                }
            }
            TypeKind::Struct => false,
        },
        AnnotationSubjectId::Value(id) => {
            let descriptor = attribute_mut(descriptors, id.attribute().label().as_str());
            match (kind, value) {
                (AnnotationKindId::Regex, SchemaAnnotationValue::Regex(regex)) => {
                    descriptor.regex = Some(regex.as_str().to_string());
                    true
                }
                (AnnotationKindId::Values, SchemaAnnotationValue::Values(values)) => {
                    descriptor.values = Some(values.iter().cloned().collect());
                    true
                }
                (AnnotationKindId::Range, SchemaAnnotationValue::Range(range)) => {
                    descriptor.range = Some(ValueRangeDescriptor {
                        min: range.lower().cloned(),
                        max: range.upper().cloned(),
                    });
                    true
                }
                _ => false,
            }
        }
        AnnotationSubjectId::Owns(id) => {
            let descriptor = object_mut(descriptors, id.owner())
                .owns
                .iter_mut()
                .find(|descriptor| descriptor.attribute == id.attribute().label().as_str())
                .expect("declared ownership annotation has a matching owns fact");
            match (kind, value) {
                (AnnotationKindId::Key, SchemaAnnotationValue::Presence) => {
                    descriptor.key = true;
                    true
                }
                (AnnotationKindId::Unique, SchemaAnnotationValue::Presence) => {
                    descriptor.unique = true;
                    true
                }
                (AnnotationKindId::Card, SchemaAnnotationValue::Cardinality(cardinality)) => {
                    descriptor.card = Some(*cardinality);
                    true
                }
                _ => apply_doc_meta(kind, value, &mut descriptor.doc, &mut descriptor.meta),
            }
        }
        AnnotationSubjectId::Relates(id) => {
            let descriptor = relation_mut(descriptors, id.relation().label().as_str())
                .relates
                .iter_mut()
                .find(|descriptor| descriptor.role == id.role().label().as_str())
                .expect("declared role annotation has a matching relates fact");
            match (kind, value) {
                (AnnotationKindId::Abstract, SchemaAnnotationValue::Presence) => {
                    descriptor.is_abstract = true;
                    true
                }
                (AnnotationKindId::Card, SchemaAnnotationValue::Cardinality(cardinality)) => {
                    descriptor.card = Some(*cardinality);
                    true
                }
                _ => apply_doc_meta(kind, value, &mut descriptor.doc, &mut descriptor.meta),
            }
        }
        AnnotationSubjectId::Plays(id) => {
            let descriptor = descriptors
                .plays
                .iter_mut()
                .find(|descriptor| {
                    descriptor.player == id.player().label().as_str()
                        && descriptor.relation == id.role().declaring_relation().as_str()
                        && descriptor.role == id.role().label().as_str()
                })
                .expect("declared playing annotation has a matching plays fact");
            match (kind, value) {
                (AnnotationKindId::Card, SchemaAnnotationValue::Cardinality(cardinality)) => {
                    descriptor.card = Some(*cardinality);
                    true
                }
                _ => apply_doc_meta(kind, value, &mut descriptor.doc, &mut descriptor.meta),
            }
        }
        AnnotationSubjectId::Sub(_) | AnnotationSubjectId::Function(_) => false,
    };

    if handled {
        Ok(())
    } else {
        Err(unsupported_fact(
            declared,
            &SchemaFact::Annotation(annotation.clone()),
            "unsupported_generated_descriptor_annotation",
            "generated descriptor snapshots cannot encode this direct annotation subject or value",
        ))
    }
}

fn apply_doc_meta(
    kind: &AnnotationKindId,
    value: &SchemaAnnotationValue,
    doc: &mut Option<String>,
    meta: &mut BTreeMap<String, String>,
) -> bool {
    match (kind, value) {
        (AnnotationKindId::Doc, SchemaAnnotationValue::Doc(value)) => {
            *doc = Some(value.as_str().to_string());
            true
        }
        (
            AnnotationKindId::Meta(key),
            SchemaAnnotationValue::Meta(CanonicalValue::String(value)),
        ) => {
            meta.insert(key.as_str().to_string(), value.as_str().to_string());
            true
        }
        _ => false,
    }
}

fn unsupported_fact(
    declared: &DeclaredSchema,
    fact: &SchemaFact,
    code: &'static str,
    message: &'static str,
) -> SchemaDiagnostics {
    one(
        Diagnostic::new(
            DiagnosticCategory::UnsupportedCapability,
            DiagnosticCode::new(code)
                .expect("static generated-descriptor diagnostic code is valid"),
            message,
        ),
        declared.source(&fact.id()).cloned(),
    )
}

fn contract(diagnostic: Diagnostic, source: &SourceSpan) -> SchemaDiagnostics {
    one(diagnostic, Some(source.clone()))
}

fn error(
    code: &'static str,
    message: &'static str,
    primary: Option<SourceSpan>,
) -> SchemaDiagnostics {
    one(
        Diagnostic::new(
            DiagnosticCategory::InvalidContract,
            DiagnosticCode::new(code)
                .expect("static generated-descriptor diagnostic code is valid"),
            message,
        ),
        primary,
    )
}

fn one(diagnostic: Diagnostic, primary: Option<SourceSpan>) -> SchemaDiagnostics {
    SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, primary))
}
