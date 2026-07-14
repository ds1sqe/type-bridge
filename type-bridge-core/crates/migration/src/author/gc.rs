//! Explicit orphan cleanup guarded by the migration-tree advisory lock.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use fs2::FileExt;

use crate::author::{
    MIGRATION_TREE_LOCK, MigrationCommitManifest, MigrationTreeFormat, TREE_FORMAT_SENTINEL,
};
use crate::error::MigrationError;

/// Result of one explicit orphan-cleanup operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrphanGcReport {
    /// Relative files removed while holding the exclusive tree lock.
    pub removed: Vec<String>,
}

/// Remove stale publication temp files and manifest-less new-format final
/// files while holding the exclusive migration-tree advisory lock.
///
/// This function is intentionally explicit; TypeBridge never runs it in a
/// background task. `minimum_age` is defense in depth, while the exclusive
/// lock is the liveness guarantee that no publisher is currently active.
pub fn collect_migration_orphans(
    migrations_dir: &Path,
    minimum_age: Duration,
) -> crate::Result<OrphanGcReport> {
    fs::create_dir_all(migrations_dir).map_err(|error| io_error(migrations_dir, &error))?;
    let lock_path = migrations_dir.join(MIGRATION_TREE_LOCK);
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| io_error(&lock_path, &error))?;
    FileExt::lock_exclusive(&lock).map_err(|error| io_error(&lock_path, &error))?;

    let sentinel = read_sentinel(migrations_dir)?;
    let legacy: BTreeSet<String> = sentinel
        .map(|sentinel| sentinel.legacy_prefix.into_iter().collect())
        .unwrap_or_default();
    let (committed_stems, protected_paths) = committed_paths(migrations_dir)?;
    let committed_numbers: BTreeSet<String> = committed_stems
        .iter()
        .map(|stem| stem[..4].to_string())
        .collect();
    let legacy_numbers: BTreeSet<String> = legacy
        .iter()
        .map(|stem: &String| stem[..4].to_string())
        .collect();

    let mut candidates = Vec::new();
    visit_files(migrations_dir, migrations_dir, &mut candidates)?;
    candidates.sort();
    let now = SystemTime::now();
    let mut removed = Vec::new();
    for (relative, path) in candidates {
        if protected_paths.contains(&relative)
            || relative == TREE_FORMAT_SENTINEL
            || relative == MIGRATION_TREE_LOCK
            || relative == "snapshots/__init__.py"
        {
            continue;
        }
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let is_temp = filename.starts_with(".tmp-");
        let is_orphan = is_manifestless_final(
            &relative,
            &legacy,
            &committed_stems,
            &legacy_numbers,
            &committed_numbers,
        );
        if !is_temp && !is_orphan {
            continue;
        }
        let metadata = fs::metadata(&path).map_err(|error| io_error(&path, &error))?;
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if now.duration_since(modified).unwrap_or_default() < minimum_age {
            continue;
        }
        fs::remove_file(&path).map_err(|error| io_error(&path, &error))?;
        removed.push(relative);
    }

    Ok(OrphanGcReport { removed })
}

fn read_sentinel(migrations_dir: &Path) -> crate::Result<Option<MigrationTreeFormat>> {
    let path = migrations_dir.join(TREE_FORMAT_SENTINEL);
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read(&path).map_err(|error| io_error(&path, &error))?;
    serde_json::from_slice(&contents)
        .map(Some)
        .map_err(MigrationError::from)
}

fn committed_paths(migrations_dir: &Path) -> crate::Result<(BTreeSet<String>, BTreeSet<String>)> {
    let mut stems = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for entry in fs::read_dir(migrations_dir).map_err(|error| io_error(migrations_dir, &error))? {
        let entry = entry.map_err(|error| io_error(migrations_dir, &error))?;
        let path = entry.path();
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(stem) = filename.strip_suffix(".manifest.json") else {
            continue;
        };
        if !is_migration_stem(stem) {
            continue;
        }
        let contents = match fs::read(&path) {
            Ok(contents) => contents,
            Err(_) => continue,
        };
        let Ok(manifest) = serde_json::from_slice::<MigrationCommitManifest>(&contents) else {
            continue;
        };
        if manifest.migration_name != stem {
            continue;
        }
        stems.insert(stem.to_string());
        paths.insert(filename.to_string());
        paths.extend(manifest.files.into_iter().map(|file| file.path));
    }
    Ok((stems, paths))
}

