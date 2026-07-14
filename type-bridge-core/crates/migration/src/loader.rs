//! Native Rust sidecar loader for migration files.
//!
//! Reads `NNNN_<name>.json` sidecar files that the generator writes beside
//! the corresponding `NNNN_<name>.py` source files.  The sidecar carries the
//! serde [`MigrationSpec`] produced from the same op list as the `.py` — so
//! the Rust CLI can hydrate a [`MigrationGraph`] without importing Python.
//!
//! The loader is pure `std::fs` + `serde_json`; it opens no TypeDB
//! transaction and has no dependency on `type_bridge_orm` (invariant 7).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::author::{
    COMMIT_MANIFEST_FORMAT_V1, ManifestExtension, MigrationCommitManifest, MigrationTreeFormat,
    TREE_FORMAT_SENTINEL, TREE_FORMAT_V1, sha256, validate_normalized_relative_path,
};
use crate::checksum::migration_file_checksum;
use crate::error::{MigrationError, Result};
use crate::spec::{MigrationGraph, MigrationSpec};

/// One verified embedder extension loaded from a manifested migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedMigrationExtension {
    /// Migration version that owns the extension.
    pub migration_name: String,
    /// Registered embedder namespace.
    pub namespace: String,
    /// Full normalized path relative to the migrations directory.
    pub path: String,
    /// Whether semantic consumers must understand the extension.
    pub critical: bool,
    /// Exact hash-verified bytes.
    pub contents: Vec<u8>,
}

/// Checked graph plus verified manifested metadata and extension bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedMigrationTree {
    /// Checked migration graph in discovery order.
    pub graph: MigrationGraph,
    /// Parsed per-version commit manifests.
    pub manifests: Vec<MigrationCommitManifest>,
    /// Verified extension bytes exposed to the embedder.
    pub extensions: Vec<LoadedMigrationExtension>,
}

/// Load the sidecar spec for a given `.py` migration path.
///
/// Derives the sidecar path by replacing the `.py` extension with `.json`
/// (same stem, sibling file).
///
/// - If the `.json` sibling **does not exist** → `Ok(None)`.  The caller
///   should fall back to the trusted-import Python path for this file.
/// - If it **exists** → read, deserialize, and return `Ok(Some(spec))`.
/// - If it exists but is malformed → `Err(MigrationError::Loader { .. })`.
///
/// # Errors
///
/// Returns [`MigrationError::Loader`] when the sidecar exists but cannot be
/// read or deserialized.
pub fn load_sidecar(py_path: &Path) -> Result<Option<MigrationSpec>> {
    let json_path = py_path.with_extension("json");
    if !json_path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&json_path).map_err(|err| MigrationError::Loader {
        message: format!("failed to read sidecar {}: {err}", json_path.display()),
    })?;
    let spec: MigrationSpec =
        serde_json::from_str(&content).map_err(|err| MigrationError::Loader {
            message: format!("failed to parse sidecar {}: {err}", json_path.display()),
        })?;
    Ok(Some(spec))
}

/// Load the checked graph using explicit legacy or manifested visibility.
///
/// Critical extensions are rejected because this convenience entry point has
/// no embedder namespace allowlist. Embedders that own critical extensions use
/// [`load_dir_checked_with_extensions`].
pub fn load_dir_checked(dir: &Path) -> Result<MigrationGraph> {
    Ok(load_dir_checked_with_extensions(dir, &BTreeSet::new())?.graph)
}

/// Load a checked migration tree and expose hash-verified extension bytes.
///
/// `known_extension_namespaces` declares the critical extension namespaces
/// the caller can semantically consume. Unknown non-critical extensions remain
/// verified and exposed; unknown critical extensions fail closed.
pub fn load_dir_checked_with_extensions(
    dir: &Path,
    known_extension_namespaces: &BTreeSet<String>,
) -> Result<CheckedMigrationTree> {
    let sentinel_path = dir.join(TREE_FORMAT_SENTINEL);
    if sentinel_path.exists() {
        return load_manifested_tree_checked(dir, known_extension_namespaces);
    }
    if discover_manifest_paths(dir)?.is_empty() {
        return Ok(CheckedMigrationTree {
            graph: load_legacy_dir_checked(dir)?,
            manifests: Vec::new(),
            extensions: Vec::new(),
        });
    }
    Err(loader_error(format!(
        "manifested migrations exist in {} without immutable tree-format sentinel {}",
        dir.display(),
        TREE_FORMAT_SENTINEL
    )))
}

