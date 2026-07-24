//! Pure reconstruction of query authority from a TypeDB schema export.
//!
//! This module deliberately lives below the ORM and provider-bearing migration
//! crate. Local prepared queries and the remote executor must validate the same
//! exported facts without creating an ORM -> migration -> ORM dependency cycle.

use serde_json::Value;
use std::collections::BTreeSet;
use type_bridge_contract::capability::CapabilitySet;
use type_bridge_contract::codec::FormatVersion;
use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::reserved::{
    LEGACY_CUTOVER_ANCHOR_ENTITY, LEGACY_CUTOVER_ANCHOR_FINGERPRINT, LEGACY_CUTOVER_ANCHOR_KEY,
    LEGACY_CUTOVER_ANCHOR_SCOPE, MANAGED_CONTROL_ENTITY, MANAGED_CONTROL_LEASE_FENCE,
    MANAGED_CONTROL_LEASE_HOLDER, MANAGED_CONTROL_LEASE_STATE, MANAGED_CONTROL_SCOPE,
    TYPEBRIDGE_INTERNAL_PREFIX,
};
use type_bridge_contract::schema::{
    DeclaredSchema, DocumentId, ManagedSchemaState, SourcedSchemaFact,
};
use type_bridge_schema::{DeltaError, ManagedDeltaContext, managed_schema_state};
use typeql::query::{QueryStructure, schema::SchemaQuery};
use typeql::schema::definable::Definable;
use typeql::schema::definable::type_::CapabilityBase;
use typeql::type_::NamedType;

use crate::descriptor::released_typeql_to_declared_presence_projection_with_references;
use crate::function_references::reject_reserved_function_references;
use crate::{
    LEGACY_LEDGER_SCHEMA_TYPEQL, is_legacy_ledger_label,
    released_typeql_to_declared_lossless_projection_with_references, typeql_to_declared,
};

/// Exact TypeDB 3 fence-mirror schema installed in a managed database.
///
/// Query-only databases may omit this partition. If any reserved V2 fact is
/// present, however, the complete partition must equal these frozen bytes.
pub const MANAGED_FENCE_SCHEMA_TYPEQL: &str = r#"define
attribute typebridge-internal-v2-control-scope, value string;
attribute typebridge-internal-v2-lease-holder, value string;
attribute typebridge-internal-v2-lease-fence, value string;
attribute typebridge-internal-v2-lease-state, value string;
attribute typebridge-internal-v2-legacy-cutover-key, value string;
attribute typebridge-internal-v2-legacy-cutover-scope, value string;
attribute typebridge-internal-v2-legacy-cutover-fingerprint, value string;
entity typebridge-internal-v2-migration-control,
    owns typebridge-internal-v2-control-scope @key,
    owns typebridge-internal-v2-lease-holder @card(0..1),
    owns typebridge-internal-v2-lease-fence @card(1..1),
    owns typebridge-internal-v2-lease-state @card(1..1);
entity typebridge-internal-v2-legacy-cutover,
    owns typebridge-internal-v2-legacy-cutover-key @key,
    owns typebridge-internal-v2-legacy-cutover-scope @card(1..1),
    owns typebridge-internal-v2-legacy-cutover-fingerprint @card(1..1);
"#;

struct QueryAuthorityPartitions {
    user: DeclaredSchema,
    internal: DeclaredSchema,
    legacy_control: DeclaredSchema,
}

struct ParsedLiveAuthorityExport {
    declared: DeclaredSchema,
    stripped_offsets: Vec<usize>,
}

/// Whether a live query schema carries the complete managed-database fence
/// partition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveQueryControlPresence {
    /// No reserved V2 control facts are installed.
    Absent,
    /// The complete frozen managed fence schema is installed.
    ManagedFence,
    /// The frozen core is present, but one of its facts carries released-only
    /// list or annotation semantics absent from the portable V2 fact graph.
    ManagedFenceWithExtensions,
}

/// Whether the exact released V1 migration-ledger schema is present.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveLegacyLedgerPresence {
    /// No released-ledger facts are installed.
    Absent,
    /// Every frozen V1 ledger fact is installed with its exact shape.
    FrozenLedger,
    /// Every portable frozen fact is installed, but one of those facts carries
    /// released-only list or annotation semantics.
    FrozenLedgerWithExtensions,
}

/// Inspect one TypeDB schema export for the exact managed writer-fence schema.
///
/// This deliberately evaluates the frozen reserved partition independently of
/// user and released-ledger partitions. A released schema may legitimately
/// contain labels which resemble control labels; only the complete canonical
/// value types, ownerships, keys, cardinalities, and annotations confer
/// managed authority. Additional reserved-prefix facts do not erase a proven
/// canonical core: they are handled as post-authority state so corruption
/// cannot reopen an adopted database. An incomplete or altered core remains
/// non-authoritative because it is indistinguishable from a released label
/// collision.
pub fn managed_fence_schema_presence(export: &str) -> Result<LiveQueryControlPresence, Diagnostic> {
    let full = parse_live_authority_export(
        export,
        is_managed_fence_label,
        "migration_typedb_control_schema_mismatch",
        "reserved fence-mirror schema differs from the frozen contract",
    )?;
    managed_fence_presence_from_declared(&full.declared, &full.stripped_offsets)
}

/// Inspect one TypeDB schema export for the exact frozen V1 ledger schema.
///
/// This check is intentionally independent from managed-fence presence so a
/// caller can first establish managed authority and inspect managed rows. That
/// ordering distinguishes a harmless released label collision from corruption
/// of an already-established cutover.
pub fn legacy_ledger_schema_presence(export: &str) -> Result<LiveLegacyLedgerPresence, Diagnostic> {
    let full = parse_live_authority_export(
        export,
        is_legacy_ledger_label,
        "migration_typedb_legacy_ledger_mismatch",
        "legacy migration-ledger facts differ from the frozen v1 contract",
    )?;
    legacy_ledger_presence_from_declared(&full.declared, &full.stripped_offsets)
}

