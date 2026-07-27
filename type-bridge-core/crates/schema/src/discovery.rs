use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use type_bridge_contract::diagnostic::{Diagnostic, DiagnosticCategory, DiagnosticCode};
use type_bridge_contract::schema::{
    DocumentFingerprint, DocumentId, MAX_DOCUMENT_ID_BYTES, SchemaDiagnostic, SchemaDiagnostics,
    SchemaDocumentSetFingerprint,
};
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;

use crate::schema_set::{SchemaDiscoveryVersion, SchemaSetManifestDocument};
use crate::source_pattern::{
    PatternSegment, ValidatedSourcePattern as PortablePattern, validate_source_pattern,
};
use crate::{SchemaDocumentSet, SchemaParseLimits};

/// Default maximum number of source patterns in one schema-set manifest.
pub const DEFAULT_MAX_SOURCE_PATTERNS: usize = 4_096;
/// Default maximum UTF-8 bytes in one portable source pattern.
pub const DEFAULT_MAX_SOURCE_PATTERN_BYTES: usize = MAX_DOCUMENT_ID_BYTES;
/// Default maximum filesystem entries inspected during one selection.
pub const DEFAULT_MAX_DISCOVERY_ENTRIES: usize = 65_536;
/// Default maximum directory depth traversed below the schema root.
pub const DEFAULT_MAX_DISCOVERY_DEPTH: usize = 64;

/// An unavailable or inconsistent observation from a schema source service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaSourceServiceError;

impl fmt::Display for SchemaSourceServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("schema source observation is unavailable")
    }
}

impl Error for SchemaSourceServiceError {}

/// The non-following or following kind observed for one source path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchemaSourceKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link observed without following it.
    Symlink,
    /// Any other filesystem-like object.
    Other,
}

/// One service-defined stable object identity.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SchemaSourceIdentity(String);

impl SchemaSourceIdentity {
    /// Creates an opaque identity token whose equality is meaningful to the service.
    pub fn new(value: impl Into<String>) -> Result<Self, SchemaSourceServiceError> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_DOCUMENT_ID_BYTES {
            return Err(SchemaSourceServiceError);
        }
        Ok(Self(value))
    }
}

/// One service-defined content or metadata revision token.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaSourceRevision(String);

impl SchemaSourceRevision {
    /// Creates an opaque revision token whose equality is meaningful to the service.
    pub fn new(value: impl Into<String>) -> Result<Self, SchemaSourceServiceError> {
        let value = value.into();
        if value.is_empty() || value.len() > MAX_DOCUMENT_ID_BYTES {
            return Err(SchemaSourceServiceError);
        }
        Ok(Self(value))
    }
}

/// One point-in-time source-path observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaSourceObservation {
    identity: SchemaSourceIdentity,
    revision: SchemaSourceRevision,
    len: u64,
    kind: SchemaSourceKind,
}

impl SchemaSourceObservation {
    /// Creates one observation supplied by an injected source service.
    #[must_use]
    pub const fn new(
        identity: SchemaSourceIdentity,
        revision: SchemaSourceRevision,
        len: u64,
        kind: SchemaSourceKind,
    ) -> Self {
        Self {
            identity,
            revision,
            len,
            kind,
        }
    }

    /// Returns the service-defined stable identity.
    #[must_use]
    pub const fn identity(&self) -> &SchemaSourceIdentity {
        &self.identity
    }

    /// Returns the service-defined revision token.
    #[must_use]
    pub const fn revision(&self) -> &SchemaSourceRevision {
        &self.revision
    }

    /// Returns the observed byte length.
    #[must_use]
    pub const fn len(&self) -> u64 {
        self.len
    }

    /// Reports whether the observed object has zero bytes.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the observed object kind.
    #[must_use]
    pub const fn kind(&self) -> SchemaSourceKind {
        self.kind
    }
}

/// One bounded file capture with observations from before and after the read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaSourceCapture {
    bytes: Vec<u8>,
    before: SchemaSourceObservation,
    after: SchemaSourceObservation,
}

impl SchemaSourceCapture {
    /// Creates a bounded capture. The discovery algorithm rechecks every claim.
    #[must_use]
    pub const fn new(
        bytes: Vec<u8>,
        before: SchemaSourceObservation,
        after: SchemaSourceObservation,
    ) -> Self {
        Self {
            bytes,
            before,
            after,
        }
    }

    /// Returns the exact captured bytes.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the observation immediately before capture.
    #[must_use]
    pub const fn before(&self) -> &SchemaSourceObservation {
        &self.before
    }

    /// Returns the observation immediately after capture.
    #[must_use]
    pub const fn after(&self) -> &SchemaSourceObservation {
        &self.after
    }
}

/// Bounded environmental observations used by deterministic schema discovery.
///
/// Implementations supply raw observations only. The shared loader retains
/// confinement, matching, alias/collision rejection, reselection, parsing, and
/// evidence construction.
pub trait SchemaSourceService {
    /// Resolves one path to its canonical physical path.
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, SchemaSourceServiceError>;

    /// Observes one path while following symbolic links.
    fn metadata(&self, path: &Path) -> Result<SchemaSourceObservation, SchemaSourceServiceError>;

    /// Observes one path without following its final symbolic link.
    fn symlink_metadata(
        &self,
        path: &Path,
    ) -> Result<SchemaSourceObservation, SchemaSourceServiceError>;

    /// Returns direct entry names in ascending platform byte order.
    fn read_directory_names(&self, path: &Path) -> Result<Vec<OsString>, SchemaSourceServiceError>;

    /// Captures at most `maximum_bytes + 1` bytes with before/after observations.
    fn capture_file(
        &self,
        path: &Path,
        maximum_bytes: usize,
    ) -> Result<SchemaSourceCapture, SchemaSourceServiceError>;
}

