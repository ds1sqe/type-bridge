use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use type_bridge_schema::{SCHEMA_DISCOVERY_V1, load_schema_set};

static NEXT_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        let sequence = NEXT_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "type-bridge-schema-set-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temporary schema-set directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, relative: &str, source: &[u8]) -> PathBuf {
        let path = self.path().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create source parent");
        }
        fs::write(&path, source).expect("write schema-set fixture");
        path
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn code(error: &type_bridge_contract::schema::SchemaDiagnostics) -> &str {
    error
        .iter()
        .next()
        .expect("one schema-set diagnostic")
        .diagnostic()
        .code()
        .as_str()
}

#[test]
fn loads_schema_set_manifest_and_preserves_exact_source() {
    let directory = TempDirectory::new();
    directory.write(
        "fragments/person.yaml",
        b"format: typebridge.schema-fragment/v1\nentities: {person: {}}\n",
    );
    let source = "# retained\nformat: typebridge.schema-set/v1\nsources:\n  - 'fragments/*.yaml'\n";
    let manifest = directory.write("schema.yaml", source.as_bytes());

    let snapshot = load_schema_set(&manifest).expect("load strict schema set");

    assert_eq!(snapshot.manifest().source(), source);
    assert_eq!(snapshot.manifest().comments().len(), 1);
    assert_eq!(snapshot.manifest().sources(), ["fragments/*.yaml"]);
    assert_eq!(snapshot.discovery_version().as_str(), SCHEMA_DISCOVERY_V1);
    assert_eq!(snapshot.evidence().sources().len(), 1);
    assert_eq!(
        snapshot.evidence().manifest_fingerprint(),
        snapshot.manifest().fingerprint()
    );
    assert_eq!(snapshot.documents().manifest(), Some(snapshot.manifest()));
}

#[test]
fn manifest_patterns_drive_canonical_fragment_order() {
    let directory = TempDirectory::new();
    directory.write("z/z.yaml", b"format: typebridge.schema-fragment/v1\n");
    directory.write("a/a.yaml", b"format: typebridge.schema-fragment/v1\n");
    let manifest = directory.write(
        "schema.yaml",
        b"format: typebridge.schema-set/v1\nsources:\n  - z/*.yaml\n  - a/*.yaml\n",
    );

    let snapshot = load_schema_set(manifest).expect("load canonical source order");
    let paths = snapshot
        .evidence()
        .sources()
        .iter()
        .map(|source| source.path().as_str())
        .collect::<Vec<_>>();
    assert_eq!(paths, ["a/a.yaml", "z/z.yaml"]);
}

#[test]
fn manifest_pattern_reordering_changes_only_manifest_fingerprint() {
    let directory = TempDirectory::new();
    directory.write("a/a.yaml", b"format: typebridge.schema-fragment/v1\n");
    directory.write("z/z.yaml", b"format: typebridge.schema-fragment/v1\n");
    let manifest = directory.write(
        "schema.yaml",
        b"format: typebridge.schema-set/v1\nsources: [a/*.yaml, z/*.yaml]\n",
    );
    let first = load_schema_set(&manifest).expect("load first manifest order");
    fs::write(
        &manifest,
        b"format: typebridge.schema-set/v1\nsources: [z/*.yaml, a/*.yaml]\n",
    )
    .expect("rewrite manifest ordering");
    let second = load_schema_set(&manifest).expect("load second manifest order");

    assert_ne!(
        first.evidence().manifest_fingerprint(),
        second.evidence().manifest_fingerprint()
    );
    assert_eq!(
        first.evidence().document_set_fingerprint(),
        second.evidence().document_set_fingerprint()
    );
    assert_eq!(first.evidence().sources(), second.evidence().sources());
}

#[test]
fn rejects_missing_unknown_wrong_and_non_string_manifest_fields() {
    for (source, expected) in [
        ("sources: [a.yaml]\n", "schema_set_format_missing"),
        (
            "format: typebridge.schema-set/v1\n",
            "schema_set_sources_missing",
        ),
        (
            "format: typebridge.schema-set/v1\nsources: [a.yaml]\nlockfile: lock.json\n",
            "unknown_schema_set_key",
        ),
        (
            "format: typebridge.schema-set/v2\nsources: [a.yaml]\n",
            "unsupported_schema_set_format",
        ),
        (
            "format: typebridge.schema-set/v1\nsources: a.yaml\n",
            "schema_set_sources_not_sequence",
        ),
        (
            "format: typebridge.schema-set/v1\nsources: [{path: a.yaml}]\n",
            "schema_set_source_not_string",
        ),
    ] {
        let directory = TempDirectory::new();
        let manifest = directory.write("schema.yaml", source.as_bytes());
        let error = load_schema_set(manifest).expect_err("reject malformed manifest");
        assert_eq!(code(&error), expected, "source: {source}");
    }
}

#[test]
fn rejects_empty_manifest_sources_and_non_utf8_source() {
    let directory = TempDirectory::new();
    let empty = directory.write(
        "schema.yaml",
        b"format: typebridge.schema-set/v1\nsources: []\n",
    );
    assert_eq!(
        code(&load_schema_set(&empty).expect_err("reject empty sources")),
        "empty_schema_source_patterns"
    );

    let non_utf8 = directory.write("schema.yaml", &[0xff, 0xfe]);
    assert_eq!(
        code(&load_schema_set(non_utf8).expect_err("reject non-UTF-8 manifest")),
        "schema_manifest_not_utf8"
    );
}

#[test]
fn equivalent_roots_produce_equal_document_set_fingerprints() {
    let first = TempDirectory::new();
    let second = TempDirectory::new();
    for directory in [&first, &second] {
        directory.write("facts/a.yaml", b"format: typebridge.schema-fragment/v1\n");
        directory.write(
            "schema.yaml",
            b"format: typebridge.schema-set/v1\nsources: [facts/*.yaml]\n",
        );
    }

    let first = load_schema_set(first.path().join("schema.yaml")).expect("load first root");
    let second = load_schema_set(second.path().join("schema.yaml")).expect("load second root");
    assert_ne!(first.root(), second.root());
    assert_eq!(
        first.evidence().document_set_fingerprint(),
        second.evidence().document_set_fingerprint()
    );
    assert_eq!(first.evidence().sources(), second.evidence().sources());
}
