//! Permanent V1-writer fence shared by TypeBridge-owned compatibility facades.
//!
//! A valid cutover is one exact managed-control row plus an atomic pair: one
//! managed-side V2 anchor and one row in the frozen V1 applied ledger. The
//! control and anchor scopes, and the anchor and sentinel fingerprints, must
//! match. Released applications had an open label namespace, so marker-like
//! names remain ordinary V1 state unless both the complete frozen control and
//! released-ledger schemas are present.

use type_bridge_contract::limits::MAX_CANONICAL_STRING_BYTES;
use type_bridge_contract::reserved::{
    LEGACY_CUTOVER_ANCHOR_ENTITY, LEGACY_CUTOVER_ANCHOR_FINGERPRINT, LEGACY_CUTOVER_ANCHOR_KEY,
    LEGACY_CUTOVER_ANCHOR_SCOPE, LEGACY_CUTOVER_ANCHOR_SINGLETON_KEY,
    LEGACY_CUTOVER_SENTINEL_APP_LABEL, LEGACY_CUTOVER_SENTINEL_APPLIED_AT,
    LEGACY_CUTOVER_SENTINEL_MIGRATION_ID, LEGACY_CUTOVER_SENTINEL_NAME, LEGACY_LEDGER_APP_LABEL,
    LEGACY_LEDGER_APPLIED_AT, LEGACY_LEDGER_APPLIED_ENTITY, LEGACY_LEDGER_CHECKSUM,
    LEGACY_LEDGER_MIGRATION_ID, LEGACY_LEDGER_NAME, LEGACY_WRITER_CUTOVER_MESSAGE,
    LEGACY_WRITER_GUARD_QUERY_TAG, MANAGED_CONTROL_ENTITY, MANAGED_CONTROL_LEASE_FENCE,
    MANAGED_CONTROL_LEASE_FREE, MANAGED_CONTROL_LEASE_HELD, MANAGED_CONTROL_LEASE_HOLDER,
    MANAGED_CONTROL_LEASE_STATE, MANAGED_CONTROL_SCOPE,
};
use type_bridge_schema_compat::{
    LiveLegacyLedgerPresence, LiveQueryControlPresence, legacy_ledger_schema_presence,
    managed_fence_schema_presence,
};

use super::backend::{QueryResult, TxType};
use super::{Database, TransactionContext};
use crate::error::{OrmError, Result};