/// Zero-sized adapter for the host filesystem.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemSchemaSourceService;

impl SchemaSourceService for SystemSchemaSourceService {
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, SchemaSourceServiceError> {
        fs::canonicalize(path).map_err(|_| SchemaSourceServiceError)
    }

    fn metadata(&self, path: &Path) -> Result<SchemaSourceObservation, SchemaSourceServiceError> {
        observation(
            &fs::metadata(path).map_err(|_| SchemaSourceServiceError)?,
            path,
        )
    }

    fn symlink_metadata(
        &self,
        path: &Path,
    ) -> Result<SchemaSourceObservation, SchemaSourceServiceError> {
        observation(
            &fs::symlink_metadata(path).map_err(|_| SchemaSourceServiceError)?,
            path,
        )
    }

    fn read_directory_names(&self, path: &Path) -> Result<Vec<OsString>, SchemaSourceServiceError> {
        let mut names = fs::read_dir(path)
            .map_err(|_| SchemaSourceServiceError)?
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name())
                    .map_err(|_| SchemaSourceServiceError)
            })
            .collect::<Result<Vec<_>, _>>()?;
        names.sort();
        Ok(names)
    }

    fn capture_file(
        &self,
        path: &Path,
        maximum_bytes: usize,
    ) -> Result<SchemaSourceCapture, SchemaSourceServiceError> {
        let mut file = File::open(path).map_err(|_| SchemaSourceServiceError)?;
        let before = observation(
            &file.metadata().map_err(|_| SchemaSourceServiceError)?,
            path,
        )?;
        let read_limit = u64::try_from(maximum_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let mut bytes = Vec::new();
        (&mut file)
            .take(read_limit)
            .read_to_end(&mut bytes)
            .map_err(|_| SchemaSourceServiceError)?;
        let after = observation(
            &file.metadata().map_err(|_| SchemaSourceServiceError)?,
            path,
        )?;
        Ok(SchemaSourceCapture::new(bytes, before, after))
    }
}

/// Resource ceilings for deterministic schema source discovery and parsing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchemaDiscoveryLimits {
    parse_limits: SchemaParseLimits,
    max_patterns: usize,
    max_pattern_bytes: usize,
    max_entries: usize,
    max_depth: usize,
}

impl SchemaDiscoveryLimits {
    /// Creates explicit discovery and parser ceilings.
    #[must_use]
    pub const fn new(
        parse_limits: SchemaParseLimits,
        max_patterns: usize,
        max_pattern_bytes: usize,
        max_entries: usize,
        max_depth: usize,
    ) -> Self {
        Self {
            parse_limits,
            max_patterns,
            max_pattern_bytes,
            max_entries,
            max_depth,
        }
    }

    /// Returns the limits applied to captured document bytes and YAML parsing.
    #[must_use]
    pub const fn parse_limits(self) -> SchemaParseLimits {
        self.parse_limits
    }

    /// Returns the source-pattern count ceiling.
    #[must_use]
    pub const fn max_patterns(self) -> usize {
        self.max_patterns
    }

    /// Returns the per-pattern UTF-8 byte ceiling.
    #[must_use]
    pub const fn max_pattern_bytes(self) -> usize {
        self.max_pattern_bytes
    }

    /// Returns the filesystem-entry inspection ceiling.
    #[must_use]
    pub const fn max_entries(self) -> usize {
        self.max_entries
    }

    /// Returns the directory traversal-depth ceiling.
    #[must_use]
    pub const fn max_depth(self) -> usize {
        self.max_depth
    }
}

impl Default for SchemaDiscoveryLimits {
    fn default() -> Self {
        Self::new(
            SchemaParseLimits::default(),
            DEFAULT_MAX_SOURCE_PATTERNS,
            DEFAULT_MAX_SOURCE_PATTERN_BYTES,
            DEFAULT_MAX_DISCOVERY_ENTRIES,
            DEFAULT_MAX_DISCOVERY_DEPTH,
        )
    }
}

/// An immutable set of captured schema source bytes parsed after revalidation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaPatternDiscoverySnapshot {
    root: PathBuf,
    manifest: PathBuf,
    documents: SchemaDocumentSet,
}

impl SchemaPatternDiscoverySnapshot {
    /// Returns the canonical schema root used for confinement.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the canonical schema-set manifest path.
    #[must_use]
    pub fn manifest(&self) -> &Path {
        &self.manifest
    }

    /// Returns the captured, fingerprinted document set.
    #[must_use]
    pub const fn documents(&self) -> &SchemaDocumentSet {
        &self.documents
    }

    /// Consumes the snapshot and returns its captured document set.
    #[must_use]
    pub fn into_documents(self) -> SchemaDocumentSet {
        self.documents
    }
}

/// One portable path and exact-source digest captured by schema discovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaSourceEvidence {
    path: DocumentId,
    fingerprint: DocumentFingerprint,
}

impl SchemaSourceEvidence {
    /// Returns the canonical schema-root-relative portable path.
    #[must_use]
    pub const fn path(&self) -> &DocumentId {
        &self.path
    }

    /// Returns the exact-source document fingerprint.
    #[must_use]
    pub const fn fingerprint(&self) -> &DocumentFingerprint {
        &self.fingerprint
    }
}

/// Reproducible Phase 2 input for the later workspace lock producer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaDiscoveryEvidence {
    discovery_version: SchemaDiscoveryVersion,
    manifest_fingerprint: DocumentFingerprint,
    sources: Vec<SchemaSourceEvidence>,
    document_set_fingerprint: SchemaDocumentSetFingerprint,
}