/// Walk `dir` and load all `NNNN_*.json` sidecar files into a sorted
/// [`MigrationGraph`].
///
/// Only files whose stems match the four-digit prefix pattern
/// (`[0-9][0-9][0-9][0-9]_*`) and whose extension is `.json` are loaded.
/// `.py` files and any other non-matching files are skipped.  The resulting
/// [`MigrationGraph`] is sorted by file stem (lexicographic / discovery
/// order), which matches Python's `discover()` sort.
///
/// This is the dir-native loader consumed by the Rust CLI (sub-plan 08).
/// It does not invoke Python, does not `exec_module`, and opens no
/// transaction.
///
/// # Errors
///
/// Returns [`MigrationError::Loader`] when the directory cannot be read or
/// a matching sidecar file cannot be read or deserialized.
pub fn load_dir(dir: &Path) -> Result<MigrationGraph> {
    if dir.join(TREE_FORMAT_SENTINEL).exists() || !discover_manifest_paths(dir)?.is_empty() {
        return load_dir_checked(dir);
    }
    let read_dir = std::fs::read_dir(dir).map_err(|err| MigrationError::Loader {
        message: format!("failed to read migrations dir {}: {err}", dir.display()),
    })?;

    let mut entries: Vec<(String, PathBuf)> = Vec::new();

    for entry in read_dir {
        let entry = entry.map_err(|err| MigrationError::Loader {
            message: format!("failed to iterate migrations dir {}: {err}", dir.display()),
        })?;
        let path = entry.path();

        // Only consider `.json` files.
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".manifest.json"))
        {
            continue;
        }
        // The stem must match `NNNN_*` (four digits then underscore).
        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_owned(),
            None => continue,
        };

        if !is_migration_stem(&stem) {
            continue;
        }

        entries.push((stem, path));
    }

    // Sort by stem for stable discovery order.
    entries.sort_by(|(a, _), (b, _)| a.cmp(b));

    let mut migrations = Vec::with_capacity(entries.len());
    for (stem, path) in entries {
        let content = std::fs::read_to_string(&path).map_err(|err| MigrationError::Loader {
            message: format!("failed to read sidecar {}: {err}", path.display()),
        })?;
        let spec: MigrationSpec =
            serde_json::from_str(&content).map_err(|err| MigrationError::Loader {
                message: format!(
                    "failed to parse sidecar {} (stem={stem}): {err}",
                    path.display()
                ),
            })?;
        validate_declared_intent(&spec, &stem)?;
        migrations.push(spec);
    }

    Ok(MigrationGraph { migrations })
}

/// Walk `dir` and load all `NNNN_*.json` sidecar files into a sorted
/// [`MigrationGraph`], then validate that each sidecar's embedded checksum
/// agrees with the current `.py` text.
///
/// This is the **checked** variant of [`load_dir`], intended for use by the
/// Rust CLI whenever it is the execution source.  The check guards against
/// sidecar drift: if a developer hand-edits the `.py` after the sidecar was
/// generated, the sidecar's `checksum` field will disagree with the fresh
/// `.py` text, and this function returns an error rather than silently
/// executing a stale sidecar.
///
/// # Invariant
///
/// The `.py` text is the sole checksum source (sub-plan 04/07 invariant).
/// The sidecar carries a copy of that checksum so the drift guard can compare
/// without importing Python; if the `.py` file is absent for a given sidecar
/// the check is skipped (legacy sidecar-only migration; no `.py` to compare).
///
/// # Errors
///
/// Returns [`MigrationError::Loader`] when:
/// - The directory or a sidecar cannot be read (same as [`load_dir`]).
/// - A sidecar's embedded `checksum` disagrees with the recomputed `.py` text
///   checksum — "sidecar drift: regenerate the migration" (D6 guard).
fn load_legacy_dir_checked(dir: &Path) -> Result<MigrationGraph> {
    let read_dir = std::fs::read_dir(dir).map_err(|err| MigrationError::Loader {
        message: format!("failed to read migrations dir {}: {err}", dir.display()),
    })?;

    let mut entries: Vec<(String, PathBuf)> = Vec::new();

    for entry in read_dir {
        let entry = entry.map_err(|err| MigrationError::Loader {
            message: format!("failed to iterate migrations dir {}: {err}", dir.display()),
        })?;
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".manifest.json"))
        {
            continue;
        }

        let stem = match path.file_stem().and_then(|s| s.to_str()) {
            Some(s) => s.to_owned(),
            None => continue,
        };

        if !is_migration_stem(&stem) {
            continue;
        }

        entries.push((stem, path));
    }

    entries.sort_by(|(a, _), (b, _)| a.cmp(b));

    let mut migrations = Vec::with_capacity(entries.len());
    for (stem, json_path) in entries {
        let content =
            std::fs::read_to_string(&json_path).map_err(|err| MigrationError::Loader {
                message: format!("failed to read sidecar {}: {err}", json_path.display()),
            })?;
        let spec: MigrationSpec =
            serde_json::from_str(&content).map_err(|err| MigrationError::Loader {
                message: format!(
                    "failed to parse sidecar {} (stem={stem}): {err}",
                    json_path.display()
                ),
            })?;

        // D6 drift guard: recompute the .py text checksum and compare to the
        // sidecar's embedded value.  The .py text is the sole checksum source
        // (04/07 invariant); the sidecar is a generated cache.  If the .py
        // was hand-edited after the sidecar was written, the checksums will
        // diverge and we reject the stale sidecar rather than executing it.
        if let Some(sidecar_checksum) = &spec.checksum {
            let py_path = json_path.with_extension("py");
            if py_path.exists() {
                let py_text =
                    std::fs::read_to_string(&py_path).map_err(|err| MigrationError::Loader {
                        message: format!(
                            "failed to read .py for drift check {}: {err}",
                            py_path.display()
                        ),
                    })?;
                let computed = migration_file_checksum(&py_text);
                if computed != *sidecar_checksum {
                    return Err(MigrationError::Loader {
                        message: format!(
                            "sidecar drift detected for {stem}: the .py file has been \
                             modified since the sidecar was generated \
                             (sidecar checksum={sidecar_checksum}, \
                             current .py checksum={computed}). \
                             Regenerate the migration to sync the sidecar."
                        ),
                    });
                }
            }
        }

        validate_declared_intent(&spec, &stem)?;
        migrations.push(spec);
    }

    Ok(MigrationGraph { migrations })
}

