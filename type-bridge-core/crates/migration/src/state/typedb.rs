//! TypeDB-backed [`MigrationStateStore`] implementation.
//!
//! [`TypeDbStateStore`] ports the exact TypeQL previously hand-written in
//! `type_bridge/migration/state.py` over the ORM
//! [`Database`][type_bridge_orm::Database] seam. The schema, the `define`
//! block, and every query string are byte-for-byte equivalent to the Python
//! source (modulo the fixed `type_bridge_migration` entity name), per
//! invariant 5 — applied-state storage is NOT redesigned.
//!
//! All work runs through [`Database::transaction_context`], mirroring the
//! Phase-05 executor: open a context for the right [`TxType`], run the query,
//! and commit write/schema contexts. No Python transaction crosses this
//! boundary (invariant 4); only the public ORM `session` API is used
//! (invariant 7).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use type_bridge_orm::Database;
use type_bridge_orm::OrmError;
use type_bridge_orm::session::backend::{BoxFuture, QueryResult, TxType};

use crate::state::MigrationStateStore;
use crate::{AppliedMigrationRecord, MigrationError, Result};

/// The migration-tracking entity type name.
///
/// Mirrors `MigrationStateManager.ENTITY_NAME` (`state.py:111`).
const ENTITY_NAME: &str = "type_bridge_migration";

/// Timestamp format mirroring Python's `strftime("%Y-%m-%dT%H:%M:%S.%f")`.
///
/// Python `%f` always emits exactly six digits (microseconds, zero-padded);
/// chrono's `%6f` is the byte-identical equivalent. This is the format written
/// into the `insert` TypeQL — `record_applied` (`state.py:250`).
const APPLIED_AT_FORMAT: &str = "%Y-%m-%dT%H:%M:%S.%6f";

/// TypeDB-backed migration state store over the ORM session seam.
///
/// Holds a shared [`Arc<Database>`] (the same handle the rest of the Rust ORM
/// path uses) and opens its own transaction contexts per operation.
pub struct TypeDbStateStore {
    db: Arc<Database>,
    /// Idempotency latch for [`ensure_schema`](Self::ensure_schema), mirroring
    /// the Python `_schema_ensured` flag (`state.py:121`).
    schema_ensured: AtomicBool,
}

impl TypeDbStateStore {
    /// Construct a store bound to a shared database handle.
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            schema_ensured: AtomicBool::new(false),
        }
    }
}

/// Format the current UTC time as the Python-compatible applied-at string.
///
/// Isolated as a pure function (parameterised on the instant) so the
/// `%Y-%m-%dT%H:%M:%S.%6f` parity with Python's `%f` is unit-testable without a
/// live clock or TypeDB.
fn format_applied_at(now: chrono::DateTime<Utc>) -> String {
    now.format(APPLIED_AT_FORMAT).to_string()
}

/// Map an ORM-layer error into the migration error hierarchy.
fn map_orm_error(error: OrmError) -> MigrationError {
    MigrationError::State {
        message: error.to_string(),
    }
}

/// Unwrap a single fetched field into its scalar string.
///
/// The Phase-05 ORM backend renders a `fetch { "k": $attr }` document so that
/// each attribute-bound variable becomes a bare JSON scalar (the attribute's
/// value), not a `{"value": ...}` wrapper (typedb-driver `json_value`,
/// `concept_document.rs`). The original Python `_extract_value`
/// (`state.py:221-237`) defensively handled BOTH shapes — a bare scalar and a
/// `{"value": ...}` object — so this parser does the same: prefer the inner
/// `"value"` when the field is an object carrying one, otherwise take the
/// scalar directly. Returns `None` for a missing/null field or an object with
/// no usable `"value"`.
fn extract_value(doc: &serde_json::Value, key: &str) -> Option<String> {
    let value = doc.get(key)?;
    extract_scalar(value)
}

/// Reduce a fetched JSON value to its string form, unwrapping a `{"value":
/// ...}` envelope when present (the `_extract_value` dict branch).
fn extract_scalar(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::Object(map) => map.get("value").and_then(extract_scalar),
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Array(_) => None,
    }
}

/// Parse a list of TypeDB fetch documents into applied migration records.
///
/// Pure and TypeDB-free so the document-shape contract is unit-testable. Each
/// document is the `fetch { "app": ..., "name": ..., "applied": ...,
/// "checksum": ... }` shape from the `load_applied` query (ported from
/// `state.py:179-191`). A document missing any of `app` / `name` / `checksum`
/// is skipped, matching the Python `if all([...])` guard (`state.py:206`);
/// `applied` is carried through as the optional ISO string.
pub fn parse_applied_documents(
    values: &[serde_json::Value],
) -> Result<Vec<AppliedMigrationRecord>> {
    let mut records = Vec::with_capacity(values.len());
    for doc in values {
        let (Some(app_label), Some(name), Some(checksum)) = (
            extract_value(doc, "app"),
            extract_value(doc, "name"),
            extract_value(doc, "checksum"),
        ) else {
            // Skip incomplete rows (mirrors the Python `all([...])` guard).
            continue;
        };
        let applied_at = extract_value(doc, "applied");
        records.push(AppliedMigrationRecord {
            app_label,
            name,
            checksum,
            applied_at,
        });
    }
    Ok(records)
}