impl SchemaDiscoveryEvidence {
    /// Returns the frozen discovery algorithm version.
    #[must_use]
    pub const fn discovery_version(&self) -> &SchemaDiscoveryVersion {
        &self.discovery_version
    }

    /// Returns the exact manifest-source fingerprint.
    #[must_use]
    pub const fn manifest_fingerprint(&self) -> &DocumentFingerprint {
        &self.manifest_fingerprint
    }

    /// Returns source paths and fingerprints in canonical path order.
    #[must_use]
    pub fn sources(&self) -> &[SchemaSourceEvidence] {
        &self.sources
    }

    /// Returns the aggregate document-set fingerprint.
    #[must_use]
    pub const fn document_set_fingerprint(&self) -> &SchemaDocumentSetFingerprint {
        &self.document_set_fingerprint
    }
}

/// One atomically captured schema-set manifest and its selected fragments.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchemaDiscoverySnapshot {
    root: PathBuf,
    manifest: SchemaSetManifestDocument,
    documents: SchemaDocumentSet,
    discovery_version: SchemaDiscoveryVersion,
    evidence: SchemaDiscoveryEvidence,
}

impl SchemaDiscoverySnapshot {
    /// Returns the canonical schema root used for confinement.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the exact parsed manifest captured with the selected fragments.
    #[must_use]
    pub const fn manifest(&self) -> &SchemaSetManifestDocument {
        &self.manifest
    }

    /// Returns the captured, fingerprinted document set.
    #[must_use]
    pub const fn documents(&self) -> &SchemaDocumentSet {
        &self.documents
    }

    /// Returns the frozen source-discovery algorithm version.
    #[must_use]
    pub const fn discovery_version(&self) -> &SchemaDiscoveryVersion {
        &self.discovery_version
    }

    /// Returns lock-producer evidence containing no absolute host paths.
    #[must_use]
    pub const fn evidence(&self) -> &SchemaDiscoveryEvidence {
        &self.evidence
    }

    /// Consumes the snapshot and returns its manifest-associated document set.
    #[must_use]
    pub fn into_documents(self) -> SchemaDocumentSet {
        self.documents
    }
}

/// Discovers and freezes schema documents with default resource ceilings.
pub fn discover_schema_documents<I, S>(
    manifest: impl AsRef<Path>,
    patterns: I,
) -> Result<SchemaPatternDiscoverySnapshot, SchemaDiagnostics>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    discover_schema_documents_with_limits(manifest, patterns, SchemaDiscoveryLimits::default())
}

/// Discovers and freezes schema documents with explicit resource ceilings.
pub fn discover_schema_documents_with_limits<I, S>(
    manifest: impl AsRef<Path>,
    patterns: I,
    limits: SchemaDiscoveryLimits,
) -> Result<SchemaPatternDiscoverySnapshot, SchemaDiagnostics>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    discover_schema_documents_with_source(manifest, patterns, limits, &SystemSchemaSourceService)
}

fn discover_schema_documents_with_source<I, P, S>(
    manifest: impl AsRef<Path>,
    patterns: I,
    limits: SchemaDiscoveryLimits,
    source: &S,
) -> Result<SchemaPatternDiscoverySnapshot, SchemaDiagnostics>
where
    I: IntoIterator<Item = P>,
    P: Into<String>,
    S: SchemaSourceService + ?Sized,
{
    let manifest_input = manifest.as_ref();
    let manifest_parent = manifest_input.parent().ok_or_else(|| {
        failure(
            DiagnosticCategory::InvalidContract,
            "schema_manifest_has_no_root",
            "schema-set manifest must have a containing directory",
            [("manifest", display_path(manifest_input))],
        )
    })?;
    let root = canonicalize_path(source, manifest_parent, "schema_root_unavailable")?;
    let canonical_manifest =
        canonicalize_path(source, manifest_input, "schema_manifest_unavailable")?;
    if !canonical_manifest.starts_with(&root) {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "schema_manifest_root_escape",
            "schema-set manifest resolves outside its schema root",
            [("manifest", display_path(manifest_input))],
        ));
    }
    let manifest_metadata = metadata(source, &canonical_manifest, "schema_manifest_unavailable")?;
    if manifest_metadata.kind != SchemaSourceKind::File {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "schema_manifest_not_regular",
            "schema-set manifest must resolve to a regular file",
            [("manifest", display_path(manifest_input))],
        ));
    }
    let manifest_state = PathState::capture(source, manifest_input, &canonical_manifest)?;

    let patterns = validate_patterns(patterns, limits)?;
    if patterns.is_empty() {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "empty_schema_source_patterns",
            "schema-set manifest must select at least one source pattern",
            std::iter::empty::<(&str, String)>(),
        ));
    }

    let selected = select_sources(source, &root, &canonical_manifest, &patterns, limits)?;
    let captured = capture_sources(source, &selected, limits.parse_limits())?;

    if !manifest_state.matches(source, manifest_input, &canonical_manifest) {
        return Err(snapshot_changed(
            "schema-set manifest changed during source discovery",
        ));
    }
    let reselected = select_sources(source, &root, &canonical_manifest, &patterns, limits)
        .map_err(|_| snapshot_changed("schema source selection changed during discovery"))?;
    reject_snapshot_change(&selected, &reselected)?;
    if !manifest_state.matches(source, manifest_input, &canonical_manifest) {
        return Err(snapshot_changed(
            "schema-set manifest changed while source selection was revalidated",
        ));
    }
    revalidate_captured_sources(source, &reselected, &captured, limits.parse_limits())?;

    let documents = SchemaDocumentSet::parse_with_limits(captured, limits.parse_limits())?;
    Ok(SchemaPatternDiscoverySnapshot {
        root,
        manifest: canonical_manifest,
        documents,
    })
}

