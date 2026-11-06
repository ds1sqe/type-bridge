//! Read-only legacy ledger import; no executor or legacy writer lives here.
use super::legacy_ledger_plan as plans;
use super::legacy_schema::{labels::*, migration_state_schema};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use type_bridge_contract::reserved::{
    LEGACY_CUTOVER_SENTINEL_APP_LABEL, LEGACY_CUTOVER_SENTINEL_APPLIED_AT,
    LEGACY_CUTOVER_SENTINEL_MIGRATION_ID, LEGACY_CUTOVER_SENTINEL_NAME,
};
use type_bridge_core_lib::__internal as tql;
use type_bridge_orm::session::backend::QueryResult;
use type_bridge_orm::{OrmError, OwnedTransaction as Transaction};

pub(super) type Result<T> = std::result::Result<T, LegacyReadError>;
#[derive(Debug)]
pub(super) enum LegacyReadError {
    State { message: String },
}
impl std::fmt::Display for LegacyReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::State { message } => f.write_str(message),
        }
    }
}
impl std::error::Error for LegacyReadError {}
impl From<tql::TqlError> for LegacyReadError {
    fn from(error: tql::TqlError) -> Self {
        Self::State {
            message: error.to_string(),
        }
    }
}
fn map_orm_error(error: OrmError) -> LegacyReadError {
    LegacyReadError::State {
        message: error.to_string(),
    }
}
/// Applied migration record as loaded from migration state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppliedMigrationRecord {
    /// Application or migration package label.
    pub app_label: String,
    /// Migration file stem, such as `0001_initial`.
    pub name: String,
    /// Checksum recorded when this migration was applied.
    pub checksum: String,
    /// Optional application timestamp carried through the Python compatibility DTO.
    #[serde(default)]
    pub applied_at: Option<String>,
}

const LEGACY_STATE_SCHEMA_PROBE_QUERY_TAG: &str =
    "typebridge-internal-legacy-state-schema-probe/v1";
/// Required state of the V2-only legacy-ledger sentinel while a caller reads a
/// legacy applied projection.
#[derive(Debug, Clone, Copy)]
pub(crate) enum LegacyCutoverSentinelExpectation<'a> {
    /// No cutover sentinel may exist.
    Absent,
    /// One exact sentinel carrying this fingerprint must exist.
    RequiredExact(&'a str),
}

/// A verified legacy applied projection with the V2-only sentinel removed.
///
/// Construction is possible only after the sentinel candidate probes and all
/// stored sentinel fields have passed the requested expectation.  Callers must
/// never filter the reserved identity directly from an unverified row list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedLegacyAppliedPartition {
    applied: Vec<AppliedMigrationRecord>,
    sentinel_fingerprint: Option<String>,
}

impl VerifiedLegacyAppliedPartition {
    /// Borrow the released user migration records, excluding the verified
    /// V2-only sentinel partition.
    pub fn applied(&self) -> &[AppliedMigrationRecord] {
        &self.applied
    }

    /// Return the verified sentinel fingerprint.
    pub fn sentinel_fingerprint(&self) -> Option<&str> {
        self.sentinel_fingerprint.as_deref()
    }

    /// Consume the partition and return released user migration records.
    #[cfg(test)]
    pub fn into_applied(self) -> Vec<AppliedMigrationRecord> {
        self.applied
    }
}

/// Failure to observe and validate the reserved V2 cutover sentinel.
#[derive(Debug)]
pub(crate) enum LegacyCutoverSentinelError {
    /// The ledger could not be queried through the retained transaction.
    Storage(LegacyReadError),
    /// Stored rows violate the exact singleton contract.
    Contract {
        /// Stable human-readable contract failure.
        message: String,
    },
}

impl From<tql::TqlError> for LegacyCutoverSentinelError {
    fn from(error: tql::TqlError) -> Self {
        Self::Storage(error.into())
    }
}

impl LegacyCutoverSentinelError {
    /// Return whether this is durable stored drift rather than provider
    /// infrastructure failure.
    pub fn is_contract_violation(&self) -> bool {
        matches!(self, Self::Contract { .. })
    }
}

impl std::fmt::Display for LegacyCutoverSentinelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(error) => write!(
                f,
                "legacy cutover sentinel storage inspection failed: {error}"
            ),
            Self::Contract { message } => {
                write!(f, "legacy cutover sentinel contract violation: {message}")
            }
        }
    }
}
impl std::error::Error for LegacyCutoverSentinelError {}
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
    let documents = owned_state_documents(transaction, APPLIED_ENTITY)
        .await
        .map_err(LegacyCutoverSentinelError::Storage)?;
    verify_partition_documents(&documents, expectation)
}