fn visit_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, PathBuf)>,
) -> crate::Result<()> {
    for entry in fs::read_dir(directory).map_err(|error| io_error(directory, &error))? {
        let entry = entry.map_err(|error| io_error(directory, &error))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| io_error(&path, &error))?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            visit_files(root, &path, files)?;
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)
                .expect("visited path stays below root")
                .to_string_lossy()
                .replace('\\', "/");
            files.push((relative, path));
        }
    }
    Ok(())
}

fn is_manifestless_final(
    relative: &str,
    legacy_stems: &BTreeSet<String>,
    committed_stems: &BTreeSet<String>,
    legacy_numbers: &BTreeSet<String>,
    committed_numbers: &BTreeSet<String>,
) -> bool {
    if !relative.contains('/') {
        let stem = relative
            .strip_suffix(".py")
            .or_else(|| relative.strip_suffix(".json"));
        return stem.is_some_and(|stem| {
            is_migration_stem(stem)
                && !legacy_stems.contains(stem)
                && !committed_stems.contains(stem)
        });
    }
    let segments: Vec<&str> = relative.split('/').collect();
    if let ["snapshots", version, ..] = segments.as_slice()
        && let Some(number) = version.strip_prefix('v')
    {
        return number.len() == 4
            && !legacy_numbers.contains(number)
            && !committed_numbers.contains(number);
    }
    if let ["ext", _namespace, stem, ..] = segments.as_slice() {
        return is_migration_stem(stem)
            && !legacy_stems.contains(*stem)
            && !committed_stems.contains(*stem);
    }
    false
}

fn is_migration_stem(stem: &str) -> bool {
    let bytes = stem.as_bytes();
    bytes.len() >= 6 && bytes[..4].iter().all(|byte| byte.is_ascii_digit()) && bytes[4] == b'_'
}

fn io_error(path: &Path, error: &std::io::Error) -> MigrationError {
    MigrationError::Loader {
        message: format!("migration orphan GC failed at {}: {error}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::thread;

    use crate::author::{
        AuthoredArtifact, AuthoredMigration, ExistingArtifactPolicy, publish_composed_migration,
    };
    use crate::checksum::migration_file_checksum;
    use crate::spec::MigrationSpec;

    use super::*;

    fn publish_one(dir: &Path) {
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
                    relative_path: "snapshots/v0001/snapshot.json".to_string(),
                    contents: b"{}".to_vec(),
                },
            ],
        };
        let composed = authored.composer().compose().unwrap();
        publish_composed_migration(dir, &composed, ExistingArtifactPolicy::ValidateIdentical)
            .unwrap();
    }

    #[test]
    fn gc_removes_only_temp_and_manifestless_new_format_files() {
        let dir = tempfile::tempdir().unwrap();
        publish_one(dir.path());
        fs::write(dir.path().join(".tmp-dead-file"), b"temp").unwrap();
        fs::write(dir.path().join("0002_orphan.py"), b"orphan").unwrap();
        fs::write(dir.path().join("keep.txt"), b"user").unwrap();

        let report = collect_migration_orphans(dir.path(), Duration::ZERO).unwrap();
        assert!(report.removed.contains(&".tmp-dead-file".to_string()));
        assert!(report.removed.contains(&"0002_orphan.py".to_string()));
        assert!(dir.path().join("0001_initial.py").exists());
        assert!(dir.path().join("0001_initial.manifest.json").exists());
        assert!(dir.path().join("keep.txt").exists());
    }

    #[test]
    fn exclusive_gc_waits_for_a_live_shared_publisher_lock() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(MIGRATION_TREE_LOCK);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        FileExt::lock_shared(&lock).unwrap();
        let root = dir.path().to_path_buf();
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            let report = collect_migration_orphans(&root, Duration::ZERO).unwrap();
            sender.send(report).unwrap();
        });

        assert!(receiver.recv_timeout(Duration::from_millis(50)).is_err());
        FileExt::unlock(&lock).unwrap();
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.join().unwrap();
    }
}
