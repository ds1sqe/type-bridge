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

use std::fs;
use std::net::UdpSocket;

use chrono::Utc;
use type_bridge_orm::session::backend::BoxFuture;
use uuid::Uuid;

use crate::plan::{MigrationAction, MigrationExecution};
use crate::{AppliedMigrationRecord, Result};

const TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S.%6f";

/// Best-effort executor identity stored with migration run-log rows.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub struct MigrationExecutorInfo {
    /// Best-effort local IP address of the process that executed the migration.
    #[serde(default)]
    pub ip: Option<String>,
    /// Best-effort MAC address of the process that executed the migration.
    #[serde(default)]
    pub mac: Option<String>,
}

/// Append/update record for one migration execution attempt.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MigrationRunRecord {
    /// Unique execution-attempt identifier.
    pub run_id: String,
    /// Application or migration package label.
    pub app_label: String,
    /// Migration file stem, such as `0001_initial`.
    pub name: String,
    /// Migration checksum observed when this run started.
    pub checksum: String,
    /// Execution direction: `apply` or `rollback`.
    pub direction: String,
    /// Execution status: `started`, `succeeded`, or `failed`.
    pub status: String,
    /// UTC timestamp when the run started.
    pub started_at: String,
    /// UTC timestamp when the run finished, when available.
    #[serde(default)]
    pub finished_at: Option<String>,
    /// Error message captured for failed runs, when available.
    #[serde(default)]
    pub error: Option<String>,
    /// Best-effort executor IP address, when available.
    #[serde(default)]
    pub executor_ip: Option<String>,
    /// Best-effort executor MAC address, when available.
    #[serde(default)]
    pub executor_mac: Option<String>,
}

/// Return the current UTC timestamp in the TypeDB datetime literal format.
pub fn migration_timestamp_now() -> String {
    Utc::now().format(TIMESTAMP_FORMAT).to_string()
}

/// Collect best-effort executor identity for audit logging.
pub fn collect_executor_info() -> MigrationExecutorInfo {
    MigrationExecutorInfo {
        ip: local_ip(),
        mac: local_mac(),
    }
}

/// Create a started run-log record for a planned migration execution.
pub fn started_run_record(
    migration: &MigrationExecution,
    checksum: String,
    executor: &MigrationExecutorInfo,
) -> MigrationRunRecord {
    MigrationRunRecord {
        run_id: Uuid::new_v4().to_string(),
        app_label: migration.app_label.clone(),
        name: migration.name.clone(),
        checksum,
        direction: direction_label(migration.action).to_string(),
        status: "started".to_string(),
        started_at: migration_timestamp_now(),
        finished_at: None,
        error: None,
        executor_ip: executor.ip.clone(),
        executor_mac: executor.mac.clone(),
    }
}

/// Return a finished copy of a migration run-log record.
pub fn finished_run_record(
    mut record: MigrationRunRecord,
    status: &str,
    error: Option<String>,
) -> MigrationRunRecord {
    record.status = status.to_string();
    record.finished_at = Some(migration_timestamp_now());
    record.error = error;
    record
}

fn direction_label(action: MigrationAction) -> &'static str {
    match action {
        MigrationAction::Apply => "apply",
        MigrationAction::Rollback => "rollback",
    }
}

fn local_ip() -> Option<String> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let addr = socket.local_addr().ok()?;
    Some(addr.ip().to_string()).filter(|ip| ip != "0.0.0.0")
}

fn local_mac() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let entries = fs::read_dir("/sys/class/net").ok()?;
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name == "lo" {
                continue;
            }
            let address_path = entry.path().join("address");
            let address = fs::read_to_string(address_path).ok()?;
            let address = address.trim().to_ascii_lowercase();
            if address.len() == 17 && address != "00:00:00:00:00:00" {
                return Some(address);
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

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

    /// Load all migration execution run-log records.
    fn load_runs(&self) -> BoxFuture<'_, Result<Vec<MigrationRunRecord>>>;

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

    /// Insert or replace one migration execution run-log record by `run_id`.
    fn record_run(&self, record: MigrationRunRecord) -> BoxFuture<'_, Result<()>>;
}
