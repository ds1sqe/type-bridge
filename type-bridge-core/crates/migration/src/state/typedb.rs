//! TypeDB-backed [`MigrationStateStore`] implementation.
//!
//! [`TypeDbStateStore`] persists the established migration projection and run
//! log over the ORM [`Database`][type_bridge_orm::Database] seam. Its per-type
//! bootstrap definitions are rendered from the canonical migration-state
//! [`SchemaInfo`][type_bridge_orm::schema::SchemaInfo], and its row queries use
//! the same semantic label constants. Existing labels, value types, keys, and
//! storage behavior remain unchanged.
//!
//! All work runs through [`Database::transaction_context`], mirroring the
//! Phase-05 executor: open a context for the right [`TxType`], run the query,
//! and commit write/schema contexts. No Python transaction crosses this
//! boundary (invariant 4); only the public ORM `session` API is used
//! (invariant 7).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
pub use type_bridge_contract::reserved::{
    LEGACY_CUTOVER_SENTINEL_APP_LABEL, LEGACY_CUTOVER_SENTINEL_APPLIED_AT,
    LEGACY_CUTOVER_SENTINEL_MIGRATION_ID, LEGACY_CUTOVER_SENTINEL_NAME,
    LEGACY_WRITER_CUTOVER_MESSAGE,
};
use type_bridge_orm::OrmError;
use type_bridge_orm::schema::{SchemaError, SchemaInfo};
use type_bridge_orm::session::backend::{BoxFuture, QueryResult, TxType};
use type_bridge_orm::{
    Database, Transaction, TransactionContext,
    require_legacy_writer_open as require_orm_legacy_writer_open,
    require_legacy_writer_open_in_transaction as require_orm_legacy_writer_open_in_transaction,
};

use crate::state::schema::labels::{
    APP_LABEL, APPLIED_AT, APPLIED_ENTITY, CHECKSUM, DIRECTION, ERROR, EXECUTOR_IP, EXECUTOR_MAC,
    FINISHED_AT, MIGRATION_ID, NAME, RUN_ENTITY, RUN_ID, STARTED_AT, STATUS,
};
use crate::state::schema::migration_state_schema;
use crate::state::{MigrationRunRecord, MigrationStateStore};
use crate::{AppliedMigrationRecord, MigrationError, Result};

/// Timestamp format mirroring Python's `strftime("%Y-%m-%dT%H:%M:%S.%f")`.
///
/// Python `%f` always emits exactly six digits (microseconds, zero-padded);
/// chrono's `%6f` is the byte-identical equivalent. This is the format written
/// into the `insert` TypeQL — `record_applied` (`state.py:250`).
const APPLIED_AT_FORMAT: &str = "%Y-%m-%dT%H:%M:%S.%6f";

const LEGACY_STATE_SCHEMA_PROBE_QUERY_TAG: &str =
    "# typebridge-internal-legacy-state-schema-probe/v1\n";

/// Required state of the V2-only legacy-ledger sentinel while a caller reads a
/// legacy applied projection.
#[derive(Debug, Clone, Copy)]
pub enum LegacyCutoverSentinelExpectation<'a> {
    /// No cutover sentinel may exist.
    Absent,
    /// The sentinel may be absent, or must exactly carry this fingerprint.
    OptionalExact(&'a str),
    /// One exact sentinel carrying this fingerprint must exist.
    RequiredExact(&'a str),
}

/// A verified legacy applied projection with the V2-only sentinel removed.
///
/// Construction is possible only after the sentinel candidate probes and all
/// stored sentinel fields have passed the requested expectation.  Callers must
/// never filter the reserved identity directly from an unverified row list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedLegacyAppliedPartition {
    applied: Vec<AppliedMigrationRecord>,
    sentinel_fingerprint: Option<String>,
}

impl VerifiedLegacyAppliedPartition {
    /// Borrow the released user migration records, excluding the verified
    /// V2-only sentinel partition.
    pub fn applied(&self) -> &[AppliedMigrationRecord] {
        &self.applied
    }

    /// Consume the partition and return released user migration records.
    pub fn into_applied(self) -> Vec<AppliedMigrationRecord> {
        self.applied
    }

    /// Return the exact sentinel fingerprint when the verified pair is present.
    pub fn sentinel_fingerprint(&self) -> Option<&str> {
        self.sentinel_fingerprint.as_deref()
    }
}

/// Failure to observe and validate the reserved V2 cutover sentinel.
#[derive(Debug, thiserror::Error)]
pub enum LegacyCutoverSentinelError {
    /// The ledger could not be queried through the retained transaction.
    #[error("legacy cutover sentinel storage inspection failed: {0}")]
    Storage(#[source] MigrationError),
    /// Stored rows violate the exact singleton contract.
    #[error("legacy cutover sentinel contract violation: {message}")]
    Contract {
        /// Stable human-readable contract failure.
        message: String,
    },
}

impl LegacyCutoverSentinelError {
    /// Return whether this is durable stored drift rather than provider
    /// infrastructure failure.
    pub fn is_contract_violation(&self) -> bool {
        matches!(self, Self::Contract { .. })
    }
}

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