fn verify_partition_documents(
    documents: &[serde_json::Value],
    expectation: LegacyCutoverSentinelExpectation<'_>,
) -> std::result::Result<VerifiedLegacyAppliedPartition, LegacyCutoverSentinelError> {
    let mut applied =
        parse_applied_documents(documents).map_err(LegacyCutoverSentinelError::Storage)?;
    let id_candidates = documents
        .iter()
        .filter(|document| {
            extract_value(document, "migration_id").as_deref()
                == Some(LEGACY_CUTOVER_SENTINEL_MIGRATION_ID)
        })
        .collect::<Vec<_>>();
    let name_candidates = documents
        .iter()
        .filter(|document| {
            extract_value(document, "name").as_deref() == Some(LEGACY_CUTOVER_SENTINEL_NAME)
        })
        .collect::<Vec<_>>();

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

    let details = id_candidates
        .iter()
        .copied()
        .filter(|document| {
            extract_value(document, "name").as_deref() == Some(LEGACY_CUTOVER_SENTINEL_NAME)
        })
        .collect::<Vec<_>>();
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
    if type_bridge_schema::parse_provider_datetime(&applied_at).ok()
        != type_bridge_schema::parse_provider_datetime(LEGACY_CUTOVER_SENTINEL_APPLIED_AT).ok()
    {
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
        LegacyCutoverSentinelExpectation::RequiredExact(expected) => expected,
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
        // Schema-only read, excepted like define/undefine (06.8).
        let result = transaction
            .query(&schema_kind_query(kind, true)?)
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

fn schema_labels_from_result(result: QueryResult, operation: &str) -> Result<BTreeSet<String>> {
    let values = match result {
        QueryResult::Documents(values) | QueryResult::Rows(values) => values,
        QueryResult::Ok => {
            return Err(LegacyReadError::State {
                message: format!("{operation} returned no document result"),
            });
        }
    };
    let mut labels = BTreeSet::new();
    for value in &values {
        let label = extract_value(value, "label").ok_or_else(|| LegacyReadError::State {
            message: format!("{operation} returned a malformed schema label"),
        })?;
        labels.insert(label);
    }
    Ok(labels)
}

fn schema_kind_query(kind: &str, tagged: bool) -> std::result::Result<String, tql::TqlError> {
    let query = tql::pipeline(vec![
        tql::match_stage(vec![tql::schema_kind_pattern("$t", kind)?]),
        tql::fetch(vec![(
            "label",
            tql::fetch_expr(tql::function("label", vec![tql::var("t")?])?),
        )])?,
    ]);
    if tagged {
        tql::render_commented(&query, LEGACY_STATE_SCHEMA_PROBE_QUERY_TAG)
    } else {
        tql::render(&query)
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
/// `state.py:179-191`). Missing `app` / `name` / `checksum` fields are refused
/// so a damaged row cannot disappear from import evidence. `applied` remains
/// part of the record as an optional ISO string.
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
            return Err(LegacyReadError::State {
                message: "legacy applied row is missing app, name or checksum".into(),
            });
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

fn state_documents(
    answers: Vec<type_bridge_orm::session::staged::PlanAnswer>,
) -> Result<Vec<serde_json::Value>> {
    use type_bridge_orm::session::staged::PlanAnswer;
    let mut documents = Vec::new();
    for answer in answers {
        let PlanAnswer::Documents(rows) = answer else {
            return Err(LegacyReadError::State {
                message: "migration state plan returned no documents".into(),
            });
        };
        for row in rows {
            let fields = row
                .get("attributes")
                .and_then(serde_json::Value::as_object)
                .ok_or_else(|| LegacyReadError::State {
                    message: "migration state document has no attributes".into(),
                })?;
            let aliases = [
                (MIGRATION_ID, "migration_id"),
                (APP_LABEL, "app"),
                (NAME, "name"),
                (APPLIED_AT, "applied"),
                (CHECKSUM, "checksum"),
            ];
            let mut document = serde_json::Map::new();
            for (field, alias) in aliases {
                let Some(value) = fields.get(field) else {
                    continue;
                };
                let value = if field == APPLIED_AT && !value.is_null() {
                    // Released driver fetches expose exactly nine fractional digits.
                    // Preserve that host format after native canonical verification.
                    let datetime = type_bridge_schema::parse_provider_datetime(
                        value.as_str().ok_or_else(|| LegacyReadError::State {
                            message: "migration timestamp document is malformed".into(),
                        })?,
                    )
                    .map_err(|error| LegacyReadError::State {
                        message: error.to_string(),
                    })?;
                    let (hour, minute, second, nanosecond) = datetime.time().components();
                    serde_json::Value::String(format!(
                        "{}T{hour:02}:{minute:02}:{second:02}.{nanosecond:09}",
                        datetime.date()
                    ))
                } else {
                    value.clone()
                };
                document.insert(alias.to_owned(), value);
            }
            documents.push(serde_json::Value::Object(document));
        }
    }
    Ok(documents)
}

async fn owned_state_documents(
    transaction: &mut Transaction,
    kind: &str,
) -> Result<Vec<serde_json::Value>> {
    let plan = plans::read(kind, &[])?;
    let limits = type_bridge_orm::QueryExecutionResourceLimits::default();
    let answers = transaction
        .execute_plans(
            &[plan],
            &limits,
            &type_bridge_orm::AnswerCancellation::default(),
            type_bridge_orm::QueryExecutionDeadline::for_limits(limits),
        )
        .await
        .map_err(map_orm_error)?;
    state_documents(answers)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sentinel() -> serde_json::Value {
        serde_json::json!({"migration_id": LEGACY_CUTOVER_SENTINEL_MIGRATION_ID,
            "app": LEGACY_CUTOVER_SENTINEL_APP_LABEL, "name": LEGACY_CUTOVER_SENTINEL_NAME,
            "applied": LEGACY_CUTOVER_SENTINEL_APPLIED_AT, "checksum": "a".repeat(64)})
    }
    #[test]
    fn verifies_exact_sentinel_before_removing_it_from_applied_set() {
        let user = serde_json::json!({"app":"app", "name":"0001", "checksum":"abc"});
        let records = [sentinel(), user];
        let fingerprint = "a".repeat(64);
        let partition = verify_partition_documents(
            &records,
            LegacyCutoverSentinelExpectation::RequiredExact(&fingerprint),
        )
        .unwrap();
        assert_eq!(partition.applied().len(), 1);
        assert_eq!(partition.applied()[0].name, "0001");
        assert_eq!(partition.sentinel_fingerprint(), Some(fingerprint.as_str()));
        assert!(
            verify_partition_documents(&records, LegacyCutoverSentinelExpectation::Absent)
                .unwrap_err()
                .is_contract_violation()
        );
    }
    #[test]
    fn refuses_missing_duplicate_split_and_foreign_sentinels() {
        let fingerprint = "a".repeat(64);
        let expected = LegacyCutoverSentinelExpectation::RequiredExact(&fingerprint);
        for rows in [vec![], vec![sentinel(), sentinel()]] {
            assert!(
                verify_partition_documents(&rows, expected)
                    .unwrap_err()
                    .is_contract_violation()
            );
        }
        for field in ["migration_id", "app", "name", "applied", "checksum"] {
            let mut changed = sentinel();
            changed[field] = serde_json::json!("foreign");
            assert!(
                verify_partition_documents(&[changed], expected)
                    .unwrap_err()
                    .is_contract_violation(),
                "{field}"
            );
        }
        let mut id_only = sentinel();
        id_only["name"] = serde_json::json!("other");
        let mut name_only = sentinel();
        name_only["migration_id"] = serde_json::json!("other");
        assert!(
            verify_partition_documents(&[id_only, name_only], expected)
                .unwrap_err()
                .is_contract_violation()
        );
    }
    #[test]
    fn accepts_equivalent_timestamp_but_refuses_malformed_timestamp() {
        let fingerprint = "a".repeat(64);
        let expected = LegacyCutoverSentinelExpectation::RequiredExact(&fingerprint);
        let mut row = sentinel();
        row["applied"] = serde_json::json!("1970-01-01T00:00:00.000000");
        assert!(verify_partition_documents(&[row.clone()], expected).is_ok());
        row["applied"] = serde_json::json!("not-a-datetime");
        assert!(
            verify_partition_documents(&[row], expected)
                .unwrap_err()
                .is_contract_violation()
        );
    }
}

#[cfg(test)]
mod parser_tests {
    use super::*;

    #[test]
    fn plain_and_wrapped_rows_preserve_order_and_optional_timestamp() {
        let rows = parse_applied_documents(&[
            serde_json::json!({"app":"app", "name":"first", "checksum":"abc"}),
            serde_json::json!({"app":{"value":"app"}, "name":{"value":"second"}, "checksum":{"value":"def"}, "applied":{"value":"2026-01-01T00:00:00"}}),
        ]).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "first");
        assert_eq!(rows[0].applied_at, None);
        assert_eq!(rows[1].checksum, "def");
        assert_eq!(rows[1].applied_at.as_deref(), Some("2026-01-01T00:00:00"));
    }

    #[test]
    fn incomplete_rows_cannot_disappear_from_import_evidence() {
        for field in ["app", "name", "checksum"] {
            for bad in [
                serde_json::Value::Null,
                serde_json::json!([]),
                serde_json::json!({}),
            ] {
                let mut row = serde_json::json!({"app":"app", "name":"first", "checksum":"abc"});
                row[field] = bad;
                assert!(parse_applied_documents(&[row.clone()]).is_err());
                assert!(
                    verify_partition_documents(&[row], LegacyCutoverSentinelExpectation::Absent)
                        .is_err()
                );
            }
        }
    }
}
