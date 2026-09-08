//! Validated filesystem writer for authored migrations.
//!
//! Authoring is pure and in-memory; this writer is the only place artifact
//! bytes touch disk. It validates the complete write set against existing
//! files *before* writing anything, so a collision cannot leave a partial
//! artifact set behind.

use std::fs;
use std::path::{Component, Path};

use crate::author::build::AuthoredMigration;
use crate::error::MigrationError;

/// How to treat artifact paths that already exist on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingArtifactPolicy {
    /// Existing files must be byte-identical to the authored contents;
    /// anything else is a collision error. Snapshots are append-only, so
    /// re-writing the same migration is idempotent under this policy.
    ValidateIdentical,
    /// Any existing artifact file is a collision error.
    Fail,
}

/// Write every artifact of `authored` under `migrations_dir`.
///
/// The snapshots package marker (`snapshots/__init__.py`) is only created
/// when absent — an existing marker is left untouched, matching the
/// historical append-only snapshot behavior.
///
/// # Errors
///
/// - [`MigrationError::AuthoringInput`] – an artifact path escapes the
///   migrations directory, or a collision/drift was detected. Nothing has
///   been written when this is returned.
/// - [`MigrationError::Loader`] – filesystem IO failed.
pub fn write_authored_migration(
    migrations_dir: &Path,
    authored: &AuthoredMigration,
    policy: ExistingArtifactPolicy,
) -> crate::Result<()> {
    // Validate the full write set before touching anything.
    for artifact in &authored.files {
        validate_relative_path(&artifact.relative_path)?;
        let path = migrations_dir.join(&artifact.relative_path);
        if !path.exists() {
            continue;
        }
        if artifact.relative_path == "snapshots/__init__.py" {
            continue;
        }
        match policy {
            ExistingArtifactPolicy::Fail => {
                return Err(MigrationError::AuthoringInput {
                    message: format!(
                        "artifact {} already exists in {}",
                        artifact.relative_path,
                        migrations_dir.display()
                    ),
                });
            }
            ExistingArtifactPolicy::ValidateIdentical => {
                let existing = fs::read(&path).map_err(|e| io_error(&path, &e))?;
                if existing != artifact.contents {
                    return Err(MigrationError::AuthoringInput {
                        message: format!(
                            "artifact {} already exists with different contents; \
                             snapshots and checked artifacts are immutable",
                            artifact.relative_path
                        ),
                    });
                }
            }
        }
    }

    for artifact in &authored.files {
        let path = migrations_dir.join(&artifact.relative_path);
        if path.exists() {
            // Validated identical above (or the append-only package marker).
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| io_error(parent, &e))?;
        }
        fs::write(&path, &artifact.contents).map_err(|e| io_error(&path, &e))?;
    }
    Ok(())
}

fn validate_relative_path(relative_path: &str) -> crate::Result<()> {
    let path = Path::new(relative_path);
    let escapes = path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    });
    if escapes {
        return Err(MigrationError::AuthoringInput {
            message: format!(
                "artifact path {relative_path:?} must stay inside the migrations directory"
            ),
        });
    }
    Ok(())
}

