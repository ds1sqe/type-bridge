use std::path::{Path, PathBuf};

use type_bridge_contract::diagnostic::DiagnosticCategory;
use type_bridge_contract::schema::{DocumentFingerprint, DocumentId, SchemaDiagnostics};

use crate::diagnostic::diagnostic;
use crate::{SchemaComment, SchemaDocument, SchemaParseLimits, YamlMapping, YamlNode};

/// The only supported schema-set manifest format in V1.
pub const SCHEMA_SET_V1_FORMAT: &str = "typebridge.schema-set/v1";
/// The frozen Phase 2 source-discovery algorithm identifier.
pub const SCHEMA_DISCOVERY_V1: &str = "typebridge.schema-discovery/v1";

/// A closed identifier for the source-discovery algorithm used by a snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaDiscoveryVersion;

impl SchemaDiscoveryVersion {
    /// Returns the frozen V1 discovery identifier.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        SCHEMA_DISCOVERY_V1
    }
}

/// Validated semantic fields from a `typebridge.schema-set/v1` manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaSetManifest {
    sources: Vec<String>,
}

impl SchemaSetManifest {
    /// Returns the exact supported format identifier.
    #[must_use]
    pub const fn format(&self) -> &'static str {
        SCHEMA_SET_V1_FORMAT
    }

    /// Returns source patterns in manifest order and spelling.
    #[must_use]
    pub fn sources(&self) -> &[String] {
        &self.sources
    }
}

/// A validated schema-set manifest retaining its exact source presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaSetManifestDocument {
    path: PathBuf,
    document: SchemaDocument,
    manifest: SchemaSetManifest,
}

impl SchemaSetManifestDocument {
    pub(crate) fn parse(
        path: PathBuf,
        source: String,
        limits: SchemaParseLimits,
    ) -> Result<Self, SchemaDiagnostics> {
        let document_id = DocumentId::new("schema-set-manifest").map_err(|error| {
            diagnostic(
                DiagnosticCategory::Integrity,
                "invalid_schema_manifest_document_id",
                error.message(),
                None,
            )
        })?;
        let document = match SchemaDocument::parse_with_limits(document_id, source, limits) {
            Err(error)
                if error.iter().next().is_some_and(|item| {
                    item.diagnostic().code().as_str() == "yaml_root_not_mapping"
                }) =>
            {
                let primary = error.iter().next().and_then(|item| item.primary()).cloned();
                return Err(diagnostic(
                    DiagnosticCategory::InvalidContract,
                    "schema_set_root_not_mapping",
                    "the schema-set manifest root must be a mapping",
                    primary,
                ));
            }
            Err(error) => return Err(error),
            Ok(document) => document,
        };
        let manifest = parse_manifest(document.root())?;
        Ok(Self {
            path,
            document,
            manifest,
        })
    }

    /// Returns the canonical manifest path captured by discovery.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns the source exactly as captured.
    #[must_use]
    pub fn source(&self) -> &str {
        self.document.source()
    }

    /// Returns the exact-source document fingerprint.
    #[must_use]
    pub const fn fingerprint(&self) -> &DocumentFingerprint {
        self.document.fingerprint()
    }

    /// Returns the retained lossless YAML root.
    #[must_use]
    pub const fn root(&self) -> &YamlMapping {
        self.document.root()
    }

    /// Returns retained comments in source order.
    #[must_use]
    pub fn comments(&self) -> &[SchemaComment] {
        self.document.comments()
    }

    /// Returns the validated manifest fields.
    #[must_use]
    pub const fn manifest(&self) -> &SchemaSetManifest {
        &self.manifest
    }

    /// Returns source patterns in manifest order and spelling.
    #[must_use]
    pub fn sources(&self) -> &[String] {
        self.manifest.sources()
    }
}

fn parse_manifest(root: &YamlMapping) -> Result<SchemaSetManifest, SchemaDiagnostics> {
    let mut format: Option<&YamlNode> = None;
    let mut sources: Option<&YamlNode> = None;
    for entry in root.entries() {
        match entry.key().value() {
            "format" => format = Some(entry.value()),
            "sources" => sources = Some(entry.value()),
            key => {
                return Err(diagnostic(
                    DiagnosticCategory::InvalidContract,
                    "unknown_schema_set_key",
                    format!("schema-set manifest key `{key}` is not supported in V1"),
                    Some(entry.key().span().clone()),
                ));
            }
        }
    }

    let format = format.ok_or_else(|| {
        diagnostic(
            DiagnosticCategory::InvalidContract,
            "schema_set_format_missing",
            "schema-set manifest requires `format`",
            Some(root.span().clone()),
        )
    })?;
    let format = format.as_scalar().ok_or_else(|| {
        diagnostic(
            DiagnosticCategory::UnsupportedCapability,
            "unsupported_schema_set_format",
            "schema-set manifest format must be `typebridge.schema-set/v1`",
            Some(format.span().clone()),
        )
    })?;
    if format.value() != SCHEMA_SET_V1_FORMAT {
        return Err(diagnostic(
            DiagnosticCategory::UnsupportedCapability,
            "unsupported_schema_set_format",
            format!(
                "schema-set manifest format `{}` is not supported",
                format.value()
            ),
            Some(format.span().clone()),
        ));
    }

    let sources = sources.ok_or_else(|| {
        diagnostic(
            DiagnosticCategory::InvalidContract,
            "schema_set_sources_missing",
            "schema-set manifest requires `sources`",
            Some(root.span().clone()),
        )
    })?;
    let sequence = sources.as_sequence().ok_or_else(|| {
        diagnostic(
            DiagnosticCategory::InvalidContract,
            "schema_set_sources_not_sequence",
            "schema-set manifest `sources` must be a sequence",
            Some(sources.span().clone()),
        )
    })?;
    if sequence.items().is_empty() {
        return Err(diagnostic(
            DiagnosticCategory::InvalidContract,
            "empty_schema_source_patterns",
            "schema-set manifest must select at least one source pattern",
            Some(sequence.span().clone()),
        ));
    }
    let mut parsed_sources = Vec::with_capacity(sequence.items().len());
    for item in sequence.items() {
        let scalar = item.as_scalar().ok_or_else(|| {
            diagnostic(
                DiagnosticCategory::InvalidContract,
                "schema_set_source_not_string",
                "every schema-set source pattern must be a string",
                Some(item.span().clone()),
            )
        })?;
        parsed_sources.push(scalar.value().to_owned());
    }
    Ok(SchemaSetManifest {
        sources: parsed_sources,
    })
}