fn managed_fence_presence_from_declared(
    full: &DeclaredSchema,
    stripped_offsets: &[usize],
) -> Result<LiveQueryControlPresence, Diagnostic> {
    if !schema_mentions_labels(full, value_mentions_reserved_label)? {
        return Ok(LiveQueryControlPresence::Absent);
    }
    let expected = frozen_fence_mirror_schema()?;
    if !contains_exact_schema_subset(full, &expected) {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_control_schema_mismatch",
            "reserved fence-mirror schema differs from the frozen contract",
        ));
    }
    if expected_facts_carry_stripped_extensions(full, &expected, stripped_offsets)? {
        Ok(LiveQueryControlPresence::ManagedFenceWithExtensions)
    } else {
        Ok(LiveQueryControlPresence::ManagedFence)
    }
}

fn legacy_ledger_presence_from_declared(
    full: &DeclaredSchema,
    stripped_offsets: &[usize],
) -> Result<LiveLegacyLedgerPresence, Diagnostic> {
    if !schema_mentions_labels(full, value_mentions_legacy_control_label)? {
        return Ok(LiveLegacyLedgerPresence::Absent);
    }
    let expected = frozen_legacy_ledger_schema()?;
    if !contains_exact_schema_subset(full, &expected) {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_legacy_ledger_mismatch",
            "legacy migration-ledger facts differ from the frozen v1 contract",
        ));
    }
    if expected_facts_carry_stripped_extensions(full, &expected, stripped_offsets)? {
        Ok(LiveLegacyLedgerPresence::FrozenLedgerWithExtensions)
    } else {
        Ok(LiveLegacyLedgerPresence::FrozenLedger)
    }
}

fn parse_live_authority_export(
    export: &str,
    relevant_label: fn(&str) -> bool,
    mismatch_code: &'static str,
    mismatch_message: &'static str,
) -> Result<ParsedLiveAuthorityExport, Diagnostic> {
    let presence_source = match writer_authority_presence_source(export, relevant_label) {
        WriterAuthorityPresenceSource::Absent => {
            let declared = DeclaredSchema::from_facts(
                FormatVersion::V1,
                CapabilitySet::new(),
                std::iter::empty::<SourcedSchemaFact>(),
            )
            .map_err(|_| {
                failure(
                    DiagnosticCategory::Integrity,
                    "migration_typedb_empty_projection_invalid",
                    "empty writer-authority projection cannot form a declared schema",
                )
            })?;
            return Ok(ParsedLiveAuthorityExport {
                declared,
                stripped_offsets: Vec::new(),
            });
        }
        WriterAuthorityPresenceSource::KnownMismatch => {
            return Err(failure(
                DiagnosticCategory::Integrity,
                mismatch_code,
                mismatch_message,
            ));
        }
        WriterAuthorityPresenceSource::Projected(source) => source,
        WriterAuthorityPresenceSource::Unfiltered => export.to_owned(),
    };
    let document = DocumentId::new("typebridge-live-writer-authority-probe.typeql")?;
    let (parsed, stripped_offsets) =
        released_typeql_to_declared_presence_projection_with_references(document, &presence_source)
            .map_err(|_| {
                failure(
                    DiagnosticCategory::InvalidContract,
                    "migration_typedb_export_invalid",
                    "TypeDB schema export cannot be normalized into V2 facts",
                )
            })?;
    Ok(ParsedLiveAuthorityExport {
        declared: parsed.into_declared(),
        stripped_offsets,
    })
}

enum WriterAuthorityPresenceSource {
    Absent,
    KnownMismatch,
    Projected(String),
    Unfiltered,
}