/// Loads a strict schema-set manifest and atomically freezes all selected documents.
pub fn load_schema_set(
    manifest: impl AsRef<Path>,
) -> Result<SchemaDiscoverySnapshot, SchemaDiagnostics> {
    load_schema_set_with_limits(manifest, SchemaDiscoveryLimits::default())
}

/// Loads a strict schema-set manifest with explicit discovery and parser ceilings.
pub fn load_schema_set_with_limits(
    manifest: impl AsRef<Path>,
    limits: SchemaDiscoveryLimits,
) -> Result<SchemaDiscoverySnapshot, SchemaDiagnostics> {
    load_schema_set_with_source(manifest, &SystemSchemaSourceService, limits)
}

/// Loads a schema set through an injected bounded source-observation service.
pub fn load_schema_set_with_source<S>(
    manifest: impl AsRef<Path>,
    source: &S,
    limits: SchemaDiscoveryLimits,
) -> Result<SchemaDiscoverySnapshot, SchemaDiagnostics>
where
    S: SchemaSourceService + ?Sized,
{
    let manifest_input = manifest.as_ref();
    let manifest_parent = manifest_input.parent().ok_or_else(|| {
        failure(
            DiagnosticCategory::InvalidContract,
            "schema_manifest_has_no_root",
            "schema-set manifest must have a containing directory",
            [("manifest", display_path(manifest_input))],
        )
    })?;
    let root = canonicalize_path(source, manifest_parent, "schema_root_unavailable")?;
    let canonical_manifest =
        canonicalize_path(source, manifest_input, "schema_manifest_unavailable")?;
    if !canonical_manifest.starts_with(&root) {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "schema_manifest_root_escape",
            "schema-set manifest resolves outside its schema root",
            [("manifest", display_path(manifest_input))],
        ));
    }
    let manifest_metadata = metadata(source, &canonical_manifest, "schema_manifest_unavailable")?;
    if manifest_metadata.kind != SchemaSourceKind::File {
        return Err(failure(
            DiagnosticCategory::InvalidContract,
            "schema_manifest_not_regular",
            "schema-set manifest must resolve to a regular file",
            [("manifest", display_path(manifest_input))],
        ));
    }
    let manifest_state = PathState::capture(source, manifest_input, &canonical_manifest)?;
    let manifest_source = capture_manifest_source(
        source,
        manifest_input,
        &canonical_manifest,
        &manifest_state,
        limits.parse_limits(),
    )?;
    if !manifest_state.matches(source, manifest_input, &canonical_manifest) {
        return Err(snapshot_changed(
            "schema-set manifest changed while it was parsed",
        ));
    }
    let manifest_document = SchemaSetManifestDocument::parse(
        canonical_manifest.clone(),
        manifest_source,
        limits.parse_limits(),
    )?;
    let patterns = validate_patterns(manifest_document.sources().iter().cloned(), limits)?;

    let selected = select_sources(source, &root, &canonical_manifest, &patterns, limits)?;
    let captured = capture_sources(source, &selected, limits.parse_limits())?;
    if !manifest_state.matches(source, manifest_input, &canonical_manifest) {
        return Err(snapshot_changed(
            "schema-set manifest changed during source discovery",
        ));
    }
    let reselected = select_sources(source, &root, &canonical_manifest, &patterns, limits)
        .map_err(|_| snapshot_changed("schema source selection changed during discovery"))?;
    reject_snapshot_change(&selected, &reselected)?;
    if !manifest_state.matches(source, manifest_input, &canonical_manifest) {
        return Err(snapshot_changed(
            "schema-set manifest changed while source selection was revalidated",
        ));
    }
    revalidate_captured_sources(source, &reselected, &captured, limits.parse_limits())?;

    let mut documents = SchemaDocumentSet::parse_with_limits(captured, limits.parse_limits())?;
    let document_set_fingerprint = documents.fingerprint()?;
    let sources = documents
        .iter()
        .map(|(path, document)| SchemaSourceEvidence {
            path: path.clone(),
            fingerprint: document.fingerprint().clone(),
        })
        .collect();
    let discovery_version = SchemaDiscoveryVersion;
    let evidence = SchemaDiscoveryEvidence {
        discovery_version: discovery_version.clone(),
        manifest_fingerprint: manifest_document.fingerprint().clone(),
        sources,
        document_set_fingerprint,
    };
    documents.attach_manifest(manifest_document.clone());
    Ok(SchemaDiscoverySnapshot {
        root,
        manifest: manifest_document,
        documents,
        discovery_version,
        evidence,
    })
}

