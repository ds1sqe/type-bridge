//! Reserved TypeBridge schema-namespace vocabulary shared across layers.
//!
//! The V2 control prefix is a cross-layer contract: the provider layer
//! installs control types under it, and every offline surface that
//! interprets user schemas (workspace genesis resolution, export
//! partitioning) must recognize it without depending on the provider
//! crate. The prefix is frozen — changing it orphans deployed control
//! state.

/// Reserved prefix for every V2 migration control type.
pub const TYPEBRIDGE_INTERNAL_PREFIX: &str = "typebridge-internal-";

/// Reserved suffix for the companion journal of one managed database.
pub const TYPEBRIDGE_JOURNAL_DATABASE_SUFFIX: &str = "__tb_journal";

/// Managed-database entity carrying the current migration fence mirror.
pub const MANAGED_CONTROL_ENTITY: &str = "typebridge-internal-migration-control";
/// Managed scope key on the migration fence mirror.
pub const MANAGED_CONTROL_SCOPE: &str = "typebridge-internal-control-scope";
/// Optional lease-holder attribute on a held migration fence mirror.
pub const MANAGED_CONTROL_LEASE_HOLDER: &str = "typebridge-internal-lease-holder";
/// Monotonic fence attribute on the migration fence mirror.
pub const MANAGED_CONTROL_LEASE_FENCE: &str = "typebridge-internal-lease-fence";
/// Held/free state attribute on the migration fence mirror.
pub const MANAGED_CONTROL_LEASE_STATE: &str = "typebridge-internal-lease-state";
/// Frozen state value for a held migration fence mirror.
pub const MANAGED_CONTROL_LEASE_HELD: &str = "held";
/// Frozen state value for a free migration fence mirror.
pub const MANAGED_CONTROL_LEASE_FREE: &str = "free";

/// Managed-database entity that proves a legacy scope completed V2 adoption.
pub const LEGACY_CUTOVER_ANCHOR_ENTITY: &str = "typebridge-internal-legacy-cutover";
/// Key attribute on the managed legacy-cutover anchor.
pub const LEGACY_CUTOVER_ANCHOR_KEY: &str = "typebridge-internal-legacy-cutover-key";
/// Managed-scope attribute on the managed legacy-cutover anchor.
pub const LEGACY_CUTOVER_ANCHOR_SCOPE: &str = "typebridge-internal-legacy-cutover-scope";
/// Fingerprint attribute binding the managed anchor to the frozen V1 ledger.
pub const LEGACY_CUTOVER_ANCHOR_FINGERPRINT: &str =
    "typebridge-internal-legacy-cutover-fingerprint";
/// Frozen singleton key carried by the managed legacy-cutover anchor.
pub const LEGACY_CUTOVER_ANCHOR_SINGLETON_KEY: &str = "typebridge-legacy-cutover-anchor/v1";

/// Atomic data-step completion marker entity.
pub const DATA_STEP_ENTITY: &str = "typebridge-internal-data-step";
/// Canonical compound completion-marker key.
pub const DATA_STEP_KEY: &str = "typebridge-internal-data-step-key";
/// Managed scope carried by the completion marker.
pub const DATA_STEP_SCOPE: &str = "typebridge-internal-data-step-scope";
/// Canonical compound migration identity carried by the marker.
pub const DATA_STEP_MIGRATION: &str = "typebridge-internal-data-step-migration";
/// Manifest-local data-step identity carried by the marker.
pub const DATA_STEP_ID: &str = "typebridge-internal-data-step-id";
/// Journal commit-boundary record for an atomic data step.
pub const DATA_STEP_EVENT_RECORD_KIND: &str = "data-step-event";

/// Frozen V1 applied-migration entity used by the cutover sentinel.
pub const LEGACY_LEDGER_APPLIED_ENTITY: &str = "type_bridge_migration";
/// Frozen V1 applied-ledger migration-id attribute.
pub const LEGACY_LEDGER_MIGRATION_ID: &str = "migration_id";
/// Frozen V1 applied-ledger application-label attribute.
pub const LEGACY_LEDGER_APP_LABEL: &str = "migration_app_label";
/// Frozen V1 applied-ledger migration-name attribute.
pub const LEGACY_LEDGER_NAME: &str = "migration_name";
/// Frozen V1 applied-ledger applied-at attribute.
pub const LEGACY_LEDGER_APPLIED_AT: &str = "migration_applied_at";
/// Frozen V1 applied-ledger checksum attribute.
pub const LEGACY_LEDGER_CHECKSUM: &str = "migration_checksum";

