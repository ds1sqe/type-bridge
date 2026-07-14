//! Assemble a complete authored migration from two schemas.
//!
//! [`author_migration`] is the pure, database-free entry point (#166): it
//! computes the schema diff, runs the canonical mapping, splices explicit
//! positioned operations, renders the reviewable `.py`, lowers the checked
//! sidecar `MigrationSpec` with a checksum over the exact `.py` bytes, and
//! renders the immutable snapshot file set — all in memory. Identical
//! inputs (including the explicit `generated_at`) produce byte-identical
//! artifacts.

use sha2::{Digest, Sha256};
use type_bridge_orm::schema::SchemaDiff;
use type_bridge_orm::schema::info::SchemaInfo;

use crate::author::map::map_schema_diff;
use crate::author::python_render::{PythonRenderRequest, render_migration_python};
use crate::author::snapshot::{SnapshotRenderRequest, render_snapshot};
use crate::checksum::migration_file_checksum;
use crate::error::MigrationError;
use crate::spec::{DeclaredMigrationIntent, MigrationDependencySpec, MigrationSpec, OperationSpec};

/// Identity and dependency metadata for one authored migration.
#[derive(Debug)]
pub struct MigrationMetadata {
    /// Migrations package name (the migrations directory name).
    pub app_label: String,
    /// Full migration stem, e.g. `0003_add_assignment`.
    pub name: String,
    /// `(app_label, migration_name)` dependencies.
    pub dependencies: Vec<(String, String)>,
    /// Timestamp text embedded verbatim in the generated `.py` docstring.
    ///
    /// Explicit so authoring stays deterministic; callers stamp wall-clock
    /// time only at the boundary.
    pub generated_at: String,
    /// `type_bridge` package version recorded in the snapshot manifest.
    pub type_bridge_version: String,
    /// `type_bridge_core` version recorded in the snapshot manifest.
    pub type_bridge_core_version: String,
}

/// Snapshot/version context for one authored migration.
#[derive(Debug)]
pub struct SnapshotContext {
    /// Snapshot version generated with this migration (e.g. `v0003`).
    pub version: String,
    /// The previous snapshot version (e.g. `v0002`), if one exists.
    ///
    /// Removal operations reference this version's bindings in the
    /// generated `.py`; the base schema must describe the same version.
    pub previous_version: Option<String>,
}

/// Explicit portable operations placed around the schema change set.
///
/// Destructive data cleanup often must run before schema removal, and
/// backfills after schema additions; placement is therefore explicit.
#[derive(Debug, Default)]
pub struct PositionedOperations {
    /// Operations executed before the mapped schema operations.
    pub before_schema: Vec<OperationSpec>,
    /// Operations executed after the mapped schema operations.
    pub after_schema: Vec<OperationSpec>,
}

/// Explicit declaration that an otherwise empty authoring request represents
/// a real semantic version transition.
///
/// TypeBridge treats the bytes as opaque identity input. It computes and owns
/// the versioned identity stored in the canonical sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredMigrationIntentInput {
    /// Caller-owned deterministic bytes describing the semantic transition.
    pub contents: Vec<u8>,
}

/// One in-memory artifact file, path relative to the migrations directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthoredArtifact {
    /// Relative, platform-independent path (`/`-separated).
    pub relative_path: String,
    /// Complete file contents.
    pub contents: Vec<u8>,
}

/// The complete authored artifact set for one migration.
#[derive(Debug)]
pub struct AuthoredMigration {
    /// Full migration stem, e.g. `0003_add_assignment`.
    pub migration_name: String,
    /// The reviewable `.py` source (also present in `files`).
    pub python_source: String,
    /// The canonical execution spec (serialized into the `.json` sidecar).
    pub spec: MigrationSpec,
    /// Every artifact file: `.py`, `.json` sidecar, and snapshot files.
    pub files: Vec<AuthoredArtifact>,
}

/// A complete offline authoring request.
#[derive(Debug)]
pub struct AuthorMigrationRequest {
    /// Base schema (what the database currently is, or the previous
    /// snapshot's schema).
    pub base: SchemaInfo,
    /// Target schema the migration should produce.
    pub target: SchemaInfo,
    /// Identity and dependency metadata.
    pub metadata: MigrationMetadata,
    /// Snapshot/version context.
    pub snapshot: SnapshotContext,
    /// Explicit positioned operations.
    pub extra_operations: PositionedOperations,
    /// Explicit semantic transition declaration. Without this declaration,
    /// an empty schema diff and empty operation lists remain a true no-op.
    pub declared_intent: Option<DeclaredMigrationIntentInput>,
    /// `(old_name, new_name)` attribute-rename directives: each replaces
    /// the diff's independent remove+add of that pair with a
    /// data-preserving staged expansion.
    pub attribute_renames: Vec<(String, String)>,
}