/// Retain only declarations and capabilities which can establish or directly
/// reference the requested frozen writer-authority partition. TypeDB exports may contain valid
/// definables which the portable V2 fact graph intentionally cannot represent
/// (notably structs and struct-valued attributes). Those unrelated definitions
/// must not prevent an exact fence from being observed. Blanking whole
/// irrelevant queries and individual declarations preserves source offsets for
/// extension/provenance checks on the retained facts.
fn writer_authority_presence_source(
    export: &str,
    relevant_label: fn(&str) -> bool,
) -> WriterAuthorityPresenceSource {
    let authority_tokens = authority_label_token_extents(export, relevant_label);
    if export.len() > crate::MAX_TYPEQL_SCHEMA_BYTES {
        return if authority_tokens.is_empty() {
            WriterAuthorityPresenceSource::Absent
        } else {
            WriterAuthorityPresenceSource::Unfiltered
        };
    }
    let mut extents = Vec::new();
    let mut relevant_declarations = 0_usize;
    let mut projectable_direct_labels = BTreeSet::new();
    let mut unprojectable_direct_labels = BTreeSet::new();
    let mut query_offset = 0_usize;
    while !export[query_offset..].trim().is_empty() {
        let Ok((query, consumed)) = typeql::parse_query_from(&export[query_offset..]) else {
            // The released compatibility parser accepts a few historical
            // forms the current TypeQL parser does not. With no authority
            // label in a code region, even an otherwise-unrepresentable
            // released schema proves this partition absent. If an authority
            // label is present, give the compatibility parser the original
            // source and fail closed if it also cannot classify the export.
            return if authority_tokens.is_empty() {
                WriterAuthorityPresenceSource::Absent
            } else {
                WriterAuthorityPresenceSource::Unfiltered
            };
        };
        if consumed == 0 {
            return WriterAuthorityPresenceSource::Unfiltered;
        }
        let Some(query_span) = absolute_span(query.span, query_offset) else {
            return WriterAuthorityPresenceSource::Unfiltered;
        };
        let QueryStructure::Schema(SchemaQuery::Define(define)) = query.structure else {
            return WriterAuthorityPresenceSource::Unfiltered;
        };
        let mut irrelevant = Vec::new();
        let mut query_has_relevant_declaration = false;
        for definable in define.definables {
            let (is_relevant, span) = match definable {
                Definable::TypeDeclaration(declaration) => {
                    let label = declaration.label.ident.as_str_unchecked();
                    let direct_label = relevant_label(label);
                    let Some(span) = absolute_span(declaration.span, query_offset) else {
                        return WriterAuthorityPresenceSource::Unfiltered;
                    };
                    let direct_reference = span_mentions_authority(
                        &authority_tokens,
                        span.begin_offset,
                        span.end_offset,
                    );
                    let mut declaration_extents = Vec::new();
                    let mut unprojectable_direct_value = false;
                    let mut has_builtin_direct_value = false;
                    if direct_label || direct_reference {
                        let Some(label_span) = absolute_span(declaration.label.span, query_offset)
                        else {
                            return WriterAuthorityPresenceSource::Unfiltered;
                        };
                        let mut separator_search_start = label_span.end_offset;
                        for capability in &declaration.capabilities {
                            let Some(capability_span) =
                                absolute_span(capability.span, query_offset)
                            else {
                                return WriterAuthorityPresenceSource::Unfiltered;
                            };
                            if capability_span.begin_offset < separator_search_start
                                || capability_span.end_offset > span.end_offset
                            {
                                return WriterAuthorityPresenceSource::Unfiltered;
                            }
                            let capability_mentions_authority = span_mentions_authority(
                                &authority_tokens,
                                capability_span.begin_offset,
                                capability_span.end_offset,
                            );
                            let establishes_frozen_value_type = direct_label
                                && capability_establishes_frozen_value_type(
                                    label,
                                    &capability.base,
                                );
                            let portable_direct_value = direct_label
                                && matches!(
                                    &capability.base,
                                    CapabilityBase::ValueType(value)
                                        if matches!(
                                            &value.value_type,
                                            NamedType::BuiltinValueType(_)
                                        )
                                );
                            has_builtin_direct_value |= portable_direct_value;
                            if direct_label
                                && matches!(
                                    &capability.base,
                                    CapabilityBase::ValueType(value)
                                        if matches!(&value.value_type, NamedType::Label(_))
                                )
                            {
                                unprojectable_direct_value = true;
                            } else if !capability_mentions_authority
                                && !establishes_frozen_value_type
                                && !portable_direct_value
                            {
                                let start = preceding_capability_separator(
                                    export,
                                    separator_search_start,
                                    capability_span.begin_offset,
                                )
                                .unwrap_or(capability_span.begin_offset);
                                declaration_extents.push(start..capability_span.end_offset);
                            }
                            separator_search_start = capability_span.end_offset;
                        }
                    }
                    unprojectable_direct_value |= direct_label
                        && expected_frozen_value_type(label).is_some()
                        && !has_builtin_direct_value;
                    let Some(blank_span) = definable_extent(export, span, query_span.end_offset)
                    else {
                        return WriterAuthorityPresenceSource::Unfiltered;
                    };
                    if unprojectable_direct_value {
                        unprojectable_direct_labels.insert(label.to_owned());
                        (false, Some(blank_span))
                    } else {
                        if direct_label {
                            projectable_direct_labels.insert(label.to_owned());
                        }
                        irrelevant.extend(declaration_extents);
                        (direct_label || direct_reference, Some(blank_span))
                    }
                }
                Definable::Function(function) => {
                    let Some(span) = absolute_span(function.span, query_offset) else {
                        return WriterAuthorityPresenceSource::Unfiltered;
                    };
                    (false, definable_extent(export, span, query_span.end_offset))
                }
                Definable::Struct(structure) => {
                    let Some(span) = absolute_span(structure.span, query_offset) else {
                        return WriterAuthorityPresenceSource::Unfiltered;
                    };
                    (false, definable_extent(export, span, query_span.end_offset))
                }
            };
            let Some(span) = span else {
                return WriterAuthorityPresenceSource::Unfiltered;
            };
            if is_relevant {
                query_has_relevant_declaration = true;
                relevant_declarations += 1;
            } else {
                irrelevant.push(span.begin_offset..span.end_offset);
            }
        }
        if query_has_relevant_declaration {
            extents.extend(irrelevant);
        } else {
            extents.push(query_span.begin_offset..query_span.end_offset);
        }
        let Some(next_query_offset) = query_offset.checked_add(consumed) else {
            return WriterAuthorityPresenceSource::Unfiltered;
        };
        if next_query_offset > export.len() {
            return WriterAuthorityPresenceSource::Unfiltered;
        }
        query_offset = next_query_offset;
    }
    if unprojectable_direct_labels
        .iter()
        .any(|label| !projectable_direct_labels.contains(label))
    {
        return WriterAuthorityPresenceSource::KnownMismatch;
    }
    if relevant_declarations == 0 {
        return WriterAuthorityPresenceSource::Absent;
    }
    extents.sort_unstable_by_key(|extent| extent.start);
    let mut prior_end = 0_usize;
    for extent in &extents {
        if extent.start < prior_end
            || extent.start >= extent.end
            || extent.end > export.len()
            || !export.is_char_boundary(extent.start)
            || !export.is_char_boundary(extent.end)
        {
            return WriterAuthorityPresenceSource::Unfiltered;
        }
        prior_end = extent.end;
    }
    WriterAuthorityPresenceSource::Projected(type_bridge_core_lib::parser::blank_source_extents(
        export, &extents,
    ))
}

fn absolute_span(
    span: Option<typeql::common::Span>,
    query_offset: usize,
) -> Option<typeql::common::Span> {
    let span = span?;
    Some(typeql::common::Span {
        begin_offset: query_offset.checked_add(span.begin_offset)?,
        end_offset: query_offset.checked_add(span.end_offset)?,
    })
}

fn preceding_capability_separator(source: &str, start: usize, end: usize) -> Option<usize> {
    use type_bridge_core_lib::parser::{SourceRegionKind, scan_source_regions};

    if start > end || end > source.len() {
        return None;
    }
    let between = &source[start..end];
    let mut separator = None;
    for (region, kind) in scan_source_regions(between) {
        if kind != SourceRegionKind::Code {
            continue;
        }
        if let Some(relative) = between[region.clone()].rfind(',') {
            separator = Some(start + region.start + relative);
        }
    }
    separator
}