/// Reject a TypeBridge-owned V1 writer through its already-open mutation
/// transaction after the database completes V2 adoption.
///
/// The complete anchor/sentinel pair is inspected in the same transaction that
/// will carry the mutation.  A malformed or partially stored pair fails closed;
/// marker-like collisions without both exact schema contracts remain ordinary
/// V1 application state and are ignored.
pub async fn require_legacy_writer_open_in_transaction(
    transaction: &TransactionContext,
) -> Result<()> {
    // The schema export is useful only while this mutation transaction keeps
    // TypeDB's mutually exclusive WRITE/SCHEMA guard. Missing custom-backend
    // support and malformed lookalikes do not prove that an open V1 namespace
    // belongs to TypeBridge. A provider that claims this seam but cannot
    // export must fail closed, as must every error after exact authority.
    let schema_export = match transaction.schema_snapshot().await {
        Ok(Some(schema_export)) => schema_export,
        Ok(None) => return Ok(()),
        Err(error) => return Err(error),
    };
    let managed_schema_has_extensions = match managed_fence_schema_presence(&schema_export) {
        Ok(LiveQueryControlPresence::ManagedFence) => false,
        Ok(LiveQueryControlPresence::ManagedFenceWithExtensions) => true,
        Ok(LiveQueryControlPresence::Absent) => return Ok(()),
        Err(error) if error.code().as_str() == "migration_typedb_control_schema_mismatch" => {
            // Released applications owned the entire label namespace. A
            // well-formed but non-canonical set of marker-like declarations is
            // therefore a determinate V1 collision, not managed authority.
            return Ok(());
        }
        Err(_) => {
            return Err(integrity_error(
                "the live schema could not establish absence or exact presence of the managed fence",
            ));
        }
    };

    let control_existence = query_values(
        transaction,
        guard_query(format!(
            "match $control isa {MANAGED_CONTROL_ENTITY}; fetch {{ \"exists\": true }};"
        )),
        "legacy writer managed-control existence probe",
    )
    .await?;
    let control_details = query_values(
        transaction,
        guard_query(format!(
            "match $control isa {MANAGED_CONTROL_ENTITY}, has {MANAGED_CONTROL_SCOPE} $scope, has {MANAGED_CONTROL_LEASE_FENCE} $fence, has {MANAGED_CONTROL_LEASE_STATE} $state; fetch {{ \"scope\": $scope, \"fence\": $fence, \"state\": $state }};"
        )),
        "legacy writer managed-control detail probe",
    )
    .await?;
    let control_holders = query_values(
        transaction,
        guard_query(format!(
            "match $control isa {MANAGED_CONTROL_ENTITY}, has {MANAGED_CONTROL_LEASE_HOLDER} $holder; fetch {{ \"holder\": $holder }};"
        )),
        "legacy writer managed-control holder probe",
    )
    .await?;
    let control = parse_control(&control_existence, &control_details, &control_holders)?;

    let anchor_existence = query_values(
        transaction,
        guard_query(format!(
            "match $anchor isa {LEGACY_CUTOVER_ANCHOR_ENTITY}; fetch {{ \"exists\": true }};"
        )),
        "legacy writer cutover-anchor existence probe",
    )
    .await?;
    let anchor_key_candidates = query_values(
        transaction,
        guard_query(format!(
            "match $anchor isa {LEGACY_CUTOVER_ANCHOR_ENTITY}, has {LEGACY_CUTOVER_ANCHOR_KEY} \"{LEGACY_CUTOVER_ANCHOR_SINGLETON_KEY}\"; fetch {{ \"exists\": true }};"
        )),
        "legacy writer cutover-anchor key probe",
    )
    .await?;
    let anchor_details = query_values(
        transaction,
        guard_query(format!(
            "match $anchor isa {LEGACY_CUTOVER_ANCHOR_ENTITY}, has {LEGACY_CUTOVER_ANCHOR_KEY} \"{LEGACY_CUTOVER_ANCHOR_SINGLETON_KEY}\", has {LEGACY_CUTOVER_ANCHOR_SCOPE} $scope, has {LEGACY_CUTOVER_ANCHOR_FINGERPRINT} $fingerprint; fetch {{ \"scope\": $scope, \"fingerprint\": $fingerprint }};"
        )),
        "legacy writer cutover-anchor detail probe",
    )
    .await?;
    let anchor = parse_anchor(&anchor_existence, &anchor_key_candidates, &anchor_details)?;

    if managed_schema_has_extensions {
        if control.is_none() && anchor.is_none() {
            return Ok(());
        }
        return Err(integrity_error(
            "a managed-control row or cutover anchor exists while the managed fence schema carries released-only extensions",
        ));
    }

    let legacy_schema_exact = matches!(
        legacy_ledger_schema_presence(&schema_export),
        Ok(LiveLegacyLedgerPresence::FrozenLedger)
    );
    if !legacy_schema_exact {
        if control.is_none() && anchor.is_none() {
            return Ok(());
        }
        return Err(integrity_error(
            "a managed-control row or cutover anchor exists without the exact frozen V1 ledger schema",
        ));
    }

    let sentinel_id_candidates = query_values(
        transaction,
        guard_query(format!(
            "match $m isa {LEGACY_LEDGER_APPLIED_ENTITY}, has {LEGACY_LEDGER_MIGRATION_ID} \"{LEGACY_CUTOVER_SENTINEL_MIGRATION_ID}\"; fetch {{ \"exists\": true }};"
        )),
        "legacy writer cutover-sentinel ID probe",
    )
    .await?;
    let sentinel_name_candidates = query_values(
        transaction,
        guard_query(format!(
            "match $m isa {LEGACY_LEDGER_APPLIED_ENTITY}, has {LEGACY_LEDGER_NAME} \"{LEGACY_CUTOVER_SENTINEL_NAME}\"; fetch {{ \"exists\": true }};"
        )),
        "legacy writer cutover-sentinel name probe",
    )
    .await?;
    let sentinel_details = query_values(
        transaction,
        guard_query(format!(
            "match $m isa {LEGACY_LEDGER_APPLIED_ENTITY}, has {LEGACY_LEDGER_MIGRATION_ID} \"{LEGACY_CUTOVER_SENTINEL_MIGRATION_ID}\", has {LEGACY_LEDGER_APP_LABEL} $app, has {LEGACY_LEDGER_NAME} \"{LEGACY_CUTOVER_SENTINEL_NAME}\", has {LEGACY_LEDGER_APPLIED_AT} $applied, has {LEGACY_LEDGER_CHECKSUM} $checksum; fetch {{ \"app\": $app, \"applied\": $applied, \"checksum\": $checksum }};"
        )),
        "legacy writer cutover-sentinel detail probe",
    )
    .await?;
    let sentinel = parse_sentinel(
        &sentinel_id_candidates,
        &sentinel_name_candidates,
        &sentinel_details,
    )?;

    decide_cutover_state(control, anchor, sentinel)
}