fn validate_patterns<I, S>(
    patterns: I,
    limits: SchemaDiscoveryLimits,
) -> Result<Vec<PortablePattern>, SchemaDiagnostics>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut validated = Vec::new();
    for value in patterns {
        if validated.len() >= limits.max_patterns() {
            return Err(resource_failure(
                "schema_source_pattern_count_limit",
                "schema source pattern count exceeds its configured limit",
                [("maximum", limits.max_patterns().to_string())],
            ));
        }
        validated.push(
            validate_source_pattern(value.into(), limits.max_pattern_bytes()).map_err(
                |diagnostic| SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, None)),
            )?,
        );
    }
    Ok(validated)
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum CandidateKind {
    File(PathBuf),
    Directory,
    NonRegular,
    RootEscape,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Candidate {
    lexical: PathBuf,
    raw_portable: String,
    portable: String,
    kind: CandidateKind,
}

fn select_sources<S: SchemaSourceService + ?Sized>(
    source: &S,
    root: &Path,
    manifest: &Path,
    patterns: &[PortablePattern],
    limits: SchemaDiscoveryLimits,
) -> Result<Vec<SelectedSource>, SchemaDiagnostics> {
    let mut candidates = Vec::new();
    let mut inspected = 0usize;
    let mut ancestors = vec![root.to_owned()];
    walk_directory(
        source,
        root,
        root,
        &[],
        0,
        limits,
        &mut inspected,
        &mut ancestors,
        &mut candidates,
    )?;
    candidates.sort_by(|left, right| {
        left.portable
            .cmp(&right.portable)
            .then_with(|| left.raw_portable.cmp(&right.raw_portable))
    });

    let manifest_identity = metadata(source, manifest, "schema_manifest_unavailable")?.identity;
    let mut selected = Vec::new();
    let mut ownership: BTreeMap<String, String> = BTreeMap::new();

    for pattern in patterns {
        let mut matched = false;
        for candidate in &candidates {
            if !pattern_matches(&pattern.segments, &candidate.portable) {
                continue;
            }
            matched = true;
            if !candidate.portable.ends_with(".yaml") {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "schema_source_not_yaml",
                    "schema source patterns may select only lowercase .yaml files",
                    [
                        ("pattern", pattern.original.clone()),
                        ("path", candidate.portable.clone()),
                    ],
                ));
            }
            let canonical = match &candidate.kind {
                CandidateKind::File(canonical) => canonical,
                CandidateKind::RootEscape => {
                    return Err(failure(
                        DiagnosticCategory::InvalidContract,
                        "schema_source_symlink_escape",
                        "schema source resolves outside the canonical schema root",
                        [("path", candidate.portable.clone())],
                    ));
                }
                CandidateKind::Directory
                | CandidateKind::NonRegular
                | CandidateKind::Unavailable => {
                    return Err(failure(
                        DiagnosticCategory::InvalidContract,
                        "schema_source_not_regular",
                        "schema source must resolve to a regular file",
                        [("path", candidate.portable.clone())],
                    ));
                }
            };
            let owner_key = candidate.raw_portable.clone();
            if let Some(first_pattern) = ownership.get(&owner_key) {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "overlapping_schema_source_patterns",
                    "schema source is selected by more than one pattern",
                    [
                        ("path", candidate.portable.clone()),
                        ("first_pattern", first_pattern.clone()),
                        ("second_pattern", pattern.original.clone()),
                    ],
                ));
            }
            ownership.insert(owner_key, pattern.original.clone());
            let state = PathState::capture(source, &candidate.lexical, canonical)?;
            if state.target.identity == manifest_identity {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "schema_manifest_selected_as_source",
                    "schema source pattern may not select the schema-set manifest",
                    [("path", candidate.portable.clone())],
                ));
            }
            selected.push(SelectedSource {
                lexical: candidate.lexical.clone(),
                raw_portable: candidate.raw_portable.clone(),
                portable: candidate.portable.clone(),
                canonical: canonical.clone(),
                state,
            });
        }
        if !matched {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "empty_schema_source_pattern",
                "every schema source pattern must match at least one source",
                [("pattern", pattern.original.clone())],
            ));
        }
    }

    if selected.len() > limits.parse_limits().max_documents() {
        return Err(resource_failure(
            "schema_document_count_limit",
            "discovered schema document count exceeds its configured limit",
            [
                ("actual", selected.len().to_string()),
                ("maximum", limits.parse_limits().max_documents().to_string()),
            ],
        ));
    }

    selected.sort_by(|left, right| left.portable.cmp(&right.portable));
    reject_path_collisions(&selected)?;
    reject_file_aliases(&selected)?;
    Ok(selected)
}

#[allow(clippy::too_many_arguments)]
fn walk_directory<S: SchemaSourceService + ?Sized>(
    source: &S,
    root: &Path,
    physical_directory: &Path,
    relative_segments: &[String],
    depth: usize,
    limits: SchemaDiscoveryLimits,
    inspected: &mut usize,
    ancestors: &mut Vec<PathBuf>,
    candidates: &mut Vec<Candidate>,
) -> Result<(), SchemaDiagnostics> {
    if depth > limits.max_depth() {
        return Err(resource_failure(
            "schema_discovery_depth_limit",
            "schema source discovery exceeds its configured directory depth",
            [("maximum", limits.max_depth().to_string())],
        ));
    }
    let mut entries = source
        .read_directory_names(physical_directory)
        .map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "schema_directory_unavailable",
                "schema source directory cannot be read",
                [("path", display_path(physical_directory))],
            )
        })?;
    entries.sort();

    for file_name in entries {
        *inspected = inspected.checked_add(1).ok_or_else(|| {
            resource_failure(
                "schema_discovery_entry_limit",
                "schema source discovery entry count overflowed",
                std::iter::empty::<(&str, String)>(),
            )
        })?;
        if *inspected > limits.max_entries() {
            return Err(resource_failure(
                "schema_discovery_entry_limit",
                "schema source discovery exceeds its configured entry limit",
                [("maximum", limits.max_entries().to_string())],
            ));
        }

        let file_name = file_name.into_string().map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "schema_source_path_not_utf8",
                "schema source paths must be valid UTF-8",
                std::iter::empty::<(&str, String)>(),
            )
        })?;
        let mut raw_segments = relative_segments.to_vec();
        raw_segments.push(file_name.clone());
        let raw_portable = raw_segments.join("/");
        let normalized_segments = raw_segments
            .iter()
            .map(|segment| segment.nfc().collect::<String>())
            .collect::<Vec<_>>();
        let portable = normalized_segments.join("/");
        let lexical = root.join(raw_segments.iter().collect::<PathBuf>());
        let canonical = source.canonicalize(&lexical);
        let kind = match canonical {
            Ok(canonical) if !canonical.starts_with(root) => CandidateKind::RootEscape,
            Ok(canonical) => match source.metadata(&canonical) {
                Ok(value) if value.kind == SchemaSourceKind::File => CandidateKind::File(canonical),
                Ok(value) if value.kind == SchemaSourceKind::Directory => CandidateKind::Directory,
                Ok(_) => CandidateKind::NonRegular,
                Err(_) => CandidateKind::Unavailable,
            },
            Err(_) => CandidateKind::Unavailable,
        };
        candidates.push(Candidate {
            lexical: lexical.clone(),
            raw_portable,
            portable,
            kind: kind.clone(),
        });

        if matches!(kind, CandidateKind::RootEscape) {
            let link_metadata = source.symlink_metadata(&lexical).ok();
            if link_metadata
                .as_ref()
                .is_some_and(|metadata| metadata.kind == SchemaSourceKind::Symlink)
            {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "schema_source_symlink_escape",
                    "schema source directory resolves outside the canonical schema root",
                    [("path", normalized_segments.join("/"))],
                ));
            }
        }
        if !matches!(kind, CandidateKind::Directory) {
            continue;
        }
        let canonical_directory =
            canonicalize_path(source, &lexical, "schema_directory_unavailable")?;
        if ancestors.contains(&canonical_directory) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "schema_source_symlink_cycle",
                "schema source directory symlink forms a cycle",
                [("path", normalized_segments.join("/"))],
            ));
        }
        ancestors.push(canonical_directory.clone());
        walk_directory(
            source,
            root,
            &canonical_directory,
            &raw_segments,
            depth + 1,
            limits,
            inspected,
            ancestors,
            candidates,
        )?;
        ancestors.pop();
    }
    Ok(())
}