fn definable_extent(
    source: &str,
    span: typeql::common::Span,
    query_end: usize,
) -> Option<typeql::common::Span> {
    if span.begin_offset >= span.end_offset
        || span.end_offset > query_end
        || query_end > source.len()
    {
        return None;
    }
    if source[span.begin_offset..span.end_offset]
        .trim_end()
        .ends_with(';')
    {
        return Some(span);
    }
    let terminator = first_code_semicolon(source, span.end_offset, query_end)?;
    Some(typeql::common::Span {
        begin_offset: span.begin_offset,
        end_offset: terminator + 1,
    })
}

fn first_code_semicolon(source: &str, start: usize, end: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut cursor = start;
    let mut quote = None;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;
    while cursor < end {
        let byte = bytes[cursor];
        if let Some(close) = quote {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == close {
                quote = None;
            }
            cursor += 1;
            continue;
        }
        if block_comment {
            if byte == b'*' && cursor + 1 < end && bytes[cursor + 1] == b'/' {
                block_comment = false;
                cursor += 2;
            } else {
                cursor += 1;
            }
            continue;
        }
        if line_comment {
            if byte == b'\n' || byte == b'\r' {
                line_comment = false;
            }
            cursor += 1;
            continue;
        }
        match byte {
            b';' => return Some(cursor),
            b'\'' | b'"' => {
                quote = Some(byte);
                cursor += 1;
            }
            b'#' => {
                line_comment = true;
                cursor += 1;
            }
            b'/' if cursor + 1 < end && bytes[cursor + 1] == b'/' => {
                line_comment = true;
                cursor += 2;
            }
            b'/' if cursor + 1 < end && bytes[cursor + 1] == b'*' => {
                block_comment = true;
                cursor += 2;
            }
            _ => cursor += 1,
        }
    }
    None
}

fn authority_label_token_extents(
    source: &str,
    relevant_label: fn(&str) -> bool,
) -> Vec<std::ops::Range<usize>> {
    use type_bridge_core_lib::parser::{SourceRegionKind, scan_source_regions};

    let is_label_byte = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-');
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    for (region, kind) in scan_source_regions(source) {
        if kind != SourceRegionKind::Code {
            continue;
        }
        let mut cursor = region.start;
        while cursor < region.end {
            if !is_label_byte(bytes[cursor]) {
                cursor += 1;
                continue;
            }
            let start = cursor;
            while cursor < region.end && is_label_byte(bytes[cursor]) {
                cursor += 1;
            }
            if relevant_label(&source[start..cursor]) {
                tokens.push(start..cursor);
            }
        }
    }
    tokens
}

fn span_mentions_authority(tokens: &[std::ops::Range<usize>], start: usize, end: usize) -> bool {
    let first_candidate = tokens.partition_point(|token| token.end <= start);
    tokens
        .get(first_candidate)
        .is_some_and(|token| start <= token.start && token.end <= end)
}

fn capability_establishes_frozen_value_type(label: &str, capability: &CapabilityBase) -> bool {
    let Some(expected) = expected_frozen_value_type(label) else {
        return false;
    };
    matches!(
        capability,
        CapabilityBase::ValueType(value)
            if matches!(
                &value.value_type,
                NamedType::BuiltinValueType(actual) if actual.token == expected
            )
    )
}

fn expected_frozen_value_type(label: &str) -> Option<typeql::token::ValueType> {
    match label {
        MANAGED_CONTROL_ENTITY
        | LEGACY_CUTOVER_ANCHOR_ENTITY
        | "type_bridge_migration"
        | "type_bridge_migration_run" => None,
        "migration_applied_at" | "migration_started_at" | "migration_finished_at" => {
            Some(typeql::token::ValueType::DateTime)
        }
        _ if is_managed_fence_label(label) || is_legacy_ledger_label(label) => {
            Some(typeql::token::ValueType::String)
        }
        _ => None,
    }
}

fn is_managed_fence_label(label: &str) -> bool {
    matches!(
        label,
        MANAGED_CONTROL_ENTITY
            | MANAGED_CONTROL_SCOPE
            | MANAGED_CONTROL_LEASE_HOLDER
            | MANAGED_CONTROL_LEASE_FENCE
            | MANAGED_CONTROL_LEASE_STATE
            | LEGACY_CUTOVER_ANCHOR_ENTITY
            | LEGACY_CUTOVER_ANCHOR_KEY
            | LEGACY_CUTOVER_ANCHOR_SCOPE
            | LEGACY_CUTOVER_ANCHOR_FINGERPRINT
    )
}

fn schema_mentions_labels(
    schema: &DeclaredSchema,
    predicate: fn(&Value) -> bool,
) -> Result<bool, Diagnostic> {
    for fact in schema.facts() {
        let identity = serde_json::to_value(fact.id()).map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_fact_identity_encode_failed",
                "schema fact identity cannot be inspected for reserved labels",
            )
        })?;
        if predicate(&identity) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn contains_exact_schema_subset(actual: &DeclaredSchema, expected: &DeclaredSchema) -> bool {
    expected
        .facts()
        .all(|fact| actual.fact(&fact.id()).is_some_and(|found| found == fact))
}

fn expected_facts_carry_stripped_extensions(
    actual: &DeclaredSchema,
    expected: &DeclaredSchema,
    stripped_offsets: &[usize],
) -> Result<bool, Diagnostic> {
    for fact in expected.facts() {
        let source = actual.source(&fact.id()).ok_or_else(|| {
            failure(
                DiagnosticCategory::Integrity,
                "migration_typedb_export_missing_provenance",
                "normalized TypeDB export fact has no source provenance",
            )
        })?;
        let start = usize::try_from(source.byte_start()).map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_export_invalid_provenance",
                "normalized TypeDB export provenance exceeds the host byte range",
            )
        })?;
        let end = usize::try_from(source.byte_end()).map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_export_invalid_provenance",
                "normalized TypeDB export provenance exceeds the host byte range",
            )
        })?;
        if stripped_offsets
            .iter()
            .any(|offset| start <= *offset && *offset < end)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Exact live query authority reconstructed from one schema export.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LiveQueryAuthorityState {
    managed: ManagedSchemaState,
    control_presence: LiveQueryControlPresence,
    legacy_control_present: bool,
}

