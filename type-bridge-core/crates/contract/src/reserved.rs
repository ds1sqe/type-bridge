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

/// Return whether a schema label belongs to the reserved control namespace.
#[must_use]
pub fn is_typebridge_internal_label(label: &str) -> bool {
    label.starts_with(TYPEBRIDGE_INTERNAL_PREFIX)
}
