//! In-memory [`MigrationStateStore`] implementation for Rust unit tests.
//!
//! [`InMemoryStateStore`] is backed by a `Mutex<Vec<AppliedMigrationRecord>>`.
//! Records are kept in insertion order (matching TypeDB application order).
//! Deduplication is enforced on `(app_label, name)`: re-recording the same
//! identity replaces the existing entry in-place, preventing duplicates.
//!
//! This implementation has **no TypeDB dependency** and is intended exclusively
//! for Rust unit tests.  It is NOT the unconfigured default — that is the
//! TypeDB-backed impl (invariant 8).

use std::sync::Mutex;

use type_bridge_orm::session::backend::BoxFuture;

use crate::state::MigrationStateStore;
use crate::{AppliedMigrationRecord, MigrationRunRecord, Result};

/// In-memory migration state store backed by a `Mutex<Vec<AppliedMigrationRecord>>`.
///
/// Records are stored in insertion order.  Deduplication on `(app_label, name)`
/// ensures `record_applied` is idempotent: a second call for the same identity
/// replaces the first entry rather than appending a duplicate.
pub struct InMemoryStateStore {
    records: Mutex<Vec<AppliedMigrationRecord>>,
    runs: Mutex<Vec<MigrationRunRecord>>,
}

impl InMemoryStateStore {
    /// Create a new, empty in-memory store.
    pub fn new() -> Self {
        Self {
            records: Mutex::new(Vec::new()),
            runs: Mutex::new(Vec::new()),
        }
    }
}

impl Default for InMemoryStateStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MigrationStateStore for InMemoryStateStore {
    /// No-op for the in-memory backend — schema bootstrap is always satisfied.
    fn ensure_schema(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }

