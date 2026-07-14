//! Per-migration commit manifests and extension-aware in-memory composition.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::author::build::{AuthoredArtifact, AuthoredMigration};
use crate::error::MigrationError;

/// Version tag for immutable per-migration commit manifests.
pub const COMMIT_MANIFEST_FORMAT_V1: &str = "type-bridge-migration-manifest-v1";
/// Filename of the immutable tree-format sentinel.
pub const TREE_FORMAT_SENTINEL: &str = ".typebridge-manifest-format.json";
/// Version tag recorded by the immutable tree-format sentinel.
pub const TREE_FORMAT_V1: &str = "type-bridge-migration-tree-v1";
/// Well-known advisory lock filename shared by publication and explicit GC.
pub const MIGRATION_TREE_LOCK: &str = ".tb-lock";

/// One extension's manifest metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestExtension {
    /// Registered embedder namespace.
    pub namespace: String,
    /// Whether semantic consumers must understand this extension.
    #[serde(default)]
    pub critical: bool,
}

/// One hash-covered file in a migration commit manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFile {
    /// Normalized path relative to the migrations directory.
    pub path: String,
    /// Exact byte length.
    pub size: u64,
    /// Lowercase full SHA-256 digest.
    pub sha256: String,
    /// Extension metadata; absent for canonical TypeBridge files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension: Option<ManifestExtension>,
}

/// Immutable commit record for one migration version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationCommitManifest {
    /// Versioned manifest format tag.
    pub format: String,
    /// Full migration stem, such as `0003_add_assignment`.
    pub migration_name: String,
    /// Complete sorted hash-covered file set.
    pub files: Vec<ManifestFile>,
}

/// Immutable marker separating an exact legacy prefix from manifested
/// versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationTreeFormat {
    /// Versioned tree-format tag.
    pub format: String,
    /// Exact sorted migration stems that remain governed by legacy discovery.
    pub legacy_prefix: Vec<String>,
}

/// Caller-owned extension staged before manifest computation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoredExtension {
    /// Registered embedder namespace.
    pub namespace: String,
    /// Path below `ext/<namespace>/<migration_name>/`.
    pub relative_path: String,
    /// Exact extension bytes.
    pub contents: Vec<u8>,
    /// Whether semantic consumers must understand this extension.
    pub critical: bool,
}

/// Complete deterministic migration byte set ready for publication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposedMigration {
    /// Full migration stem.
    pub migration_name: String,
    /// Canonical and extension files, excluding the manifest itself.
    pub files: Vec<AuthoredArtifact>,
    /// Parsed commit manifest.
    pub manifest: MigrationCommitManifest,
    /// Canonical manifest JSON bytes, including a trailing newline.
    pub manifest_bytes: Vec<u8>,
}

impl ComposedMigration {
    /// Final manifest filename relative to the migrations directory.
    pub fn manifest_path(&self) -> String {
        format!("{}.manifest.json", self.migration_name)
    }

    /// Return the full deterministic publication set, with the manifest last.
    pub fn complete_files(&self) -> Vec<AuthoredArtifact> {
        let mut files = self.files.clone();
        files.push(AuthoredArtifact {
            relative_path: self.manifest_path(),
            contents: self.manifest_bytes.clone(),
        });
        files
    }

    /// Namespaces the composing caller can declare understood during the
    /// mandatory post-publication checked read-back.
    pub fn extension_namespaces(&self) -> BTreeSet<String> {
        self.manifest
            .files
            .iter()
            .filter_map(|file| file.extension.as_ref().map(|ext| ext.namespace.clone()))
            .collect()
    }
}

/// Mutable composition step over an immutable canonical authored result.
pub struct MigrationComposer<'a> {
    authored: &'a AuthoredMigration,
    extensions: Vec<AuthoredExtension>,
}

impl AuthoredMigration {
    /// Begin extension-aware composition without mutating canonical authored
    /// bytes.
    pub fn composer(&self) -> MigrationComposer<'_> {
        MigrationComposer {
            authored: self,
            extensions: Vec::new(),
        }
    }
}

