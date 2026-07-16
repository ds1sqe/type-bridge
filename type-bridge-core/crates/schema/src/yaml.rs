use std::collections::BTreeMap;

use granit_parser::{Event, Parser, Placement, ScalarStyle, Span, StructureStyle};
use type_bridge_contract::diagnostic::DiagnosticCategory;
use type_bridge_contract::schema::{
    DocumentFingerprint, DocumentId, SchemaDiagnostic, SchemaDiagnostics, SourceSpan,
};

use crate::document::{
    CommentPlacement, SchemaComment, SchemaDocument, SchemaParseLimits, YamlCollectionStyle,
    YamlMapping, YamlMappingEntry, YamlNode, YamlScalar, YamlScalarStyle, YamlSequence,
    resource_diagnostic,
};
pub(crate) use crate::diagnostic::{diagnostic, diagnostic_with_related};

#[derive(Clone, Debug)]
enum StructuralEvent {
    Scalar {
        value: String,
        raw: String,
        style: YamlScalarStyle,
        span: SourceSpan,
    },
    SequenceStart {
        style: YamlCollectionStyle,
        span: SourceSpan,
    },
    SequenceEnd(SourceSpan),
    MappingStart {
        style: YamlCollectionStyle,
        span: SourceSpan,
    },
    MappingEnd(SourceSpan),
}

impl StructuralEvent {
    fn span(&self) -> &SourceSpan {
        match self {
            Self::Scalar { span, .. }
            | Self::SequenceStart { span, .. }
            | Self::SequenceEnd(span)
            | Self::MappingStart { span, .. }
            | Self::MappingEnd(span) => span,
        }
    }
}

pub(crate) fn parse_document_with_limits(
    id: DocumentId,
    source: String,
    limits: SchemaParseLimits,
) -> Result<SchemaDocument, SchemaDiagnostics> {
    if source.len() > limits.max_document_bytes() {
        return Err(resource_diagnostic(
            "schema_document_size_limit",
            format!(
                "schema document `{}` exceeds the limit of {} bytes",
                id.as_str(),
                limits.max_document_bytes()
            ),
            None,
        ));
    }

    let fingerprint = DocumentFingerprint::compute(source.as_bytes())
        .map_err(|error| SchemaDiagnostics::one(SchemaDiagnostic::new(error, None)))?;
    let mut structural = Vec::new();
    let mut comments = Vec::new();
    let mut document_count = 0usize;

    for item in Parser::new_from_str(&source) {
        let (event, span) = item.map_err(|error| {
            diagnostic(
                DiagnosticCategory::InvalidContract,
                "malformed_yaml",
                format!("invalid YAML in `{}`: {}", id.as_str(), error.info()),
                None,
            )
        })?;
        let source_span = source_span(&id, &span)?;

        match event {
            Event::Nothing | Event::StreamStart | Event::StreamEnd | Event::DocumentEnd => {}
            Event::DocumentStart(_, version) => {
                document_count += 1;
                if document_count > 1 {
                    return Err(diagnostic(
                        DiagnosticCategory::InvalidContract,
                        "multiple_yaml_documents",
                        "a schema source must contain exactly one YAML document",
                        Some(source_span),
                    ));
                }
                if version.is_some() {
                    return Err(diagnostic(
                        DiagnosticCategory::InvalidContract,
                        "yaml_directive_not_supported",
                        "YAML directives are not supported by schema documents",
                        Some(source_span),
                    ));
                }
            }
            Event::Alias(_) => {
                return Err(diagnostic(
                    DiagnosticCategory::InvalidContract,
                    "yaml_alias_not_supported",
                    "YAML aliases are not supported by schema documents",
                    Some(source_span),
                ));
            }
            Event::Comment(text, placement) => comments.push(SchemaComment::new(
                text.into_owned(),
                comment_placement(placement),
                source_span,
            )),
            Event::Scalar(value, style, anchor, tag) => {
                reject_anchor_or_tag(anchor, tag.is_some(), source_span.clone())?;
                let raw = span.slice(&source).ok_or_else(|| {
                    diagnostic(
                        DiagnosticCategory::Integrity,
                        "yaml_source_span_unavailable",
                        "the YAML parser did not retain a stable scalar source span",
                        Some(source_span.clone()),
                    )
                })?;
                structural.push(StructuralEvent::Scalar {
                    value: value.into_owned(),
                    raw: raw.to_owned(),
                    style: scalar_style(style),
                    span: source_span,
                });
            }
            Event::SequenceStart(style, anchor, tag) => {
                reject_anchor_or_tag(anchor, tag.is_some(), source_span.clone())?;
                structural.push(StructuralEvent::SequenceStart {
                    style: collection_style(style),
                    span: source_span,
                });
            }
            Event::SequenceEnd => structural.push(StructuralEvent::SequenceEnd(source_span)),
            Event::MappingStart(style, anchor, tag) => {
                reject_anchor_or_tag(anchor, tag.is_some(), source_span.clone())?;
                structural.push(StructuralEvent::MappingStart {
                    style: collection_style(style),
                    span: source_span,
                });
            }
            Event::MappingEnd => structural.push(StructuralEvent::MappingEnd(source_span)),
        }
    }

    if document_count == 0 {
        return Err(diagnostic(
            DiagnosticCategory::InvalidContract,
            "empty_yaml_document",
            "a schema source must contain one YAML document",
            None,
        ));
    }

    let mut cursor = Cursor::new(structural, limits);
    let root = cursor.parse_node(1, false)?;
    if cursor.index != cursor.events.len() {
        return Err(diagnostic(
            DiagnosticCategory::InvalidContract,
            "trailing_yaml_content",
            "unexpected structural content follows the root YAML node",
            Some(cursor.events[cursor.index].span().clone()),
        ));
    }

    let root = match root {
        YamlNode::Mapping(mapping) => mapping,
        other => {
            return Err(diagnostic(
                DiagnosticCategory::InvalidContract,
                "yaml_root_not_mapping",
                "the schema document root must be a mapping",
                Some(other.span().clone()),
            ));
        }
    };

    Ok(SchemaDocument::new(
        id,
        source,
        fingerprint,
        root,
        comments,
    ))
}