    async fn type_exists(&self, type_name: &str) -> Result<bool> {
        let kind = state_type_kind(type_name).ok_or_else(|| MigrationError::State {
            message: format!("unknown migration-state schema label: {type_name}"),
        })?;
        let check_query = format!(
            "\n            match {kind} $t;\n            fetch {{ \"label\": label($t) }};\n        "
        );

        let ctx = self
            .db
            .transaction_context(TxType::Read)
            .await
            .map_err(map_orm_error)?;
        let checked = ctx
            .query(&check_query)
            .await
            .map_err(map_orm_error)
            .and_then(|result| schema_labels_from_result(result, "migration state type probe"))
            .map(|labels| labels.contains(type_name));
        let closed = ctx.close().await.map_err(map_orm_error);
        match (checked, closed) {
            (Ok(exists), Ok(())) => Ok(exists),
            (Err(primary), Ok(())) => Err(primary),
            (Ok(_), Err(cleanup)) => Err(cleanup),
            (Err(primary), Err(_)) => Err(primary),
        }
    }

    async fn ensure_type(&self, type_name: &str, define_typeql: &str) -> Result<()> {
        if self.type_exists(type_name).await? {
            return Ok(());
        }

        let ctx = self
            .db
            .transaction_context(TxType::Schema)
            .await
            .map_err(map_orm_error)?;
        if let Err(error) = require_legacy_writer_open_in_transaction(&ctx).await {
            let _ = ctx.rollback().await;
            return Err(error);
        }
        match ctx.query(define_typeql).await {
            Ok(_) => {
                ctx.commit().await.map_err(map_orm_error)?;
                Ok(())
            }
            Err(error) => {
                if let Err(cleanup) = ctx.rollback().await {
                    return Err(schema_rollback_cleanup_failure(&error, &cleanup));
                }
                if self.type_exists(type_name).await? {
                    Ok(())
                } else {
                    Err(map_orm_error(error))
                }
            }
        }
    }

    async fn ensure_schema_for_read(&self) -> Result<()> {
        if self.schema_ensured.load(Ordering::Acquire) {
            return Ok(());
        }

        // An adopted database already has the complete frozen legacy state
        // schema.  Archival reads must remain available there and must expose
        // the sentinel to ordinary legacy planning; only a genuinely missing
        // type enters the writer-guarded bootstrap path.
        let mut transaction = self.db.read_transaction().await.map_err(map_orm_error)?;
        let inspected = legacy_state_schema_presence(&mut transaction)
            .await
            .map_err(legacy_sentinel_error_into_migration_error);
        let closed = transaction.close().await.map_err(map_orm_error);
        let presence = match (inspected, closed) {
            (Ok(presence), Ok(())) => presence,
            (Err(primary), Ok(())) => return Err(primary),
            (Ok(_), Err(cleanup)) => return Err(cleanup),
            (Err(primary), Err(_)) => return Err(primary),
        };
        if presence == LegacyStateSchemaPresence::Complete {
            self.schema_ensured.store(true, Ordering::Release);
            return Ok(());
        }

        // Released readers repaired interrupted incremental bootstraps. Keep
        // that behavior for absent and partial unadopted schemas by entering
        // the ordinary writer-guarded bootstrap. An adopted target is rejected
        // by the sentinel before any repair mutation.
        self.ensure_schema().await
    }

    async fn query_documents(&self, query: &str) -> Result<Vec<serde_json::Value>> {
        let ctx = self
            .db
            .transaction_context(TxType::Read)
            .await
            .map_err(map_orm_error)?;
        let queried = ctx
            .query(query)
            .await
            .map_err(map_orm_error)
            .map(query_result_values);
        let closed = ctx.close().await.map_err(map_orm_error);
        match (queried, closed) {
            (Ok(values), Ok(())) => Ok(values),
            (Err(primary), Ok(())) => Err(primary),
            (Ok(_), Err(cleanup)) => Err(cleanup),
            (Err(primary), Err(_)) => Err(primary),
        }
    }

    /// Read the complete released applied ledger through an already-retained
    /// transaction.
    ///
    /// Legacy-frontier cutover uses a managed schema transaction as an
    /// exclusive guard against V1 write transactions. The caller must ensure
    /// the released state schema already exists before opening that guard;
    /// this method performs no bootstrap and never commits or closes it.
    pub async fn load_applied_in_transaction(
        transaction: &mut Transaction,
    ) -> Result<Vec<AppliedMigrationRecord>> {
        let result = transaction
            .query(&applied_query())
            .await
            .map_err(map_orm_error)?;
        parse_applied_documents(&query_result_values(result))
    }