impl<'a> MigrationComposer<'a> {
    /// Add one namespaced extension before manifest computation.
    pub fn add_extension(
        &mut self,
        namespace: impl Into<String>,
        relative_path: impl Into<String>,
        contents: impl Into<Vec<u8>>,
        critical: bool,
    ) -> crate::Result<&mut Self> {
        let extension = AuthoredExtension {
            namespace: namespace.into(),
            relative_path: relative_path.into(),
            contents: contents.into(),
            critical,
        };
        validate_namespace(&extension.namespace)?;
        validate_extension_relative_path(&extension.relative_path)?;
        self.extensions.push(extension);
        Ok(self)
    }

    /// Compose canonical files, extensions, and a deterministic manifest in
    /// memory.
    pub fn compose(&self) -> crate::Result<ComposedMigration> {
        let mut files = self
            .authored
            .files
            .iter()
            .filter(|file| file.relative_path != "snapshots/__init__.py")
            .cloned()
            .collect::<Vec<_>>();

        for file in &files {
            validate_canonical_path(&self.authored.migration_name, &file.relative_path)?;
        }

        let mut extension_by_path = BTreeMap::<String, ManifestExtension>::new();
        for extension in &self.extensions {
            let path = format!(
                "ext/{}/{}/{}",
                extension.namespace, self.authored.migration_name, extension.relative_path
            );
            extension_by_path.insert(
                path.clone(),
                ManifestExtension {
                    namespace: extension.namespace.clone(),
                    critical: extension.critical,
                },
            );
            files.push(AuthoredArtifact {
                relative_path: path,
                contents: extension.contents.clone(),
            });
        }

        files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        validate_unique_paths(&files)?;

        let manifest = MigrationCommitManifest {
            format: COMMIT_MANIFEST_FORMAT_V1.to_string(),
            migration_name: self.authored.migration_name.clone(),
            files: files
                .iter()
                .map(|file| ManifestFile {
                    path: file.relative_path.clone(),
                    size: file.contents.len() as u64,
                    sha256: sha256(&file.contents),
                    extension: extension_by_path.get(&file.relative_path).cloned(),
                })
                .collect(),
        };
        let mut manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
        manifest_bytes.push(b'\n');

        Ok(ComposedMigration {
            migration_name: self.authored.migration_name.clone(),
            files,
            manifest,
            manifest_bytes,
        })
    }
}

/// Compute a lowercase full SHA-256 digest.
pub(crate) fn sha256(contents: &[u8]) -> String {
    let digest = Sha256::digest(contents);
    format!("{digest:x}")
}

/// Validate a path read from a manifest or supplied by an embedder.
pub(crate) fn validate_normalized_relative_path(path: &str) -> crate::Result<()> {
    if path.is_empty()
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains('\\')
        || path.split('/').any(str::is_empty)
    {
        return invalid_path(
            path,
            "must be a normalized non-empty '/'-separated relative path",
        );
    }
    if Path::new(path).components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return invalid_path(path, "must stay inside the migrations directory");
    }
    for segment in path.split('/') {
        if segment == "." || segment == ".." {
            return invalid_path(path, "must not contain '.' or '..' segments");
        }
        if is_windows_reserved_name(segment) {
            return invalid_path(path, "contains a Windows reserved device name");
        }
    }
    Ok(())
}

fn validate_canonical_path(migration_name: &str, path: &str) -> crate::Result<()> {
    validate_normalized_relative_path(path)?;
    let root_py = format!("{migration_name}.py");
    let root_json = format!("{migration_name}.json");
    let version = &migration_name[..4];
    let snapshot_prefix = format!("snapshots/v{version}/");
    if path == root_py || path == root_json || path.starts_with(&snapshot_prefix) {
        return Ok(());
    }
    invalid_path(path, "is not a canonical file for this migration version")
}

fn validate_namespace(namespace: &str) -> crate::Result<()> {
    let valid = !namespace.is_empty()
        && namespace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    let reserved = matches!(
        namespace.to_ascii_lowercase().as_str(),
        "typebridge" | "type-bridge" | "tb"
    );
    if valid && !reserved {
        Ok(())
    } else {
        Err(MigrationError::AuthoringInput {
            message: format!(
                "extension namespace {namespace:?} must use ASCII letters/digits/._- and must not use a TypeBridge-reserved namespace"
            ),
        })
    }
}

