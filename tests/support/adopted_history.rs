//! Persist completed adoption state without invoking the retired importer.

use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use type_bridge_contract::reserved::{
    IMPORT_LEGACY_JOURNAL_CONTROL_SCHEMA_TYPEQL as JOURNAL_CONTROL_SCHEMA_TYPEQL,
    IMPORT_LEGACY_MANAGED_FENCE_SCHEMA_TYPEQL as MANAGED_FENCE_SCHEMA_TYPEQL,
};
use type_bridge_contract::{
    codec::to_canonical_json,
    fingerprint::{CanonicalizationVersion, Fingerprint, FingerprintDomain},
    migration::{MigrationAppLabel, MigrationId, MigrationName},
    schema::DocumentId,
};
use type_bridge_orm::{Database, TxType};
use type_bridge_schema::ManagedDeltaContext;
use type_bridge_schema_compat::{LEGACY_LEDGER_SCHEMA_TYPEQL, parse_adopted_genesis_authority};
use type_bridge_schema_migration::{
    AppliedRecord, ExecutionFence, ExecutionScope, LeaseHolderId, LegacyAppliedSetDigest,
    LegacyMigrationChecksum, LegacyMigrationReference, MigrationLease,
    VerifiedSchemaMigrationManifest, build_legacy_frontier_bridge, verified_manifest_digest,
};

pub fn bridge(context: &ManagedDeltaContext, source: &str) -> VerifiedSchemaMigrationManifest {
    let authority =
        parse_adopted_genesis_authority(DocumentId::new("adopted.typeql").unwrap(), source)
            .unwrap();
    let id = |name| {
        MigrationId::from_components(
            MigrationAppLabel::new("example").unwrap(),
            MigrationName::new(name).unwrap(),
        )
    };
    let frontier = vec![LegacyMigrationReference::new(
        id("0001_initial"),
        LegacyMigrationChecksum::new("11".repeat(8)).unwrap(),
    )];
    build_legacy_frontier_bridge(
        MigrationId::from_components(
            MigrationAppLabel::new("smoke").unwrap(),
            MigrationName::new("0000_legacy_frontier").unwrap(),
        ),
        frontier.clone(),
        LegacyAppliedSetDigest::compute(frontier).unwrap(),
        authority.declared(),
        context,
    )
    .unwrap()
}

async fn execute(database: &Database, query: &str, kind: TxType) {
    database
        .execute_raw(query, kind)
        .await
        .unwrap_or_else(|error| panic!("fixture query {query}: {error}"));
}

pub async fn seed(
    managed: &Arc<Database>,
    _journal: &Arc<Database>,
    context: &ManagedDeltaContext,
    bridge: &VerifiedSchemaMigrationManifest,
    source: &str,
) {
    let journal = managed.derived_legacy_journal_database();
    journal.create_database().await.unwrap();
    let journal = &journal;
    let scope = context.scope_id().as_str();
    let authority =
        parse_adopted_genesis_authority(DocumentId::new("adopted.typeql").unwrap(), source)
            .unwrap();
    let applied_set = bridge.legacy_applied_set().unwrap();
    let preimage = to_canonical_json(&json!({
        "applied_set": {"algorithm": applied_set.algorithm(), "canonicalization": applied_set.canonicalization(), "digest": applied_set.as_str()},
        "bridge_manifest_sha256": verified_manifest_digest(bridge).unwrap().to_hex(),
        "journal_database": journal.database_name(), "legacy_identity": authority.legacy_identity(),
        "managed_database": managed.database_name(), "managed_scope": scope,
    })).unwrap();
    let anchor = Fingerprint::compute(
        FingerprintDomain::new("typebridge.migration.legacy-cutover-anchor").unwrap(),
        CanonicalizationVersion::new("typebridge.legacy-cutover-anchor/v1").unwrap(),
        None,
        &preimage,
    )
    .digest()
    .to_hex();
    for schema in [
        source,
        LEGACY_LEDGER_SCHEMA_TYPEQL,
        MANAGED_FENCE_SCHEMA_TYPEQL,
    ] {
        execute(managed, schema, TxType::Schema).await;
    }
    execute(journal, JOURNAL_CONTROL_SCHEMA_TYPEQL, TxType::Schema).await;
    let lease = MigrationLease::new(
        ExecutionScope::new(context.scope_id().clone()),
        LeaseHolderId::new("completed-adoption").unwrap(),
        ExecutionFence::new(1).unwrap(),
    );
    let record = AppliedRecord::from_verified_manifest_contract(&lease, bridge).unwrap();
    let payload = to_canonical_json(&json!({
        "fence": 1, "format": "typebridge.migration-execution-record/v1", "kind": "applied",
        "manifest_digest": record.manifest_digest().to_hex(), "migration_id": record.migration_id(), "scope": scope,
        "source_declared": record.source_declared().as_fingerprint(), "source_semantics": record.source_semantics().as_fingerprint(),
        "target_declared": record.target_declared().as_fingerprint(), "target_semantics": record.target_semantics().as_fingerprint(),
    })).unwrap();
    let digest = format!("{:x}", Sha256::digest(&payload));
    let literal = |value: &str| serde_json::to_string(value).unwrap();
    let control = format!(
        "insert $control isa typebridge-internal-v2-migration-control, has typebridge-internal-v2-control-scope {}, has typebridge-internal-v2-lease-fence \"1\", has typebridge-internal-v2-lease-state \"free\"",
        literal(scope)
    );
    execute(managed, &format!("{control};"), TxType::Write).await;
    execute(managed, &format!("insert $legacy isa type_bridge_migration, has migration_id \"example:0001_initial\", has migration_app_label \"example\", has migration_name \"0001_initial\", has migration_applied_at 1970-01-01T00:00:00.000000000, has migration_checksum \"{}\";", "11".repeat(8)), TxType::Write).await;
    execute(managed, &format!("insert $anchor isa typebridge-internal-v2-legacy-cutover, has typebridge-internal-v2-legacy-cutover-key \"typebridge-legacy-cutover-anchor/v1\", has typebridge-internal-v2-legacy-cutover-scope {}, has typebridge-internal-v2-legacy-cutover-fingerprint \"{anchor}\";", literal(scope)), TxType::Write).await;
    execute(managed, &format!("insert $sentinel isa type_bridge_migration, has migration_id \"type_bridge_v2_internal:__legacy_writer_cutover__\", has migration_app_label \"type_bridge_v2_internal\", has migration_name \"__legacy_writer_cutover__\", has migration_applied_at 1970-01-01T00:00:00.000000000, has migration_checksum \"{anchor}\";"), TxType::Write).await;
    execute(
        journal,
        &format!("{control}, has typebridge-internal-v2-next-sequence \"1\";"),
        TxType::Write,
    )
    .await;
    execute(journal, &format!("insert $owner isa typebridge-internal-v2-journal-owner, has typebridge-internal-v2-journal-owner-key \"typebridge-journal-owner/v1\", has typebridge-internal-v2-journal-owner-managed-database {}, has typebridge-internal-v2-journal-owner-managed-scope {};", literal(managed.database_name()), literal(scope)), TxType::Write).await;
    execute(journal, &format!("insert $record isa typebridge-internal-v2-migration-journal, has typebridge-internal-v2-record-key {}, has typebridge-internal-v2-control-scope {}, has typebridge-internal-v2-record-sequence \"1\", has typebridge-internal-v2-record-kind \"applied\", has typebridge-internal-v2-record-payload {}, has typebridge-internal-v2-record-payload-digest \"{digest}\";", literal(&format!("{scope}#1")), literal(scope), literal(std::str::from_utf8(&payload).unwrap())), TxType::Write).await;
}

