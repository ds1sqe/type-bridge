//! Crash-atomic manifest-last publication for composed migrations.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fs2::FileExt;

use crate::author::manifest::{
    ComposedMigration, MIGRATION_TREE_LOCK, MigrationTreeFormat, TREE_FORMAT_SENTINEL,
    TREE_FORMAT_V1,
};
use crate::author::write::ExistingArtifactPolicy;
use crate::error::MigrationError;
use crate::loader::load_dir_checked_with_extensions;

/// Observable publication checkpoints used by cross-platform fault-injection
/// tests and embedders that need precise crash-boundary evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicationPoint {
    /// A nonce-scoped temporary file has been fully written.
    AfterWrite(String),
    /// A nonce-scoped temporary file has been synced as best-effort hardening.
    AfterSync(String),
    /// A non-manifest final name has been linked without clobbering.
    AfterFilePublish(String),
    /// The per-version manifest commit point has been published.
    AfterManifestPublish(String),
}

/// Publish a composed migration with no fault-injection observer.
pub fn publish_composed_migration(
    migrations_dir: &Path,
    composed: &ComposedMigration,
    policy: ExistingArtifactPolicy,
) -> crate::Result<()> {
    publish_composed_migration_with_observer(migrations_dir, composed, policy, |_| Ok(()))
}

/// Publish a composed migration and invoke `observer` after every durable
/// checkpoint.
///
/// The per-version manifest is always the final authoritative name. Any
/// observer error simulates an interruption at that point; manifest-less final
/// files remain invisible to the checked loader and can be re-used by an
/// identical retry or removed by explicit GC.
pub fn publish_composed_migration_with_observer<F>(
    migrations_dir: &Path,
    composed: &ComposedMigration,
    policy: ExistingArtifactPolicy,
    mut observer: F,
) -> crate::Result<()>
where
    F: FnMut(PublicationPoint) -> crate::Result<()>,
{
    fs::create_dir_all(migrations_dir).map_err(|error| io_error(migrations_dir, &error))?;
    let lock = open_lock(migrations_dir)?;
    FileExt::lock_shared(&lock).map_err(|error| io_error(&lock_path(migrations_dir), &error))?;

    ensure_tree_format_sentinel(migrations_dir, &mut observer)?;
    if committed_manifest_is_identical(migrations_dir, composed, policy)? {
        checked_read_back(migrations_dir, composed)?;
        return Ok(());
    }
    validate_next_version(migrations_dir, &composed.migration_name)?;
    validate_existing_files(migrations_dir, composed, policy)?;
    ensure_snapshot_package_marker(migrations_dir, &mut observer)?;

    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let mut staged = Vec::<PathBuf>::new();
    let result = (|| {
        for artifact in &composed.files {
            let final_path = migrations_dir.join(&artifact.relative_path);
            if final_path.exists() {
                continue;
            }
            let temp_path = stage_file(
                migrations_dir,
                &artifact.relative_path,
                &artifact.contents,
                &nonce,
                &mut observer,
            )?;
            staged.push(temp_path.clone());
            publish_no_clobber(&temp_path, &final_path, &artifact.contents, policy)?;
            fs::remove_file(&temp_path).map_err(|error| io_error(&temp_path, &error))?;
            staged.retain(|path| path != &temp_path);
            observer(PublicationPoint::AfterFilePublish(
                artifact.relative_path.clone(),
            ))?;
        }

        let manifest_relative = composed.manifest_path();
        let manifest_path = migrations_dir.join(&manifest_relative);
        let manifest_temp = stage_file(
            migrations_dir,
            &manifest_relative,
            &composed.manifest_bytes,
            &nonce,
            &mut observer,
        )?;
        staged.push(manifest_temp.clone());
        publish_no_clobber(
            &manifest_temp,
            &manifest_path,
            &composed.manifest_bytes,
            policy,
        )?;
        fs::remove_file(&manifest_temp).map_err(|error| io_error(&manifest_temp, &error))?;
        staged.retain(|path| path != &manifest_temp);
        sync_parent(&manifest_path)?;
        observer(PublicationPoint::AfterManifestPublish(manifest_relative))?;
        checked_read_back(migrations_dir, composed)
    })();

    for path in staged {
        let _ = fs::remove_file(path);
    }
    result
}

