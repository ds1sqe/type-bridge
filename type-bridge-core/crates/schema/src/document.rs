use std::collections::BTreeMap;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::schema::{
    DocumentFingerprint, DocumentId, SchemaDiagnostic, SchemaDiagnostics,
    SchemaDocumentSetFingerprint, SourceSpan,
};

use crate::schema_set::SchemaSetManifestDocument;

/// Default maximum number of documents in one schema document set.
pub const DEFAULT_MAX_DOCUMENTS: usize = 4_096;
/// Default maximum aggregate UTF-8 source bytes in one document set.
pub const DEFAULT_MAX_AGGREGATE_BYTES: usize = 64 * 1024 * 1024;
/// Default maximum UTF-8 source bytes in one document.
pub const DEFAULT_MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
/// Default maximum YAML node nesting depth.
pub const DEFAULT_MAX_DEPTH: usize = 64;
/// Default maximum YAML nodes in one document.
pub const DEFAULT_MAX_NODES: usize = 65_536;
/// Default maximum source-token bytes in one YAML scalar.
///
/// Canonical decoded strings have their own lower contract bound. The parser
/// permits quoting and escape overhead up to the already-bounded document
/// source ceiling.
pub const DEFAULT_MAX_SCALAR_SOURCE_BYTES: usize = DEFAULT_MAX_DOCUMENT_BYTES;

/// Resource ceilings applied before a schema document becomes trusted input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaParseLimits {
    max_documents: usize,
    max_aggregate_bytes: usize,
    max_document_bytes: usize,
    max_depth: usize,
    max_nodes: usize,
    max_scalar_source_bytes: usize,
}

impl SchemaParseLimits {
    /// Creates explicit parser ceilings.
    #[must_use]
    pub const fn new(
        max_documents: usize,
        max_aggregate_bytes: usize,
        max_document_bytes: usize,
        max_depth: usize,
        max_nodes: usize,
        max_scalar_source_bytes: usize,
    ) -> Self {
        Self {
            max_documents,
            max_aggregate_bytes,
            max_document_bytes,
            max_depth,
            max_nodes,
            max_scalar_source_bytes,
        }
    }

    /// Returns the document-count ceiling.
    #[must_use]
    pub const fn max_documents(self) -> usize {
        self.max_documents
    }

    /// Returns the aggregate-source-byte ceiling.
    #[must_use]
    pub const fn max_aggregate_bytes(self) -> usize {
        self.max_aggregate_bytes
    }

    /// Returns the per-document source-byte ceiling.
    #[must_use]
    pub const fn max_document_bytes(self) -> usize {
        self.max_document_bytes
    }

    /// Returns the node-depth ceiling.
    #[must_use]
    pub const fn max_depth(self) -> usize {
        self.max_depth
    }

    /// Returns the per-document node-count ceiling.
    #[must_use]
    pub const fn max_nodes(self) -> usize {
        self.max_nodes
    }

    /// Returns the YAML scalar source-token byte ceiling.
    #[must_use]
    pub const fn max_scalar_source_bytes(self) -> usize {
        self.max_scalar_source_bytes
    }

    /// Returns the YAML scalar source-token byte ceiling.
    ///
    /// This compatibility spelling preserves callers of the original parser
    /// limit API; the limit applies to exact YAML source bytes, not decoded
    /// canonical string bytes.
    #[must_use]
    pub const fn max_scalar_bytes(self) -> usize {
        self.max_scalar_source_bytes()
    }
}

impl Default for SchemaParseLimits {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_DOCUMENTS,
            DEFAULT_MAX_AGGREGATE_BYTES,
            DEFAULT_MAX_DOCUMENT_BYTES,
            DEFAULT_MAX_DEPTH,
            DEFAULT_MAX_NODES,
            DEFAULT_MAX_SCALAR_SOURCE_BYTES,
        )
    }
}

/// YAML scalar spelling retained by the lossless document layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum YamlScalarStyle {
    /// Unquoted scalar.
    Plain,
    /// Single-quoted scalar.
    SingleQuoted,
    /// Double-quoted scalar.
    DoubleQuoted,
    /// Literal block scalar.
    Literal,
    /// Folded block scalar.
    Folded,
}

