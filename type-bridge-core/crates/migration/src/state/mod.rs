//! Migration state backend seam.
//!
//! [`MigrationStateStore`] is the trait that abstracts applied-state storage.
//! Two implementations exist:
//!
//! - [`memory::InMemoryStateStore`] — insertion-ordered, mutex-backed store
//!   used for Rust unit tests with no TypeDB connection.
//! - [`typedb::TypeDbStateStore`] — persists state over the ORM
//!   [`Database`][type_bridge_orm::session::Database] seam.
//!
//! [`schema`] is the public, canonical description of the TypeDB types owned by
//! the default store. Bootstrap and language bindings both consume that same
//! [`SchemaInfo`][type_bridge_orm::schema::SchemaInfo].

pub mod memory;
pub mod schema;
pub mod typedb;

pub use memory::InMemoryStateStore;
pub use schema::{
    MigrationStateSchemaKind, applied_migration_entity_label, is_migration_state_type,
    migration_state_schema,
};
pub use typedb::TypeDbStateStore;

use std::net::UdpSocket;

use chrono::Utc;
use network_interface::{NetworkInterface, NetworkInterfaceConfig};
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
    NetworkInterface::show()
        .ok()?
        .into_iter()
        .filter(|interface| !interface.internal)
        .find_map(|interface| {
            interface
                .mac_addr
                .as_deref()
                .and_then(normalize_mac_address)
        })
}

fn normalize_mac_address(value: &str) -> Option<String> {
    let mut bytes = [0_u8; 6];
    let mut parts = value.split([':', '-']);
    for byte in &mut bytes {
        let part = parts.next()?;
        if part.len() != 2 {
            return None;
        }
        *byte = u8::from_str_radix(part, 16).ok()?;
    }
    if parts.next().is_some() || bytes.iter().all(|byte| *byte == 0) {
        return None;
    }
    Some(format_mac_address(bytes))
}

fn format_mac_address(bytes: [u8; 6]) -> String {
    format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]
    )
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

#[cfg(test)]
mod tests {
    use super::{format_mac_address, normalize_mac_address};

    #[test]
    fn formats_mac_addresses_in_lowercase_colon_notation() {
        assert_eq!(
            format_mac_address([0x00, 0x11, 0xAB, 0xCD, 0xEF, 0x42]),
            "00:11:ab:cd:ef:42"
        );
    }

    #[test]
    fn normalizes_supported_mac_address_strings() {
        assert_eq!(
            normalize_mac_address("00:11:AB:CD:EF:42").as_deref(),
            Some("00:11:ab:cd:ef:42")
        );
        assert_eq!(
            normalize_mac_address("00-11-ab-cd-ef-42").as_deref(),
            Some("00:11:ab:cd:ef:42")
        );
    }

    #[test]
    fn rejects_invalid_or_zero_mac_address_strings() {
        assert_eq!(normalize_mac_address("00:00:00:00:00:00"), None);
        assert_eq!(normalize_mac_address("00:11:22:33:44"), None);
        assert_eq!(normalize_mac_address("00:11:22:33:44:zz"), None);
    }
}