fn open_lock(migrations_dir: &Path) -> crate::Result<File> {
    let path = lock_path(migrations_dir);
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|error| io_error(&path, &error))
}

fn lock_path(migrations_dir: &Path) -> PathBuf {
    migrations_dir.join(MIGRATION_TREE_LOCK)
}

fn ensure_tree_format_sentinel<F>(migrations_dir: &Path, observer: &mut F) -> crate::Result<()>
where
    F: FnMut(PublicationPoint) -> crate::Result<()>,
{
    let path = migrations_dir.join(TREE_FORMAT_SENTINEL);
    if path.exists() {
        let bytes = fs::read(&path).map_err(|error| io_error(&path, &error))?;
        let sentinel: MigrationTreeFormat = serde_json::from_slice(&bytes)?;
        if sentinel.format != TREE_FORMAT_V1 {
            return Err(MigrationError::AuthoringInput {
                message: format!(
                    "migration tree {} uses unsupported format {:?}",
                    migrations_dir.display(),
                    sentinel.format
                ),
            });
        }
        return Ok(());
    }

    let legacy = crate::loader::load_dir_checked(migrations_dir)?;
    let sentinel = MigrationTreeFormat {
        format: TREE_FORMAT_V1.to_string(),
        legacy_prefix: legacy
            .migrations
            .into_iter()
            .map(|migration| migration.name)
            .collect(),
    };
    let mut contents = serde_json::to_vec_pretty(&sentinel)?;
    contents.push(b'\n');
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let temp = stage_file(
        migrations_dir,
        TREE_FORMAT_SENTINEL,
        &contents,
        &nonce,
        observer,
    )?;
    match fs::hard_link(&temp, &path) {
        Ok(()) => {
            sync_parent(&path)?;
            observer(PublicationPoint::AfterFilePublish(
                TREE_FORMAT_SENTINEL.to_string(),
            ))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = fs::read(&path).map_err(|read_error| io_error(&path, &read_error))?;
            let existing: MigrationTreeFormat = serde_json::from_slice(&existing)?;
            if existing != sentinel {
                let _ = fs::remove_file(&temp);
                return Err(MigrationError::AuthoringInput {
                    message:
                        "concurrent first publisher established a different legacy-prefix boundary"
                            .to_string(),
                });
            }
        }
        Err(error) => {
            let _ = fs::remove_file(&temp);
            return Err(io_error(&path, &error));
        }
    }
    fs::remove_file(&temp).map_err(|error| io_error(&temp, &error))?;
    Ok(())
}

fn committed_manifest_is_identical(
    migrations_dir: &Path,
    composed: &ComposedMigration,
    policy: ExistingArtifactPolicy,
) -> crate::Result<bool> {
    let path = migrations_dir.join(composed.manifest_path());
    if !path.exists() {
        return Ok(false);
    }
    if policy == ExistingArtifactPolicy::Fail {
        return Err(MigrationError::AuthoringInput {
            message: format!("manifest {} already exists", path.display()),
        });
    }
    let existing = fs::read(&path).map_err(|error| io_error(&path, &error))?;
    if existing == composed.manifest_bytes {
        Ok(true)
    } else {
        Err(MigrationError::AuthoringInput {
            message: format!(
                "manifest {} already exists with different contents",
                path.display()
            ),
        })
    }
}