fn discover_manifest_paths(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let entries = std::fs::read_dir(dir).map_err(|error| {
        loader_error(format!(
            "failed to read migrations dir {}: {error}",
            dir.display()
        ))
    })?;
    let mut manifests = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            loader_error(format!(
                "failed to iterate migrations dir {}: {error}",
                dir.display()
            ))
        })?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".manifest.json") else {
            continue;
        };
        if is_migration_stem(stem) {
            manifests.push((stem.to_string(), path));
        }
    }
    manifests.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(manifests)
}

fn load_manifested_tree_checked(
    dir: &Path,
    known_extension_namespaces: &BTreeSet<String>,
) -> Result<CheckedMigrationTree> {
    let sentinel_path = dir.join(TREE_FORMAT_SENTINEL);
    let sentinel_bytes = read_regular_file(dir, TREE_FORMAT_SENTINEL)?;
    let sentinel: MigrationTreeFormat =
        serde_json::from_slice(&sentinel_bytes).map_err(|error| {
            loader_error(format!(
                "failed to parse tree-format sentinel {}: {error}",
                sentinel_path.display()
            ))
        })?;
    if sentinel.format != TREE_FORMAT_V1 {
        return Err(loader_error(format!(
            "unknown migration tree format {:?} in {}",
            sentinel.format,
            sentinel_path.display()
        )));
    }
    validate_legacy_prefix(&sentinel.legacy_prefix)?;

    let mut migrations = Vec::new();
    for stem in &sentinel.legacy_prefix {
        let json_path = dir.join(format!("{stem}.json"));
        if !json_path.exists() {
            return Err(loader_error(format!(
                "legacy-prefix sidecar {} is missing",
                json_path.display()
            )));
        }
        migrations.push(load_one_spec_checked(dir, stem)?);
    }

    let discovered = discover_manifest_paths(dir)?;
    validate_manifest_sequence(&sentinel.legacy_prefix, &discovered)?;

    let mut manifests = Vec::with_capacity(discovered.len());
    let mut extensions = Vec::new();
    for (stem, manifest_path) in discovered {
        let (manifest, spec, mut loaded_extensions) =
            verify_manifest(dir, &stem, &manifest_path, known_extension_namespaces)?;
        migrations.push(spec);
        manifests.push(manifest);
        extensions.append(&mut loaded_extensions);
    }

    let graph = MigrationGraph { migrations };
    let errors = crate::graph::validate_graph(&graph, &[]);
    if !errors.is_empty() {
        return Err(MigrationError::Planning { errors });
    }
    Ok(CheckedMigrationTree {
        graph,
        manifests,
        extensions,
    })
}