/// Reserved applied-ledger application label for the V2 cutover sentinel.
pub const LEGACY_CUTOVER_SENTINEL_APP_LABEL: &str = "type_bridge_v2_internal";
/// Reserved applied-ledger migration name for the V2 cutover sentinel.
pub const LEGACY_CUTOVER_SENTINEL_NAME: &str = "__legacy_writer_cutover__";
/// Reserved applied-ledger key for the V2 cutover sentinel.
pub const LEGACY_CUTOVER_SENTINEL_MIGRATION_ID: &str =
    "type_bridge_v2_internal:__legacy_writer_cutover__";
/// Deterministic TypeDB datetime carried by the V2 cutover sentinel.
pub const LEGACY_CUTOVER_SENTINEL_APPLIED_AT: &str = "1970-01-01T00:00:00.000000000";
/// Error text released 2.x writers emit when they observe an exact,
/// anchor-bound cutover. Nothing in 3.0 reads it; it is kept as a frozen name.
pub const LEGACY_WRITER_CUTOVER_MESSAGE: &str =
    "legacy migration writes are permanently disabled for this database after V2 adoption";

/// Internal query tag used by reusable legacy-writer guards and test backends.
#[doc(hidden)]
pub const LEGACY_WRITER_GUARD_QUERY_TAG: &str = "# typebridge-internal-legacy-writer-guard/v2\n";

/// Return whether a schema label belongs to the reserved control namespace.
#[must_use]
pub fn is_typebridge_internal_label(label: &str) -> bool {
    label.starts_with(TYPEBRIDGE_INTERNAL_PREFIX)
}

/// Migration journal/control vocabulary: control entity.
pub const CONTROL_ENTITY: &str = MANAGED_CONTROL_ENTITY;
/// Migration journal/control vocabulary: journal entity.
pub const JOURNAL_ENTITY: &str = "typebridge-internal-migration-journal";
/// Migration journal/control vocabulary: journal owner entity.
pub const JOURNAL_OWNER_ENTITY: &str = "typebridge-internal-journal-owner";
/// Migration journal/control vocabulary: journal owner key.
pub const JOURNAL_OWNER_KEY: &str = "typebridge-internal-journal-owner-key";
/// Migration journal/control vocabulary: journal owner managed database.
pub const JOURNAL_OWNER_MANAGED_DATABASE: &str =
    "typebridge-internal-journal-owner-managed-database";
/// Migration journal/control vocabulary: journal owner managed scope.
pub const JOURNAL_OWNER_MANAGED_SCOPE: &str = "typebridge-internal-journal-owner-managed-scope";
/// Migration journal/control vocabulary: control scope.
pub const CONTROL_SCOPE: &str = MANAGED_CONTROL_SCOPE;
/// Migration journal/control vocabulary: lease holder.
pub const LEASE_HOLDER: &str = MANAGED_CONTROL_LEASE_HOLDER;
/// Migration journal/control vocabulary: lease fence.
pub const LEASE_FENCE: &str = MANAGED_CONTROL_LEASE_FENCE;
/// Migration journal/control vocabulary: lease state.
pub const LEASE_STATE: &str = MANAGED_CONTROL_LEASE_STATE;
/// Migration journal/control vocabulary: next sequence.
pub const NEXT_SEQUENCE: &str = "typebridge-internal-next-sequence";
/// Migration journal/control vocabulary: record key.
pub const RECORD_KEY: &str = "typebridge-internal-record-key";
/// Migration journal/control vocabulary: record sequence.
pub const RECORD_SEQUENCE: &str = "typebridge-internal-record-sequence";
/// Migration journal/control vocabulary: record kind.
pub const RECORD_KIND: &str = "typebridge-internal-record-kind";
/// Migration journal/control vocabulary: record payload.
pub const RECORD_PAYLOAD: &str = "typebridge-internal-record-payload";
/// Migration journal/control vocabulary: record payload digest.
pub const RECORD_PAYLOAD_DIGEST: &str = "typebridge-internal-record-payload-digest";
/// Migration journal/control vocabulary: legacy cutover entity.
pub const LEGACY_CUTOVER_ENTITY: &str = LEGACY_CUTOVER_ANCHOR_ENTITY;
/// Migration journal/control vocabulary: legacy cutover key.
pub const LEGACY_CUTOVER_KEY: &str = LEGACY_CUTOVER_ANCHOR_KEY;
/// Migration journal/control vocabulary: legacy cutover scope.
pub const LEGACY_CUTOVER_SCOPE: &str = LEGACY_CUTOVER_ANCHOR_SCOPE;
/// Migration journal/control vocabulary: legacy cutover fingerprint.
pub const LEGACY_CUTOVER_FINGERPRINT: &str = LEGACY_CUTOVER_ANCHOR_FINGERPRINT;
/// Migration journal/control vocabulary: legacy cutover singleton key.
pub const LEGACY_CUTOVER_SINGLETON_KEY: &str = LEGACY_CUTOVER_ANCHOR_SINGLETON_KEY;

