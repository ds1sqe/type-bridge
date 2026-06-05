//! Migration state backend seam.
//!
//! [`MigrationStateStore`] is the trait that abstracts applied-state storage.
//! Two implementations exist:
//!
//! - [`memory::InMemoryStateStore`] — insertion-ordered, mutex-backed store
//!   used for Rust unit tests with no TypeDB connection.
//! - [`typedb::TypeDbStateStore`] — ports the exact TypeQL from `state.py` over
//!   the ORM [`Database`][type_bridge_orm::session::Database] seam.

pub mod memory;
pub mod typedb;

pub use memory::InMemoryStateStore;
pub use typedb::TypeDbStateStore;

use type_bridge_orm::session::backend::BoxFuture;

use crate::{AppliedMigrationRecord, Result};

/// Seam trait for migration applied-state storage.
///
/// Implementations must be [`Send`] + [`Sync`] and expose four operations that
/// map 1-to-1 to the existing Python `MigrationStateManager` surface:
///
/// - [`ensure_schema`][Self::ensure_schema] — idempotent schema bootstrap.
/// - [`load_applied`][Self::load_applied] — full applied-state read.
/// - [`record_applied`][Self::record_applied] — insert/replace one record.
/// - [`record_unapplied`][Self::record_unapplied] — remove one record (absent
///   → `Ok`, matching Python delete semantics).
///
/// Method futures are returned as [`BoxFuture`] so the trait is object-safe
/// and can be used as `Box<dyn MigrationStateStore>`.
pub trait MigrationStateStore: Send + Sync {
    /// Ensure the state-storage schema exists, creating it if absent.
    ///
    /// This is idempotent: calling it on an already-initialised store is a
    /// no-op `Ok(())`.
    fn ensure_schema(&self) -> BoxFuture<'_, Result<()>>;

    /// Load all applied migration records in stable insertion order.
    fn load_applied(&self) -> BoxFuture<'_, Result<Vec<AppliedMigrationRecord>>>;

    /// Record a migration as applied, inserting or replacing by `(app_label,
    /// name)` identity.
    ///
    /// Calling this twice with the same identity is idempotent: the second
    /// call replaces the first; no duplicates are stored.
    fn record_applied(&self, record: AppliedMigrationRecord) -> BoxFuture<'_, Result<()>>;

    /// Remove the applied record identified by `(app_label, name)`.
    ///
    /// If no such record exists the call succeeds silently, matching the
    /// Python `record_unapplied` delete-absent no-op.
    fn record_unapplied<'a>(
        &'a self,
        app_label: &'a str,
        name: &'a str,
    ) -> BoxFuture<'a, Result<()>>;
}