fn validate_legacy_prefix(prefix: &[String]) -> Result<()> {
    let mut previous: Option<&str> = None;
    for stem in prefix {
        if !is_migration_stem(stem) {
            return Err(loader_error(format!(
                "tree-format sentinel contains invalid legacy migration stem {stem:?}"
            )));
        }
        if previous.is_some_and(|previous| previous >= stem.as_str()) {
            return Err(loader_error(
                "tree-format sentinel legacy_prefix must be strictly sorted and unique".to_string(),
            ));
        }
        previous = Some(stem);
    }
    Ok(())
}

fn validate_manifest_sequence(
    legacy_prefix: &[String],
    manifests: &[(String, PathBuf)],
) -> Result<()> {
    let first_expected = legacy_prefix
        .last()
        .map(|stem| migration_number(stem))
        .transpose()?
        .map_or(1, |number| number + 1);
    for (offset, (stem, _)) in manifests.iter().enumerate() {
        let expected = first_expected + offset as u32;
        let number = migration_number(stem)?;
        if number != expected {
            return Err(loader_error(format!(
                "manifested migration chain gap: expected version {expected:04}, found {stem}"
            )));
        }
    }
    Ok(())
}

fn migration_number(stem: &str) -> Result<u32> {
    stem[..4].parse::<u32>().map_err(|error| {
        loader_error(format!(
            "invalid migration number in stem {stem:?}: {error}"
        ))
    })
}

fn verify_manifest(
    dir: &Path,
    expected_stem: &str,
    manifest_path: &Path,
    known_extension_namespaces: &BTreeSet<String>,
) -> Result<(
    MigrationCommitManifest,
    MigrationSpec,
    Vec<LoadedMigrationExtension>,
)> {
    let manifest_relative = format!("{expected_stem}.manifest.json");
    let manifest_bytes = read_regular_file(dir, &manifest_relative)?;
    let manifest: MigrationCommitManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|error| {
            loader_error(format!(
                "failed to parse migration manifest {}: {error}",
                manifest_path.display()
            ))
        })?;
    if manifest.format != COMMIT_MANIFEST_FORMAT_V1 {
        return Err(loader_error(format!(
            "unknown migration manifest format {:?} in {}",
            manifest.format,
            manifest_path.display()
        )));
    }
    if manifest.migration_name != expected_stem {
        return Err(loader_error(format!(
            "manifest {} declares migration {:?}, expected {expected_stem:?}",
            manifest_path.display(),
            manifest.migration_name
        )));
    }

    let mut previous_path: Option<&str> = None;
    let mut folded_paths = BTreeSet::new();
    let mut verified = BTreeMap::<String, Vec<u8>>::new();
    let mut extensions = Vec::new();
    for entry in &manifest.files {
        validate_normalized_relative_path(&entry.path).map_err(authoring_as_loader)?;
        if previous_path.is_some_and(|previous| previous >= entry.path.as_str()) {
            return Err(loader_error(format!(
                "manifest {} file entries must be strictly path-sorted and unique",
                manifest_path.display()
            )));
        }
        previous_path = Some(&entry.path);
        if !folded_paths.insert(entry.path.to_ascii_lowercase()) {
            return Err(loader_error(format!(
                "manifest {} contains case-insensitively colliding paths",
                manifest_path.display()
            )));
        }
        validate_manifest_entry(expected_stem, entry.extension.as_ref(), &entry.path)?;
        let contents = read_regular_file(dir, &entry.path)?;
        if contents.len() as u64 != entry.size || sha256(&contents) != entry.sha256 {
            return Err(loader_error(format!(
                "manifest hash/size drift for {} referenced by {}",
                entry.path,
                manifest_path.display()
            )));
        }
        if let Some(extension) = &entry.extension {
            if extension.critical && !known_extension_namespaces.contains(&extension.namespace) {
                return Err(loader_error(format!(
                    "critical extension namespace {:?} in {} is not understood by this consumer",
                    extension.namespace,
                    manifest_path.display()
                )));
            }
            extensions.push(LoadedMigrationExtension {
                migration_name: expected_stem.to_string(),
                namespace: extension.namespace.clone(),
                path: entry.path.clone(),
                critical: extension.critical,
                contents: contents.clone(),
            });
        }
        verified.insert(entry.path.clone(), contents);
    }

    let py_path = format!("{expected_stem}.py");
    let json_path = format!("{expected_stem}.json");
    let py_bytes = verified.get(&py_path).ok_or_else(|| {
        loader_error(format!("manifest {expected_stem} does not cover {py_path}"))
    })?;
    let json_bytes = verified.get(&json_path).ok_or_else(|| {
        loader_error(format!(
            "manifest {expected_stem} does not cover {json_path}"
        ))
    })?;
    if !verified
        .keys()
        .any(|path| path == &format!("snapshots/v{}/snapshot.json", &expected_stem[..4]))
    {
        return Err(loader_error(format!(
            "manifest {expected_stem} does not cover its snapshot.json"
        )));
    }

    let spec: MigrationSpec = serde_json::from_slice(json_bytes).map_err(|error| {
        loader_error(format!(
            "failed to parse manifested sidecar {json_path}: {error}"
        ))
    })?;
    validate_declared_intent(&spec, expected_stem)?;
    if spec.name != expected_stem {
        return Err(loader_error(format!(
            "manifested sidecar name {:?} does not match {expected_stem:?}",
            spec.name
        )));
    }
    verify_python_checksum(expected_stem, &spec, py_bytes)?;
    Ok((manifest, spec, extensions))
}

