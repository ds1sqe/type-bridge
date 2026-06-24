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

use crate::state::{MigrationRunRecord, MigrationStateStore};
use crate::{AppliedMigrationRecord, MigrationError, Result};

/// The migration-tracking entity type name.
///
/// Mirrors `MigrationStateManager.ENTITY_NAME` (`state.py:111`).
const ENTITY_NAME: &str = "type_bridge_migration";

/// The append/update migration run-log entity type name.
const RUN_ENTITY_NAME: &str = "type_bridge_migration_run";

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

    async fn type_exists(&self, type_name: &str) -> bool {
        let check_query = format!(
            "\n            match $t type {type_name};\n            fetch {{ \"exists\": true }};\n        "
        );

        match self.db.transaction_context(TxType::Read).await {
            Ok(ctx) => match ctx.query(&check_query).await {
                Ok(QueryResult::Documents(docs)) => !docs.is_empty(),
                Ok(QueryResult::Rows(rows)) => !rows.is_empty(),
                Ok(QueryResult::Ok) => false,
                Err(_) => false,
            },
            Err(_) => false,
        }
    }

    async fn ensure_type(&self, type_name: &str, define_typeql: &str) -> Result<()> {
        if self.type_exists(type_name).await {
            return Ok(());
        }

        let ctx = self
            .db
            .transaction_context(TxType::Schema)
            .await
            .map_err(map_orm_error)?;
        match ctx.query(define_typeql).await {
            Ok(_) => {
                ctx.commit().await.map_err(map_orm_error)?;
                Ok(())
            }
            Err(error) => {
                let _ = ctx.rollback().await;
                if self.type_exists(type_name).await {
                    Ok(())
                } else {
                    Err(map_orm_error(error))
                }
            }
        }
    }

    async fn query_documents(&self, query: &str) -> Result<Vec<serde_json::Value>> {
        let ctx = self
            .db
            .transaction_context(TxType::Read)
            .await
            .map_err(map_orm_error)?;
        let result = ctx.query(query).await.map_err(map_orm_error)?;
        Ok(query_result_values(result))
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

fn query_result_values(result: QueryResult) -> Vec<serde_json::Value> {
    match result {
        QueryResult::Documents(docs) => docs,
        QueryResult::Rows(rows) => rows,
        QueryResult::Ok => Vec::new(),
    }
}

fn typeql_string_literal(value: &str) -> String {
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
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

/// Parse TypeDB fetch documents into migration run-log records.
pub fn parse_run_documents(values: &[serde_json::Value]) -> Result<Vec<MigrationRunRecord>> {
    let mut records = Vec::with_capacity(values.len());
    for doc in values {
        let (
            Some(run_id),
            Some(app_label),
            Some(name),
            Some(checksum),
            Some(direction),
            Some(status),
            Some(started_at),
        ) = (
            extract_value(doc, "run_id"),
            extract_value(doc, "app"),
            extract_value(doc, "name"),
            extract_value(doc, "checksum"),
            extract_value(doc, "direction"),
            extract_value(doc, "status"),
            extract_value(doc, "started"),
        )
        else {
            continue;
        };
        records.push(MigrationRunRecord {
            run_id,
            app_label,
            name,
            checksum,
            direction,
            status,
            started_at,
            finished_at: None,
            error: None,
            executor_ip: None,
            executor_mac: None,
        });
    }
    Ok(records)
}

fn optional_run_field_query(attribute: &str, alias: &str) -> String {
    format!(
        "\nmatch\n$r isa {RUN_ENTITY_NAME},\n    has migration_run_id $run_id,\n    has {attribute} ${alias};\nfetch {{\n    \"run_id\": $run_id,\n    \"{alias}\": ${alias}\n}};\n"
    )
}

fn merge_optional_run_field(
    runs: &mut [MigrationRunRecord],
    docs: &[serde_json::Value],
    target: &str,
    source: &str,
) {
    for doc in docs {
        let (Some(run_id), Some(value)) =
            (extract_value(doc, "run_id"), extract_value(doc, source))
        else {
            continue;
        };
        let Some(run) = runs.iter_mut().find(|run| run.run_id == run_id) else {
            continue;
        };
        match target {
            "finished_at" => run.finished_at = Some(value),
            "error" => run.error = Some(value),
            "executor_ip" => run.executor_ip = Some(value),
            "executor_mac" => run.executor_mac = Some(value),
            _ => {}
        }
    }
}

fn run_insert_query(record: &MigrationRunRecord) -> String {
    let mut fields = vec![
        format!(
            "    has migration_run_id {}",
            typeql_string_literal(&record.run_id)
        ),
        format!(
            "    has migration_app_label {}",
            typeql_string_literal(&record.app_label)
        ),
        format!(
            "    has migration_name {}",
            typeql_string_literal(&record.name)
        ),
        format!(
            "    has migration_checksum {}",
            typeql_string_literal(&record.checksum)
        ),
        format!(
            "    has migration_direction {}",
            typeql_string_literal(&record.direction)
        ),
        format!(
            "    has migration_status {}",
            typeql_string_literal(&record.status)
        ),
        format!("    has migration_started_at {}", record.started_at),
    ];

    if let Some(finished_at) = &record.finished_at {
        fields.push(format!("    has migration_finished_at {finished_at}"));
    }
    if let Some(error) = &record.error {
        fields.push(format!(
            "    has migration_error {}",
            typeql_string_literal(error)
        ));
    }
    if let Some(executor_ip) = &record.executor_ip {
        fields.push(format!(
            "    has migration_executor_ip {}",
            typeql_string_literal(executor_ip)
        ));
    }
    if let Some(executor_mac) = &record.executor_mac {
        fields.push(format!(
            "    has migration_executor_mac {}",
            typeql_string_literal(executor_mac)
        ));
    }

    format!(
        "\ninsert $r isa {RUN_ENTITY_NAME},\n{};\n",
        fields.join(",\n")
    )
}

impl MigrationStateStore for TypeDbStateStore {
    fn ensure_schema(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if self.schema_ensured.load(Ordering::Acquire) {
                return Ok(());
            }

            for (name, value_type) in [
                ("migration_id", "string"),
                ("migration_app_label", "string"),
                ("migration_name", "string"),
                ("migration_applied_at", "datetime"),
                ("migration_checksum", "string"),
                ("migration_run_id", "string"),
                ("migration_direction", "string"),
                ("migration_status", "string"),
                ("migration_started_at", "datetime"),
                ("migration_finished_at", "datetime"),
                ("migration_error", "string"),
                ("migration_executor_ip", "string"),
                ("migration_executor_mac", "string"),
            ] {
                let define = format!("define\nattribute {name}, value {value_type};\n");
                self.ensure_type(name, &define).await?;
            }

            let applied_entity = format!(
                "define\nentity {ENTITY_NAME},\n    owns migration_id @key,\n    owns migration_app_label,\n    owns migration_name,\n    owns migration_applied_at,\n    owns migration_checksum;\n"
            );
            self.ensure_type(ENTITY_NAME, &applied_entity).await?;

            let run_entity = format!(
                "define\nentity {RUN_ENTITY_NAME},\n    owns migration_run_id @key,\n    owns migration_app_label,\n    owns migration_name,\n    owns migration_checksum,\n    owns migration_direction,\n    owns migration_status,\n    owns migration_started_at,\n    owns migration_finished_at,\n    owns migration_error,\n    owns migration_executor_ip,\n    owns migration_executor_mac;\n"
            );
            self.ensure_type(RUN_ENTITY_NAME, &run_entity).await?;

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

            let values = query_result_values(result);
            parse_applied_documents(&values)
        })
    }

    fn load_runs(&self) -> BoxFuture<'_, Result<Vec<MigrationRunRecord>>> {
        Box::pin(async move {
            self.ensure_schema().await?;

            let query = format!(
                "\nmatch\n$r isa {RUN_ENTITY_NAME},\n    has migration_run_id $run_id,\n    has migration_app_label $app,\n    has migration_name $name,\n    has migration_checksum $checksum,\n    has migration_direction $direction,\n    has migration_status $status,\n    has migration_started_at $started;\nfetch {{\n    \"run_id\": $run_id,\n    \"app\": $app,\n    \"name\": $name,\n    \"checksum\": $checksum,\n    \"direction\": $direction,\n    \"status\": $status,\n    \"started\": $started\n}};\n"
            );
            let mut runs = parse_run_documents(&self.query_documents(&query).await?)?;

            let finished_query = optional_run_field_query("migration_finished_at", "finished");
            let finished_docs = self.query_documents(&finished_query).await?;
            merge_optional_run_field(&mut runs, &finished_docs, "finished_at", "finished");

            let error_query = optional_run_field_query("migration_error", "error");
            let error_docs = self.query_documents(&error_query).await?;
            merge_optional_run_field(&mut runs, &error_docs, "error", "error");

            let ip_query = optional_run_field_query("migration_executor_ip", "executor_ip");
            let ip_docs = self.query_documents(&ip_query).await?;
            merge_optional_run_field(&mut runs, &ip_docs, "executor_ip", "executor_ip");

            let mac_query = optional_run_field_query("migration_executor_mac", "executor_mac");
            let mac_docs = self.query_documents(&mac_query).await?;
            merge_optional_run_field(&mut runs, &mac_docs, "executor_mac", "executor_mac");

            Ok(runs)
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
            let migration_id = typeql_string_literal(&migration_id);
            let app = typeql_string_literal(&record.app_label);
            let name = typeql_string_literal(&record.name);
            let checksum = typeql_string_literal(&record.checksum);

            // Idempotent replace: delete any existing row for this migration_id
            // (the @key) before inserting, so re-recording an already-applied
            // migration updates in place instead of failing the @key constraint.
            // This gives the TypeDB store the same dedup semantics the in-memory
            // store has behind the shared seam.
            let delete_existing = format!(
                "\nmatch\n$m isa {ENTITY_NAME},\n    has migration_id {migration_id};\ndelete $m;\n"
            );

            // Field set + storage schema match state.py:253-260. `applied_at` is
            // emitted unquoted (a TypeQL datetime literal); every other field quoted.
            let insert = format!(
                "\ninsert $m isa {ENTITY_NAME},\n    has migration_id {migration_id},\n    has migration_app_label {app},\n    has migration_name {name},\n    has migration_applied_at {applied_at},\n    has migration_checksum {checksum};\n",
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
            let app_label = typeql_string_literal(app_label);
            let name = typeql_string_literal(name);
            let query = format!(
                "\nmatch\n$m isa {ENTITY_NAME},\n    has migration_app_label {app_label},\n    has migration_name {name};\ndelete $m;\n"
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

    fn record_run(&self, record: MigrationRunRecord) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.ensure_schema().await?;

            let run_id = typeql_string_literal(&record.run_id);
            let delete_existing = format!(
                "\nmatch\n$r isa {RUN_ENTITY_NAME},\n    has migration_run_id {run_id};\ndelete $r;\n"
            );
            let insert = run_insert_query(&record);

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

    #[test]
    fn parse_run_documents_extracts_required_fields() {
        let docs = vec![serde_json::json!({
            "run_id": "run-1",
            "app": "app",
            "name": "0001_initial",
            "checksum": "abc123",
            "direction": "apply",
            "status": "started",
            "started": "2026-06-05T00:00:00.000000"
        })];

        let records = parse_run_documents(&docs).unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].run_id, "run-1");
        assert_eq!(records[0].direction, "apply");
        assert_eq!(records[0].status, "started");
        assert_eq!(records[0].finished_at, None);
    }

    #[test]
    fn merge_optional_run_field_updates_matching_record_only() {
        let mut records = vec![MigrationRunRecord {
            run_id: "run-1".to_string(),
            app_label: "app".to_string(),
            name: "0001_initial".to_string(),
            checksum: "abc123".to_string(),
            direction: "apply".to_string(),
            status: "started".to_string(),
            started_at: "2026-06-05T00:00:00.000000".to_string(),
            finished_at: None,
            error: None,
            executor_ip: None,
            executor_mac: None,
        }];
        let docs = vec![serde_json::json!({
            "run_id": "run-1",
            "finished": "2026-06-05T00:00:01.000000"
        })];

        merge_optional_run_field(&mut records, &docs, "finished_at", "finished");

        assert_eq!(
            records[0].finished_at.as_deref(),
            Some("2026-06-05T00:00:01.000000")
        );
    }

    #[test]
    fn typeql_string_literal_escapes_user_controlled_text() {
        assert_eq!(typeql_string_literal("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
    }

    #[test]
    fn run_insert_query_includes_optional_fields_when_present() {
        let record = MigrationRunRecord {
            run_id: "run-1".to_string(),
            app_label: "app".to_string(),
            name: "0001_initial".to_string(),
            checksum: "abc123".to_string(),
            direction: "apply".to_string(),
            status: "failed".to_string(),
            started_at: "2026-06-05T00:00:00.000000".to_string(),
            finished_at: Some("2026-06-05T00:00:01.000000".to_string()),
            error: Some("quote: \"boom\"".to_string()),
            executor_ip: Some("127.0.0.1".to_string()),
            executor_mac: Some("00:11:22:33:44:55".to_string()),
        };

        let query = run_insert_query(&record);

        assert!(query.contains("has migration_run_id \"run-1\""));
        assert!(query.contains("has migration_finished_at 2026-06-05T00:00:01.000000"));
        assert!(query.contains("has migration_error \"quote: \\\"boom\\\"\""));
        assert!(query.contains("has migration_executor_ip \"127.0.0.1\""));
        assert!(query.contains("has migration_executor_mac \"00:11:22:33:44:55\""));
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