impl LiveQueryAuthorityState {
    /// Borrow the reconstructed managed semantic state.
    #[must_use]
    pub const fn managed(&self) -> &ManagedSchemaState {
        &self.managed
    }

    /// Return the exact managed-control schema presence observed in the export.
    #[must_use]
    pub const fn control_presence(&self) -> LiveQueryControlPresence {
        self.control_presence
    }

    /// Return whether the released V1 migration-ledger schema is installed.
    #[must_use]
    pub const fn legacy_control_present(&self) -> bool {
        self.legacy_control_present
    }
}

/// Rebuild live schema authority for a read-only V2 query executor.
///
/// The declared schema donates only format, capability, scope, and semantic
/// profile context. Its fingerprint never substitutes for the exported facts.
/// Reserved V2 and released-ledger partitions are accepted only when complete
/// and byte-semantically equal to their frozen contracts.
pub fn rebuild_live_query_authority_state(
    document: DocumentId,
    export: &str,
    context_schema: &DeclaredSchema,
    context: &ManagedDeltaContext,
) -> Result<ManagedSchemaState, Diagnostic> {
    rebuild_live_query_authority(document, export, context_schema, context)
        .map(|authority| authority.managed)
}

/// Rebuild live schema authority together with exact control-schema presence.
pub fn rebuild_live_query_authority(
    document: DocumentId,
    export: &str,
    context_schema: &DeclaredSchema,
    context: &ManagedDeltaContext,
) -> Result<LiveQueryAuthorityState, Diagnostic> {
    let donor = managed_schema_state(context_schema, context).map_err(map_schema_error)?;
    let parsed = released_typeql_to_declared_lossless_projection_with_references(document, export)
        .map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_export_invalid",
                "TypeDB schema export cannot be normalized into V2 facts",
            )
        })?;
    reject_reserved_function_references(&parsed)?;
    let partitioned = partition_declared_schema(parsed.into_declared())?;
    let control_presence = if partitioned.internal.facts().next().is_some() {
        verify_fence_mirror_partition(&partitioned.internal)?;
        LiveQueryControlPresence::ManagedFence
    } else {
        LiveQueryControlPresence::Absent
    };
    let legacy_control_present = partitioned.legacy_control.facts().next().is_some();
    verify_legacy_control_partition(&partitioned.legacy_control)?;
    let managed =
        rebuild_candidate_state(&partitioned.user, context.available_capabilities(), &donor)?;
    Ok(LiveQueryAuthorityState {
        managed,
        control_presence,
        legacy_control_present,
    })
}

fn partition_declared_schema(full: DeclaredSchema) -> Result<QueryAuthorityPartitions, Diagnostic> {
    let mut user = Vec::new();
    let mut internal = Vec::new();
    let mut legacy_control = Vec::new();
    for fact in full.facts() {
        let id = fact.id();
        let source = full.source(&id).cloned().ok_or_else(|| {
            failure(
                DiagnosticCategory::Integrity,
                "migration_typedb_export_missing_provenance",
                "normalized TypeDB export fact has no source provenance",
            )
        })?;
        let id_value = serde_json::to_value(&id).map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_fact_identity_encode_failed",
                "schema fact identity cannot be inspected for reserved labels",
            )
        })?;
        let sourced = SourcedSchemaFact::new(fact.clone(), source);
        if value_mentions_reserved_label(&id_value) {
            internal.push(sourced);
        } else if value_mentions_legacy_control_label(&id_value) {
            legacy_control.push(sourced);
        } else {
            user.push(sourced);
        }
    }

    let user =
        DeclaredSchema::from_facts(full.format(), CapabilitySet::new(), user).map_err(|_| {
            failure(
                DiagnosticCategory::Integrity,
                "reserved_schema_cross_reference",
                "user schema facts reference the reserved TypeDB control namespace",
            )
        })?;
    let internal = DeclaredSchema::from_facts(full.format(), CapabilitySet::new(), internal)
        .map_err(|_| {
            failure(
                DiagnosticCategory::Integrity,
                "reserved_schema_cross_reference",
                "reserved TypeDB control facts reference user schema declarations",
            )
        })?;
    let legacy_control =
        DeclaredSchema::from_facts(full.format(), CapabilitySet::new(), legacy_control).map_err(
            |_| {
                failure(
                    DiagnosticCategory::Integrity,
                    "reserved_schema_cross_reference",
                    "legacy migration-ledger facts reference user schema declarations",
                )
            },
        )?;

    Ok(QueryAuthorityPartitions {
        user,
        internal,
        legacy_control,
    })
}

fn verify_fence_mirror_partition(internal: &DeclaredSchema) -> Result<(), Diagnostic> {
    let expected = frozen_fence_mirror_schema()?;
    if internal.declared_identity_fingerprint() != expected.declared_identity_fingerprint() {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_control_schema_mismatch",
            "reserved fence-mirror schema differs from the frozen contract",
        ));
    }
    Ok(())
}

fn frozen_fence_mirror_schema() -> Result<DeclaredSchema, Diagnostic> {
    let expected_document = DocumentId::new("typebridge-managed-fence-schema.typeql")?;
    typeql_to_declared(expected_document, MANAGED_FENCE_SCHEMA_TYPEQL).map_err(|_| {
        failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_frozen_schema_invalid",
            "frozen TypeDB fence-mirror schema cannot be normalized",
        )
    })
}

fn verify_legacy_control_partition(legacy_control: &DeclaredSchema) -> Result<(), Diagnostic> {
    if legacy_control.facts().next().is_none() {
        return Ok(());
    }
    let expected = frozen_legacy_ledger_schema()?;
    if legacy_control.declared_identity_fingerprint() != expected.declared_identity_fingerprint() {
        return Err(failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_legacy_ledger_mismatch",
            "legacy migration-ledger facts differ from the frozen v1 contract",
        ));
    }
    Ok(())
}

fn frozen_legacy_ledger_schema() -> Result<DeclaredSchema, Diagnostic> {
    let expected_document = DocumentId::new("typebridge-legacy-ledger-schema.typeql")?;
    typeql_to_declared(expected_document, LEGACY_LEDGER_SCHEMA_TYPEQL).map_err(|_| {
        failure(
            DiagnosticCategory::InvalidContract,
            "migration_typedb_frozen_schema_invalid",
            "frozen legacy migration-ledger schema cannot be normalized",
        )
    })
}

