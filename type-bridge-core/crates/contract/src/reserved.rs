//! Reserved TypeBridge schema-namespace vocabulary shared across layers.
//!
//! The V2 control prefix is a cross-layer contract: the provider layer
//! installs control types under it, and every offline surface that
//! interprets user schemas (workspace genesis resolution, export
//! partitioning) must recognize it without depending on the provider
//! crate. The prefix is frozen — changing it orphans deployed control
//! state.

/// Reserved prefix for every V2 migration control type.
pub const TYPEBRIDGE_INTERNAL_PREFIX: &str = "typebridge-internal-v2-";

/// Reserved suffix for the companion journal of one managed database.
pub const TYPEBRIDGE_JOURNAL_DATABASE_SUFFIX: &str = "__tbv2_journal";

/// Managed-database entity carrying the current migration fence mirror.
pub const MANAGED_CONTROL_ENTITY: &str = "typebridge-internal-v2-migration-control";
/// Managed scope key on the migration fence mirror.
pub const MANAGED_CONTROL_SCOPE: &str = "typebridge-internal-v2-control-scope";
/// Optional lease-holder attribute on a held migration fence mirror.
pub const MANAGED_CONTROL_LEASE_HOLDER: &str = "typebridge-internal-v2-lease-holder";
/// Monotonic fence attribute on the migration fence mirror.
pub const MANAGED_CONTROL_LEASE_FENCE: &str = "typebridge-internal-v2-lease-fence";
/// Held/free state attribute on the migration fence mirror.
pub const MANAGED_CONTROL_LEASE_STATE: &str = "typebridge-internal-v2-lease-state";
/// Frozen state value for a held migration fence mirror.
pub const MANAGED_CONTROL_LEASE_HELD: &str = "held";
/// Frozen state value for a free migration fence mirror.
pub const MANAGED_CONTROL_LEASE_FREE: &str = "free";

/// Managed-database entity that proves a legacy scope completed V2 adoption.
pub const LEGACY_CUTOVER_ANCHOR_ENTITY: &str = "typebridge-internal-v2-legacy-cutover";
/// Key attribute on the managed legacy-cutover anchor.
pub const LEGACY_CUTOVER_ANCHOR_KEY: &str = "typebridge-internal-v2-legacy-cutover-key";
/// Managed-scope attribute on the managed legacy-cutover anchor.
pub const LEGACY_CUTOVER_ANCHOR_SCOPE: &str = "typebridge-internal-v2-legacy-cutover-scope";
/// Fingerprint attribute binding the managed anchor to the frozen V1 ledger.
pub const LEGACY_CUTOVER_ANCHOR_FINGERPRINT: &str =
    "typebridge-internal-v2-legacy-cutover-fingerprint";
/// Frozen singleton key carried by the managed legacy-cutover anchor.
pub const LEGACY_CUTOVER_ANCHOR_SINGLETON_KEY: &str = "typebridge-legacy-cutover-anchor/v1";

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
/// Stable error text returned when an exact, anchor-bound cutover is observed.
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