/// Author the complete artifact set for one migration, in memory.
///
/// Returns `Ok(None)` when the diff is empty and no explicit operations
/// were supplied — a no-op authoring request produces no artifact.
///
/// # Errors
///
/// - [`MigrationError::UnsupportedChange`] – a diff field or explicit
///   operation has no canonical lowering.
/// - [`MigrationError::AuthoringInput`] – inconsistent inputs.
/// - [`MigrationError::SchemaGeneration`] – target schema cannot render.
pub fn author_migration(
    request: &AuthorMigrationRequest,
) -> crate::Result<Option<AuthoredMigration>> {
    validate_stem(&request.metadata.name)?;

    let diff = SchemaDiff::compute(&request.base, &request.target);
    let schema_operations = map_schema_diff(
        &request.base,
        &request.target,
        &diff,
        &request.attribute_renames,
    )?;

    if schema_operations.is_empty()
        && request.extra_operations.before_schema.is_empty()
        && request.extra_operations.after_schema.is_empty()
        && request.declared_intent.is_none()
    {
        return Ok(None);
    }

    let declared_intent = request.declared_intent.as_ref().map(|input| {
        let digest = Sha256::digest(&input.contents);
        DeclaredMigrationIntent::V1 {
            identity: format!("{digest:x}"),
        }
    });

    let mut operations = Vec::with_capacity(
        request.extra_operations.before_schema.len()
            + schema_operations.len()
            + request.extra_operations.after_schema.len(),
    );
    // Normalization fills structured copy_attribute forms with their
    // synthesized TypeQL so the sidecar always carries executable strings.
    for op in &request.extra_operations.before_schema {
        operations.push(op.clone().normalized()?);
    }
    operations.extend(schema_operations);
    for op in &request.extra_operations.after_schema {
        operations.push(op.clone().normalized()?);
    }

    let python_source = render_migration_python(&PythonRenderRequest {
        operations: &operations,
        app_label: &request.metadata.app_label,
        name: migration_suffix(&request.metadata.name),
        dependencies: &request.metadata.dependencies,
        generated_at: &request.metadata.generated_at,
        pre_version: request.snapshot.previous_version.as_deref(),
        post_version: &request.snapshot.version,
        base: &request.base,
        target: &request.target,
        declared_intent: declared_intent.as_ref(),
    })?;

    // The sidecar checksum is computed from the exact returned `.py` bytes:
    // the loader recomputes it from the file on disk (drift gate).
    let checksum = migration_file_checksum(&python_source);

    let spec = MigrationSpec {
        app_label: request.metadata.app_label.clone(),
        name: request.metadata.name.clone(),
        dependencies: request
            .metadata
            .dependencies
            .iter()
            .map(|(app_label, migration_name)| MigrationDependencySpec {
                app_label: app_label.clone(),
                migration_name: migration_name.clone(),
            })
            .collect(),
        operations,
        declared_intent,
        checksum: Some(checksum),
        reversible: true,
    };
    let sidecar_json = serde_json::to_string(&spec)?;

    let snapshot = render_snapshot(&SnapshotRenderRequest {
        target: &request.target,
        version: &request.snapshot.version,
        source_migration: &request.metadata.name,
        type_bridge_version: &request.metadata.type_bridge_version,
        type_bridge_core_version: &request.metadata.type_bridge_core_version,
    })?;

    let mut files = vec![
        AuthoredArtifact {
            relative_path: format!("{}.py", request.metadata.name),
            contents: python_source.clone().into_bytes(),
        },
        AuthoredArtifact {
            relative_path: format!("{}.json", request.metadata.name),
            contents: sidecar_json.into_bytes(),
        },
    ];
    files.extend(
        snapshot
            .files
            .into_iter()
            .map(|(relative_path, contents)| AuthoredArtifact {
                relative_path,
                contents,
            }),
    );

    Ok(Some(AuthoredMigration {
        migration_name: request.metadata.name.clone(),
        python_source,
        spec,
        files,
    }))
}

/// Return the name suffix after the `NNNN_` numbering prefix.
fn migration_suffix(stem: &str) -> &str {
    match stem.split_once('_') {
        Some((number, suffix))
            if number.len() == 4 && number.bytes().all(|b| b.is_ascii_digit()) =>
        {
            suffix
        }
        _ => stem,
    }
}