    /// Read the released applied projection and validate the reserved V2
    /// sentinel through the same retained transaction snapshot.
    ///
    /// The sentinel is removed only after both independent identity probes,
    /// singleton cardinality, every stored field, and the expected anchor
    /// fingerprint have been checked.  This is the only supported filtering
    /// boundary for V2 legacy-frontier continuity and digest calculations.
    pub async fn load_verified_legacy_partition_in_transaction(
        transaction: &mut Transaction,
        expectation: LegacyCutoverSentinelExpectation<'_>,
    ) -> std::result::Result<VerifiedLegacyAppliedPartition, LegacyCutoverSentinelError> {
        match legacy_state_schema_presence(transaction).await? {
            LegacyStateSchemaPresence::Absent => {
                if matches!(
                    expectation,
                    LegacyCutoverSentinelExpectation::RequiredExact(_)
                ) {
                    return Err(sentinel_contract_error(
                        "the V2 bridge is active but the frozen legacy ledger schema is absent",
                    ));
                }
                return Ok(VerifiedLegacyAppliedPartition {
                    applied: Vec::new(),
                    sentinel_fingerprint: None,
                });
            }
            LegacyStateSchemaPresence::Partial => {
                return Err(sentinel_contract_error(
                    "the frozen legacy ledger schema is partially present",
                ));
            }
            LegacyStateSchemaPresence::Complete => {}
        }
        let result = transaction
            .query(&applied_query())
            .await
            .map_err(map_sentinel_storage_error)?;
        let mut applied = parse_applied_documents(&query_result_values(result))
            .map_err(LegacyCutoverSentinelError::Storage)?;

        let id_candidates = sentinel_query_values(
            transaction,
            &format!(
                "match $m isa {APPLIED_ENTITY}, has {MIGRATION_ID} {}; fetch {{ \"exists\": true }};",
                typeql_string_literal(LEGACY_CUTOVER_SENTINEL_MIGRATION_ID),
            ),
        )
        .await?;
        let name_candidates = sentinel_query_values(
            transaction,
            &format!(
                "match $m isa {APPLIED_ENTITY}, has {NAME} {}; fetch {{ \"exists\": true }};",
                typeql_string_literal(LEGACY_CUTOVER_SENTINEL_NAME),
            ),
        )
        .await?;

        if id_candidates.is_empty() && name_candidates.is_empty() {
            if matches!(
                expectation,
                LegacyCutoverSentinelExpectation::RequiredExact(_)
            ) {
                return Err(sentinel_contract_error(
                    "the V2 bridge is active but its legacy-writer sentinel is missing",
                ));
            }
            return Ok(VerifiedLegacyAppliedPartition {
                applied,
                sentinel_fingerprint: None,
            });
        }

        if matches!(expectation, LegacyCutoverSentinelExpectation::Absent) {
            return Err(sentinel_contract_error(
                "a legacy-writer sentinel exists without an active or pending V2 bridge",
            ));
        }
        if id_candidates.len() != 1 || name_candidates.len() != 1 {
            return Err(sentinel_contract_error(
                "the legacy-writer sentinel is duplicated or has split identity rows",
            ));
        }

        let details = sentinel_query_values(
            transaction,
            &format!(
                "match $m isa {APPLIED_ENTITY}, has {MIGRATION_ID} {}, has {APP_LABEL} $app, has {NAME} {}, has {APPLIED_AT} $applied, has {CHECKSUM} $checksum; fetch {{ \"app\": $app, \"applied\": $applied, \"checksum\": $checksum }};",
                typeql_string_literal(LEGACY_CUTOVER_SENTINEL_MIGRATION_ID),
                typeql_string_literal(LEGACY_CUTOVER_SENTINEL_NAME),
            ),
        )
        .await?;
        if details.len() != 1 {
            return Err(sentinel_contract_error(
                "the legacy-writer sentinel is missing required exact fields",
            ));
        }
        let detail = &details[0];
        let app = extract_value(detail, "app").ok_or_else(|| {
            sentinel_contract_error("the legacy-writer sentinel app label is malformed")
        })?;
        let applied_at = extract_value(detail, "applied").ok_or_else(|| {
            sentinel_contract_error("the legacy-writer sentinel applied timestamp is malformed")
        })?;
        let fingerprint = extract_value(detail, "checksum").ok_or_else(|| {
            sentinel_contract_error("the legacy-writer sentinel checksum is malformed")
        })?;
        if app != LEGACY_CUTOVER_SENTINEL_APP_LABEL {
            return Err(sentinel_contract_error(
                "the legacy-writer sentinel carries a foreign application label",
            ));
        }
        if applied_at != LEGACY_CUTOVER_SENTINEL_APPLIED_AT {
            return Err(sentinel_contract_error(
                "the legacy-writer sentinel carries a foreign applied timestamp",
            ));
        }
        if !is_lower_hex_fingerprint(&fingerprint) {
            return Err(sentinel_contract_error(
                "the legacy-writer sentinel checksum is not a lowercase 64-hex fingerprint",
            ));
        }
        let expected = match expectation {
            LegacyCutoverSentinelExpectation::OptionalExact(expected)
            | LegacyCutoverSentinelExpectation::RequiredExact(expected) => expected,
            LegacyCutoverSentinelExpectation::Absent => unreachable!("handled above"),
        };
        if fingerprint != expected {
            return Err(sentinel_contract_error(
                "the legacy-writer sentinel checksum differs from the managed cutover anchor",
            ));
        }

        let original_len = applied.len();
        applied.retain(|record| {
            record.app_label != LEGACY_CUTOVER_SENTINEL_APP_LABEL
                || record.name != LEGACY_CUTOVER_SENTINEL_NAME
        });
        if original_len.saturating_sub(applied.len()) != 1 {
            return Err(sentinel_contract_error(
                "the exact legacy-writer sentinel is absent from the released applied projection",
            ));
        }

        Ok(VerifiedLegacyAppliedPartition {
            applied,
            sentinel_fingerprint: Some(fingerprint),
        })
    }