fn validate_next_version(migrations_dir: &Path, migration_name: &str) -> crate::Result<()> {
    let sentinel_path = migrations_dir.join(TREE_FORMAT_SENTINEL);
    let sentinel: MigrationTreeFormat = serde_json::from_slice(
        &fs::read(&sentinel_path).map_err(|error| io_error(&sentinel_path, &error))?,
    )?;
    let mut latest = sentinel
        .legacy_prefix
        .last()
        .map(|stem| stem[..4].parse::<u32>())
        .transpose()
        .map_err(|error| MigrationError::AuthoringInput {
            message: format!("invalid legacy-prefix migration number: {error}"),
        })?
        .unwrap_or(0);
    for entry in fs::read_dir(migrations_dir).map_err(|error| io_error(migrations_dir, &error))? {
        let entry = entry.map_err(|error| io_error(migrations_dir, &error))?;
        let Some(filename) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(stem) = filename.strip_suffix(".manifest.json") else {
            continue;
        };
        if stem.len() >= 5
            && stem.as_bytes()[..4]
                .iter()
                .all(|byte| byte.is_ascii_digit())
            && stem.as_bytes()[4] == b'_'
        {
            let number =
                stem[..4]
                    .parse::<u32>()
                    .map_err(|error| MigrationError::AuthoringInput {
                        message: format!("invalid committed migration number in {stem:?}: {error}"),
                    })?;
            latest = latest.max(number);
        }
    }
    let requested =
        migration_name[..4]
            .parse::<u32>()
            .map_err(|error| MigrationError::AuthoringInput {
                message: format!(
                    "invalid authored migration number in {migration_name:?}: {error}"
                ),
            })?;
    if requested == latest + 1 {
        Ok(())
    } else {
        Err(MigrationError::AuthoringInput {
            message: format!(
                "manifested publication must append version {:04}; requested {migration_name}",
                latest + 1
            ),
        })
    }
}

fn validate_existing_files(
    migrations_dir: &Path,
    composed: &ComposedMigration,
    policy: ExistingArtifactPolicy,
) -> crate::Result<()> {
    for artifact in &composed.files {
        let path = migrations_dir.join(&artifact.relative_path);
        if !path.exists() {
            continue;
        }
        if policy == ExistingArtifactPolicy::Fail {
            return Err(MigrationError::AuthoringInput {
                message: format!("artifact {} already exists", path.display()),
            });
        }
        let existing = fs::read(&path).map_err(|error| io_error(&path, &error))?;
        if existing != artifact.contents {
            return Err(MigrationError::AuthoringInput {
                message: format!(
                    "artifact {} already exists with different contents",
                    path.display()
                ),
            });
        }
    }
    Ok(())
}

fn ensure_snapshot_package_marker<F>(migrations_dir: &Path, observer: &mut F) -> crate::Result<()>
where
    F: FnMut(PublicationPoint) -> crate::Result<()>,
{
    const CONTENTS: &[u8] = b"# TypeBridge migration snapshots package\n";
    let relative = "snapshots/__init__.py";
    let path = migrations_dir.join(relative);
    if path.exists() {
        return Ok(());
    }
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let temp = stage_file(migrations_dir, relative, CONTENTS, &nonce, observer)?;
    match fs::hard_link(&temp, &path) {
        Ok(()) => {
            sync_parent(&path)?;
            observer(PublicationPoint::AfterFilePublish(relative.to_string()))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            let _ = fs::remove_file(&temp);
            return Err(io_error(&path, &error));
        }
    }
    fs::remove_file(&temp).map_err(|error| io_error(&temp, &error))?;
    Ok(())
}

fn stage_file<F>(
    migrations_dir: &Path,
    relative_path: &str,
    contents: &[u8],
    nonce: &str,
    observer: &mut F,
) -> crate::Result<PathBuf>
where
    F: FnMut(PublicationPoint) -> crate::Result<()>,
{
    let final_path = migrations_dir.join(relative_path);
    let parent = final_path
        .parent()
        .ok_or_else(|| MigrationError::AuthoringInput {
            message: format!("artifact {relative_path:?} has no parent directory"),
        })?;
    fs::create_dir_all(parent).map_err(|error| io_error(parent, &error))?;
    let filename = final_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| MigrationError::AuthoringInput {
            message: format!("artifact {relative_path:?} has a non-UTF-8 filename"),
        })?;
    let temp_path = parent.join(format!(".tmp-{nonce}-{filename}"));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp_path)
        .map_err(|error| io_error(&temp_path, &error))?;
    file.write_all(contents)
        .map_err(|error| io_error(&temp_path, &error))?;
    observer(PublicationPoint::AfterWrite(relative_path.to_string()))?;
    file.sync_all()
        .map_err(|error| io_error(&temp_path, &error))?;
    observer(PublicationPoint::AfterSync(relative_path.to_string()))?;
    Ok(temp_path)
}