/// YAML collection spelling retained by the lossless document layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum YamlCollectionStyle {
    /// Indentation-delimited collection.
    Block,
    /// Bracket-delimited collection.
    Flow,
}

/// Placement reported for a source comment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommentPlacement {
    /// Comment immediately above the associated syntax.
    Above,
    /// Comment to the right of syntax on the same line.
    Right,
    /// Free-standing comment.
    Free,
    /// Comment after the last item in a collection.
    Last,
}

/// A comment retained with its exact source location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaComment {
    text: String,
    placement: CommentPlacement,
    span: SourceSpan,
}

impl SchemaComment {
    pub(crate) fn new(text: String, placement: CommentPlacement, span: SourceSpan) -> Self {
        Self {
            text,
            placement,
            span,
        }
    }

    /// Returns the decoded comment text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the parser-reported placement.
    #[must_use]
    pub const fn placement(&self) -> CommentPlacement {
        self.placement
    }

    /// Returns the exact source span.
    #[must_use]
    pub const fn span(&self) -> &SourceSpan {
        &self.span
    }
}

/// A decoded YAML scalar with its exact spelling and source location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct YamlScalar {
    value: String,
    raw: String,
    style: YamlScalarStyle,
    span: SourceSpan,
}

impl YamlScalar {
    pub(crate) fn new(
        value: String,
        raw: String,
        style: YamlScalarStyle,
        span: SourceSpan,
    ) -> Self {
        Self {
            value,
            raw,
            style,
            span,
        }
    }

    /// Returns the parser-decoded scalar value.
    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Returns the exact scalar spelling from the source document.
    #[must_use]
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// Returns the scalar style.
    #[must_use]
    pub const fn style(&self) -> YamlScalarStyle {
        self.style
    }

    /// Returns the exact source span.
    #[must_use]
    pub const fn span(&self) -> &SourceSpan {
        &self.span
    }
}

/// One string-keyed YAML mapping entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct YamlMappingEntry {
    key: YamlScalar,
    value: YamlNode,
}

impl YamlMappingEntry {
    pub(crate) const fn new(key: YamlScalar, value: YamlNode) -> Self {
        Self { key, value }
    }

    /// Returns the string key.
    #[must_use]
    pub const fn key(&self) -> &YamlScalar {
        &self.key
    }

    /// Returns the mapped node.
    #[must_use]
    pub const fn value(&self) -> &YamlNode {
        &self.value
    }
}

/// A lossless, insertion-ordered YAML mapping.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct YamlMapping {
    entries: Vec<YamlMappingEntry>,
    style: YamlCollectionStyle,
    span: SourceSpan,
}

impl YamlMapping {
    pub(crate) const fn new(
        entries: Vec<YamlMappingEntry>,
        style: YamlCollectionStyle,
        span: SourceSpan,
    ) -> Self {
        Self {
            entries,
            style,
            span,
        }
    }

    /// Returns entries in source order.
    #[must_use]
    pub fn entries(&self) -> &[YamlMappingEntry] {
        &self.entries
    }

    /// Returns the collection style.
    #[must_use]
    pub const fn style(&self) -> YamlCollectionStyle {
        self.style
    }

    /// Returns the collection source span.
    #[must_use]
    pub const fn span(&self) -> &SourceSpan {
        &self.span
    }
}

/// A lossless YAML sequence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct YamlSequence {
    items: Vec<YamlNode>,
    style: YamlCollectionStyle,
    span: SourceSpan,
}

impl YamlSequence {
    pub(crate) const fn new(
        items: Vec<YamlNode>,
        style: YamlCollectionStyle,
        span: SourceSpan,
    ) -> Self {
        Self { items, style, span }
    }

    /// Returns items in source order.
    #[must_use]
    pub fn items(&self) -> &[YamlNode] {
        &self.items
    }