    /// Stage the complete V2 cutover sentinel in a caller-retained managed
    /// transaction.  The caller commits this in the same transaction as the
    /// managed cutover anchor.
    pub async fn insert_legacy_cutover_sentinel_in_transaction(
        transaction: &mut Transaction,
        anchor_fingerprint: &str,
    ) -> Result<()> {
        if !is_lower_hex_fingerprint(anchor_fingerprint) {
            return Err(MigrationError::State {
                message: "legacy cutover sentinel requires a lowercase 64-hex anchor fingerprint"
                    .to_owned(),
            });
        }
        let query = format!(
            "insert $m isa {APPLIED_ENTITY}, has {MIGRATION_ID} {}, has {APP_LABEL} {}, has {NAME} {}, has {APPLIED_AT} {LEGACY_CUTOVER_SENTINEL_APPLIED_AT}, has {CHECKSUM} {};",
            typeql_string_literal(LEGACY_CUTOVER_SENTINEL_MIGRATION_ID),
            typeql_string_literal(LEGACY_CUTOVER_SENTINEL_APP_LABEL),
            typeql_string_literal(LEGACY_CUTOVER_SENTINEL_NAME),
            typeql_string_literal(anchor_fingerprint),
        );
        transaction.query(&query).await.map_err(map_orm_error)?;
        Ok(())
    }
}

/// Fail before a legacy writer uses an already-open transaction when an exact,
/// managed-anchor-bound V2 cutover pair is present.
pub async fn require_legacy_writer_open_in_transaction(
    transaction: &TransactionContext,
) -> Result<()> {
    require_orm_legacy_writer_open_in_transaction(transaction)
        .await
        .map_err(map_legacy_guard_error)
}

#[cfg(test)]
pub(crate) fn is_legacy_state_schema_probe_query(query: &str) -> bool {
    query.starts_with(LEGACY_STATE_SCHEMA_PROBE_QUERY_TAG)
}

/// Read-only entry guard for legacy writer surfaces whose external side
/// effects cannot share a TypeDB transaction.
pub async fn require_legacy_writer_open(database: &Database) -> Result<()> {
    require_orm_legacy_writer_open(database)
        .await
        .map_err(map_legacy_guard_error)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyStateSchemaPresence {
    Absent,
    Partial,
    Complete,
}

async fn legacy_state_schema_presence(
    transaction: &mut Transaction,
) -> std::result::Result<LegacyStateSchemaPresence, LegacyCutoverSentinelError> {
    let state_schema = migration_state_schema();
    let mut present = 0_usize;
    let expected_by_root = [
        (
            "attribute",
            state_schema
                .attributes
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        ),
        (
            "entity",
            state_schema
                .entities
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        ),
        (
            "relation",
            state_schema
                .relations
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        ),
    ];
    let total = expected_by_root
        .iter()
        .map(|(_, labels)| labels.len())
        .sum::<usize>();
    let all_expected = expected_by_root
        .iter()
        .flat_map(|(_, labels)| labels.iter().copied())
        .collect::<BTreeSet<_>>();
    let mut expected_labels_seen_in_any_kind = 0_usize;
    for (kind, expected) in expected_by_root {
        let result = transaction
            .query(&format!(
                "{LEGACY_STATE_SCHEMA_PROBE_QUERY_TAG}match {kind} $t; fetch {{ \"label\": label($t) }};"
            ))
            .await
            .map_err(map_sentinel_storage_error)?;
        let observed = schema_labels_from_result(result, "legacy state schema probe")
            .map_err(LegacyCutoverSentinelError::Storage)?;
        expected_labels_seen_in_any_kind += observed
            .iter()
            .filter(|label| all_expected.contains(label.as_str()))
            .count();
        present += expected
            .into_iter()
            .filter(|label| observed.contains(*label))
            .count();
    }
    if present == 0 && expected_labels_seen_in_any_kind == 0 {
        return Ok(LegacyStateSchemaPresence::Absent);
    }
    if present != total || expected_labels_seen_in_any_kind != total {
        return Ok(LegacyStateSchemaPresence::Partial);
    }
    Ok(LegacyStateSchemaPresence::Complete)
}

fn state_type_kind(type_name: &str) -> Option<&'static str> {
    let schema = migration_state_schema();
    if schema.attributes.contains_key(type_name) {
        Some("attribute")
    } else if schema.entities.contains_key(type_name) {
        Some("entity")
    } else if schema.relations.contains_key(type_name) {
        Some("relation")
    } else {
        None
    }
}