struct Cursor {
    events: Vec<StructuralEvent>,
    index: usize,
    nodes: usize,
    limits: SchemaParseLimits,
}

impl Cursor {
    const fn new(events: Vec<StructuralEvent>, limits: SchemaParseLimits) -> Self {
        Self {
            events,
            index: 0,
            nodes: 0,
            limits,
        }
    }

    fn parse_node(
        &mut self,
        depth: usize,
        mapping_key: bool,
    ) -> Result<YamlNode, SchemaDiagnostics> {
        let current = self.events.get(self.index).cloned().ok_or_else(|| {
            diagnostic(
                DiagnosticCategory::InvalidContract,
                "unexpected_yaml_end",
                "unexpected end of YAML structure",
                None,
            )
        })?;

        if depth > self.limits.max_depth() {
            return Err(resource_diagnostic(
                "schema_yaml_depth_limit",
                format!(
                    "YAML nesting exceeds the limit of {}",
                    self.limits.max_depth()
                ),
                Some(current.span().clone()),
            ));
        }
        self.nodes = self.nodes.checked_add(1).ok_or_else(|| {
            resource_diagnostic(
                "schema_yaml_node_limit",
                "YAML node count overflowed",
                Some(current.span().clone()),
            )
        })?;
        if self.nodes > self.limits.max_nodes() {
            return Err(resource_diagnostic(
                "schema_yaml_node_limit",
                format!(
                    "YAML node count exceeds the limit of {}",
                    self.limits.max_nodes()
                ),
                Some(current.span().clone()),
            ));
        }

        match current {
            StructuralEvent::Scalar {
                value,
                raw,
                style,
                span,
            } => {
                self.index += 1;
                if raw.len() > self.limits.max_scalar_source_bytes() {
                    return Err(resource_diagnostic(
                        "schema_yaml_scalar_limit",
                        format!(
                            "YAML scalar source token exceeds the limit of {} bytes",
                            self.limits.max_scalar_source_bytes()
                        ),
                        Some(span),
                    ));
                }
                if style == YamlScalarStyle::Plain && ambiguous_plain_scalar(&value) {
                    return Err(diagnostic(
                        DiagnosticCategory::InvalidContract,
                        "ambiguous_plain_yaml_scalar",
                        format!(
                            "plain scalar `{value}` is ambiguous; quote it to preserve string semantics"
                        ),
                        Some(span),
                    ));
                }
                if mapping_key && value == "<<" {
                    return Err(diagnostic(
                        DiagnosticCategory::InvalidContract,
                        "yaml_merge_key_not_supported",
                        "YAML merge keys are not supported by schema documents",
                        Some(span),
                    ));
                }
                Ok(YamlNode::Scalar(YamlScalar::new(value, raw, style, span)))
            }
            StructuralEvent::SequenceStart { style, span } => {
                self.index += 1;
                let mut items = Vec::new();
                loop {
                    match self.events.get(self.index).cloned() {
                        Some(StructuralEvent::SequenceEnd(end)) => {
                            self.index += 1;
                            return Ok(YamlNode::Sequence(YamlSequence::new(
                                items,
                                style,
                                joined_span(&span, &end)?,
                            )));
                        }
                        Some(_) => items.push(self.parse_node(depth + 1, false)?),
                        None => {
                            return Err(diagnostic(
                                DiagnosticCategory::InvalidContract,
                                "unterminated_yaml_sequence",
                                "YAML sequence has no closing event",
                                Some(span),
                            ));
                        }
                    }
                }
            }
            StructuralEvent::MappingStart { style, span } => {
                self.index += 1;
                let mut entries = Vec::new();
                let mut keys = BTreeMap::<String, SourceSpan>::new();
                loop {
                    match self.events.get(self.index).cloned() {
                        Some(StructuralEvent::MappingEnd(end)) => {
                            self.index += 1;
                            return Ok(YamlNode::Mapping(YamlMapping::new(
                                entries,
                                style,
                                joined_span(&span, &end)?,
                            )));
                        }
                        Some(_) => {
                            let key_node = self.parse_node(depth + 1, true)?;
                            let key = match key_node {
                                YamlNode::Scalar(key) => key,
                                other => {
                                    return Err(diagnostic(
                                        DiagnosticCategory::InvalidContract,
                                        "non_string_yaml_key",
                                        "schema document mapping keys must be strings",
                                        Some(other.span().clone()),
                                    ));
                                }
                            };
                            if let Some(first) = keys.get(key.value()) {
                                return Err(diagnostic_with_related(
                                    DiagnosticCategory::InvalidContract,
                                    "duplicate_yaml_key",
                                    format!("YAML mapping key `{}` is duplicated", key.value()),
                                    key.span().clone(),
                                    first.clone(),
                                    "first key is here",
                                ));
                            }
                            keys.insert(key.value().to_owned(), key.span().clone());
                            let value = self.parse_node(depth + 1, false)?;
                            entries.push(YamlMappingEntry::new(key, value));
                        }
                        None => {
                            return Err(diagnostic(
                                DiagnosticCategory::InvalidContract,
                                "unterminated_yaml_mapping",
                                "YAML mapping has no closing event",
                                Some(span),
                            ));
                        }
                    }
                }
            }
            StructuralEvent::SequenceEnd(span) | StructuralEvent::MappingEnd(span) => {
                Err(diagnostic(
                    DiagnosticCategory::InvalidContract,
                    "unexpected_yaml_collection_end",
                    "unexpected YAML collection closing event",
                    Some(span),
                ))
            }
        }
    }
}