fn decide_cutover_state(
    control: Option<ManagedControl>,
    anchor: Option<CutoverAnchor>,
    sentinel: Option<CutoverSentinel>,
) -> Result<()> {
    match (control, anchor, sentinel) {
        (None, None, None) | (Some(_), None, None) => Ok(()),
        (None, _, _) => Err(integrity_error(
            "a cutover anchor or sentinel exists without a managed-control row",
        )),
        (Some(_), None, Some(_)) => Err(integrity_error(
            "the V1 cutover sentinel exists without a managed cutover anchor",
        )),
        (Some(_), Some(_), None) => Err(integrity_error(
            "the managed cutover anchor exists without a V1 sentinel",
        )),
        (Some(control), Some(anchor), Some(sentinel)) => {
            if anchor.scope != control.scope {
                return Err(integrity_error(
                    "the managed cutover anchor scope differs from the managed-control scope",
                ));
            }
            if sentinel.fingerprint != anchor.fingerprint {
                return Err(integrity_error(
                    "the V1 cutover sentinel does not match the managed cutover anchor",
                ));
            }

            Err(OrmError::Transaction(
                LEGACY_WRITER_CUTOVER_MESSAGE.to_owned(),
            ))
        }
    }
}

#[derive(Debug)]
struct ManagedControl {
    scope: String,
}

#[derive(Debug)]
struct CutoverAnchor {
    scope: String,
    fingerprint: String,
}

#[derive(Debug)]
struct CutoverSentinel {
    fingerprint: String,
}

fn parse_control(
    existence: &[serde_json::Value],
    details: &[serde_json::Value],
    holders: &[serde_json::Value],
) -> Result<Option<ManagedControl>> {
    if existence.is_empty() && details.is_empty() && holders.is_empty() {
        return Ok(None);
    }
    if existence.len() != 1 || details.len() != 1 {
        return Err(integrity_error(
            "the managed-control row is duplicated or missing required fields",
        ));
    }
    let detail = &details[0];
    let scope = field(detail, "scope")
        .ok_or_else(|| integrity_error("the managed-control scope is malformed"))?;
    let fence = field(detail, "fence")
        .ok_or_else(|| integrity_error("the managed-control fence is malformed"))?;
    let state = field(detail, "state")
        .ok_or_else(|| integrity_error("the managed-control state is malformed"))?;
    if !is_managed_scope(&scope) {
        return Err(integrity_error(
            "the managed-control row carries a malformed scope",
        ));
    }
    if !is_canonical_nonzero_u64(&fence) {
        return Err(integrity_error(
            "the managed-control fence is not a canonical nonzero u64",
        ));
    }
    match state.as_str() {
        MANAGED_CONTROL_LEASE_HELD => {
            if holders.len() != 1
                || field(&holders[0], "holder").is_none_or(|holder| !is_lease_holder(&holder))
            {
                return Err(integrity_error(
                    "the held managed-control row has no one canonical lease holder",
                ));
            }
        }
        MANAGED_CONTROL_LEASE_FREE => {
            if !holders.is_empty() {
                return Err(integrity_error(
                    "the free managed-control row unexpectedly carries a lease holder",
                ));
            }
        }
        _ => {
            return Err(integrity_error(
                "the managed-control row carries an unknown lease state",
            ));
        }
    }
    Ok(Some(ManagedControl { scope }))
}

fn parse_anchor(
    existence: &[serde_json::Value],
    key_candidates: &[serde_json::Value],
    details: &[serde_json::Value],
) -> Result<Option<CutoverAnchor>> {
    if existence.is_empty() && key_candidates.is_empty() && details.is_empty() {
        return Ok(None);
    }
    if existence.len() != 1 || key_candidates.len() != 1 || details.len() != 1 {
        return Err(integrity_error(
            "the managed cutover anchor is duplicated or missing required exact fields",
        ));
    }
    let anchor = &details[0];
    let scope = field(anchor, "scope")
        .ok_or_else(|| integrity_error("the managed cutover anchor scope is malformed"))?;
    let fingerprint = field(anchor, "fingerprint")
        .ok_or_else(|| integrity_error("the managed cutover anchor fingerprint is malformed"))?;
    if !is_managed_scope(&scope) {
        return Err(integrity_error(
            "the managed cutover anchor carries a malformed scope",
        ));
    }
    if !is_lower_hex_fingerprint(&fingerprint) {
        return Err(integrity_error(
            "the managed cutover anchor fingerprint is not lowercase 64-hex",
        ));
    }
    Ok(Some(CutoverAnchor { scope, fingerprint }))
}