impl MigrationStateStore for TypeDbStateStore {
    fn ensure_schema(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if self.schema_ensured.load(Ordering::Acquire) {
                return Ok(());
            }

            // Check whether the entity type already exists. Ported from
            // state.py:132-135 (read tx + fetch).
            let check_query = format!(
                "\n            match $t type {ENTITY_NAME};\n            fetch {{ \"exists\": true }};\n        "
            );

            let exists = match self.db.transaction_context(TxType::Read).await {
                Ok(ctx) => match ctx.query(&check_query).await {
                    // A non-empty document set means the type is defined.
                    Ok(QueryResult::Documents(docs)) => !docs.is_empty(),
                    Ok(QueryResult::Rows(rows)) => !rows.is_empty(),
                    Ok(QueryResult::Ok) => false,
                    // Type absent → the check query errors; fall through to
                    // define, matching the Python bare `except` (state.py:143).
                    Err(_) => false,
                },
                // Opening the read tx failed the same way the Python `except`
                // swallows it: proceed to define.
                Err(_) => false,
            };

            if exists {
                self.schema_ensured.store(true, Ordering::Release);
                return Ok(());
            }

            // Create the migration tracking schema. Ported verbatim from
            // state.py:149-162 (composite-key entity + five attributes).
            let schema = format!(
                "define\nattribute migration_id, value string;\nattribute migration_app_label, value string;\nattribute migration_name, value string;\nattribute migration_applied_at, value datetime;\nattribute migration_checksum, value string;\n\nentity {ENTITY_NAME},\n    owns migration_id @key,\n    owns migration_app_label,\n    owns migration_name,\n    owns migration_applied_at,\n    owns migration_checksum;\n"
            );

            let ctx = self
                .db
                .transaction_context(TxType::Schema)
                .await
                .map_err(map_orm_error)?;
            ctx.query(&schema).await.map_err(map_orm_error)?;
            ctx.commit().await.map_err(map_orm_error)?;

            self.schema_ensured.store(true, Ordering::Release);
            Ok(())
        })
    }

    fn load_applied(&self) -> BoxFuture<'_, Result<Vec<AppliedMigrationRecord>>> {
        Box::pin(async move {
            self.ensure_schema().await?;

            // Ported verbatim from state.py:179-191.
            let query = format!(
                "\nmatch\n$m isa {ENTITY_NAME},\n    has migration_app_label $app,\n    has migration_name $name,\n    has migration_applied_at $applied,\n    has migration_checksum $checksum;\nfetch {{\n    \"app\": $app,\n    \"name\": $name,\n    \"applied\": $applied,\n    \"checksum\": $checksum\n}};\n"
            );

            let ctx = self
                .db
                .transaction_context(TxType::Read)
                .await
                .map_err(map_orm_error)?;
            let result = ctx.query(&query).await.map_err(map_orm_error)?;

            let values = match result {
                QueryResult::Documents(docs) => docs,
                QueryResult::Rows(rows) => rows,
                QueryResult::Ok => Vec::new(),
            };
            parse_applied_documents(&values)
        })
    }

    fn record_applied(&self, record: AppliedMigrationRecord) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.ensure_schema().await?;

            // Rust stamps applied_at when the record carries none, mirroring
            // state.py:249-250 (`datetime.now(UTC).strftime(...)`).
            let applied_at = record
                .applied_at
                .clone()
                .unwrap_or_else(|| format_applied_at(Utc::now()));

            let migration_id = format!("{}:{}", record.app_label, record.name);

            // Idempotent replace: delete any existing row for this migration_id
            // (the @key) before inserting, so re-recording an already-applied
            // migration updates in place instead of failing the @key constraint.
            // This gives the TypeDB store the same dedup semantics the in-memory
            // store has behind the shared seam.
            let delete_existing = format!(
                "\nmatch\n$m isa {ENTITY_NAME},\n    has migration_id \"{migration_id}\";\ndelete $m;\n"
            );

            // Field set + storage schema match state.py:253-260. `applied_at` is
            // emitted unquoted (a TypeQL datetime literal); every other field quoted.
            let insert = format!(
                "\ninsert $m isa {ENTITY_NAME},\n    has migration_id \"{migration_id}\",\n    has migration_app_label \"{app}\",\n    has migration_name \"{name}\",\n    has migration_applied_at {applied_at},\n    has migration_checksum \"{checksum}\";\n",
                app = record.app_label,
                name = record.name,
                checksum = record.checksum,
            );

            let ctx = self
                .db
                .transaction_context(TxType::Write)
                .await
                .map_err(map_orm_error)?;
            ctx.query(&delete_existing).await.map_err(map_orm_error)?;
            ctx.query(&insert).await.map_err(map_orm_error)?;
            ctx.commit().await.map_err(map_orm_error)?;
            Ok(())
        })
    }

    fn record_unapplied<'a>(
        &'a self,
        app_label: &'a str,
        name: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.ensure_schema().await?;

            // Storage schema matches state.py:288-293; the delete clause uses the
            // TypeDB 3.x form `delete $m;` (the older `delete $m isa <type>;` that
            // the Python code carried is a parse error on TypeDB 3.x).
            let query = format!(
                "\nmatch\n$m isa {ENTITY_NAME},\n    has migration_app_label \"{app_label}\",\n    has migration_name \"{name}\";\ndelete $m;\n"
            );

            let ctx = self
                .db
                .transaction_context(TxType::Write)
                .await
                .map_err(map_orm_error)?;
            ctx.query(&query).await.map_err(map_orm_error)?;
            ctx.commit().await.map_err(map_orm_error)?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};

    // ── document parsing (no TypeDB) ────────────────────────────────────────
    //
    // Feeds a hand-built fetch-document list in the REAL TypeDB shape: each
    // attribute-bound variable renders as a bare JSON scalar (typedb-driver
    // 3.8.1 `json_value`), NOT a `{"value": ...}` wrapper. This is the P0
    // silent-failure guard — a shape mismatch yields empty state with no error.

    #[test]
    fn parse_applied_documents_extracts_bare_scalar_fields() {
        let docs = vec![serde_json::json!({
            "app": "myapp",
            "name": "0001_initial",
            "applied": "2026-06-05T00:00:00.000000000",
            "checksum": "abc123"
        })];

        let records = parse_applied_documents(&docs).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].app_label, "myapp");
        assert_eq!(records[0].name, "0001_initial");
        assert_eq!(records[0].checksum, "abc123");
        assert_eq!(
            records[0].applied_at.as_deref(),
            Some("2026-06-05T00:00:00.000000000")
        );
    }

    #[test]
    fn parse_applied_documents_also_unwraps_value_envelope() {
        // Defensive: the original `_extract_value` accepted a `{"value": ...}`
        // object form too. Mixed shapes in one list must both parse.
        let docs = vec![serde_json::json!({
            "app": {"value": "myapp"},
            "name": {"value": "0002_next"},
            "applied": {"value": "2026-06-05T01:02:03.000000000"},
            "checksum": {"value": "def456"}
        })];

        let records = parse_applied_documents(&docs).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].app_label, "myapp");
        assert_eq!(records[0].name, "0002_next");
        assert_eq!(records[0].checksum, "def456");
        assert_eq!(
            records[0].applied_at.as_deref(),
            Some("2026-06-05T01:02:03.000000000")
        );
    }

    #[test]
    fn parse_applied_documents_empty_list_is_empty() {
        let records = parse_applied_documents(&[]).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn parse_applied_documents_skips_incomplete_rows() {
        // Missing `checksum` → skipped, mirroring the Python `all([...])` guard.
        let docs = vec![
            serde_json::json!({
                "app": "myapp",
                "name": "0001_initial",
                "applied": "2026-06-05T00:00:00.000000000"
            }),
            serde_json::json!({
                "app": "myapp",
                "name": "0002_next",
                "applied": "2026-06-05T00:00:00.000000000",
                "checksum": "ok"
            }),
        ];

        let records = parse_applied_documents(&docs).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "0002_next");
    }

    #[test]
    fn parse_applied_documents_carries_missing_applied_as_none() {
        let docs = vec![serde_json::json!({
            "app": "myapp",
            "name": "0001_initial",
            "checksum": "abc123"
        })];

        let records = parse_applied_documents(&docs).unwrap();
        assert_eq!(records.len(), 1);
        assert!(records[0].applied_at.is_none());
    }

    // ── timestamp parity (no TypeDB) ────────────────────────────────────────
    //
    // Asserts the Rust format string is byte-identical to Python's
    // `strftime("%Y-%m-%dT%H:%M:%S.%f")`: 6 fractional digits, zero-padded,
    // no timezone suffix.

    #[test]
    fn format_applied_at_matches_python_strftime() {
        // 2026-06-05 14:09:08.123456 UTC.
        let dt = Utc
            .with_ymd_and_hms(2026, 6, 5, 14, 9, 8)
            .unwrap()
            .with_nanosecond(123_456_000)
            .unwrap();
        assert_eq!(format_applied_at(dt), "2026-06-05T14:09:08.123456");
    }

    #[test]
    fn format_applied_at_zero_pads_microseconds() {
        // Python `%f` zero-pads to exactly 6 digits; a sub-microsecond value
        // truncates and pads identically under chrono `%6f`.
        let dt = Utc
            .with_ymd_and_hms(2026, 1, 2, 3, 4, 5)
            .unwrap()
            .with_nanosecond(7_000)
            .unwrap();
        // 7000 ns = 7 microseconds → ".000007".
        assert_eq!(format_applied_at(dt), "2026-01-02T03:04:05.000007");
    }
}
