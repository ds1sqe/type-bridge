//! Frozen TypeDB control namespace for migration leases and journal rows.

pub use type_bridge_contract::reserved::{
    TYPEBRIDGE_INTERNAL_PREFIX, is_typebridge_internal_label,
};
pub use type_bridge_schema_compat::MANAGED_FENCE_SCHEMA_TYPEQL;

pub(crate) use type_bridge_contract::reserved::{
    APPLIED_RECORD_KIND, BACKFILL_EVENT_RECORD_KIND, CONTROL_ENTITY, CONTROL_SCOPE,
    EVENT_RECORD_KIND, JOURNAL_ENTITY, JOURNAL_OWNER_ENTITY, JOURNAL_OWNER_KEY,
    JOURNAL_OWNER_MANAGED_DATABASE, JOURNAL_OWNER_MANAGED_SCOPE, JOURNAL_OWNER_SINGLETON_KEY,
    LEASE_FENCE, LEASE_FREE, LEASE_HELD, LEASE_HOLDER, LEASE_STATE, LEGACY_CUTOVER_ENTITY,
    LEGACY_CUTOVER_FINGERPRINT, LEGACY_CUTOVER_KEY, LEGACY_CUTOVER_SCOPE,
    LEGACY_CUTOVER_SINGLETON_KEY, NEXT_SEQUENCE, PLAN_RECORD_KIND, RECORD_KEY, RECORD_KIND,
    RECORD_PAYLOAD, RECORD_PAYLOAD_DIGEST, RECORD_SEQUENCE, ROLLBACK_EVENT_RECORD_KIND,
    ROLLBACK_PLAN_RECORD_KIND, ROLLED_BACK_RECORD_KIND,
};

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

pub use type_bridge_contract::reserved::JOURNAL_CONTROL_SCHEMA_TYPEQL;

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