fn parse_sentinel(
    id_candidates: &[serde_json::Value],
    name_candidates: &[serde_json::Value],
    details: &[serde_json::Value],
) -> Result<Option<CutoverSentinel>> {
    if id_candidates.is_empty() && name_candidates.is_empty() && details.is_empty() {
        return Ok(None);
    }
    if id_candidates.len() != 1 || name_candidates.len() != 1 || details.len() != 1 {
        return Err(integrity_error(
            "the V1 cutover sentinel is duplicated, split, or missing required exact fields",
        ));
    }
    let sentinel = &details[0];
    let app = field(sentinel, "app")
        .ok_or_else(|| integrity_error("the V1 cutover sentinel app label is malformed"))?;
    let applied = field(sentinel, "applied")
        .ok_or_else(|| integrity_error("the V1 cutover sentinel timestamp is malformed"))?;
    let fingerprint = field(sentinel, "checksum")
        .ok_or_else(|| integrity_error("the V1 cutover sentinel checksum is malformed"))?;
    if app != LEGACY_CUTOVER_SENTINEL_APP_LABEL
        || applied != LEGACY_CUTOVER_SENTINEL_APPLIED_AT
        || !is_lower_hex_fingerprint(&fingerprint)
    {
        return Err(integrity_error(
            "the V1 cutover sentinel carries malformed frozen fields",
        ));
    }
    Ok(Some(CutoverSentinel { fingerprint }))
}

/// Read-only preflight for TypeBridge-owned V1 writer surfaces whose first
/// mutation cannot share a TypeDB transaction (for example database deletion).
///
/// Mutation paths that can share a transaction must repeat the guard inside
/// that WRITE or SCHEMA transaction so this compatibility preflight does not
/// become a time-of-check/time-of-use authority.
pub async fn require_legacy_writer_open(database: &Database) -> Result<()> {
    let transaction = database.transaction_context(TxType::Read).await?;
    let checked = require_legacy_writer_open_in_transaction(&transaction).await;
    let closed = transaction.close().await;
    match (checked, closed) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(()), Err(cleanup)) => Err(cleanup),
        (Err(primary), Err(_)) => Err(primary),
    }
}

async fn query_values(
    transaction: &TransactionContext,
    query: String,
    operation: &str,
) -> Result<Vec<serde_json::Value>> {
    match transaction.query(&query).await? {
        QueryResult::Documents(values) | QueryResult::Rows(values) => Ok(values),
        QueryResult::Ok => Err(integrity_error(format!(
            "{operation} returned no document result"
        ))),
    }
}

fn guard_query(query: impl AsRef<str>) -> String {
    format!("{LEGACY_WRITER_GUARD_QUERY_TAG}{}", query.as_ref())
}

fn field(document: &serde_json::Value, key: &str) -> Option<String> {
    string_scalar(document.get(key)?)
}

fn string_scalar(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(object) => object.get("value").and_then(string_scalar),
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::Array(_) => None,
    }
}

fn is_lower_hex_fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_canonical_nonzero_u64(value: &str) -> bool {
    value
        .parse::<u64>()
        .is_ok_and(|parsed| parsed != 0 && parsed.to_string() == value)
}

fn is_managed_scope(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_CANONICAL_STRING_BYTES
}