fn schema_labels_from_result(result: QueryResult, operation: &str) -> Result<BTreeSet<String>> {
    let values = match result {
        QueryResult::Documents(values) | QueryResult::Rows(values) => values,
        QueryResult::Ok => {
            return Err(MigrationError::State {
                message: format!("{operation} returned no document result"),
            });
        }
    };
    let mut labels = BTreeSet::new();
    for value in &values {
        let label = extract_value(value, "label").ok_or_else(|| MigrationError::State {
            message: format!("{operation} returned a malformed schema label"),
        })?;
        labels.insert(label);
    }
    Ok(labels)
}

fn legacy_sentinel_error_into_migration_error(error: LegacyCutoverSentinelError) -> MigrationError {
    match error {
        LegacyCutoverSentinelError::Storage(error) => error,
        LegacyCutoverSentinelError::Contract { message } => MigrationError::State { message },
    }
}

fn applied_query() -> String {
    format!(
        "\nmatch\n$m isa {APPLIED_ENTITY},\n    has {APP_LABEL} $app,\n    has {NAME} $name,\n    has {APPLIED_AT} $applied,\n    has {CHECKSUM} $checksum;\nfetch {{\n    \"app\": $app,\n    \"name\": $name,\n    \"applied\": $applied,\n    \"checksum\": $checksum\n}};\n"
    )
}

async fn sentinel_query_values(
    transaction: &mut Transaction,
    query: &str,
) -> std::result::Result<Vec<serde_json::Value>, LegacyCutoverSentinelError> {
    let result = transaction
        .query(query)
        .await
        .map_err(map_sentinel_storage_error)?;
    match result {
        QueryResult::Documents(values) | QueryResult::Rows(values) => Ok(values),
        QueryResult::Ok => Err(LegacyCutoverSentinelError::Storage(MigrationError::State {
            message: "legacy cutover sentinel fetch returned no document result".to_owned(),
        })),
    }
}

fn map_sentinel_storage_error(error: OrmError) -> LegacyCutoverSentinelError {
    LegacyCutoverSentinelError::Storage(map_orm_error(error))
}

fn sentinel_contract_error(message: impl Into<String>) -> LegacyCutoverSentinelError {
    LegacyCutoverSentinelError::Contract {
        message: message.into(),
    }
}

fn is_lower_hex_fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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

fn map_legacy_guard_error(error: OrmError) -> MigrationError {
    match error {
        OrmError::Transaction(message) if message == LEGACY_WRITER_CUTOVER_MESSAGE => {
            MigrationError::State { message }
        }
        error => map_orm_error(error),
    }
}

fn map_schema_error(error: SchemaError) -> MigrationError {
    MigrationError::State {
        message: error.to_string(),
    }
}

fn schema_rollback_cleanup_failure(primary: &OrmError, cleanup: &OrmError) -> MigrationError {
    MigrationError::State {
        message: format!(
            "schema bootstrap query failed and rollback was not acknowledged; primary: {primary}; cleanup: {cleanup}"
        ),
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
        "\nmatch\n$r isa {RUN_ENTITY},\n    has {RUN_ID} $run_id,\n    has {attribute} ${alias};\nfetch {{\n    \"run_id\": $run_id,\n    \"{alias}\": ${alias}\n}};\n"
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
        format!("    has {RUN_ID} {}", typeql_string_literal(&record.run_id)),
        format!(
            "    has {APP_LABEL} {}",
            typeql_string_literal(&record.app_label)
        ),
        format!("    has {NAME} {}", typeql_string_literal(&record.name)),
        format!(
            "    has {CHECKSUM} {}",
            typeql_string_literal(&record.checksum)
        ),
        format!(
            "    has {DIRECTION} {}",
            typeql_string_literal(&record.direction)
        ),
        format!("    has {STATUS} {}", typeql_string_literal(&record.status)),
        format!("    has {STARTED_AT} {}", record.started_at),
    ];

    if let Some(finished_at) = &record.finished_at {
        fields.push(format!("    has {FINISHED_AT} {finished_at}"));
    }
    if let Some(error) = &record.error {
        fields.push(format!("    has {ERROR} {}", typeql_string_literal(error)));
    }
    if let Some(executor_ip) = &record.executor_ip {
        fields.push(format!(
            "    has {EXECUTOR_IP} {}",
            typeql_string_literal(executor_ip)
        ));
    }
    if let Some(executor_mac) = &record.executor_mac {
        fields.push(format!(
            "    has {EXECUTOR_MAC} {}",
            typeql_string_literal(executor_mac)
        ));
    }

    format!("\ninsert $r isa {RUN_ENTITY},\n{};\n", fields.join(",\n"))
}