/// Migration journal/control vocabulary: lease held.
pub const LEASE_HELD: &str = MANAGED_CONTROL_LEASE_HELD;
/// Migration journal/control vocabulary: lease free.
pub const LEASE_FREE: &str = MANAGED_CONTROL_LEASE_FREE;
/// Migration journal/control vocabulary: plan record kind.
pub const PLAN_RECORD_KIND: &str = "plan";
/// Migration journal/control vocabulary: event record kind.
pub const EVENT_RECORD_KIND: &str = "event";
/// Migration journal/control vocabulary: backfill event record kind.
pub const BACKFILL_EVENT_RECORD_KIND: &str = "backfill-event";
/// Migration journal/control vocabulary: applied record kind.
pub const APPLIED_RECORD_KIND: &str = "applied";
/// Migration journal/control vocabulary: rollback plan record kind.
pub const ROLLBACK_PLAN_RECORD_KIND: &str = "rollback-plan";
/// Migration journal/control vocabulary: rollback event record kind.
pub const ROLLBACK_EVENT_RECORD_KIND: &str = "rollback-event";
/// Migration journal/control vocabulary: rolled back record kind.
pub const ROLLED_BACK_RECORD_KIND: &str = "rolled-back";
/// Migration journal/control vocabulary: journal owner singleton key.
pub const JOURNAL_OWNER_SINGLETON_KEY: &str = "typebridge-journal-owner/v1";

/// Exact TypeDB 3 authoritative lease and journal schema.
///
/// This schema is installed only in the one-to-one paired journal database.
/// Fence and sequence values use canonical decimal strings rather than TypeDB
/// `long`, preserving the provider-neutral full nonzero `u64` domains.
pub const JOURNAL_CONTROL_SCHEMA_TYPEQL: &str = r#"define
attribute typebridge-internal-journal-owner-key, value string;
attribute typebridge-internal-journal-owner-managed-database, value string;
attribute typebridge-internal-journal-owner-managed-scope, value string;
attribute typebridge-internal-control-scope, value string;
attribute typebridge-internal-lease-holder, value string;
attribute typebridge-internal-lease-fence, value string;
attribute typebridge-internal-lease-state, value string;
attribute typebridge-internal-next-sequence, value string;
attribute typebridge-internal-record-key, value string;
attribute typebridge-internal-record-sequence, value string;
attribute typebridge-internal-record-kind, value string;
attribute typebridge-internal-record-payload, value string;
attribute typebridge-internal-record-payload-digest, value string;
entity typebridge-internal-journal-owner,
    owns typebridge-internal-journal-owner-key @key,
    owns typebridge-internal-journal-owner-managed-database @card(1..1),
    owns typebridge-internal-journal-owner-managed-scope @card(1..1);
entity typebridge-internal-migration-control,
    owns typebridge-internal-control-scope @key,
    owns typebridge-internal-lease-holder @card(0..1),
    owns typebridge-internal-lease-fence @card(1..1),
    owns typebridge-internal-lease-state @card(1..1),
    owns typebridge-internal-next-sequence @card(1..1);
entity typebridge-internal-migration-journal,
    owns typebridge-internal-record-key @key,
    owns typebridge-internal-control-scope @card(1..1),
    owns typebridge-internal-record-sequence @card(1..1),
    owns typebridge-internal-record-kind @card(1..1),
    owns typebridge-internal-record-payload @card(1..1),
    owns typebridge-internal-record-payload-digest @card(1..1);
"#;

/// Exact TypeDB 3 fence-mirror schema installed in a managed database.
///
/// Query-only databases may omit this partition. If any reserved V2 fact is
/// present, however, the complete partition must equal these frozen bytes.
pub const MANAGED_FENCE_SCHEMA_TYPEQL: &str = r#"define
attribute typebridge-internal-control-scope, value string;
attribute typebridge-internal-lease-holder, value string;
attribute typebridge-internal-lease-fence, value string;
attribute typebridge-internal-lease-state, value string;
attribute typebridge-internal-legacy-cutover-key, value string;
attribute typebridge-internal-legacy-cutover-scope, value string;
attribute typebridge-internal-legacy-cutover-fingerprint, value string;
entity typebridge-internal-migration-control,
    owns typebridge-internal-control-scope @key,
    owns typebridge-internal-lease-holder @card(0..1),
    owns typebridge-internal-lease-fence @card(1..1),
    owns typebridge-internal-lease-state @card(1..1);
entity typebridge-internal-legacy-cutover,
    owns typebridge-internal-legacy-cutover-key @key,
    owns typebridge-internal-legacy-cutover-scope @card(1..1),
    owns typebridge-internal-legacy-cutover-fingerprint @card(1..1);