    /// Returns the collection style.
    #[must_use]
    pub const fn style(&self) -> YamlCollectionStyle {
        self.style
    }

    /// Returns the collection source span.
    #[must_use]
    pub const fn span(&self) -> &SourceSpan {
        &self.span
    }
}

/// A YAML node accepted by the closed schema-document grammar.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum YamlNode {
    /// Scalar node.
    Scalar(YamlScalar),
    /// Sequence node.
    Sequence(YamlSequence),
    /// Mapping node.
    Mapping(YamlMapping),
}

impl YamlNode {
    /// Returns the node source span.
    #[must_use]
    pub const fn span(&self) -> &SourceSpan {
        match self {
            Self::Scalar(value) => value.span(),
            Self::Sequence(value) => value.span(),
            Self::Mapping(value) => value.span(),
        }
    }

    /// Returns this node as a mapping, if it is one.
    #[must_use]
    pub const fn as_mapping(&self) -> Option<&YamlMapping> {
        match self {
            Self::Mapping(value) => Some(value),
            Self::Scalar(_) | Self::Sequence(_) => None,
        }
    }

    /// Returns this node as a sequence, if it is one.
    #[must_use]
    pub const fn as_sequence(&self) -> Option<&YamlSequence> {
        match self {
            Self::Sequence(value) => Some(value),
            Self::Scalar(_) | Self::Mapping(_) => None,
        }
    }

    /// Returns this node as a scalar, if it is one.
    #[must_use]
    pub const fn as_scalar(&self) -> Option<&YamlScalar> {
        match self {
            Self::Scalar(value) => Some(value),
            Self::Sequence(_) | Self::Mapping(_) => None,
        }
    }
}

/// One parsed schema source document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaDocument {
    id: DocumentId,
    source: String,
    fingerprint: DocumentFingerprint,
    root: YamlMapping,
    comments: Vec<SchemaComment>,
}

impl SchemaDocument {
    pub(crate) const fn new(
        id: DocumentId,
        source: String,
        fingerprint: DocumentFingerprint,
        root: YamlMapping,
        comments: Vec<SchemaComment>,
    ) -> Self {
        Self {
            id,
            source,
            fingerprint,
            root,
            comments,
        }
    }

    /// Parses one document with the default resource ceilings.
    pub fn parse(id: DocumentId, source: impl Into<String>) -> Result<Self, SchemaDiagnostics> {
        Self::parse_with_limits(id, source, SchemaParseLimits::default())
    }

    /// Parses one document with explicit resource ceilings.
    pub fn parse_with_limits(
        id: DocumentId,
        source: impl Into<String>,
        limits: SchemaParseLimits,
    ) -> Result<Self, SchemaDiagnostics> {
        crate::yaml::parse_document_with_limits(id, source.into(), limits)
    }

    /// Returns the stable document identifier.
    #[must_use]
    pub const fn id(&self) -> &DocumentId {
        &self.id
    }

    /// Returns the source exactly as supplied by the caller.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Returns the source-byte fingerprint.
    #[must_use]
    pub const fn fingerprint(&self) -> &DocumentFingerprint {
        &self.fingerprint
    }

    /// Returns the required root mapping.
    #[must_use]
    pub const fn root(&self) -> &YamlMapping {
        &self.root
    }

    /// Returns all comments in source order.
    #[must_use]
    pub fn comments(&self) -> &[SchemaComment] {
        &self.comments
    }
}

/// A deterministic, identifier-keyed collection of schema documents.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SchemaDocumentSet {
    documents: BTreeMap<DocumentId, SchemaDocument>,
    manifest: Option<SchemaSetManifestDocument>,
}

impl SchemaDocumentSet {
    /// Parses documents with default resource ceilings.
    pub fn parse<I, S>(sources: I) -> Result<Self, SchemaDiagnostics>
    where
        I: IntoIterator<Item = (DocumentId, S)>,
        S: Into<String>,
    {
        Self::parse_with_limits(sources, SchemaParseLimits::default())
    }