fn io_error(path: &Path, error: &std::io::Error) -> MigrationError {
    MigrationError::Loader {
        message: format!(
            "failed writing authored migration at {}: {error}",
            path.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use crate::author::build::AuthoredArtifact;
    use crate::spec::MigrationSpec;

    use super::*;

    fn authored(files: Vec<(&str, &[u8])>) -> AuthoredMigration {
        AuthoredMigration {
            migration_name: "0001_initial".to_string(),
            python_source: String::new(),
            spec: MigrationSpec {
                app_label: "migrations".to_string(),
                name: "0001_initial".to_string(),
                dependencies: vec![],
                operations: vec![],
                checksum: None,
                source_sha256: None,
                reversible: true,
            },
            files: files
                .into_iter()
                .map(|(path, contents)| AuthoredArtifact {
                    relative_path: path.to_string(),
                    contents: contents.to_vec(),
                })
                .collect(),
        }
    }

    #[test]
    fn writes_and_is_idempotent_under_validate_identical() {
        let dir = tempfile::tempdir().expect("tempdir");
        let migration = authored(vec![
            ("0001_initial.py", b"py".as_slice()),
            ("snapshots/__init__.py", b"# marker\n".as_slice()),
            ("snapshots/v0001/schema.tql", b"define".as_slice()),
        ]);

        write_authored_migration(
            dir.path(),
            &migration,
            ExistingArtifactPolicy::ValidateIdentical,
        )
        .expect("first write succeeds");
        write_authored_migration(
            dir.path(),
            &migration,
            ExistingArtifactPolicy::ValidateIdentical,
        )
        .expect("identical rewrite succeeds");

        assert_eq!(
            fs::read(dir.path().join("snapshots/v0001/schema.tql")).expect("file present"),
            b"define"
        );
    }

    #[test]
    fn drifted_existing_artifact_blocks_the_whole_write() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("snapshots/v0001")).expect("mkdir");
        fs::write(dir.path().join("snapshots/v0001/schema.tql"), b"other").expect("seed");

        let migration = authored(vec![
            ("0001_initial.py", b"py".as_slice()),
            ("snapshots/v0001/schema.tql", b"define".as_slice()),
        ]);

        let error = write_authored_migration(
            dir.path(),
            &migration,
            ExistingArtifactPolicy::ValidateIdentical,
        )
        .expect_err("drifted snapshot must collide");

        assert!(matches!(error, MigrationError::AuthoringInput { .. }));
        // Nothing else was written.
        assert!(!dir.path().join("0001_initial.py").exists());
    }

    #[test]
    fn fail_policy_rejects_any_existing_artifact() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("0001_initial.py"), b"py").expect("seed");

        let migration = authored(vec![("0001_initial.py", b"py".as_slice())]);

        let error = write_authored_migration(dir.path(), &migration, ExistingArtifactPolicy::Fail)
            .expect_err("existing artifact must fail");
        assert!(matches!(error, MigrationError::AuthoringInput { .. }));
    }

    #[test]
    fn existing_snapshots_package_marker_is_left_untouched() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("snapshots")).expect("mkdir");
        fs::write(dir.path().join("snapshots/__init__.py"), b"# custom\n").expect("seed");

        let migration = authored(vec![
            ("snapshots/__init__.py", b"# marker\n".as_slice()),
            ("snapshots/v0001/schema.tql", b"define".as_slice()),
        ]);

        write_authored_migration(
            dir.path(),
            &migration,
            ExistingArtifactPolicy::ValidateIdentical,
        )
        .expect("write succeeds");

        assert_eq!(
            fs::read(dir.path().join("snapshots/__init__.py")).expect("marker present"),
            b"# custom\n"
        );
    }

    #[test]
    fn authored_output_passes_checked_loading_validation_and_planning() {
        use std::collections::BTreeMap;

        use type_bridge_orm::_schema::info::{AttributeSchemaEntry, EntitySchemaEntry, SchemaInfo};
        use type_bridge_orm::ValueType;

        use crate::author::build::{
            AuthorMigrationRequest, MigrationMetadata, PositionedOperations, SnapshotContext,
            author_migration,
        };
        use crate::checksum::migration_file_checksum;
        use crate::graph::validate_graph;
        use crate::plan::plan;

        let mut target = SchemaInfo::default();
        target.attributes.insert(
            "name".to_string(),
            AttributeSchemaEntry::new("name", ValueType::String),
        );
        target.entities.insert(
            "person".to_string(),
            EntitySchemaEntry {
                type_name: "person".to_string(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: BTreeMap::new(),
            },
        );

        let authored = author_migration(&AuthorMigrationRequest {
            base: SchemaInfo::default(),
            target,
            metadata: MigrationMetadata {
                app_label: "migrations".to_string(),
                name: "0001_initial".to_string(),
                dependencies: vec![],
                generated_at: "2026-07-13T00:00:00+00:00".to_string(),
                type_bridge_version: "1.5.7".to_string(),
                type_bridge_core_version: "1.5.7".to_string(),
            },
            snapshot: SnapshotContext {
                version: "v0001".to_string(),
                previous_version: None,
            },
            extra_operations: PositionedOperations::default(),
            attribute_renames: vec![],
        })
        .expect("authoring should succeed")
        .expect("changes must author");

        let dir = tempfile::tempdir().expect("tempdir");
        write_authored_migration(
            dir.path(),
            &authored,
            ExistingArtifactPolicy::ValidateIdentical,
        )
        .expect("write succeeds");

        // The checked loader recomputes the checksum from the .py bytes on
        // disk; drift here would prove the sidecar checksum contract broke.
        let graph = crate::loader::load_dir_checked(dir.path()).expect("checked load succeeds");
        assert_eq!(graph.migrations.len(), 1);
        assert_eq!(
            graph.migrations[0].checksum.as_deref(),
            Some(migration_file_checksum(&authored.python_source).as_str())
        );

        assert!(validate_graph(&graph, &[]).is_empty());

        let execution_plan = plan(&graph, &[], None).expect("planning succeeds");
        assert_eq!(execution_plan.to_apply.len(), 1);
        assert_eq!(execution_plan.to_apply[0].steps.len(), 2);
    }

    #[test]
    fn escaping_paths_are_rejected() {
        let dir = tempfile::tempdir().expect("tempdir");
        let migration = authored(vec![("../escape.py", b"py".as_slice())]);

        let error = write_authored_migration(
            dir.path(),
            &migration,
            ExistingArtifactPolicy::ValidateIdentical,
        )
        .expect_err("path escape must be rejected");
        assert!(matches!(error, MigrationError::AuthoringInput { .. }));
    }
}