fn pattern_matches(pattern: &[PatternSegment], path: &str) -> bool {
    let path = path.split('/').collect::<Vec<_>>();
    let mut table = vec![vec![false; path.len() + 1]; pattern.len() + 1];
    table[0][0] = true;
    for pattern_index in 1..=pattern.len() {
        match &pattern[pattern_index - 1] {
            PatternSegment::Recursive => {
                for path_index in 0..=path.len() {
                    table[pattern_index][path_index] = table[pattern_index - 1][path_index]
                        || (path_index > 0 && table[pattern_index][path_index - 1]);
                }
            }
            PatternSegment::Component(component) => {
                for path_index in 1..=path.len() {
                    table[pattern_index][path_index] = table[pattern_index - 1][path_index - 1]
                        && component_matches(component, path[path_index - 1]);
                }
            }
        }
    }
    table[pattern.len()][path.len()]
}

fn component_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let value = value.chars().collect::<Vec<_>>();
    let mut previous = vec![false; value.len() + 1];
    previous[0] = true;
    for token in pattern {
        let mut current = vec![false; value.len() + 1];
        if token == '*' {
            current[0] = previous[0];
        }
        for index in 1..=value.len() {
            current[index] = match token {
                '*' => previous[index] || current[index - 1],
                '?' => previous[index - 1],
                literal => previous[index - 1] && literal == value[index - 1],
            };
        }
        previous = current;
    }
    previous[value.len()]
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SelectedSource {
    lexical: PathBuf,
    raw_portable: String,
    portable: String,
    canonical: PathBuf,
    state: PathState,
}

fn reject_path_collisions(selected: &[SelectedSource]) -> Result<(), SchemaDiagnostics> {
    let mut collisions: BTreeMap<String, String> = BTreeMap::new();
    for source in selected {
        let key = source.portable.case_fold().nfc().collect::<String>();
        if let Some(first) = collisions.get(&key) {
            if first != &source.raw_portable {
                return Err(failure(
                    DiagnosticCategory::InvalidContract,
                    "schema_source_path_collision",
                    "schema sources collide after NFC normalization or Unicode case-folding",
                    [
                        ("first_path", first.clone()),
                        ("second_path", source.raw_portable.clone()),
                    ],
                ));
            }
        } else {
            collisions.insert(key, source.raw_portable.clone());
        }
    }
    for source in selected {
        if source.raw_portable != source.portable {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "schema_source_path_not_nfc",
                "schema source paths must use NFC spelling",
                [("path", source.raw_portable.clone())],
            ));
        }
    }
    Ok(())
}

fn reject_file_aliases(selected: &[SelectedSource]) -> Result<(), SchemaDiagnostics> {
    let mut identities: BTreeMap<SchemaSourceIdentity, String> = BTreeMap::new();
    for source in selected {
        if let Some(first) = identities.get(&source.state.target.identity) {
            return Err(failure(
                DiagnosticCategory::InvalidContract,
                "schema_source_file_alias",
                "multiple schema source paths resolve to the same file identity",
                [
                    ("first_path", first.clone()),
                    ("second_path", source.portable.clone()),
                ],
            ));
        }
        identities.insert(
            source.state.target.identity.clone(),
            source.portable.clone(),
        );
    }
    Ok(())
}