fn validate_stem(stem: &str) -> crate::Result<()> {
    let valid = stem.split_once('_').is_some_and(|(number, suffix)| {
        number.len() == 4 && number.bytes().all(|b| b.is_ascii_digit()) && !suffix.is_empty()
    });
    if valid {
        Ok(())
    } else {
        Err(MigrationError::AuthoringInput {
            message: format!(
                "migration name {stem:?} must have the form NNNN_name (e.g. 0003_add_assignment)"
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use type_bridge_orm::ValueType;
    use type_bridge_orm::schema::info::{AttributeSchemaEntry, EntitySchemaEntry};

    use super::*;
    use crate::spec::DECLARED_TRANSITION_SCHEME_V1;

    fn person_schema() -> SchemaInfo {
        let mut info = SchemaInfo::default();
        info.attributes.insert(
            "name".to_string(),
            AttributeSchemaEntry::new("name", ValueType::String),
        );
        info.entities.insert(
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
        info
    }

    fn request(base: SchemaInfo, target: SchemaInfo) -> AuthorMigrationRequest {
        AuthorMigrationRequest {
            base,
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
            declared_intent: None,
            attribute_renames: vec![],
        }
    }

    #[test]
    fn no_changes_authors_nothing() {
        let schema = person_schema();
        let authored =
            author_migration(&request(schema.clone(), schema)).expect("authoring should succeed");
        assert!(authored.is_none());
    }

    #[test]
    fn declared_semantic_transition_authors_canonical_zero_operation_version() {
        let schema = person_schema();
        let mut request = request(schema.clone(), schema);
        request.declared_intent = Some(DeclaredMigrationIntentInput {
            contents: br#"{"description":"semantic-only"}"#.to_vec(),
        });

        let authored = author_migration(&request)
            .expect("authoring should succeed")
            .expect("declared transition must author");

        assert!(authored.spec.operations.is_empty());
        let intent = authored
            .spec
            .declared_intent
            .as_ref()
            .expect("declared identity");
        assert_eq!(intent.scheme(), DECLARED_TRANSITION_SCHEME_V1);
        assert_eq!(intent.identity().len(), 64);
        assert!(
            authored
                .python_source
                .contains("Migration: declared semantic transition")
        );
        assert!(authored.python_source.contains(intent.identity()));
        assert!(
            authored
                .python_source
                .contains("operations: ClassVar[list[Operation]] = []")
        );
        assert!(
            authored
                .files
                .iter()
                .any(|file| file.relative_path == "0001_initial.json")
        );
        assert!(
            authored
                .files
                .iter()
                .any(|file| file.relative_path == "snapshots/v0001/snapshot.json")
        );
    }

    #[test]
    fn authoring_is_deterministic_and_checksummed() {
        let base = SchemaInfo::default();
        let target = person_schema();

        let first = author_migration(&request(base.clone(), target.clone()))
            .expect("authoring should succeed")
            .expect("changes must author");
        let second = author_migration(&request(base, target))
            .expect("authoring should succeed")
            .expect("changes must author");

        assert_eq!(first.python_source, second.python_source);
        let first_files: Vec<(&str, &[u8])> = first
            .files
            .iter()
            .map(|f| (f.relative_path.as_str(), f.contents.as_slice()))
            .collect();
        let second_files: Vec<(&str, &[u8])> = second
            .files
            .iter()
            .map(|f| (f.relative_path.as_str(), f.contents.as_slice()))
            .collect();
        assert_eq!(first_files, second_files);

        assert_eq!(
            first.spec.checksum.as_deref(),
            Some(migration_file_checksum(&first.python_source).as_str())
        );
    }

    #[test]
    fn authored_artifacts_cover_the_full_set() {
        let authored = author_migration(&request(SchemaInfo::default(), person_schema()))
            .expect("authoring should succeed")
            .expect("changes must author");

        let paths: Vec<&str> = authored
            .files
            .iter()
            .map(|f| f.relative_path.as_str())
            .collect();
        assert_eq!(
            paths,
            vec![
                "0001_initial.py",
                "0001_initial.json",
                "snapshots/__init__.py",
                "snapshots/v0001/__init__.py",
                "snapshots/v0001/attributes.py",
                "snapshots/v0001/entities.py",
                "snapshots/v0001/registry.py",
                "snapshots/v0001/relations.py",
                "snapshots/v0001/schema.tql",
                "snapshots/v0001/snapshot.json",
            ]
        );
    }

    #[test]
    fn positioned_operations_wrap_the_schema_change_set() {
        let mut req = request(SchemaInfo::default(), person_schema());
        req.metadata.name = "0002_cleanup_then_backfill".to_string();
        req.extra_operations
            .before_schema
            .push(OperationSpec::RunTypeql {
                forward: "match $x isa legacy; delete $x;".to_string(),
                reverse: None,
            });
        req.extra_operations
            .after_schema
            .push(OperationSpec::RunTypeql {
                forward: "insert $p isa person;".to_string(),
                reverse: None,
            });

        let authored = author_migration(&req)
            .expect("authoring should succeed")
            .expect("changes must author");

        let kinds: Vec<String> = authored
            .spec
            .operations
            .iter()
            .map(|op| {
                serde_json::to_value(op).expect("op serializes")["kind"]
                    .as_str()
                    .expect("kind present")
                    .to_string()
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["run_typeql", "add_attribute", "add_entity", "run_typeql"]
        );
    }

    #[test]
    fn structured_copy_attribute_is_normalized_into_the_sidecar() {
        let mut req = request(SchemaInfo::default(), person_schema());
        req.metadata.name = "0002_backfill_names".to_string();
        req.extra_operations
            .after_schema
            .push(OperationSpec::CopyAttribute {
                owner: Some("person".to_string()),
                source: Some("legacy-name".to_string()),
                dest: Some("name".to_string()),
                filter: None,
                forward: None,
                reverse: None,
            });

        let authored = author_migration(&req)
            .expect("authoring should succeed")
            .expect("changes must author");

        assert!(
            authored
                .python_source
                .contains("ops.CopyAttribute(Person, source='legacy-name', dest='name'),")
        );
        let copy = authored
            .spec
            .operations
            .last()
            .expect("operations must not be empty");
        let OperationSpec::CopyAttribute {
            forward, reverse, ..
        } = copy
        else {
            panic!("last operation must be the copy_attribute");
        };
        assert_eq!(
            forward.as_deref(),
            Some(
                "match\n  $x isa person, has legacy-name $v;\n  not { $x has name $d; };\ninsert\n  $x has name == $v;"
            )
        );
        assert_eq!(
            reverse.as_deref(),
            Some("match $x isa person, has name $v;\ndelete $v of $x;")
        );
    }

    #[test]
    fn attribute_rename_authors_the_staged_expansion() {
        let mut base = SchemaInfo::default();
        base.attributes.insert(
            "legacy-name".to_string(),
            AttributeSchemaEntry::new("legacy-name", ValueType::String),
        );
        base.entities.insert(
            "person".to_string(),
            EntitySchemaEntry {
                type_name: "person".to_string(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![type_bridge_orm::schema::info::OwnedAttributeEntry {
                    attr_name: "legacy-name".to_string(),
                    value_type: ValueType::String,
                    annotations: vec![type_bridge_orm::Annotation::Key],
                    is_ordered: false,
                    doc: None,
                    meta: BTreeMap::new(),
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: BTreeMap::new(),
            },
        );
        let mut target = SchemaInfo::default();
        target.attributes.insert(
            "display-name".to_string(),
            AttributeSchemaEntry::new("display-name", ValueType::String),
        );
        target.entities.insert(
            "person".to_string(),
            EntitySchemaEntry {
                type_name: "person".to_string(),
                is_abstract: false,
                parent_type: None,
                owned_attributes: vec![type_bridge_orm::schema::info::OwnedAttributeEntry {
                    attr_name: "display-name".to_string(),
                    value_type: ValueType::String,
                    annotations: vec![type_bridge_orm::Annotation::Key],
                    is_ordered: false,
                    doc: None,
                    meta: BTreeMap::new(),
                }],
                plays_cardinalities: BTreeMap::new(),
                doc: None,
                meta: BTreeMap::new(),
            },
        );

        let mut req = request(base, target);
        req.metadata.name = "0002_rename_name".to_string();
        req.attribute_renames = vec![("legacy-name".to_string(), "display-name".to_string())];

        let authored = author_migration(&req)
            .expect("authoring should succeed")
            .expect("changes must author");

        let kinds: Vec<String> = authored
            .spec
            .operations
            .iter()
            .map(|op| {
                serde_json::to_value(op).expect("op serializes")["kind"]
                    .as_str()
                    .expect("kind present")
                    .to_string()
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "add_attribute",
                "add_ownership",
                "copy_attribute",
                "modify_ownership", // tighten the new ownership
                "modify_ownership", // loosen the old ownership pre-delete
                "run_typeql",
                "remove_ownership",
                "remove_attribute",
            ]
        );
        // The .py shows the reviewable primitive recipe, not an opaque
        // rename marker.
        assert!(authored.python_source.contains("ops.CopyAttribute("));
        assert!(!authored.python_source.contains("ops.RenameAttribute("));
    }

    #[test]
    fn invalid_stem_is_rejected() {
        let mut req = request(SchemaInfo::default(), person_schema());
        req.metadata.name = "initial".to_string();

        let error = author_migration(&req).expect_err("stem must be validated");
        assert!(matches!(error, MigrationError::AuthoringInput { .. }));
    }
}