fn source_span(id: &DocumentId, span: &Span) -> Result<SourceSpan, SchemaDiagnostics> {
    let range = span.byte_range().ok_or_else(|| {
        diagnostic(
            DiagnosticCategory::Integrity,
            "yaml_source_span_unavailable",
            "the YAML parser did not retain stable byte offsets",
            None,
        )
    })?;
    SourceSpan::new(
        id.clone(),
        range.start as u64,
        range.end as u64,
        span.start.line() as u32,
        span.start.col() as u32 + 1,
        span.end.line() as u32,
        span.end.col() as u32 + 1,
    )
    .map_err(|error| SchemaDiagnostics::one(SchemaDiagnostic::new(error, None)))
}

fn joined_span(start: &SourceSpan, end: &SourceSpan) -> Result<SourceSpan, SchemaDiagnostics> {
    SourceSpan::new(
        start.document().clone(),
        start.byte_start(),
        end.byte_end(),
        start.line(),
        start.column(),
        end.end_line(),
        end.end_column(),
    )
    .map_err(|error| SchemaDiagnostics::one(SchemaDiagnostic::new(error, None)))
}

fn reject_anchor_or_tag(
    anchor: usize,
    has_tag: bool,
    span: SourceSpan,
) -> Result<(), SchemaDiagnostics> {
    if anchor != 0 {
        return Err(diagnostic(
            DiagnosticCategory::InvalidContract,
            "yaml_anchor_not_supported",
            "YAML anchors are not supported by schema documents",
            Some(span),
        ));
    }
    if has_tag {
        return Err(diagnostic(
            DiagnosticCategory::InvalidContract,
            "yaml_tag_not_supported",
            "YAML tags are not supported by schema documents",
            Some(span),
        ));
    }
    Ok(())
}

fn ambiguous_plain_scalar(value: &str) -> bool {
    matches_ignore_ascii_case(value, &["yes", "no", "on", "off"])
        || looks_like_iso_date_or_timestamp(value)
}

fn matches_ignore_ascii_case(value: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| value.eq_ignore_ascii_case(candidate))
}

fn looks_like_iso_date_or_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 10
        || !bytes[..4].iter().all(u8::is_ascii_digit)
        || bytes[4] != b'-'
        || !bytes[5..7].iter().all(u8::is_ascii_digit)
        || bytes[7] != b'-'
        || !bytes[8..10].iter().all(u8::is_ascii_digit)
    {
        return false;
    }
    bytes.len() == 10 || matches!(bytes[10], b'T' | b't' | b' ')
}

fn scalar_style(style: ScalarStyle) -> YamlScalarStyle {
    match style {
        ScalarStyle::Plain => YamlScalarStyle::Plain,
        ScalarStyle::SingleQuoted => YamlScalarStyle::SingleQuoted,
        ScalarStyle::DoubleQuoted => YamlScalarStyle::DoubleQuoted,
        ScalarStyle::Literal => YamlScalarStyle::Literal,
        ScalarStyle::Folded => YamlScalarStyle::Folded,
    }
}

fn collection_style(style: StructureStyle) -> YamlCollectionStyle {
    match style {
        StructureStyle::Block => YamlCollectionStyle::Block,
        StructureStyle::Flow => YamlCollectionStyle::Flow,
    }
}

fn comment_placement(placement: Placement) -> CommentPlacement {
    match placement {
        Placement::Above => CommentPlacement::Above,
        Placement::Right => CommentPlacement::Right,
        Placement::Free => CommentPlacement::Free,
        Placement::Last => CommentPlacement::Last,
    }
}