fn capture_sources<S: SchemaSourceService + ?Sized>(
    service: &S,
    selected: &[SelectedSource],
    limits: SchemaParseLimits,
) -> Result<Vec<(DocumentId, String)>, SchemaDiagnostics> {
    let mut captured = Vec::with_capacity(selected.len());
    let mut aggregate_bytes = 0usize;
    for source in selected {
        if !source
            .state
            .matches(service, &source.lexical, &source.canonical)
        {
            return Err(snapshot_changed("schema source changed before it was read"));
        }
        if source.state.target.len > limits.max_document_bytes() as u64 {
            return Err(resource_failure(
                "schema_document_size_limit",
                "schema source exceeds its configured byte limit",
                [
                    ("path", source.portable.clone()),
                    ("maximum_bytes", limits.max_document_bytes().to_string()),
                ],
            ));
        }

        let SchemaSourceCapture {
            bytes,
            before,
            after,
        } = service
            .capture_file(&source.canonical, limits.max_document_bytes())
            .map_err(|_| snapshot_changed("schema source became unavailable while being read"))?;
        if before != source.state.target {
            return Err(snapshot_changed(
                "schema source identity changed before it was read",
            ));
        }
        if bytes.len() > limits.max_document_bytes() {
            return Err(resource_failure(
                "schema_document_size_limit",
                "schema source exceeds its configured byte limit",
                [
                    ("path", source.portable.clone()),
                    ("maximum_bytes", limits.max_document_bytes().to_string()),
                ],
            ));
        }
        if before != after
            || !source
                .state
                .matches(service, &source.lexical, &source.canonical)
        {
            return Err(snapshot_changed("schema source changed while it was read"));
        }

        aggregate_bytes = aggregate_bytes.checked_add(bytes.len()).ok_or_else(|| {
            resource_failure(
                "schema_aggregate_size_limit",
                "schema aggregate source size overflowed",
                std::iter::empty::<(&str, String)>(),
            )
        })?;
        if aggregate_bytes > limits.max_aggregate_bytes() {
            return Err(resource_failure(
                "schema_aggregate_size_limit",
                "schema aggregate source size exceeds its configured limit",
                [("maximum_bytes", limits.max_aggregate_bytes().to_string())],
            ));
        }
        let source_text = String::from_utf8(bytes).map_err(|_| {
            failure(
                DiagnosticCategory::InvalidContract,
                "schema_source_not_utf8",
                "schema source content must be valid UTF-8",
                [("path", source.portable.clone())],
            )
        })?;
        let document = DocumentId::new(source.portable.clone()).map_err(|diagnostic| {
            SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, None))
        })?;
        captured.push((document, source_text));
    }
    Ok(captured)
}

fn revalidate_captured_sources<S: SchemaSourceService + ?Sized>(
    source: &S,
    selected: &[SelectedSource],
    captured: &[(DocumentId, String)],
    limits: SchemaParseLimits,
) -> Result<(), SchemaDiagnostics> {
    let current = capture_sources(source, selected, limits)
        .map_err(|_| snapshot_changed("schema source content changed before parsing"))?;
    if current == captured {
        Ok(())
    } else {
        Err(snapshot_changed(
            "schema source content changed before parsing",
        ))
    }
}

#[cfg(test)]
mod content_integrity_tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    struct TempDirectory(PathBuf);

    impl TempDirectory {
        fn new() -> Self {
            let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "type-bridge-schema-content-integrity-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(path.join("fragments")).expect("create test schema directory");
            fs::write(
                path.join("schema.yaml"),
                "format: typebridge.schema-set/v1\n",
            )
            .expect("write test manifest");
            fs::write(path.join("fragments/a.yaml"), "root: a\n")
                .expect("write initial schema source");
            Self(path)
        }
    }

    impl Drop for TempDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn discovery_revalidation_detects_same_length_content_replacement() {
        let directory = TempDirectory::new();
        let root = fs::canonicalize(&directory.0).expect("canonicalize test root");
        let manifest =
            fs::canonicalize(directory.0.join("schema.yaml")).expect("canonicalize test manifest");
        let limits = SchemaDiscoveryLimits::default();
        let service = SystemSchemaSourceService;
        let patterns =
            validate_patterns(["fragments/a.yaml"], limits).expect("validate test source pattern");
        let mut selected = select_sources(&service, &root, &manifest, &patterns, limits)
            .expect("select initial schema source");
        let captured = capture_sources(&service, &selected, limits.parse_limits())
            .expect("capture initial schema source");

        fs::write(directory.0.join("fragments/a.yaml"), "root: b\n")
            .expect("replace schema source with same-length content");
        assert_eq!(captured[0].1.len(), "root: b\n".len());

        selected[0].state =
            PathState::capture(&service, &selected[0].lexical, &selected[0].canonical)
                .expect("neutralize metadata detection to exercise the content guard");
        let error =
            revalidate_captured_sources(&service, &selected, &captured, limits.parse_limits())
                .expect_err("same-length content replacement must fail discovery");

        assert_eq!(
            error
                .iter()
                .next()
                .expect("one integrity diagnostic")
                .diagnostic()
                .code()
                .as_str(),
            "schema_discovery_snapshot_changed",
        );
    }
}