fn validate_manifest_entry(
    migration_name: &str,
    extension: Option<&ManifestExtension>,
    path: &str,
) -> Result<()> {
    match extension {
        Some(extension) => {
            let expected_prefix = format!("ext/{}/{migration_name}/", extension.namespace);
            if !path.starts_with(&expected_prefix) || path.len() == expected_prefix.len() {
                return Err(loader_error(format!(
                    "extension path {path:?} does not match namespace {:?} and migration {migration_name}",
                    extension.namespace
                )));
            }
        }
        None => {
            let root_py = format!("{migration_name}.py");
            let root_json = format!("{migration_name}.json");
            let snapshot_prefix = format!("snapshots/v{}/", &migration_name[..4]);
            if path != root_py && path != root_json && !path.starts_with(&snapshot_prefix) {
                return Err(loader_error(format!(
                    "canonical manifest path {path:?} does not belong to {migration_name}"
                )));
            }
        }
    }
    Ok(())
}

fn load_one_spec_checked(dir: &Path, stem: &str) -> Result<MigrationSpec> {
    let json_relative = format!("{stem}.json");
    let json_bytes = read_regular_file(dir, &json_relative)?;
    let spec: MigrationSpec = serde_json::from_slice(&json_bytes).map_err(|error| {
        loader_error(format!(
            "failed to parse legacy sidecar {json_relative}: {error}"
        ))
    })?;
    validate_declared_intent(&spec, stem)?;
    let py_relative = format!("{stem}.py");
    if dir.join(&py_relative).exists() {
        let py_bytes = read_regular_file(dir, &py_relative)?;
        verify_python_checksum(stem, &spec, &py_bytes)?;
    }
    Ok(spec)
}

fn verify_python_checksum(stem: &str, spec: &MigrationSpec, py_bytes: &[u8]) -> Result<()> {
    let Some(sidecar_checksum) = &spec.checksum else {
        return Ok(());
    };
    let py_text = std::str::from_utf8(py_bytes).map_err(|error| {
        loader_error(format!(
            "migration Python source for {stem} is not UTF-8: {error}"
        ))
    })?;
    let computed = migration_file_checksum(py_text);
    if computed == *sidecar_checksum {
        Ok(())
    } else {
        Err(loader_error(format!(
            "sidecar drift detected for {stem}: sidecar checksum={sidecar_checksum}, current .py checksum={computed}"
        )))
    }
}

fn validate_declared_intent(spec: &MigrationSpec, stem: &str) -> Result<()> {
    if spec
        .declared_intent
        .as_ref()
        .is_some_and(|intent| !intent.has_valid_identity())
    {
        return Err(loader_error(format!(
            "migration {stem} carries a malformed declared-transition identity"
        )));
    }
    Ok(())
}

fn read_regular_file(dir: &Path, relative_path: &str) -> Result<Vec<u8>> {
    validate_normalized_relative_path(relative_path).map_err(authoring_as_loader)?;
    let mut current = dir.to_path_buf();
    for segment in relative_path.split('/') {
        current.push(segment);
        let metadata = std::fs::symlink_metadata(&current).map_err(|error| {
            loader_error(format!("failed to inspect {}: {error}", current.display()))
        })?;
        if metadata.file_type().is_symlink() {
            return Err(loader_error(format!(
                "manifested path {} must not traverse a symbolic link",
                current.display()
            )));
        }
    }
    let metadata = std::fs::metadata(&current).map_err(|error| {
        loader_error(format!("failed to inspect {}: {error}", current.display()))
    })?;
    if !metadata.is_file() {
        return Err(loader_error(format!(
            "manifested path {} is not a regular file",
            current.display()
        )));
    }
    std::fs::read(&current)
        .map_err(|error| loader_error(format!("failed to read {}: {error}", current.display())))
}