fn publish_no_clobber(
    temp_path: &Path,
    final_path: &Path,
    expected: &[u8],
    policy: ExistingArtifactPolicy,
) -> crate::Result<()> {
    match fs::hard_link(temp_path, final_path) {
        Ok(()) => {
            sync_parent(final_path)?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if policy == ExistingArtifactPolicy::ValidateIdentical {
                let existing =
                    fs::read(final_path).map_err(|read_error| io_error(final_path, &read_error))?;
                if existing == expected {
                    return Ok(());
                }
            }
            Err(MigrationError::AuthoringInput {
                message: format!(
                    "artifact {} was concurrently published with different contents",
                    final_path.display()
                ),
            })
        }
        Err(error) => Err(io_error(final_path, &error)),
    }
}

fn checked_read_back(migrations_dir: &Path, composed: &ComposedMigration) -> crate::Result<()> {
    let known: BTreeSet<String> = composed.extension_namespaces();
    let loaded = load_dir_checked_with_extensions(migrations_dir, &known)?;
    if loaded
        .manifests
        .iter()
        .any(|manifest| manifest == &composed.manifest)
    {
        Ok(())
    } else {
        Err(MigrationError::Loader {
            message: format!(
                "post-publication checked read-back did not expose {}",
                composed.migration_name
            ),
        })
    }
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> crate::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| io_error(parent, &error))
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path) -> crate::Result<()> {
    // Windows exposes no directory-fsync contract equivalent to POSIX. The
    // guarantee is loader-visible crash atomicity, not power-loss durability.
    Ok(())
}