    /// Parses documents with explicit per-document and aggregate ceilings.
    pub fn parse_with_limits<I, S>(
        sources: I,
        limits: SchemaParseLimits,
    ) -> Result<Self, SchemaDiagnostics>
    where
        I: IntoIterator<Item = (DocumentId, S)>,
        S: Into<String>,
    {
        let mut documents: BTreeMap<DocumentId, SchemaDocument> = BTreeMap::new();
        let mut aggregate_bytes = 0usize;

        for (id, source) in sources {
            if documents.len() >= limits.max_documents() {
                return Err(resource_diagnostic(
                    "schema_document_count_limit",
                    format!(
                        "schema document count exceeds the limit of {}",
                        limits.max_documents()
                    ),
                    None,
                ));
            }

            let source = source.into();
            aggregate_bytes = aggregate_bytes.checked_add(source.len()).ok_or_else(|| {
                resource_diagnostic(
                    "schema_aggregate_size_limit",
                    "schema aggregate source size overflowed",
                    None,
                )
            })?;
            if aggregate_bytes > limits.max_aggregate_bytes() {
                return Err(resource_diagnostic(
                    "schema_aggregate_size_limit",
                    format!(
                        "schema aggregate source size exceeds the limit of {} bytes",
                        limits.max_aggregate_bytes()
                    ),
                    None,
                ));
            }

            if let Some(existing) = documents.get(&id) {
                return Err(crate::yaml::diagnostic_with_related(
                    DiagnosticCategory::InvalidContract,
                    "duplicate_schema_document",
                    format!("schema document identifier `{}` is duplicated", id.as_str()),
                    existing.root().span().clone(),
                    existing.root().span().clone(),
                    "first document with this identifier",
                ));
            }

            let document = SchemaDocument::parse_with_limits(id.clone(), source, limits)?;
            documents.insert(id, document);
        }

        Ok(Self {
            documents,
            manifest: None,
        })
    }

    pub(crate) fn attach_manifest(&mut self, manifest: SchemaSetManifestDocument) {
        self.manifest = Some(manifest);
    }

    /// Returns the schema-set manifest retained by file-backed loading, if any.
    #[must_use]
    pub const fn manifest(&self) -> Option<&SchemaSetManifestDocument> {
        self.manifest.as_ref()
    }

    /// Fingerprints ordered portable document paths and exact source fingerprints.
    pub fn fingerprint(&self) -> Result<SchemaDocumentSetFingerprint, SchemaDiagnostics> {
        let mut canonical = Vec::new();
        canonical.extend_from_slice(
            &u64::try_from(self.documents.len())
                .expect("schema document count is bounded below u64::MAX")
                .to_be_bytes(),
        );
        for (id, document) in &self.documents {
            canonical.extend_from_slice(
                &u64::try_from(id.as_str().len())
                    .expect("document identifier length is bounded below u64::MAX")
                    .to_be_bytes(),
            );
            canonical.extend_from_slice(id.as_str().as_bytes());
            canonical.extend_from_slice(&document.fingerprint().as_fingerprint().digest().bytes());
        }
        SchemaDocumentSetFingerprint::compute(&canonical)
            .map_err(|error| SchemaDiagnostics::one(SchemaDiagnostic::new(error, None)))
    }

    /// Returns the number of documents.
    #[must_use]
    pub fn len(&self) -> usize {
        self.documents.len()
    }

    /// Reports whether the set has no documents.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }

    /// Returns a document by stable identifier.
    #[must_use]
    pub fn get(&self, id: &DocumentId) -> Option<&SchemaDocument> {
        self.documents.get(id)
    }

    /// Iterates documents in stable identifier order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&DocumentId, &SchemaDocument)> {
        self.documents.iter()
    }
}

pub(crate) fn resource_diagnostic(
    code: &'static str,
    message: impl Into<String>,
    primary: Option<SourceSpan>,
) -> SchemaDiagnostics {
    let diagnostic = Diagnostic::new(
        DiagnosticCategory::ResourceLimit,
        DiagnosticCode::new(code).expect("static schema diagnostic code is valid"),
        message,
    );
    SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, primary))
}
