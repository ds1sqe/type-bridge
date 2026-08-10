//! TypeDB-backed fenced execution storage for V2 schema migrations.
//!
//! This public leaf crate is the only layer that knows both the provider-neutral
//! migration execution contracts and the TypeDB ORM. It does not extend or
//! import the archival V1 migration state store.
//!
//! Most embedders should use [`TypeDbMigrationRunner`]. Lower-level composition
//! must construct one [`TypeDbExecutionBinding`] and reuse it for both
//! [`TypeDbMigrationStore`] and [`TypeDbMigrationProvider`]; leases from an
//! independent or unbound execution binding fail before TypeDB I/O.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub mod readme_doctests {}

mod control_schema;
mod legacy_import;
mod observation;
mod provider;
mod runner;
mod store;
mod wire;

pub use control_schema::{
    JOURNAL_CONTROL_SCHEMA_TYPEQL, MANAGED_FENCE_SCHEMA_TYPEQL, TYPEBRIDGE_INTERNAL_PREFIX,
    control_schema_labels, is_typebridge_internal_label,
};
pub use legacy_import::{
    digest_legacy_applied_records, extract_legacy_applied_set_digest, extract_legacy_frontier,
    verify_legacy_continuity,
};
pub use observation::{
    LiveQueryControlPresence, PartitionedDeclaredSchema, observe_managed_state_from_export,
    partition_typeql_export, rebuild_live_managed_state, rebuild_live_query_authority,
    rebuild_live_query_authority_state,
};
pub use provider::{
    TypeDbExecutionBinding, TypeDbMigrationProvider, execution_capability_vocabulary,
    require_supported_migration_execution_binding, require_supported_migration_server,
};
pub use runner::{
    MigrationDirectoryApplyError, MigrationDirectoryApplyOutcome,
    MigrationDirectoryRollbackOutcome, TypeDbMigrationRunner,
};
pub use store::{
    TypeDbMigrationStore, VerifiedMigrationCatalog, derived_journal_database_name,
    require_active_managed_fence,
};