fn capture_manifest_source<S: SchemaSourceService + ?Sized>(
    source: &S,
    lexical: &Path,
    canonical: &Path,
    state: &PathState,
    limits: SchemaParseLimits,
) -> Result<String, SchemaDiagnostics> {
    if !state.matches(source, lexical, canonical) {
        return Err(snapshot_changed(
            "schema-set manifest changed before it was read",
        ));
    }
    if state.target.len > limits.max_document_bytes() as u64 {
        return Err(resource_failure(
            "schema_manifest_size_limit",
            "schema-set manifest exceeds its configured byte limit",
            [("maximum_bytes", limits.max_document_bytes().to_string())],
        ));
    }
    let SchemaSourceCapture {
        bytes,
        before,
        after,
    } = source
        .capture_file(canonical, limits.max_document_bytes())
        .map_err(|_| snapshot_changed("schema-set manifest became unavailable while being read"))?;
    if before != state.target {
        return Err(snapshot_changed(
            "schema-set manifest identity changed before it was read",
        ));
    }
    if bytes.len() > limits.max_document_bytes() {
        return Err(resource_failure(
            "schema_manifest_size_limit",
            "schema-set manifest exceeds its configured byte limit",
            [("maximum_bytes", limits.max_document_bytes().to_string())],
        ));
    }
    if before != after || !state.matches(source, lexical, canonical) {
        return Err(snapshot_changed(
            "schema-set manifest changed while it was read",
        ));
    }
    String::from_utf8(bytes).map_err(|_| {
        failure(
            DiagnosticCategory::InvalidContract,
            "schema_manifest_not_utf8",
            "schema-set manifest content must be valid UTF-8",
            std::iter::empty::<(&str, String)>(),
        )
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PathState {
    lexical: SchemaSourceObservation,
    target: SchemaSourceObservation,
}

impl PathState {
    fn capture<S: SchemaSourceService + ?Sized>(
        source: &S,
        lexical: &Path,
        canonical: &Path,
    ) -> Result<Self, SchemaDiagnostics> {
        let lexical_metadata = source
            .symlink_metadata(lexical)
            .map_err(|_| snapshot_changed("schema source path metadata became unavailable"))?;
        let target_metadata = metadata(source, canonical, "schema_source_unavailable")?;
        Ok(Self {
            lexical: lexical_metadata,
            target: target_metadata,
        })
    }

    fn matches<S: SchemaSourceService + ?Sized>(
        &self,
        source: &S,
        lexical: &Path,
        canonical: &Path,
    ) -> bool {
        let Ok(current_canonical) = source.canonicalize(lexical) else {
            return false;
        };
        if current_canonical != canonical {
            return false;
        }
        let Ok(lexical_stamp) = source.symlink_metadata(lexical) else {
            return false;
        };
        let Ok(target_stamp) = source.metadata(canonical) else {
            return false;
        };
        self.lexical == lexical_stamp && self.target == target_stamp
    }
}

fn observation(
    metadata: &Metadata,
    path: &Path,
) -> Result<SchemaSourceObservation, SchemaSourceServiceError> {
    let modified = metadata.modified().map_err(|_| SchemaSourceServiceError)?;
    let revision = match modified.duration_since(UNIX_EPOCH) {
        Ok(duration) => format!("after:{}:{}", duration.as_secs(), duration.subsec_nanos()),
        Err(error) => {
            let duration = error.duration();
            format!("before:{}:{}", duration.as_secs(), duration.subsec_nanos())
        }
    };
    let kind = if metadata.is_file() {
        SchemaSourceKind::File
    } else if metadata.is_dir() {
        SchemaSourceKind::Directory
    } else if metadata.is_symlink() {
        SchemaSourceKind::Symlink
    } else {
        SchemaSourceKind::Other
    };
    Ok(SchemaSourceObservation::new(
        system_identity(metadata, path)?,
        SchemaSourceRevision::new(revision)?,
        metadata.len(),
        kind,
    ))
}

#[cfg(unix)]
fn system_identity(
    metadata: &Metadata,
    _path: &Path,
) -> Result<SchemaSourceIdentity, SchemaSourceServiceError> {
    use std::os::unix::fs::MetadataExt;
    SchemaSourceIdentity::new(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn system_identity(
    _metadata: &Metadata,
    path: &Path,
) -> Result<SchemaSourceIdentity, SchemaSourceServiceError> {
    SchemaSourceIdentity::new(path.to_string_lossy())
}

fn reject_snapshot_change<T: PartialEq>(
    before: &[T],
    after: &[T],
) -> Result<(), SchemaDiagnostics> {
    if before == after {
        Ok(())
    } else {
        Err(snapshot_changed(
            "schema source selection changed during discovery",
        ))
    }
}

fn canonicalize_path<S: SchemaSourceService + ?Sized>(
    source: &S,
    path: &Path,
    code: &'static str,
) -> Result<PathBuf, SchemaDiagnostics> {
    source.canonicalize(path).map_err(|_| {
        failure(
            DiagnosticCategory::InvalidContract,
            code,
            "schema discovery path cannot be resolved",
            [("path", display_path(path))],
        )
    })
}

fn metadata<S: SchemaSourceService + ?Sized>(
    source: &S,
    path: &Path,
    code: &'static str,
) -> Result<SchemaSourceObservation, SchemaDiagnostics> {
    source.metadata(path).map_err(|_| {
        failure(
            DiagnosticCategory::InvalidContract,
            code,
            "schema discovery path metadata cannot be read",
            [("path", display_path(path))],
        )
    })
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn snapshot_changed(message: &'static str) -> SchemaDiagnostics {
    failure(
        DiagnosticCategory::Integrity,
        "schema_discovery_snapshot_changed",
        message,
        std::iter::empty::<(&str, String)>(),
    )
}

fn resource_failure<I, K>(
    code: &'static str,
    message: &'static str,
    details: I,
) -> SchemaDiagnostics
where
    I: IntoIterator<Item = (K, String)>,
    K: Into<String>,
{
    failure(DiagnosticCategory::ResourceLimit, code, message, details)
}

fn failure<I, K>(
    category: DiagnosticCategory,
    code: &'static str,
    message: &'static str,
    details: I,
) -> SchemaDiagnostics
where
    I: IntoIterator<Item = (K, String)>,
    K: Into<String>,
{
    let mut diagnostic = Diagnostic::new(
        category,
        DiagnosticCode::new(code).expect("static schema diagnostic code is valid"),
        message,
    );
    for (key, value) in details {
        diagnostic = diagnostic.with_detail(key, value);
    }
    SchemaDiagnostics::one(SchemaDiagnostic::new(diagnostic, None))
}

#[cfg(test)]
mod tests {
    use super::reject_snapshot_change;

    #[test]
    fn changed_selection_uses_the_integrity_diagnostic() {
        let error = reject_snapshot_change(&["before"], &["after"])
            .expect_err("changed selections must fail closed");
        let item = error.iter().next().expect("one diagnostic");
        assert_eq!(item.diagnostic().category().as_str(), "integrity");
        assert_eq!(
            item.diagnostic().code().as_str(),
            "schema_discovery_snapshot_changed"
        );
        assert!(item.primary().is_none());
    }
}
