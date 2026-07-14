//! Canonical migration authoring core (#166).
//!
//! Owns the single `SchemaDiff -> ordered operations` mapping shared by the
//! live `makemigrations` flow and the offline authoring API. Everything here
//! is pure: no database connection, no filesystem access, no clock reads.

mod build;
mod gc;
mod manifest;
mod map;
mod publish;
mod python_render;
mod snapshot;
mod write;

pub use build::{
    AuthorMigrationRequest, AuthoredArtifact, AuthoredMigration, DeclaredMigrationIntentInput,
    MigrationMetadata, PositionedOperations, SnapshotContext, author_migration,
};
pub use gc::{OrphanGcReport, collect_migration_orphans};
pub use manifest::{
    AuthoredExtension, COMMIT_MANIFEST_FORMAT_V1, ComposedMigration, MIGRATION_TREE_LOCK,
    ManifestExtension, ManifestFile, MigrationCommitManifest, MigrationComposer,
    MigrationTreeFormat, TREE_FORMAT_SENTINEL, TREE_FORMAT_V1,
};
pub(crate) use manifest::{sha256, validate_normalized_relative_path};
pub use map::map_schema_diff;
pub use publish::{
    PublicationPoint, publish_composed_migration, publish_composed_migration_with_observer,
};
pub use python_render::{
    PythonRenderRequest, migration_class_name, py_repr, render_migration_python,
};
pub use snapshot::{RenderedSnapshot, SnapshotRenderRequest, render_snapshot};
pub use write::{ExistingArtifactPolicy, write_authored_migration};
