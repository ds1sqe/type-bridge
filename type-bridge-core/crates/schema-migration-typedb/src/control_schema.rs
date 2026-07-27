//! Frozen TypeDB control namespace for migration leases and journal rows.

use type_bridge_contract::reserved::{
    LEGACY_CUTOVER_ANCHOR_ENTITY, LEGACY_CUTOVER_ANCHOR_FINGERPRINT, LEGACY_CUTOVER_ANCHOR_KEY,
    LEGACY_CUTOVER_ANCHOR_SCOPE, LEGACY_CUTOVER_ANCHOR_SINGLETON_KEY,
};
pub use type_bridge_contract::reserved::{
    TYPEBRIDGE_INTERNAL_PREFIX, is_typebridge_internal_label,
};
pub use type_bridge_schema_compat::MANAGED_FENCE_SCHEMA_TYPEQL;

pub(crate) const CONTROL_ENTITY: &str = "typebridge-internal-v2-migration-control";
pub(crate) const JOURNAL_ENTITY: &str = "typebridge-internal-v2-migration-journal";
pub(crate) const JOURNAL_OWNER_ENTITY: &str = "typebridge-internal-v2-journal-owner";
pub(crate) const JOURNAL_OWNER_KEY: &str = "typebridge-internal-v2-journal-owner-key";
pub(crate) const JOURNAL_OWNER_MANAGED_DATABASE: &str =
    "typebridge-internal-v2-journal-owner-managed-database";
pub(crate) const JOURNAL_OWNER_MANAGED_SCOPE: &str =
    "typebridge-internal-v2-journal-owner-managed-scope";
pub(crate) const CONTROL_SCOPE: &str = "typebridge-internal-v2-control-scope";
pub(crate) const LEASE_HOLDER: &str = "typebridge-internal-v2-lease-holder";
pub(crate) const LEASE_FENCE: &str = "typebridge-internal-v2-lease-fence";
pub(crate) const LEASE_STATE: &str = "typebridge-internal-v2-lease-state";
pub(crate) const NEXT_SEQUENCE: &str = "typebridge-internal-v2-next-sequence";
pub(crate) const RECORD_KEY: &str = "typebridge-internal-v2-record-key";
pub(crate) const RECORD_SEQUENCE: &str = "typebridge-internal-v2-record-sequence";
pub(crate) const RECORD_KIND: &str = "typebridge-internal-v2-record-kind";
pub(crate) const RECORD_PAYLOAD: &str = "typebridge-internal-v2-record-payload";
pub(crate) const RECORD_PAYLOAD_DIGEST: &str = "typebridge-internal-v2-record-payload-digest";
pub(crate) const LEGACY_CUTOVER_ENTITY: &str = LEGACY_CUTOVER_ANCHOR_ENTITY;
pub(crate) const LEGACY_CUTOVER_KEY: &str = LEGACY_CUTOVER_ANCHOR_KEY;
pub(crate) const LEGACY_CUTOVER_SCOPE: &str = LEGACY_CUTOVER_ANCHOR_SCOPE;
pub(crate) const LEGACY_CUTOVER_FINGERPRINT: &str = LEGACY_CUTOVER_ANCHOR_FINGERPRINT;
pub(crate) const LEGACY_CUTOVER_SINGLETON_KEY: &str = LEGACY_CUTOVER_ANCHOR_SINGLETON_KEY;

pub(crate) const LEASE_HELD: &str = "held";
pub(crate) const LEASE_FREE: &str = "free";
pub(crate) const PLAN_RECORD_KIND: &str = "plan";
pub(crate) const EVENT_RECORD_KIND: &str = "event";
pub(crate) const APPLIED_RECORD_KIND: &str = "applied";
pub(crate) const ROLLBACK_PLAN_RECORD_KIND: &str = "rollback-plan";
pub(crate) const ROLLBACK_EVENT_RECORD_KIND: &str = "rollback-event";
pub(crate) const ROLLED_BACK_RECORD_KIND: &str = "rolled-back";
pub(crate) const JOURNAL_OWNER_SINGLETON_KEY: &str = "typebridge-journal-owner/v1";

const CONTROL_LABELS: &[&str] = &[
    CONTROL_ENTITY,
    JOURNAL_ENTITY,
    JOURNAL_OWNER_ENTITY,
    JOURNAL_OWNER_KEY,
    JOURNAL_OWNER_MANAGED_DATABASE,
    JOURNAL_OWNER_MANAGED_SCOPE,
    CONTROL_SCOPE,
    LEASE_HOLDER,
    LEASE_FENCE,
    LEASE_STATE,
    NEXT_SEQUENCE,
    RECORD_KEY,
    RECORD_SEQUENCE,
    RECORD_KIND,
    RECORD_PAYLOAD,
    RECORD_PAYLOAD_DIGEST,
    LEGACY_CUTOVER_ENTITY,
    LEGACY_CUTOVER_KEY,
    LEGACY_CUTOVER_SCOPE,
    LEGACY_CUTOVER_FINGERPRINT,
];

/// Return every frozen control-schema label in canonical order.
#[must_use]
pub const fn control_schema_labels() -> &'static [&'static str] {
    CONTROL_LABELS
}

/// Exact TypeDB 3 authoritative lease and journal schema.
///
/// This schema is installed only in the one-to-one paired journal database.
/// Fence and sequence values use canonical decimal strings rather than TypeDB
/// `long`, preserving the provider-neutral full nonzero `u64` domains.
pub const JOURNAL_CONTROL_SCHEMA_TYPEQL: &str = r#"define
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_label_registry_is_unique_reserved_and_declared() {
        let mut unique = std::collections::BTreeSet::new();
        for label in control_schema_labels() {
            assert!(unique.insert(*label), "duplicate control label: {label}");
            assert!(is_typebridge_internal_label(label));
            assert!(
                JOURNAL_CONTROL_SCHEMA_TYPEQL.contains(label)
                    || MANAGED_FENCE_SCHEMA_TYPEQL.contains(label),
                "control label is absent from both frozen schemas: {label}"
            );
        }
        for label in [
            CONTROL_ENTITY,
            CONTROL_SCOPE,
            LEASE_HOLDER,
            LEASE_FENCE,
            LEASE_STATE,
        ] {
            assert!(MANAGED_FENCE_SCHEMA_TYPEQL.contains(label));
        }
        for label in [
            LEGACY_CUTOVER_ENTITY,
            LEGACY_CUTOVER_KEY,
            LEGACY_CUTOVER_SCOPE,
            LEGACY_CUTOVER_FINGERPRINT,
        ] {
            assert!(MANAGED_FENCE_SCHEMA_TYPEQL.contains(label));
        }
    }
}