fn rebuild_candidate_state(
    user: &DeclaredSchema,
    available_capabilities: &CapabilitySet,
    candidate: &ManagedSchemaState,
) -> Result<ManagedSchemaState, Diagnostic> {
    let semantic_profile = candidate
        .managed_semantic_schema()
        .as_fingerprint()
        .semantic_profile()
        .ok_or_else(|| {
            failure(
                DiagnosticCategory::InvalidContract,
                "migration_typedb_candidate_profile_missing",
                "candidate managed state carries no semantic-profile identity",
            )
        })?
        .clone();
    let context = ManagedDeltaContext::new(
        candidate.scope().id().clone(),
        semantic_profile,
        available_capabilities.clone(),
    );
    let facts = user
        .facts()
        .map(|fact| {
            let id = fact.id();
            let source = user.source(&id).cloned().ok_or_else(|| {
                failure(
                    DiagnosticCategory::Integrity,
                    "migration_typedb_export_missing_provenance",
                    "normalized TypeDB export fact has no source provenance",
                )
            })?;
            Ok(SourcedSchemaFact::new(fact.clone(), source))
        })
        .collect::<Result<Vec<_>, Diagnostic>>()?;
    let declared = DeclaredSchema::from_facts(
        candidate.format(),
        candidate.required_capabilities().clone(),
        facts,
    )
    .map_err(|_| {
        failure(
            DiagnosticCategory::Integrity,
            "migration_typedb_observation_rebuild_failed",
            "live managed facts cannot form a declared schema under the candidate context",
        )
    })?;
    managed_schema_state(&declared, &context).map_err(map_schema_error)
}

fn map_schema_error(error: DeltaError) -> Diagnostic {
    match error {
        DeltaError::Contract(diagnostic) => diagnostic,
        DeltaError::Schema(diagnostics) => diagnostics
            .iter()
            .next()
            .map(|entry| entry.diagnostic().clone())
            .unwrap_or_else(|| {
                failure(
                    DiagnosticCategory::Integrity,
                    "migration_typedb_observation_rebuild_failed",
                    "live managed schema does not resolve under the candidate context",
                )
            }),
    }
}

