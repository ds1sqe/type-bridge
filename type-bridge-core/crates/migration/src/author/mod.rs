//! Canonical migration authoring core (#166).
//!
//! Owns the single `SchemaDiff -> ordered operations` mapping shared by the
//! live `makemigrations` flow and the offline authoring API. Everything here
//! is pure: no database connection, no filesystem access, no clock reads.

mod build;
mod map;
mod python_render;
mod snapshot;
mod write;

pub use build::{
    AuthorMigrationRequest, AuthoredArtifact, AuthoredMigration, MigrationMetadata,
    PositionedOperations, SnapshotContext, author_migration,
};
pub use map::map_schema_diff;
pub use python_render::{
    PythonRenderRequest, migration_class_name, py_repr, render_migration_python,
};
pub use snapshot::{RenderedSnapshot, SnapshotRenderRequest, render_snapshot};
pub use write::{ExistingArtifactPolicy, write_authored_migration};