fn validate_extension_relative_path(path: &str) -> crate::Result<()> {
    validate_normalized_relative_path(path)?;
    if path.starts_with("ext/") {
        return invalid_path(
            path,
            "is relative to its namespace and must not start with ext/",
        );
    }
    Ok(())
}

fn validate_unique_paths(files: &[AuthoredArtifact]) -> crate::Result<()> {
    let mut exact = BTreeSet::new();
    let mut folded = BTreeMap::<String, String>::new();
    for file in files {
        validate_normalized_relative_path(&file.relative_path)?;
        if !exact.insert(file.relative_path.clone()) {
            return invalid_path(&file.relative_path, "appears more than once");
        }
        let key = file.relative_path.to_ascii_lowercase();
        if let Some(previous) = folded.insert(key, file.relative_path.clone()) {
            return Err(MigrationError::AuthoringInput {
                message: format!(
                    "artifact paths {previous:?} and {:?} collide case-insensitively",
                    file.relative_path
                ),
            });
        }
    }
    Ok(())
}

fn is_windows_reserved_name(segment: &str) -> bool {
    let base = segment
        .split_once('.')
        .map_or(segment, |(base, _)| base)
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && matches!(&base[..3], "COM" | "LPT")
            && matches!(base.as_bytes()[3], b'1'..=b'9'))
}

fn invalid_path<T>(path: &str, reason: &str) -> crate::Result<T> {
    Err(MigrationError::AuthoringInput {
        message: format!("artifact path {path:?} {reason}"),
    })
}

#[cfg(test)]
mod tests {
    use crate::spec::MigrationSpec;

    use super::*;

    fn authored() -> AuthoredMigration {
        AuthoredMigration {
            migration_name: "0001_initial".to_string(),
            python_source: "py".to_string(),
            spec: MigrationSpec {
                app_label: "migrations".to_string(),
                name: "0001_initial".to_string(),
                dependencies: vec![],
                operations: vec![],
                declared_intent: None,
                checksum: None,
                reversible: true,
            },
            files: vec![
                AuthoredArtifact {
                    relative_path: "0001_initial.py".to_string(),
                    contents: b"py".to_vec(),
                },
                AuthoredArtifact {
                    relative_path: "0001_initial.json".to_string(),
                    contents: b"json".to_vec(),
                },
                AuthoredArtifact {
                    relative_path: "snapshots/__init__.py".to_string(),
                    contents: b"derived".to_vec(),
                },
                AuthoredArtifact {
                    relative_path: "snapshots/v0001/schema.tql".to_string(),
                    contents: b"define".to_vec(),
                },
            ],
        }
    }

    #[test]
    fn compose_hashes_canonical_and_namespaced_extension_bytes() {
        let authored = authored();
        let mut composer = authored.composer();
        composer
            .add_extension("paladin", "companion.json", b"semantic".to_vec(), true)
            .unwrap();
        let composed = composer.compose().unwrap();

        assert_eq!(
            composed
                .manifest
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "0001_initial.json",
                "0001_initial.py",
                "ext/paladin/0001_initial/companion.json",
                "snapshots/v0001/schema.tql",
            ]
        );
        assert_eq!(
            composed.complete_files().last().unwrap().relative_path,
            "0001_initial.manifest.json"
        );
        let extension = composed
            .manifest
            .files
            .iter()
            .find(|file| file.extension.is_some())
            .unwrap();
        assert!(extension.extension.as_ref().unwrap().critical);
        assert_eq!(extension.sha256, sha256(b"semantic"));
    }

    #[test]
    fn compose_rejects_traversal_case_collision_and_reserved_names() {
        let authored = authored();
        for path in ["../escape", "folder\\escape", "CON/file", "a//b"] {
            let mut composer = authored.composer();
            assert!(
                composer
                    .add_extension("paladin", path, b"x".to_vec(), false)
                    .is_err()
            );
        }

        let mut composer = authored.composer();
        composer
            .add_extension("paladin", "Meta.json", b"one".to_vec(), false)
            .unwrap();
        composer
            .add_extension("paladin", "meta.json", b"two".to_vec(), false)
            .unwrap();
        assert!(composer.compose().is_err());
    }
}