fn value_mentions_reserved_label(value: &Value) -> bool {
    match value {
        Value::String(value) => value.starts_with(TYPEBRIDGE_INTERNAL_PREFIX),
        Value::Array(values) => values.iter().any(value_mentions_reserved_label),
        Value::Object(values) => values.values().any(value_mentions_reserved_label),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn value_mentions_legacy_control_label(value: &Value) -> bool {
    match value {
        Value::String(value) => is_legacy_ledger_label(value.as_str()),
        Value::Array(values) => values.iter().any(value_mentions_legacy_control_label),
        Value::Object(values) => values.values().any(value_mentions_legacy_control_label),
        Value::Null | Value::Bool(_) | Value::Number(_) => false,
    }
}

fn failure(category: DiagnosticCategory, code: &'static str, message: &'static str) -> Diagnostic {
    Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static diagnostic code is valid"),
        message,
    )
}

#[cfg(test)]
mod tests {
    use type_bridge_contract::capability::CapabilitySet;
    use type_bridge_contract::fingerprint::SemanticProfileId;
    use type_bridge_contract::managed_scope::ManagedScopeId;

    use super::*;

    fn context() -> ManagedDeltaContext {
        ManagedDeltaContext::new(
            ManagedScopeId::new("query-live-authority").expect("scope"),
            SemanticProfileId::new("typedb-3.12.1/v1").expect("profile"),
            CapabilitySet::new(),
        )
    }

    fn declared(source: &str) -> DeclaredSchema {
        typeql_to_declared(
            DocumentId::new("query-live-authority-declared.typeql").expect("document"),
            source,
        )
        .expect("declared schema")
    }

    #[test]
    fn exact_export_rebuilds_declared_authority() {
        let declared = declared("define entity person;");
        let expected = managed_schema_state(&declared, &context()).expect("expected authority");
        let live = rebuild_live_query_authority_state(
            DocumentId::new("query-live-authority-export.typeql").expect("document"),
            "define entity person;",
            &declared,
            &context(),
        )
        .expect("live authority");
        assert_eq!(live, expected);
    }

    #[test]
    fn rich_rebuild_reports_exact_control_presence() {
        let declared = declared("define entity person;");
        let query_only = rebuild_live_query_authority(
            DocumentId::new("query-live-authority-query-only.typeql").expect("document"),
            "define entity person;",
            &declared,
            &context(),
        )
        .expect("query-only authority");
        assert_eq!(
            query_only.control_presence(),
            LiveQueryControlPresence::Absent
        );
        assert!(!query_only.legacy_control_present());

        let managed = rebuild_live_query_authority(
            DocumentId::new("query-live-authority-managed.typeql").expect("document"),
            &format!("{MANAGED_FENCE_SCHEMA_TYPEQL}\ndefine entity person;"),
            &declared,
            &context(),
        )
        .expect("managed authority");
        assert_eq!(
            managed.control_presence(),
            LiveQueryControlPresence::ManagedFence
        );
        assert_eq!(managed.managed(), query_only.managed());
    }

    #[test]
    fn live_authority_refuses_lossy_released_projection() {
        let declared = declared("define attribute tag, value string; entity person, owns tag;");
        for export in [
            "define attribute tag, value string; entity person, owns tag[];",
            "define attribute tag, value string; entity person, owns tag @distinct;",
        ] {
            let error = rebuild_live_query_authority_state(
                DocumentId::new("query-live-authority-lossy.typeql").expect("document"),
                export,
                &declared,
                &context(),
            )
            .expect_err("unrepresentable live semantics must never be projected away");
            assert_eq!(error.code().as_str(), "migration_typedb_export_invalid");
        }
    }

    #[test]
    fn live_authority_checks_function_body_references_before_fact_partitioning() {
        let declared = declared("define entity person;");
        let export = "define\nentity person;\n\
                      fun inspect($candidate: person) -> { person }:\n\
                        match $candidate isa migration_id;\n\
                        return { $candidate };\n";
        let error = rebuild_live_query_authority_state(
            DocumentId::new("query-live-authority-function.typeql").expect("document"),
            export,
            &declared,
            &context(),
        )
        .expect_err("body-only ledger reference must fail before projection");
        assert_eq!(error.code().as_str(), "reserved_schema_cross_reference");
    }

    #[test]
    fn partial_reserved_partition_fails_closed() {
        let declared = declared("define entity person;");
        let error = rebuild_live_query_authority_state(
            DocumentId::new("query-live-authority-reserved.typeql").expect("document"),
            "define entity person; attribute typebridge-internal-v2-control-scope, value string;",
            &declared,
            &context(),
        )
        .expect_err("partial control partition");
        assert_eq!(
            error.code().as_str(),
            "migration_typedb_control_schema_mismatch"
        );
    }

    #[test]
    fn writer_fence_presence_requires_the_exact_frozen_schema() {
        assert_eq!(
            managed_fence_schema_presence("define entity person;").expect("ordinary schema"),
            LiveQueryControlPresence::Absent
        );
        assert_eq!(
            managed_fence_schema_presence(MANAGED_FENCE_SCHEMA_TYPEQL)
                .expect("canonical fence schema"),
            LiveQueryControlPresence::ManagedFence
        );
    }

    #[test]
    fn writer_fence_presence_rejects_all_label_lookalikes_without_capabilities() {
        let lookalikes = r#"define
attribute typebridge-internal-v2-control-scope, value string;
attribute typebridge-internal-v2-lease-holder, value string;
attribute typebridge-internal-v2-lease-fence, value string;
attribute typebridge-internal-v2-lease-state, value string;
attribute typebridge-internal-v2-legacy-cutover-key, value string;
attribute typebridge-internal-v2-legacy-cutover-scope, value string;
attribute typebridge-internal-v2-legacy-cutover-fingerprint, value string;
entity typebridge-internal-v2-migration-control;
entity typebridge-internal-v2-legacy-cutover;
"#;
        let error = managed_fence_schema_presence(lookalikes)
            .expect_err("labels alone must not establish managed authority");
        assert_eq!(
            error.code().as_str(),
            "migration_typedb_control_schema_mismatch"
        );
    }

    #[test]
    fn writer_fence_presence_cannot_be_reopened_by_extra_reserved_facts() {
        let export =
            format!("{MANAGED_FENCE_SCHEMA_TYPEQL}\ndefine entity typebridge-internal-v2-extra;");
        assert_eq!(
            managed_fence_schema_presence(&export).expect("canonical core remains authoritative"),
            LiveQueryControlPresence::ManagedFence
        );
    }

    #[test]
    fn writer_authority_requires_the_exact_frozen_legacy_ledger() {
        let exact = format!("{MANAGED_FENCE_SCHEMA_TYPEQL}\n{LEGACY_LEDGER_SCHEMA_TYPEQL}");
        assert_eq!(
            managed_fence_schema_presence(&exact).expect("exact managed authority"),
            LiveQueryControlPresence::ManagedFence
        );
        assert_eq!(
            legacy_ledger_schema_presence(&exact).expect("exact ledger authority"),
            LiveLegacyLedgerPresence::FrozenLedger
        );

        let lookalike = format!(
            "{MANAGED_FENCE_SCHEMA_TYPEQL}\ndefine\n\
             attribute migration_id, value string;\n\
             attribute migration_app_label, value string;\n\
             attribute migration_name, value string;\n\
             attribute migration_applied_at, value datetime;\n\
             attribute migration_checksum, value string;\n\
             entity type_bridge_migration;"
        );
        let error = legacy_ledger_schema_presence(&lookalike)
            .expect_err("legacy labels without frozen owns are not authority");
        assert_eq!(
            error.code().as_str(),
            "migration_typedb_legacy_ledger_mismatch"
        );
    }

    #[test]
    fn writer_presence_ignores_function_body_references_but_query_authority_rejects_them() {
        let export = format!(
            "{MANAGED_FENCE_SCHEMA_TYPEQL}\n{LEGACY_LEDGER_SCHEMA_TYPEQL}\n\
             define\nentity person;\n\
             fun inspect($candidate: person) -> {{ person }}:\n\
               match $candidate isa person;\n\
               $control isa typebridge-internal-v2-migration-control;\n\
               return {{ $candidate }};\n"
        );

        assert_eq!(
            managed_fence_schema_presence(&export).expect("writer presence ignores function body"),
            LiveQueryControlPresence::ManagedFence
        );
        assert_eq!(
            legacy_ledger_schema_presence(&export).expect("ledger presence ignores function body"),
            LiveLegacyLedgerPresence::FrozenLedger
        );

        let error = rebuild_live_query_authority_state(
            DocumentId::new("query-live-authority-function-reserved.typeql").expect("document"),
            &export,
            &declared("define entity person;"),
            &context(),
        )
        .expect_err("query authority must retain the reserved-reference prohibition");
        assert_eq!(error.code().as_str(), "reserved_schema_cross_reference");
    }

    #[test]
    fn writer_presence_ignores_unrelated_struct_valued_attributes() {
        let export = format!(
            "{MANAGED_FENCE_SCHEMA_TYPEQL}\n{LEGACY_LEDGER_SCHEMA_TYPEQL}\n\
             define\n\
             struct payload: field value string;\n\
             attribute payload-attr, value payload;"
        );

        assert_eq!(
            managed_fence_schema_presence(&export)
                .expect("an unrelated structured value cannot hide the managed fence"),
            LiveQueryControlPresence::ManagedFence
        );
        assert_eq!(
            legacy_ledger_schema_presence(&export)
                .expect("an unrelated structured value cannot hide the frozen ledger"),
            LiveLegacyLedgerPresence::FrozenLedger
        );
    }

    #[test]
    fn writer_presence_classifies_a_structured_frozen_attribute_as_a_mismatch() {
        let export = MANAGED_FENCE_SCHEMA_TYPEQL.replace(
            "attribute typebridge-internal-v2-control-scope, value string;",
            "struct payload: field value string;\n\
             attribute typebridge-internal-v2-control-scope, value payload;",
        );
        let error = managed_fence_schema_presence(&export)
            .expect_err("a structured frozen attribute is not the canonical fence");
        assert_eq!(
            error.code().as_str(),
            "migration_typedb_control_schema_mismatch"
        );
    }

    #[test]
    fn writer_presence_fallback_scans_only_code_label_tokens() {
        let without_code_reference = format!(
            "this is future syntax \"{MANAGED_CONTROL_ENTITY}\"; # {MANAGED_CONTROL_SCOPE}"
        );
        assert_eq!(
            managed_fence_schema_presence(&without_code_reference)
                .expect("comments and strings cannot establish managed authority"),
            LiveQueryControlPresence::Absent
        );

        let with_code_reference = format!("this is future syntax {MANAGED_CONTROL_ENTITY}");
        let error = managed_fence_schema_presence(&with_code_reference)
            .expect_err("an unclassifiable authority-label reference must fail closed");
        assert_eq!(error.code().as_str(), "migration_typedb_export_invalid");
    }

    #[test]
    fn writer_presence_oversize_fallback_preserves_unrelated_v1_schemas() {
        let plain = "x".repeat(crate::MAX_TYPEQL_SCHEMA_BYTES + 1);
        assert!(matches!(
            writer_authority_presence_source(&plain, is_managed_fence_label),
            WriterAuthorityPresenceSource::Absent
        ));

        let trivia_prefix = format!("# {MANAGED_CONTROL_ENTITY}\n\"{MANAGED_CONTROL_SCOPE}\"\n");
        let trivia = format!(
            "{trivia_prefix}{}",
            "x".repeat(crate::MAX_TYPEQL_SCHEMA_BYTES + 1 - trivia_prefix.len())
        );
        assert!(matches!(
            writer_authority_presence_source(&trivia, is_managed_fence_label),
            WriterAuthorityPresenceSource::Absent
        ));

        let authority = format!(
            "{MANAGED_CONTROL_ENTITY} {}",
            "x".repeat(crate::MAX_TYPEQL_SCHEMA_BYTES)
        );
        assert!(matches!(
            writer_authority_presence_source(&authority, is_managed_fence_label),
            WriterAuthorityPresenceSource::Unfiltered
        ));
    }

    #[test]
    fn writer_presence_retains_direct_cross_references_to_frozen_facts() {
        let export = format!(
            "{MANAGED_FENCE_SCHEMA_TYPEQL}\n\
             define entity observer, owns {MANAGED_CONTROL_SCOPE};"
        );
        assert_eq!(
            managed_fence_schema_presence(&export)
                .expect("a direct cross-reference cannot hide the canonical core"),
            LiveQueryControlPresence::ManagedFence
        );
    }

    #[test]
    fn writer_presence_projects_unrelated_capabilities_on_frozen_label_collisions() {
        let collision = format!(
            "define\n\
             struct payload: field value string;\n\
             attribute payload-attr, value payload;\n\
             entity {MANAGED_CONTROL_ENTITY}, owns payload-attr;"
        );
        let error = managed_fence_schema_presence(&collision)
            .expect_err("an incomplete reserved-label collision is not managed authority");
        assert_eq!(
            error.code().as_str(),
            "migration_typedb_control_schema_mismatch"
        );

        let canonical_with_extension = format!(
            "{MANAGED_FENCE_SCHEMA_TYPEQL}\n\
             define\n\
             struct payload: field value string;\n\
             attribute payload-attr, value payload;\n\
             entity {MANAGED_CONTROL_ENTITY}, owns payload-attr;"
        );
        assert_eq!(
            managed_fence_schema_presence(&canonical_with_extension)
                .expect("an unrelated structured extension cannot hide the canonical core"),
            LiveQueryControlPresence::ManagedFence
        );
    }

    #[test]
    fn writer_presence_reports_extensions_only_on_frozen_partition_facts() {
        let managed_extended = MANAGED_FENCE_SCHEMA_TYPEQL.replace(
            "owns typebridge-internal-v2-lease-holder @card(0..1)",
            "owns typebridge-internal-v2-lease-holder[] @distinct @card(0..1)",
        );
        assert_eq!(
            managed_fence_schema_presence(&managed_extended)
                .expect("portable core remains visible"),
            LiveQueryControlPresence::ManagedFenceWithExtensions
        );

        let ledger_extended = LEGACY_LEDGER_SCHEMA_TYPEQL.replacen(
            "owns migration_checksum;",
            "owns migration_checksum[] @distinct;",
            1,
        );
        assert_eq!(
            legacy_ledger_schema_presence(&ledger_extended)
                .expect("portable ledger remains visible"),
            LiveLegacyLedgerPresence::FrozenLedgerWithExtensions
        );

        let user_extended = format!(
            "{MANAGED_FENCE_SCHEMA_TYPEQL}\n{LEGACY_LEDGER_SCHEMA_TYPEQL}\n\
             define\nattribute tag, value string; entity person, owns tag[] @distinct;"
        );
        assert_eq!(
            managed_fence_schema_presence(&user_extended).expect("user extension is unrelated"),
            LiveQueryControlPresence::ManagedFence
        );
        assert_eq!(
            legacy_ledger_schema_presence(&user_extended).expect("user extension is unrelated"),
            LiveLegacyLedgerPresence::FrozenLedger
        );

        let extra_reserved_extended = format!(
            "{MANAGED_FENCE_SCHEMA_TYPEQL}\n\
             define\n\
             attribute typebridge-internal-v2-extra-value, value string;\n\
             entity typebridge-internal-v2-extra,\n\
               owns typebridge-internal-v2-extra-value[] @distinct;"
        );
        assert_eq!(
            managed_fence_schema_presence(&extra_reserved_extended)
                .expect("an unrelated reserved fact cannot alter the canonical core"),
            LiveQueryControlPresence::ManagedFence
        );
    }
}
