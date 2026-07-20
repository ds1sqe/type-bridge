use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use type_bridge_schema::{
    SchemaDiscoveryLimits, SchemaParseLimits, SchemaSourceCapture, SchemaSourceObservation,
    SchemaSourceService, SchemaSourceServiceError, SystemSchemaSourceService,
    discover_schema_documents, discover_schema_documents_with_limits, load_schema_set,
    load_schema_set_with_source,
};

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "type-bridge-schema-discovery-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temporary schema directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn manifest(&self) -> PathBuf {
        let manifest = self.path().join("schema.yaml");
        fs::write(&manifest, "format: typebridge.schema-set/v1\n")
            .expect("write schema-set manifest");
        manifest
    }

    fn write_source(&self, relative: &str, source: &str) {
        let path = self.path().join(relative);
        fs::create_dir_all(path.parent().expect("source parent")).expect("create source parent");
        fs::write(path, source).expect("write schema source");
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ForwardingSource(SystemSchemaSourceService);

impl SchemaSourceService for ForwardingSource {
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, SchemaSourceServiceError> {
        self.0.canonicalize(path)
    }

    fn metadata(&self, path: &Path) -> Result<SchemaSourceObservation, SchemaSourceServiceError> {
        self.0.metadata(path)
    }

    fn symlink_metadata(
        &self,
        path: &Path,
    ) -> Result<SchemaSourceObservation, SchemaSourceServiceError> {
        self.0.symlink_metadata(path)
    }

    fn read_directory_names(&self, path: &Path) -> Result<Vec<OsString>, SchemaSourceServiceError> {
        self.0.read_directory_names(path)
    }

    fn capture_file(
        &self,
        path: &Path,
        maximum_bytes: usize,
    ) -> Result<SchemaSourceCapture, SchemaSourceServiceError> {
        self.0.capture_file(path, maximum_bytes)
    }
}

struct MutatingSource {
    system: SystemSchemaSourceService,
    target: PathBuf,
    captures: AtomicUsize,
}

impl SchemaSourceService for MutatingSource {
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, SchemaSourceServiceError> {
        self.system.canonicalize(path)
    }

    fn metadata(&self, path: &Path) -> Result<SchemaSourceObservation, SchemaSourceServiceError> {
        self.system.metadata(path)
    }

    fn symlink_metadata(
        &self,
        path: &Path,
    ) -> Result<SchemaSourceObservation, SchemaSourceServiceError> {
        self.system.symlink_metadata(path)
    }

    fn read_directory_names(&self, path: &Path) -> Result<Vec<OsString>, SchemaSourceServiceError> {
        self.system.read_directory_names(path)
    }

    fn capture_file(
        &self,
        path: &Path,
        maximum_bytes: usize,
    ) -> Result<SchemaSourceCapture, SchemaSourceServiceError> {
        let captured = self.system.capture_file(path, maximum_bytes)?;
        if path == self.target && self.captures.fetch_add(1, Ordering::SeqCst) == 0 {
            fs::write(path, "root: b\n").map_err(|_| SchemaSourceServiceError)?;
        }
        Ok(captured)
    }
}

fn write_loadable_manifest(directory: &TempDirectory, source: &str) -> PathBuf {
    let manifest = directory.path().join("schema.yaml");
    fs::write(
        &manifest,
        format!("format: typebridge.schema-set/v1\nsources:\n  - {source}\n"),
    )
    .expect("write loadable schema-set manifest");
    manifest
}

fn code(error: &type_bridge_contract::schema::SchemaDiagnostics) -> &str {
    error
        .iter()
        .next()
        .expect("one discovery diagnostic")
        .diagnostic()
        .code()
        .as_str()
}

#[test]
fn recursive_glob_captures_documents_in_portable_byte_order() {
    let directory = TempDirectory::new();
    let manifest = directory.manifest();
    directory.write_source("fragments/z.yaml", "root: z\n");
    directory.write_source("fragments/nested/a.yaml", "root: a\n");

    let snapshot = discover_schema_documents(&manifest, ["fragments/**/*.yaml"])
        .expect("discover stable sources");
    let paths = snapshot
        .documents()
        .iter()
        .map(|(id, _)| id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(paths, ["fragments/nested/a.yaml", "fragments/z.yaml"]);
    assert_eq!(snapshot.root(), directory.path());
    assert_eq!(snapshot.manifest(), manifest);
}

#[test]
fn portable_patterns_reject_traversal_absolute_and_extended_globs() {
    let directory = TempDirectory::new();
    let manifest = directory.manifest();

    for (pattern, expected) in [
        ("../outside.yaml", "invalid_schema_source_path"),
        ("/absolute.yaml", "invalid_schema_source_path"),
        ("C:/absolute.yaml", "invalid_schema_source_path"),
        ("fragments\\*.yaml", "invalid_schema_source_path"),
        ("fragments/[ab].yaml", "unsupported_schema_glob_syntax"),
        ("fragments/{a,b}.yaml", "unsupported_schema_glob_syntax"),
        ("fragments/a**.yaml", "unsupported_schema_glob_syntax"),
    ] {
        let error = discover_schema_documents(&manifest, [pattern]).expect_err("reject pattern");
        assert_eq!(code(&error), expected, "pattern {pattern}");
    }

    let decomposed = "fragments/cafe\u{301}.yaml";
    let error =
        discover_schema_documents(&manifest, [decomposed]).expect_err("non-NFC pattern must fail");
    assert_eq!(code(&error), "schema_source_pattern_not_nfc");
}

#[test]
fn every_pattern_must_match_and_overlaps_are_not_deduplicated() {
    let directory = TempDirectory::new();
    let manifest = directory.manifest();
    directory.write_source("fragments/a.yaml", "root: a\n");

    let empty =
        discover_schema_documents(&manifest, ["missing/*.yaml"]).expect_err("empty glob must fail");
    assert_eq!(code(&empty), "empty_schema_source_pattern");

    let overlap = discover_schema_documents(&manifest, ["fragments/*.yaml", "fragments/a.yaml"])
        .expect_err("overlapping patterns must fail");
    assert_eq!(code(&overlap), "overlapping_schema_source_patterns");
}

#[test]
fn manifest_non_yaml_and_non_regular_matches_fail_closed() {
    let directory = TempDirectory::new();
    let manifest = directory.manifest();
    directory.write_source("fragments/readme.txt", "not yaml\n");
    fs::create_dir_all(directory.path().join("fragments/folder.yaml"))
        .expect("create yaml-named directory");

    let selected_manifest = discover_schema_documents(&manifest, ["schema.yaml"])
        .expect_err("manifest cannot select itself");
    assert_eq!(
        code(&selected_manifest),
        "schema_manifest_selected_as_source"
    );

    let non_yaml = discover_schema_documents(&manifest, ["fragments/readme.txt"])
        .expect_err("non-yaml source must fail");
    assert_eq!(code(&non_yaml), "schema_source_not_yaml");

    let directory_match = discover_schema_documents(&manifest, ["fragments/folder.yaml"])
        .expect_err("directory source must fail");
    assert_eq!(code(&directory_match), "schema_source_not_regular");
}

#[test]
fn unicode_casefold_collisions_fail_before_platform_order_can_choose() {
    let directory = TempDirectory::new();
    let manifest = directory.manifest();
    directory.write_source("fragments/Straße.yaml", "root: first\n");
    directory.write_source("fragments/STRASSE.yaml", "root: second\n");

    let error = discover_schema_documents(&manifest, ["fragments/*.yaml"])
        .expect_err("case-fold collision must fail");
    assert_eq!(code(&error), "schema_source_path_collision");
}

#[test]
fn document_and_walk_limits_apply_before_unbounded_loading() {
    let directory = TempDirectory::new();
    let manifest = directory.manifest();
    directory.write_source("fragments/a.yaml", "root: a\n");
    directory.write_source("fragments/b.yaml", "root: b\n");

    let parse_limits = SchemaParseLimits::new(1, 64, 64, 8, 32, 32);
    let limits = SchemaDiscoveryLimits::new(parse_limits, 4, 64, 16, 8);
    let documents = discover_schema_documents_with_limits(&manifest, ["fragments/*.yaml"], limits)
        .expect_err("document ceiling must fail");
    assert_eq!(code(&documents), "schema_document_count_limit");

    let limits = SchemaDiscoveryLimits::new(parse_limits, 4, 64, 1, 8);
    let entries = discover_schema_documents_with_limits(&manifest, ["fragments/a.yaml"], limits)
        .expect_err("walk ceiling must fail");
    assert_eq!(code(&entries), "schema_discovery_entry_limit");
}

#[test]
fn system_and_injected_source_loading_are_exactly_equivalent() {
    let directory = TempDirectory::new();
    directory.write_source("fragments/z.yaml", "root: z\n");
    directory.write_source("fragments/a.yaml", "root: a\n");
    let manifest = write_loadable_manifest(&directory, "fragments/*.yaml");

    let system = load_schema_set(&manifest).expect("load through system entry point");
    let injected = load_schema_set_with_source(
        &manifest,
        &ForwardingSource::default(),
        SchemaDiscoveryLimits::default(),
    )
    .expect("load through injected source service");

    assert_eq!(injected, system);
}

#[test]
fn injected_source_mutation_fails_full_recapture() {
    let directory = TempDirectory::new();
    directory.write_source("fragments/a.yaml", "root: a\n");
    let manifest = write_loadable_manifest(&directory, "fragments/a.yaml");
    let target = fs::canonicalize(directory.path().join("fragments/a.yaml"))
        .expect("canonicalize mutation target");
    let source = MutatingSource {
        system: SystemSchemaSourceService,
        target,
        captures: AtomicUsize::new(0),
    };

    let error = load_schema_set_with_source(&manifest, &source, SchemaDiscoveryLimits::default())
        .expect_err("mutation after first capture must fail");
    assert_eq!(code(&error), "schema_discovery_snapshot_changed");
}

#[cfg(unix)]
#[test]
fn symlink_escape_and_file_aliases_are_rejected() {
    use std::os::unix::fs::symlink;

    let directory = TempDirectory::new();
    let manifest = directory.manifest();
    directory.write_source("fragments/source.yaml", "root: source\n");
    symlink(
        directory.path().join("fragments/source.yaml"),
        directory.path().join("fragments/alias.yaml"),
    )
    .expect("create file alias");

    let alias = discover_schema_documents(&manifest, ["fragments/*.yaml"])
        .expect_err("file alias must fail");
    assert_eq!(code(&alias), "schema_source_file_alias");

    fs::remove_file(directory.path().join("fragments/alias.yaml")).expect("remove alias");
    let outside = TempDirectory::new();
    outside.write_source("escaped.yaml", "root: escaped\n");
    symlink(outside.path(), directory.path().join("fragments/outside"))
        .expect("create escaping directory link");

    let escape = discover_schema_documents(&manifest, ["fragments/**/*.yaml"])
        .expect_err("root escape must fail");
    assert_eq!(code(&escape), "schema_source_symlink_escape");
}

#[cfg(unix)]
#[test]
fn injected_source_preserves_escape_and_alias_rejections() {
    use std::os::unix::fs::symlink;

    let directory = TempDirectory::new();
    directory.write_source("fragments/source.yaml", "root: source\n");
    let manifest = write_loadable_manifest(&directory, "fragments/*.yaml");
    symlink(
        directory.path().join("fragments/source.yaml"),
        directory.path().join("fragments/alias.yaml"),
    )
    .expect("create injected alias");

    let alias = load_schema_set_with_source(
        &manifest,
        &ForwardingSource::default(),
        SchemaDiscoveryLimits::default(),
    )
    .expect_err("injected alias must fail");
    assert_eq!(code(&alias), "schema_source_file_alias");

    fs::remove_file(directory.path().join("fragments/alias.yaml")).expect("remove alias");
    let outside = TempDirectory::new();
    outside.write_source("escaped.yaml", "root: escaped\n");
    symlink(outside.path(), directory.path().join("fragments/outside"))
        .expect("create injected escape");
    let manifest = write_loadable_manifest(&directory, "fragments/**/*.yaml");

    let escape = load_schema_set_with_source(
        &manifest,
        &ForwardingSource::default(),
        SchemaDiscoveryLimits::default(),
    )
    .expect_err("injected root escape must fail");
    assert_eq!(code(&escape), "schema_source_symlink_escape");
}