attribute typebridge-internal-data-step-key, value string;
attribute typebridge-internal-data-step-scope, value string;
attribute typebridge-internal-data-step-migration, value string;
attribute typebridge-internal-data-step-id, value string;
entity typebridge-internal-data-step,
    owns typebridge-internal-data-step-key @key,
    owns typebridge-internal-data-step-scope @card(1..1),
    owns typebridge-internal-data-step-migration @card(1..1),
    owns typebridge-internal-data-step-id @card(1..1);
"#;

/// Frozen journal suffix accepted only for migration import.
pub const IMPORT_LEGACY_JOURNAL_SUFFIX: &str = "__tbv2_journal";
/// Frozen internal prefix accepted only for migration import.
pub const IMPORT_LEGACY_INTERNAL_PREFIX: &str = "typebridge-internal-v2-";
/// Import evidence kind binding a frozen applied partition and cutover.
pub const LEGACY_IMPORT_RECORD_KIND: &str = "legacy-import";
/// Terminal import record written before publishing the new managed mirror.
pub const IMPORT_COMPLETE_RECORD_KIND: &str = "import-complete";

/// Map a current control label to its frozen import spelling.
#[doc(hidden)]
pub fn import_legacy_control_label(label: &str) -> std::borrow::Cow<'_, str> {
    match label.strip_prefix(TYPEBRIDGE_INTERNAL_PREFIX) {
        Some(tail) => std::borrow::Cow::Owned(format!("{IMPORT_LEGACY_INTERNAL_PREFIX}{tail}")),
        None => std::borrow::Cow::Borrowed(label),
    }
}
/// Frozen pre-3.0 schema, read only by migration import.
pub const IMPORT_LEGACY_JOURNAL_CONTROL_SCHEMA_TYPEQL: &str = r#"define
attribute typebridge-internal-v2-journal-owner-key, value string;
attribute typebridge-internal-v2-journal-owner-managed-database, value string;
attribute typebridge-internal-v2-journal-owner-managed-scope, value string;
attribute typebridge-internal-v2-control-scope, value string;
attribute typebridge-internal-v2-lease-holder, value string;
attribute typebridge-internal-v2-lease-fence, value string;
attribute typebridge-internal-v2-lease-state, value string;
attribute typebridge-internal-v2-next-sequence, value string;
attribute typebridge-internal-v2-record-key, value string;
attribute typebridge-internal-v2-record-sequence, value string;
attribute typebridge-internal-v2-record-kind, value string;
attribute typebridge-internal-v2-record-payload, value string;
attribute typebridge-internal-v2-record-payload-digest, value string;
entity typebridge-internal-v2-journal-owner,
    owns typebridge-internal-v2-journal-owner-key @key,
    owns typebridge-internal-v2-journal-owner-managed-database @card(1..1),
    owns typebridge-internal-v2-journal-owner-managed-scope @card(1..1);
entity typebridge-internal-v2-migration-control,
    owns typebridge-internal-v2-control-scope @key,
    owns typebridge-internal-v2-lease-holder @card(0..1),
    owns typebridge-internal-v2-lease-fence @card(1..1),
    owns typebridge-internal-v2-lease-state @card(1..1),
    owns typebridge-internal-v2-next-sequence @card(1..1);
entity typebridge-internal-v2-migration-journal,
    owns typebridge-internal-v2-record-key @key,
    owns typebridge-internal-v2-control-scope @card(1..1),
    owns typebridge-internal-v2-record-sequence @card(1..1),
    owns typebridge-internal-v2-record-kind @card(1..1),
    owns typebridge-internal-v2-record-payload @card(1..1),
    owns typebridge-internal-v2-record-payload-digest @card(1..1);
"#;
/// Frozen pre-3.0 schema, read only by migration import.
pub const IMPORT_LEGACY_MANAGED_FENCE_SCHEMA_TYPEQL: &str = r#"define
attribute typebridge-internal-v2-control-scope, value string;
attribute typebridge-internal-v2-lease-holder, value string;
attribute typebridge-internal-v2-lease-fence, value string;
attribute typebridge-internal-v2-lease-state, value string;
attribute typebridge-internal-v2-legacy-cutover-key, value string;
attribute typebridge-internal-v2-legacy-cutover-scope, value string;
attribute typebridge-internal-v2-legacy-cutover-fingerprint, value string;
entity typebridge-internal-v2-migration-control,
    owns typebridge-internal-v2-control-scope @key,
    owns typebridge-internal-v2-lease-holder @card(0..1),
    owns typebridge-internal-v2-lease-fence @card(1..1),
    owns typebridge-internal-v2-lease-state @card(1..1);
entity typebridge-internal-v2-legacy-cutover,
    owns typebridge-internal-v2-legacy-cutover-key @key,
    owns typebridge-internal-v2-legacy-cutover-scope @card(1..1),
    owns typebridge-internal-v2-legacy-cutover-fingerprint @card(1..1);
"#;
