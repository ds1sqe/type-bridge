//! TypeDB-backed fenced execution storage for V2 schema migrations.
//!
//! This unpublished leaf crate is the only layer that knows both the
//! provider-neutral migration execution contracts and the TypeDB ORM. It does
//! not extend or import the archival V1 migration state store.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod control_schema;
mod observation;
mod provider;
mod runner;
mod store;
mod wire;

pub use control_schema::{
    JOURNAL_CONTROL_SCHEMA_TYPEQL, MANAGED_FENCE_SCHEMA_TYPEQL,
    TYPEBRIDGE_INTERNAL_PREFIX, control_schema_labels, is_typebridge_internal_label,
};
pub use observation::{
    PartitionedDeclaredSchema, observe_managed_state_from_export,
    partition_typeql_export,
};
pub use provider::{TypeDbMigrationProvider, execution_capability_vocabulary};
pub use runner::{
    MigrationDirectoryApplyError, MigrationDirectoryApplyOutcome,
    MigrationDirectoryRollbackOutcome, TypeDbMigrationRunner,
};
pub use store::{
    TypeDbMigrationStore, VerifiedMigrationCatalog,
    derived_journal_database_name, require_active_managed_fence,
};