/// Released 2.x writers stay fenced: import and later migrations keep the
/// exact v2 managed-fence schema, its control row, and an anchor whose
/// fingerprint matches the frozen V1 cutover sentinel.
pub async fn assert_released_writers_fenced(managed: &Database) {
    use type_bridge_contract::reserved::{
        LEGACY_CUTOVER_ANCHOR_ENTITY, LEGACY_CUTOVER_ANCHOR_FINGERPRINT,
        LEGACY_CUTOVER_SENTINEL_MIGRATION_ID, LEGACY_LEDGER_APPLIED_ENTITY, LEGACY_LEDGER_CHECKSUM,
        LEGACY_LEDGER_MIGRATION_ID, MANAGED_CONTROL_ENTITY, MANAGED_CONTROL_SCOPE,
        import_legacy_control_label as v2,
    };
    use type_bridge_orm::session::backend::QueryResult;
    use type_bridge_schema_compat::{
        LiveQueryControlPresence, import_legacy_fence_schema_presence,
    };

    let export = managed.schema_text().await.unwrap();
    assert!(
        matches!(
            import_legacy_fence_schema_presence(&export),
            Ok(LiveQueryControlPresence::ManagedFence)
        ),
        "the exact v2 managed-fence schema must remain: {export}"
    );
    let rows = |query: String| async move {
        match managed.execute_raw(&query, TxType::Read).await.unwrap() {
            QueryResult::Documents(rows) => rows,
            other => panic!("{query}: expected documents, got {other:?}"),
        }
    };
    let control = rows(format!(
        "match $c isa {}, has {} $s; fetch {{ \"s\": $s }};",
        v2(MANAGED_CONTROL_ENTITY),
        v2(MANAGED_CONTROL_SCOPE)
    ))
    .await;
    assert_eq!(control.len(), 1, "one v2 control row: {control:?}");
    let anchor = rows(format!(
        "match $a isa {}, has {} $f; fetch {{ \"f\": $f }};",
        v2(LEGACY_CUTOVER_ANCHOR_ENTITY),
        v2(LEGACY_CUTOVER_ANCHOR_FINGERPRINT)
    ))
    .await;
    let sentinel = rows(format!(
        "match $s isa {LEGACY_LEDGER_APPLIED_ENTITY}, has {LEGACY_LEDGER_MIGRATION_ID} \"{LEGACY_CUTOVER_SENTINEL_MIGRATION_ID}\", has {LEGACY_LEDGER_CHECKSUM} $f; fetch {{ \"f\": $f }};"
    ))
    .await;
    assert_eq!(anchor.len(), 1, "one v2 cutover anchor: {anchor:?}");
    assert_eq!(sentinel.len(), 1, "one cutover sentinel: {sentinel:?}");
    assert_eq!(
        anchor[0]["f"], sentinel[0]["f"],
        "anchor and sentinel stay bound"
    );
}