impl MigrationStateStore for TypeDbStateStore {
    fn ensure_schema(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            // Even the latched fast path is a legacy writer entry point.  The
            // read guard rejects an already-adopted target; each actual schema
            // transaction repeats the guard for race-free mutation ordering.
            require_legacy_writer_open(self.db.as_ref()).await?;
            if self.schema_ensured.load(Ordering::Acquire) {
                return Ok(());
            }

            let state_schema = migration_state_schema();

            for (name, attribute) in &state_schema.attributes {
                let mut definition = SchemaInfo::default();
                definition
                    .attributes
                    .insert(name.clone(), attribute.clone());
                let define = definition.to_typeql().map_err(map_schema_error)?;
                self.ensure_type(name, &define).await?;
            }

            for (name, entity) in &state_schema.entities {
                let mut definition = SchemaInfo::default();
                definition.entities.insert(name.clone(), entity.clone());
                let define = definition.to_typeql().map_err(map_schema_error)?;
                self.ensure_type(name, &define).await?;
            }

            for (name, relation) in &state_schema.relations {
                let mut definition = SchemaInfo::default();
                definition.relations.insert(name.clone(), relation.clone());
                let define = definition.to_typeql().map_err(map_schema_error)?;
                self.ensure_type(name, &define).await?;
            }

            self.schema_ensured.store(true, Ordering::Release);
            Ok(())
        })
    }

    fn load_applied(&self) -> BoxFuture<'_, Result<Vec<AppliedMigrationRecord>>> {
        Box::pin(async move {
            self.ensure_schema_for_read().await?;

            // Ported verbatim from state.py:179-191.
            let query = applied_query();

            let values = self.query_documents(&query).await?;
            parse_applied_documents(&values)
        })
    }

    fn load_runs(&self) -> BoxFuture<'_, Result<Vec<MigrationRunRecord>>> {
        Box::pin(async move {
            self.ensure_schema_for_read().await?;

            let query = format!(
                "\nmatch\n$r isa {RUN_ENTITY},\n    has {RUN_ID} $run_id,\n    has {APP_LABEL} $app,\n    has {NAME} $name,\n    has {CHECKSUM} $checksum,\n    has {DIRECTION} $direction,\n    has {STATUS} $status,\n    has {STARTED_AT} $started;\nfetch {{\n    \"run_id\": $run_id,\n    \"app\": $app,\n    \"name\": $name,\n    \"checksum\": $checksum,\n    \"direction\": $direction,\n    \"status\": $status,\n    \"started\": $started\n}};\n"
            );
            let mut runs = parse_run_documents(&self.query_documents(&query).await?)?;

            let finished_query = optional_run_field_query(FINISHED_AT, "finished");
            let finished_docs = self.query_documents(&finished_query).await?;
            merge_optional_run_field(&mut runs, &finished_docs, "finished_at", "finished");

            let error_query = optional_run_field_query(ERROR, "error");
            let error_docs = self.query_documents(&error_query).await?;
            merge_optional_run_field(&mut runs, &error_docs, "error", "error");

            let ip_query = optional_run_field_query(EXECUTOR_IP, "executor_ip");
            let ip_docs = self.query_documents(&ip_query).await?;
            merge_optional_run_field(&mut runs, &ip_docs, "executor_ip", "executor_ip");

            let mac_query = optional_run_field_query(EXECUTOR_MAC, "executor_mac");
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
                "\nmatch\n$m isa {APPLIED_ENTITY},\n    has {MIGRATION_ID} {migration_id};\ndelete $m;\n"
            );

            // Field set + storage schema match state.py:253-260. `applied_at` is
            // emitted unquoted (a TypeQL datetime literal); every other field quoted.
            let insert = format!(
                "\ninsert $m isa {APPLIED_ENTITY},\n    has {MIGRATION_ID} {migration_id},\n    has {APP_LABEL} {app},\n    has {NAME} {name},\n    has {APPLIED_AT} {applied_at},\n    has {CHECKSUM} {checksum};\n",
            );

            let ctx = self
                .db
                .transaction_context(TxType::Write)
                .await
                .map_err(map_orm_error)?;
            if let Err(error) = require_legacy_writer_open_in_transaction(&ctx).await {
                let _ = ctx.rollback().await;
                return Err(error);
            }
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
                "\nmatch\n$m isa {APPLIED_ENTITY},\n    has {APP_LABEL} {app_label},\n    has {NAME} {name};\ndelete $m;\n"
            );

            let ctx = self
                .db
                .transaction_context(TxType::Write)
                .await
                .map_err(map_orm_error)?;
            if let Err(error) = require_legacy_writer_open_in_transaction(&ctx).await {
                let _ = ctx.rollback().await;
                return Err(error);
            }
            ctx.query(&query).await.map_err(map_orm_error)?;
            ctx.commit().await.map_err(map_orm_error)?;
            Ok(())
        })
    }

    fn record_run(&self, record: MigrationRunRecord) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            self.ensure_schema().await?;

            let run_id = typeql_string_literal(&record.run_id);
            let delete_existing =
                format!("\nmatch\n$r isa {RUN_ENTITY},\n    has {RUN_ID} {run_id};\ndelete $r;\n");
            let insert = run_insert_query(&record);

            let ctx = self
                .db
                .transaction_context(TxType::Write)
                .await
                .map_err(map_orm_error)?;
            if let Err(error) = require_legacy_writer_open_in_transaction(&ctx).await {
                let _ = ctx.rollback().await;
                return Err(error);
            }
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
    use crate::testing::{MockEvent, MockMigrationBackend};
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

    #[tokio::test]
    async fn state_readers_close_every_read_context() {
        let responses = vec![
            QueryResult::Documents(vec![serde_json::json!({
                "app": "myapp",
                "name": "0001_initial",
                "applied": "2026-06-05T00:00:00.000000000",
                "checksum": "abc123"
            })]),
            QueryResult::Documents(Vec::new()),
            QueryResult::Documents(Vec::new()),
            QueryResult::Documents(Vec::new()),
            QueryResult::Documents(Vec::new()),
            QueryResult::Documents(Vec::new()),
        ];
        let (backend, log) = MockMigrationBackend::with_state_read_responses(responses);
        let store =
            TypeDbStateStore::new(Arc::new(Database::with_backend(Box::new(backend), "test")));

        assert_eq!(store.load_applied().await.unwrap().len(), 1);
        assert!(store.load_runs().await.unwrap().is_empty());

        let events = log.lock().unwrap();
        let opens = events
            .iter()
            .filter(|event| matches!(event, MockEvent::OpenTx(TxType::Read)))
            .count();
        let closes = events
            .iter()
            .filter(|event| matches!(event, MockEvent::Close))
            .count();
        assert_eq!(opens, 7, "one schema inspection plus six ledger reads");
        assert_eq!(closes, opens, "every read context must be acknowledged");
    }

    #[tokio::test]
    async fn load_applied_preserves_query_error_when_close_also_fails() {
        // Close 0 terminates the successful schema-presence inspection. The
        // applied-ledger query and close then fail together at indexes 0 and 1.
        let (backend, log) = MockMigrationBackend::with_state_read_and_close_failure(0, 1);
        let store =
            TypeDbStateStore::new(Arc::new(Database::with_backend(Box::new(backend), "test")));

        let error = store
            .load_applied()
            .await
            .expect_err("the ledger query must fail");
        let message = error.to_string();
        assert!(message.contains("injected query failure for testing"));
        assert!(!message.contains("injected close failure for testing"));
        assert_eq!(
            log.lock()
                .unwrap()
                .iter()
                .filter(|event| matches!(event, MockEvent::Close))
                .count(),
            2,
            "the failed ledger read must still acknowledge close"
        );
    }

    #[tokio::test]
    async fn load_runs_preserves_query_error_when_close_also_fails() {
        let (backend, log) = MockMigrationBackend::with_state_read_and_close_failure(0, 1);
        let store =
            TypeDbStateStore::new(Arc::new(Database::with_backend(Box::new(backend), "test")));

        let error = store
            .load_runs()
            .await
            .expect_err("the base run-log query must fail");
        let message = error.to_string();
        assert!(message.contains("injected query failure for testing"));
        assert!(!message.contains("injected close failure for testing"));
        assert_eq!(
            log.lock()
                .unwrap()
                .iter()
                .filter(|event| matches!(event, MockEvent::Close))
                .count(),
            2,
            "the failed run-log read must still acknowledge close"
        );
    }

    #[tokio::test]
    async fn unadopted_partial_state_schema_is_repaired_before_read() {
        let (backend, log, labels) =
            MockMigrationBackend::with_partial_state_schema(&[RUN_ENTITY], false);
        let store =
            TypeDbStateStore::new(Arc::new(Database::with_backend(Box::new(backend), "test")));

        assert!(store.load_applied().await.unwrap().is_empty());
        assert!(labels.lock().unwrap().contains(RUN_ENTITY));
        assert!(log.lock().unwrap().iter().any(|event| {
            matches!(event, MockEvent::Query(TxType::Schema, query) if query.contains(&format!("entity {RUN_ENTITY}")))
        }));
    }

    #[tokio::test]
    async fn adopted_partial_state_schema_fails_before_repair() {
        let (backend, log, labels) =
            MockMigrationBackend::with_partial_state_schema(&[RUN_ENTITY], true);
        let store =
            TypeDbStateStore::new(Arc::new(Database::with_backend(Box::new(backend), "test")));

        let error = store
            .load_applied()
            .await
            .expect_err("the sentinel must block partial-schema repair");
        assert!(error.to_string().contains(LEGACY_WRITER_CUTOVER_MESSAGE));
        assert!(!labels.lock().unwrap().contains(RUN_ENTITY));
        assert!(
            !log.lock()
                .unwrap()
                .iter()
                .any(|event| matches!(event, MockEvent::Query(TxType::Schema, _))),
            "adopted schema repair must not reach a define query"
        );
    }

    #[tokio::test]
    async fn exact_sentinel_partition_is_verified_before_filtering() {
        let fingerprint = "a".repeat(64);
        let sentinel = serde_json::json!({
            "app": LEGACY_CUTOVER_SENTINEL_APP_LABEL,
            "name": LEGACY_CUTOVER_SENTINEL_NAME,
            "applied": LEGACY_CUTOVER_SENTINEL_APPLIED_AT,
            "checksum": fingerprint,
        });
        let responses = vec![
            QueryResult::Documents(vec![sentinel]),
            QueryResult::Documents(vec![serde_json::json!({"exists": true})]),
            QueryResult::Documents(vec![serde_json::json!({"exists": true})]),
            QueryResult::Documents(vec![serde_json::json!({
                "app": LEGACY_CUTOVER_SENTINEL_APP_LABEL,
                "applied": LEGACY_CUTOVER_SENTINEL_APPLIED_AT,
                "checksum": fingerprint,
            })]),
        ];
        let (backend, _) = MockMigrationBackend::with_state_read_responses(responses);
        let database = Database::with_backend(Box::new(backend), "test");
        let mut transaction = database.read_transaction().await.unwrap();

        let partition = TypeDbStateStore::load_verified_legacy_partition_in_transaction(
            &mut transaction,
            LegacyCutoverSentinelExpectation::RequiredExact(&fingerprint),
        )
        .await
        .expect("verify exact sentinel");
        transaction.close().await.unwrap();

        assert!(partition.applied().is_empty());
        assert_eq!(partition.sentinel_fingerprint(), Some(fingerprint.as_str()));
    }

    #[tokio::test]
    async fn malformed_sentinel_timestamp_is_not_filtered() {
        let fingerprint = "b".repeat(64);
        let malformed_timestamp = "1970-01-01T00:00:00";
        let responses = vec![
            QueryResult::Documents(vec![serde_json::json!({
                "app": LEGACY_CUTOVER_SENTINEL_APP_LABEL,
                "name": LEGACY_CUTOVER_SENTINEL_NAME,
                "applied": malformed_timestamp,
                "checksum": fingerprint,
            })]),
            QueryResult::Documents(vec![serde_json::json!({"exists": true})]),
            QueryResult::Documents(vec![serde_json::json!({"exists": true})]),
            QueryResult::Documents(vec![serde_json::json!({
                "app": LEGACY_CUTOVER_SENTINEL_APP_LABEL,
                "applied": malformed_timestamp,
                "checksum": fingerprint,
            })]),
        ];
        let (backend, _) = MockMigrationBackend::with_state_read_responses(responses);
        let database = Database::with_backend(Box::new(backend), "test");
        let mut transaction = database.read_transaction().await.unwrap();

        let error = TypeDbStateStore::load_verified_legacy_partition_in_transaction(
            &mut transaction,
            LegacyCutoverSentinelExpectation::RequiredExact(&fingerprint),
        )
        .await
        .expect_err("malformed sentinel must fail closed");
        transaction.close().await.unwrap();

        assert!(error.is_contract_violation());
        assert!(error.to_string().contains("foreign applied timestamp"));
    }

    #[test]
    fn sentinel_name_is_outside_the_released_numbered_loader_namespace() {
        assert!(
            !LEGACY_CUTOVER_SENTINEL_NAME
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_digit)
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
    fn schema_bootstrap_rollback_failure_preserves_primary_and_cleanup() {
        let primary = OrmError::QueryExecution("define failed".to_owned());
        let cleanup = OrmError::Transaction("rollback failed".to_owned());
        let error = schema_rollback_cleanup_failure(&primary, &cleanup).to_string();
        assert!(error.contains("define failed"), "{error}");
        assert!(error.contains("rollback failed"), "{error}");
        assert!(error.contains("rollback was not acknowledged"), "{error}");
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