    /// Return a clone of all stored records in stable insertion order.
    fn load_applied(&self) -> BoxFuture<'_, Result<Vec<AppliedMigrationRecord>>> {
        let snapshot = self.records.lock().unwrap().clone();
        Box::pin(async move { Ok(snapshot) })
    }

    /// Return a clone of all stored run-log records in stable insertion order.
    fn load_runs(&self) -> BoxFuture<'_, Result<Vec<MigrationRunRecord>>> {
        let snapshot = self.runs.lock().unwrap().clone();
        Box::pin(async move { Ok(snapshot) })
    }

    /// Insert or replace a record by `(app_label, name)` identity.
    ///
    /// If a record with the same identity already exists, it is replaced
    /// in-place (preserving its position in insertion order).  If no existing
    /// record is found the new record is appended.
    fn record_applied(&self, record: AppliedMigrationRecord) -> BoxFuture<'_, Result<()>> {
        {
            let mut records = self.records.lock().unwrap();
            if let Some(existing) = records
                .iter_mut()
                .find(|r| r.app_label == record.app_label && r.name == record.name)
            {
                *existing = record;
            } else {
                records.push(record);
            }
        }
        Box::pin(async { Ok(()) })
    }

    /// Remove the record identified by `(app_label, name)`.
    ///
    /// If no such record exists the call succeeds silently.
    fn record_unapplied<'a>(
        &'a self,
        app_label: &'a str,
        name: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        {
            let mut records = self.records.lock().unwrap();
            records.retain(|r| !(r.app_label == app_label && r.name == name));
        }
        Box::pin(async { Ok(()) })
    }

    /// Insert or replace a run-log record by `run_id`.
    fn record_run(&self, record: MigrationRunRecord) -> BoxFuture<'_, Result<()>> {
        {
            let mut runs = self.runs.lock().unwrap();
            if let Some(existing) = runs.iter_mut().find(|r| r.run_id == record.run_id) {
                *existing = record;
            } else {
                runs.push(record);
            }
        }
        Box::pin(async { Ok(()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::MigrationStateStore;

    fn record(app: &str, name: &str) -> AppliedMigrationRecord {
        AppliedMigrationRecord {
            app_label: app.to_string(),
            name: name.to_string(),
            checksum: format!("{app}-{name}-checksum"),
            applied_at: Some("2026-06-05T00:00:00.000000".to_string()),
        }
    }

    fn run_record(run_id: &str, status: &str) -> MigrationRunRecord {
        MigrationRunRecord {
            run_id: run_id.to_string(),
            app_label: "app".to_string(),
            name: "0001_initial".to_string(),
            checksum: "checksum".to_string(),
            direction: "apply".to_string(),
            status: status.to_string(),
            started_at: "2026-06-05T00:00:00.000000".to_string(),
            finished_at: None,
            error: None,
            executor_ip: None,
            executor_mac: None,
        }
    }

    // ── ensure_schema ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn ensure_schema_is_a_no_op_ok() {
        let store = InMemoryStateStore::new();
        let result = store.ensure_schema().await;
        assert!(result.is_ok());
        // Calling again is also fine (idempotent).
        let result2 = store.ensure_schema().await;
        assert!(result2.is_ok());
    }

    // ── empty load ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn load_applied_on_empty_store_returns_empty_vec() {
        let store = InMemoryStateStore::new();
        let applied = store.load_applied().await.unwrap();
        assert!(applied.is_empty());
    }

    #[tokio::test]
    async fn load_runs_on_empty_store_returns_empty_vec() {
        let store = InMemoryStateStore::new();
        let runs = store.load_runs().await.unwrap();
        assert!(runs.is_empty());
    }

    // ── record then load ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn record_applied_then_load_returns_the_record() {
        let store = InMemoryStateStore::new();
        let rec = record("myapp", "0001_initial");

        store.record_applied(rec.clone()).await.unwrap();

        let applied = store.load_applied().await.unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0], rec);
    }

    #[tokio::test]
    async fn record_run_then_load_returns_the_record() {
        let store = InMemoryStateStore::new();
        let rec = run_record("run-1", "started");

        store.record_run(rec.clone()).await.unwrap();

        let runs = store.load_runs().await.unwrap();
        assert_eq!(runs, vec![rec]);
    }

    // ── idempotent re-record ───────────────────────────────────────────────────

    #[tokio::test]
    async fn record_applied_twice_does_not_duplicate() {
        let store = InMemoryStateStore::new();
        let rec = record("myapp", "0001_initial");

        store.record_applied(rec.clone()).await.unwrap();
        // Re-record with a different checksum to confirm replacement, not duplicate.
        let rec2 = AppliedMigrationRecord {
            checksum: "updated-checksum".to_string(),
            ..rec.clone()
        };
        store.record_applied(rec2.clone()).await.unwrap();

        let applied = store.load_applied().await.unwrap();
        assert_eq!(applied.len(), 1, "must not duplicate on re-record");
        assert_eq!(applied[0].checksum, "updated-checksum");
    }

    #[tokio::test]
    async fn record_run_twice_replaces_existing_run() {
        let store = InMemoryStateStore::new();
        let rec = run_record("run-1", "started");
        let finished = MigrationRunRecord {
            status: "succeeded".to_string(),
            finished_at: Some("2026-06-05T00:00:01.000000".to_string()),
            ..rec.clone()
        };

        store.record_run(rec).await.unwrap();
        store.record_run(finished.clone()).await.unwrap();

        let runs = store.load_runs().await.unwrap();
        assert_eq!(runs, vec![finished]);
    }

    // ── record unapplied removes the entry ─────────────────────────────────────

    #[tokio::test]
    async fn record_unapplied_removes_the_entry() {
        let store = InMemoryStateStore::new();
        store
            .record_applied(record("myapp", "0001_initial"))
            .await
            .unwrap();
        store
            .record_applied(record("myapp", "0002_next"))
            .await
            .unwrap();

        store
            .record_unapplied("myapp", "0001_initial")
            .await
            .unwrap();

        let applied = store.load_applied().await.unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].name, "0002_next");
    }

    // ── unrecord absent is a no-op ─────────────────────────────────────────────

    #[tokio::test]
    async fn record_unapplied_absent_is_ok_no_op() {
        let store = InMemoryStateStore::new();
        // Store is empty — removing a non-existent entry must not error.
        let result = store.record_unapplied("myapp", "0001_initial").await;
        assert!(result.is_ok());

        // Store has a different entry — still no error for the absent name.
        store
            .record_applied(record("myapp", "0002_next"))
            .await
            .unwrap();
        let result2 = store.record_unapplied("myapp", "0001_initial").await;
        assert!(result2.is_ok());
        // The unrelated entry is intact.
        let applied = store.load_applied().await.unwrap();
        assert_eq!(applied.len(), 1);
    }

    // ── insertion order is preserved ───────────────────────────────────────────

    #[tokio::test]
    async fn insertion_order_is_stable_across_record_and_unrecord_sequences() {
        let store = InMemoryStateStore::new();

        store
            .record_applied(record("app", "0001_initial"))
            .await
            .unwrap();
        store
            .record_applied(record("app", "0002_second"))
            .await
            .unwrap();
        store
            .record_applied(record("app", "0003_third"))
            .await
            .unwrap();

        // Remove the middle one.
        store.record_unapplied("app", "0002_second").await.unwrap();

        let applied = store.load_applied().await.unwrap();
        assert_eq!(applied.len(), 2);
        assert_eq!(applied[0].name, "0001_initial");
        assert_eq!(applied[1].name, "0003_third");

        // Re-record the middle one — it should be appended at the end.
        store
            .record_applied(record("app", "0002_second"))
            .await
            .unwrap();
        let applied2 = store.load_applied().await.unwrap();
        assert_eq!(applied2.len(), 3);
        assert_eq!(applied2[0].name, "0001_initial");
        assert_eq!(applied2[1].name, "0003_third");
        assert_eq!(applied2[2].name, "0002_second");
    }

    // ── trait-object integration smoke ────────────────────────────────────────
    //
    // Verifies the seam (invariant 8's in-memory side): a full
    // record → load → unrecord → load round-trip exercised through
    // `Box<dyn MigrationStateStore>`, proving the trait is object-safe and
    // the impl works behind the abstraction with no TypeDB connection.

    #[tokio::test]
    async fn trait_object_round_trip_is_consistent() {
        let store: Box<dyn MigrationStateStore> = Box::new(InMemoryStateStore::new());

        // Ensure schema is a no-op.
        store.ensure_schema().await.unwrap();

        // Start empty.
        assert!(store.load_applied().await.unwrap().is_empty());

        // Record two migrations.
        let rec_a = record("orders", "0001_initial");
        let rec_b = record("orders", "0002_add_index");
        store.record_applied(rec_a.clone()).await.unwrap();
        store.record_applied(rec_b.clone()).await.unwrap();

        let applied = store.load_applied().await.unwrap();
        assert_eq!(applied.len(), 2);
        assert_eq!(applied[0], rec_a);
        assert_eq!(applied[1], rec_b);

        // Unrecord the first; only the second remains.
        store
            .record_unapplied("orders", "0001_initial")
            .await
            .unwrap();
        let after_unrecord = store.load_applied().await.unwrap();
        assert_eq!(after_unrecord.len(), 1);
        assert_eq!(after_unrecord[0], rec_b);

        // Unrecord the last; store is empty again.
        store
            .record_unapplied("orders", "0002_add_index")
            .await
            .unwrap();
        assert!(store.load_applied().await.unwrap().is_empty());
    }
}