fn authoring_as_loader(error: MigrationError) -> MigrationError {
    loader_error(error.to_string())
}

fn loader_error(message: String) -> MigrationError {
    MigrationError::Loader { message }
}

/// Return `true` if the stem matches the migration naming convention:
/// four ASCII digits followed by an underscore and at least one more character.
fn is_migration_stem(stem: &str) -> bool {
    let bytes = stem.as_bytes();
    if bytes.len() < 6 {
        return false;
    }
    bytes[..4].iter().all(|b| b.is_ascii_digit()) && bytes[4] == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::author::{AuthoredArtifact, AuthoredMigration, MigrationTreeFormat};
    use crate::spec::{MigrationSpec, OperationSpec};

    /// Build a minimal but valid `MigrationSpec` for testing.
    fn make_spec(name: &str) -> MigrationSpec {
        MigrationSpec {
            app_label: "test_app".to_string(),
            name: name.to_string(),
            dependencies: vec![],
            operations: vec![OperationSpec::RunTypeql {
                forward: format!("define attribute {name}, value string;"),
                reverse: None,
            }],
            declared_intent: None,
            checksum: Some("abc123".to_string()),
            reversible: false,
        }
    }

    /// Write a `MigrationSpec` to a `.json` file in `dir` using the given stem.
    fn write_sidecar(dir: &Path, stem: &str, spec: &MigrationSpec) {
        let json = serde_json::to_string(spec).unwrap();
        std::fs::write(dir.join(format!("{stem}.json")), json).unwrap();
    }

    /// Write an empty `.py` file in `dir` using the given stem.
    fn write_py(dir: &Path, stem: &str) {
        std::fs::write(dir.join(format!("{stem}.py")), b"class Migration: pass\n").unwrap();
    }

    fn composed(stem: &str, critical: bool) -> crate::author::ComposedMigration {
        let py = "class Migration: pass\n";
        let spec = MigrationSpec {
            app_label: "test_app".to_string(),
            name: stem.to_string(),
            dependencies: vec![],
            operations: vec![],
            declared_intent: None,
            checksum: Some(migration_file_checksum(py)),
            reversible: true,
        };
        let authored = AuthoredMigration {
            migration_name: stem.to_string(),
            python_source: py.to_string(),
            spec: spec.clone(),
            files: vec![
                AuthoredArtifact {
                    relative_path: format!("{stem}.py"),
                    contents: py.as_bytes().to_vec(),
                },
                AuthoredArtifact {
                    relative_path: format!("{stem}.json"),
                    contents: serde_json::to_vec(&spec).unwrap(),
                },
                AuthoredArtifact {
                    relative_path: format!("snapshots/v{}/snapshot.json", &stem[..4]),
                    contents: b"{}".to_vec(),
                },
            ],
        };
        let mut composer = authored.composer();
        composer
            .add_extension("paladin", "companion.json", b"semantic".to_vec(), critical)
            .unwrap();
        composer.compose().unwrap()
    }

    fn write_manifested(dir: &Path, composed: &crate::author::ComposedMigration) {
        let sentinel = MigrationTreeFormat {
            format: TREE_FORMAT_V1.to_string(),
            legacy_prefix: vec![],
        };
        std::fs::write(
            dir.join(TREE_FORMAT_SENTINEL),
            serde_json::to_vec(&sentinel).unwrap(),
        )
        .unwrap();
        for file in composed.complete_files() {
            let path = dir.join(&file.relative_path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, file.contents).unwrap();
        }
    }

    #[test]
    fn manifested_loader_verifies_and_exposes_known_critical_extensions() {
        let tmp = tempfile::tempdir().unwrap();
        let composed = composed("0001_initial", true);
        write_manifested(tmp.path(), &composed);
        let known = BTreeSet::from(["paladin".to_string()]);

        let loaded = load_dir_checked_with_extensions(tmp.path(), &known).unwrap();
        assert_eq!(loaded.graph.migrations.len(), 1);
        assert_eq!(loaded.manifests, vec![composed.manifest]);
        assert_eq!(loaded.extensions.len(), 1);
        assert_eq!(loaded.extensions[0].contents, b"semantic");
        assert!(load_dir_checked(tmp.path()).is_err());
    }

    #[test]
    fn manifestless_new_format_files_are_invisible_but_hash_drift_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let composed = composed("0001_initial", false);
        write_manifested(tmp.path(), &composed);
        let manifest_path = tmp.path().join(composed.manifest_path());
        std::fs::remove_file(&manifest_path).unwrap();

        let loaded = load_dir_checked(tmp.path()).unwrap();
        assert!(loaded.migrations.is_empty());

        std::fs::write(&manifest_path, &composed.manifest_bytes).unwrap();
        std::fs::write(tmp.path().join("0001_initial.py"), b"drift").unwrap();
        assert!(load_dir_checked(tmp.path()).is_err());
    }

    // ── load_sidecar ──────────────────────────────────────────────────────────

    #[test]
    fn load_sidecar_returns_some_for_valid_json() {
        let tmp = tempfile::tempdir().unwrap();
        let spec = make_spec("0001_initial");
        write_sidecar(tmp.path(), "0001_initial", &spec);
        write_py(tmp.path(), "0001_initial");

        let py_path = tmp.path().join("0001_initial.py");
        let result = load_sidecar(&py_path).unwrap();

        assert_eq!(result, Some(spec));
    }

    #[test]
    fn load_sidecar_returns_none_when_no_json_sibling() {
        let tmp = tempfile::tempdir().unwrap();
        write_py(tmp.path(), "0001_initial");

        let py_path = tmp.path().join("0001_initial.py");
        let result = load_sidecar(&py_path).unwrap();

        assert_eq!(result, None);
    }

    #[test]
    fn load_sidecar_returns_error_on_malformed_json() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("0001_initial.json"), b"{ not valid json }").unwrap();
        write_py(tmp.path(), "0001_initial");

        let py_path = tmp.path().join("0001_initial.py");
        let result = load_sidecar(&py_path);

        assert!(result.is_err(), "expected Err on malformed JSON");
        let err = result.unwrap_err();
        assert!(
            matches!(err, MigrationError::Loader { .. }),
            "expected MigrationError::Loader, got: {err:?}"
        );
    }

    // ── load_dir ─────────────────────────────────────────────────────────────

    #[test]
    fn load_dir_loads_sidecars_and_skips_bare_py() {
        let tmp = tempfile::tempdir().unwrap();

        // Two sidecar-bearing migrations.
        let spec1 = make_spec("0001_initial");
        let spec2 = make_spec("0002_add_attr");
        write_sidecar(tmp.path(), "0001_initial", &spec1);
        write_py(tmp.path(), "0001_initial");
        write_sidecar(tmp.path(), "0002_add_attr", &spec2);
        write_py(tmp.path(), "0002_add_attr");

        // One legacy .py with NO sidecar — must be skipped by load_dir.
        write_py(tmp.path(), "0003_legacy");

        let graph = load_dir(tmp.path()).unwrap();

        assert_eq!(
            graph.migrations.len(),
            2,
            "expected exactly two specs from the two sidecars"
        );
        assert_eq!(
            graph.migrations[0], spec1,
            "first spec should be 0001_initial"
        );
        assert_eq!(
            graph.migrations[1], spec2,
            "second spec should be 0002_add_attr"
        );
    }

    #[test]
    fn load_dir_sorts_by_stem() {
        let tmp = tempfile::tempdir().unwrap();

        // Write in reverse order to verify sort is applied.
        let spec2 = make_spec("0002_b");
        let spec1 = make_spec("0001_a");
        write_sidecar(tmp.path(), "0002_b", &spec2);
        write_sidecar(tmp.path(), "0001_a", &spec1);

        let graph = load_dir(tmp.path()).unwrap();

        assert_eq!(graph.migrations.len(), 2);
        assert_eq!(graph.migrations[0].name, "0001_a");
        assert_eq!(graph.migrations[1].name, "0002_b");
    }

    #[test]
    fn load_dir_integration_smoke_sidecar_and_no_sidecar() {
        // Integration smoke: dir with one sidecar-bearing .py+.json pair and one
        // legacy .py-only file; load_dir returns MigrationGraph with exactly the
        // one sidecar spec, confirming prefer-sidecar / fall-back-to-None seam
        // for the pure-Rust (CLI) consumption path.
        let tmp = tempfile::tempdir().unwrap();

        let spec = make_spec("0001_initial");
        write_sidecar(tmp.path(), "0001_initial", &spec);
        write_py(tmp.path(), "0001_initial");

        // Legacy: .py only, no sidecar.
        write_py(tmp.path(), "0002_legacy");

        let graph = load_dir(tmp.path()).unwrap();

        assert_eq!(
            graph.migrations.len(),
            1,
            "load_dir must load only JSON sidecars; the bare .py must not appear"
        );
        assert_eq!(graph.migrations[0], spec);
    }

    // ── load_dir_checked (D6 drift guard) ────────────────────────────────────

    /// Build a `MigrationSpec` whose `checksum` is computed over a real `.py`
    /// text body using `migration_file_checksum`, so the drift guard accepts it.
    fn make_spec_with_real_checksum(name: &str, py_text: &str) -> MigrationSpec {
        use crate::checksum::migration_file_checksum;
        MigrationSpec {
            app_label: "test_app".to_string(),
            name: name.to_string(),
            dependencies: vec![],
            operations: vec![OperationSpec::RunTypeql {
                forward: format!("define attribute {name}, value string;"),
                reverse: None,
            }],
            declared_intent: None,
            checksum: Some(migration_file_checksum(py_text)),
            reversible: false,
        }
    }

    #[test]
    fn load_dir_checked_accepts_matching_checksum() {
        // A sidecar whose embedded checksum was computed from the same .py text
        // that is on disk must be accepted by the drift guard.
        let tmp = tempfile::tempdir().unwrap();
        let py_text = "class Migration: pass\n";

        let spec = make_spec_with_real_checksum("0001_initial", py_text);
        write_sidecar(tmp.path(), "0001_initial", &spec);
        std::fs::write(tmp.path().join("0001_initial.py"), py_text.as_bytes()).unwrap();

        let graph = load_dir_checked(tmp.path()).unwrap();
        assert_eq!(graph.migrations.len(), 1);
        assert_eq!(graph.migrations[0].name, "0001_initial");
    }

    #[test]
    fn load_dir_checked_rejects_stale_sidecar() {
        // If the .py is hand-edited AFTER the sidecar was generated, the
        // embedded checksum will disagree with the current .py text.  The
        // drift guard must reject the sidecar rather than silently executing it.
        let tmp = tempfile::tempdir().unwrap();
        let original_py_text = "class Migration: pass\n";
        let mutated_py_text = "class Migration: pass\n# hand-edited after sidecar generation\n";

        // Sidecar checksum reflects the ORIGINAL .py text.
        let spec = make_spec_with_real_checksum("0001_initial", original_py_text);
        write_sidecar(tmp.path(), "0001_initial", &spec);

        // Write the MUTATED .py text to disk — the sidecar is now stale.
        std::fs::write(
            tmp.path().join("0001_initial.py"),
            mutated_py_text.as_bytes(),
        )
        .unwrap();

        let result = load_dir_checked(tmp.path());
        assert!(
            result.is_err(),
            "load_dir_checked must reject a stale sidecar"
        );
        let err = result.unwrap_err();
        assert!(
            matches!(err, MigrationError::Loader { .. }),
            "expected MigrationError::Loader, got {err:?}"
        );
        // The error message must guide the developer to regenerate.
        let msg = err.to_string();
        assert!(
            msg.contains("sidecar drift") || msg.contains("regenerate"),
            "error message should mention sidecar drift or regenerate; got: {msg}"
        );
    }

    #[test]
    fn load_dir_checked_skips_drift_check_when_no_py_file() {
        // When there is no .py file beside the sidecar (sidecar-only migration),
        // the drift check is skipped — the sidecar is loaded as-is.
        let tmp = tempfile::tempdir().unwrap();

        // Write a sidecar with an arbitrary checksum; no .py companion.
        let spec = make_spec("0001_initial");
        write_sidecar(tmp.path(), "0001_initial", &spec);
        // Deliberately do NOT write a .py file.

        let graph = load_dir_checked(tmp.path()).unwrap();
        assert_eq!(
            graph.migrations.len(),
            1,
            "sidecar without .py companion must still be loaded"
        );
    }

    #[test]
    fn load_dir_checked_skips_drift_check_when_no_checksum_in_sidecar() {
        // A sidecar with no `checksum` field cannot be drift-checked; it is
        // accepted unconditionally (same policy as load_dir).
        let tmp = tempfile::tempdir().unwrap();
        let py_text = "class Migration: pass\n";

        let mut spec = make_spec("0001_initial");
        spec.checksum = None; // no checksum embedded
        write_sidecar(tmp.path(), "0001_initial", &spec);
        std::fs::write(tmp.path().join("0001_initial.py"), py_text.as_bytes()).unwrap();

        let graph = load_dir_checked(tmp.path()).unwrap();
        assert_eq!(graph.migrations.len(), 1);
    }
}