fn is_lease_holder(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn integrity_error(message: impl AsRef<str>) -> OrmError {
    OrmError::Transaction(format!(
        "legacy migration cutover state is inconsistent: {}",
        message.as_ref()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FINGERPRINT: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    fn control(scope: &str) -> ManagedControl {
        ManagedControl {
            scope: scope.to_owned(),
        }
    }

    fn anchor(scope: &str, fingerprint: &str) -> CutoverAnchor {
        CutoverAnchor {
            scope: scope.to_owned(),
            fingerprint: fingerprint.to_owned(),
        }
    }

    fn sentinel(fingerprint: &str) -> CutoverSentinel {
        CutoverSentinel {
            fingerprint: fingerprint.to_owned(),
        }
    }

    #[test]
    fn pre_adoption_states_remain_open() {
        decide_cutover_state(None, None, None).expect("schema-only bootstrap stays open");
        decide_cutover_state(Some(control("scope")), None, None)
            .expect("managed control before adoption stays open");
    }

    #[test]
    fn exact_bound_pair_returns_only_the_canonical_closed_error() {
        let error = decide_cutover_state(
            Some(control("scope")),
            Some(anchor("scope", FINGERPRINT)),
            Some(sentinel(FINGERPRINT)),
        )
        .expect_err("an exact adopted scope is closed");
        assert_eq!(
            error.to_string(),
            format!("Transaction error: {LEGACY_WRITER_CUTOVER_MESSAGE}")
        );
    }

    #[test]
    fn every_incomplete_or_mismatched_authority_combination_is_integrity() {
        let other = "1111111111111111111111111111111111111111111111111111111111111111";
        for result in [
            decide_cutover_state(None, Some(anchor("scope", FINGERPRINT)), None),
            decide_cutover_state(None, None, Some(sentinel(FINGERPRINT))),
            decide_cutover_state(
                None,
                Some(anchor("scope", FINGERPRINT)),
                Some(sentinel(FINGERPRINT)),
            ),
            decide_cutover_state(
                Some(control("scope")),
                Some(anchor("scope", FINGERPRINT)),
                None,
            ),
            decide_cutover_state(Some(control("scope")), None, Some(sentinel(FINGERPRINT))),
            decide_cutover_state(
                Some(control("scope")),
                Some(anchor("other", FINGERPRINT)),
                Some(sentinel(FINGERPRINT)),
            ),
            decide_cutover_state(
                Some(control("scope")),
                Some(anchor("scope", FINGERPRINT)),
                Some(sentinel(other)),
            ),
        ] {
            let error = result.expect_err("incomplete authority must fail closed");
            assert!(error.to_string().contains("cutover state is inconsistent"));
            assert!(!error.to_string().contains(LEGACY_WRITER_CUTOVER_MESSAGE));
        }
    }

    #[test]
    fn partial_and_duplicate_rows_are_rejected() {
        let exists = vec![serde_json::json!({"exists": true})];
        let duplicate = vec![
            serde_json::json!({"exists": true}),
            serde_json::json!({"exists": true}),
        ];
        let control_detail = vec![serde_json::json!({
            "scope": "scope",
            "fence": "1",
            "state": MANAGED_CONTROL_LEASE_FREE,
        })];
        assert!(parse_control(&exists, &[], &[]).is_err());
        assert!(parse_control(&duplicate, &control_detail, &[]).is_err());

        let anchor_detail = vec![serde_json::json!({
            "scope": "scope",
            "fingerprint": FINGERPRINT,
        })];
        assert!(parse_anchor(&exists, &[], &anchor_detail).is_err());
        assert!(parse_anchor(&duplicate, &exists, &anchor_detail).is_err());

        let sentinel_detail = vec![serde_json::json!({
            "app": LEGACY_CUTOVER_SENTINEL_APP_LABEL,
            "applied": LEGACY_CUTOVER_SENTINEL_APPLIED_AT,
            "checksum": FINGERPRINT,
        })];
        assert!(parse_sentinel(&exists, &[], &sentinel_detail).is_err());
        assert!(parse_sentinel(&duplicate, &exists, &sentinel_detail).is_err());
    }

    #[test]
    fn control_row_requires_canonical_state_fence_and_holder() {
        let exists = vec![serde_json::json!({"exists": true})];
        let detail = |fence: &str, state: &str| {
            vec![serde_json::json!({
                "scope": "scope",
                "fence": fence,
                "state": state,
            })]
        };
        let holder = vec![serde_json::json!({"holder": "worker-1"})];

        assert!(parse_control(&exists, &detail("1", MANAGED_CONTROL_LEASE_FREE), &[]).is_ok());
        assert!(parse_control(&exists, &detail("1", MANAGED_CONTROL_LEASE_HELD), &holder,).is_ok());
        assert!(parse_control(&exists, &detail("01", MANAGED_CONTROL_LEASE_FREE), &[]).is_err());
        assert!(parse_control(&exists, &detail("0", MANAGED_CONTROL_LEASE_FREE), &[]).is_err());
        assert!(parse_control(&exists, &detail("1", "foreign"), &[]).is_err());
        assert!(
            parse_control(
                &exists,
                &[serde_json::json!({
                    "scope": "scope",
                    "fence": 1,
                    "state": MANAGED_CONTROL_LEASE_FREE,
                })],
                &[],
            )
            .is_err()
        );
        assert!(parse_control(&exists, &detail("1", MANAGED_CONTROL_LEASE_HELD), &[]).is_err());
        assert!(
            parse_control(&exists, &detail("1", MANAGED_CONTROL_LEASE_FREE), &holder,).is_err()
        );
    }
}