fn io_error(path: &Path, error: &std::io::Error) -> MigrationError {
    MigrationError::Loader {
        message: format!(
            "migration publication IO failed at {}: {error}",
            path.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::thread;

    use crate::author::{AuthoredArtifact, AuthoredMigration};
    use crate::checksum::migration_file_checksum;
    use crate::spec::{MigrationDependencySpec, MigrationSpec};

    use super::*;

    fn composed() -> ComposedMigration {
        composed_with_extension(b"semantic")
    }

    fn composed_with_extension(extension_contents: &[u8]) -> ComposedMigration {
        let stem = "0001_initial";
        let py = "class Migration: pass\n";
        let spec = MigrationSpec {
            app_label: "migrations".to_string(),
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
                    relative_path: "snapshots/__init__.py".to_string(),
                    contents: b"derived".to_vec(),
                },
                AuthoredArtifact {
                    relative_path: "snapshots/v0001/snapshot.json".to_string(),
                    contents: b"{}".to_vec(),
                },
            ],
        };
        let mut composer = authored.composer();
        composer
            .add_extension(
                "paladin",
                "companion.json",
                extension_contents.to_vec(),
                true,
            )
            .unwrap();
        composer.compose().unwrap()
    }

    #[test]
    fn publishes_manifest_last_and_checked_read_back_exposes_complete_version() {
        let dir = tempfile::tempdir().unwrap();
        let composed = composed();
        let mut points = Vec::new();
        publish_composed_migration_with_observer(
            dir.path(),
            &composed,
            ExistingArtifactPolicy::ValidateIdentical,
            |point| {
                points.push(point);
                Ok(())
            },
        )
        .unwrap();

        assert!(matches!(
            points.last(),
            Some(PublicationPoint::AfterManifestPublish(_))
        ));
        let known = BTreeSet::from(["paladin".to_string()]);
        let loaded = load_dir_checked_with_extensions(dir.path(), &known).unwrap();
        assert_eq!(loaded.graph.migrations.len(), 1);
        assert_eq!(loaded.extensions[0].contents, b"semantic");
    }

    #[test]
    fn every_injected_failure_is_prior_or_complete_and_identical_retry_recovers() {
        let probe_dir = tempfile::tempdir().unwrap();
        let composed = composed();
        let mut all_points = Vec::new();
        publish_composed_migration_with_observer(
            probe_dir.path(),
            &composed,
            ExistingArtifactPolicy::ValidateIdentical,
            |point| {
                all_points.push(point);
                Ok(())
            },
        )
        .unwrap();

        let known = BTreeSet::from(["paladin".to_string()]);
        for fail_at in 0..all_points.len() {
            let dir = tempfile::tempdir().unwrap();
            let mut index = 0usize;
            let error = publish_composed_migration_with_observer(
                dir.path(),
                &composed,
                ExistingArtifactPolicy::ValidateIdentical,
                |_| {
                    let current = index;
                    index += 1;
                    if current == fail_at {
                        Err(MigrationError::Loader {
                            message: format!("injected failure at {fail_at}"),
                        })
                    } else {
                        Ok(())
                    }
                },
            )
            .expect_err("selected checkpoint must fail");
            assert!(error.to_string().contains("injected failure"));

            let visible = load_dir_checked_with_extensions(dir.path(), &known).unwrap();
            assert!(visible.graph.migrations.len() <= 1);
            if visible.graph.migrations.len() == 1 {
                assert_eq!(visible.manifests, vec![composed.manifest.clone()]);
                assert_eq!(visible.extensions[0].contents, b"semantic");
            }

            publish_composed_migration(
                dir.path(),
                &composed,
                ExistingArtifactPolicy::ValidateIdentical,
            )
            .unwrap();
            let recovered = load_dir_checked_with_extensions(dir.path(), &known).unwrap();
            assert_eq!(recovered.graph.migrations.len(), 1);
        }
    }

    #[test]
    fn first_manifested_publish_freezes_and_preserves_exact_legacy_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let legacy_py = "class Migration: pass\n";
        let legacy = MigrationSpec {
            app_label: "migrations".to_string(),
            name: "0001_initial".to_string(),
            dependencies: vec![],
            operations: vec![],
            declared_intent: None,
            checksum: Some(migration_file_checksum(legacy_py)),
            reversible: true,
        };
        fs::write(dir.path().join("0001_initial.py"), legacy_py).unwrap();
        fs::write(
            dir.path().join("0001_initial.json"),
            serde_json::to_vec(&legacy).unwrap(),
        )
        .unwrap();

        let stem = "0002_semantic";
        let py = "class Migration: pass\n";
        let spec = MigrationSpec {
            app_label: "migrations".to_string(),
            name: stem.to_string(),
            dependencies: vec![MigrationDependencySpec {
                app_label: "migrations".to_string(),
                migration_name: "0001_initial".to_string(),
            }],
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
                    relative_path: "snapshots/v0002/snapshot.json".to_string(),
                    contents: b"{}".to_vec(),
                },
            ],
        };
        let composed = authored.composer().compose().unwrap();
        publish_composed_migration(
            dir.path(),
            &composed,
            ExistingArtifactPolicy::ValidateIdentical,
        )
        .unwrap();

        let sentinel: MigrationTreeFormat =
            serde_json::from_slice(&fs::read(dir.path().join(TREE_FORMAT_SENTINEL)).unwrap())
                .unwrap();
        assert_eq!(sentinel.legacy_prefix, vec!["0001_initial"]);
        let loaded = crate::loader::load_dir_checked(dir.path()).unwrap();
        assert_eq!(
            loaded
                .migrations
                .iter()
                .map(|migration| migration.name.as_str())
                .collect::<Vec<_>>(),
            vec!["0001_initial", "0002_semantic"]
        );
    }

    #[test]
    fn same_version_race_is_idempotent_or_a_deterministic_collision() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let composed = composed();
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let root = root.clone();
            let composed = composed.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                publish_composed_migration(
                    &root,
                    &composed,
                    ExistingArtifactPolicy::ValidateIdentical,
                )
            }));
        }
        for handle in handles {
            handle.join().unwrap().unwrap();
        }

        let conflicting = composed_with_extension(b"different");
        assert!(
            publish_composed_migration(
                dir.path(),
                &conflicting,
                ExistingArtifactPolicy::ValidateIdentical,
            )
            .is_err()
        );
    }
}
