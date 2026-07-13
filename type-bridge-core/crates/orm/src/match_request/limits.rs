//! Canonical, implementation-independent structural limits for match requests.
//!
//! These ceilings protect validation and diagnostic construction. Runtime
//! budgets (timeouts, provider page sizes, memory, and transaction policy) are
//! executor concerns and intentionally do not live here.

/// Maximum number of selected output slots in either fetch-shape form.
pub const MAX_SELECTED_SLOTS: usize = 16;

/// Maximum number of bindings in one match plan.
pub const MAX_BINDINGS: usize = 256;

/// Maximum number of nodes in a predicate tree.
pub const MAX_PREDICATE_NODES: usize = 4_096;

/// Maximum nesting depth of a predicate tree, counting its root as depth one.
pub const MAX_PREDICATE_DEPTH: usize = 64;

/// Maximum number of direct children in one `and` or `or` expression.
pub const MAX_BOOLEAN_TERMS: usize = 256;

/// Maximum number of explicitly allowed cross-join binding pairs.
pub const MAX_ALLOWED_CROSS_JOINS: usize = 1_024;

/// Maximum number of public row/root ordering terms.
pub const MAX_ORDER_TERMS: usize = 64;

/// Maximum number of ordering terms for one collected output slot.
pub const MAX_COLLECTION_ORDER_TERMS: usize = 64;

/// Maximum UTF-8 byte length of one named output slot.
pub const MAX_OUTPUT_NAME_BYTES: usize = 128;

/// Maximum UTF-8 byte length of a serialized semantic identity.
pub const MAX_SEMANTIC_ID_BYTES: usize = 512;

/// Maximum number of bytes in one canonical serialized diagnostic.
pub const MAX_DIAGNOSTIC_BYTES: usize = 65_536;

/// The complete set of canonical structural ceilings.
///
/// Keeping the values in a struct gives validation one auditable input while
/// the public constants keep individual boundaries easy to discover and test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StructuralLimits {
    /// Selected output slots.
    pub selected_slots: usize,
    /// Plan bindings.
    pub bindings: usize,
    /// Total predicate nodes.
    pub predicate_nodes: usize,
    /// Predicate-tree depth.
    pub predicate_depth: usize,
    /// Children in one boolean expression.
    pub boolean_terms: usize,
    /// Explicitly allowed cross joins.
    pub allowed_cross_joins: usize,
    /// Public row/root ordering terms.
    pub order_terms: usize,
    /// Per-collection ordering terms.
    pub collection_order_terms: usize,
    /// Named output-slot UTF-8 bytes.
    pub output_name_bytes: usize,
    /// Semantic identity UTF-8 bytes.
    pub semantic_id_bytes: usize,
    /// Canonical diagnostic bytes.
    pub diagnostic_bytes: usize,
}

impl StructuralLimits {
    /// Canonical cross-language protocol limits.
    pub const CANONICAL: Self = Self {
        selected_slots: MAX_SELECTED_SLOTS,
        bindings: MAX_BINDINGS,
        predicate_nodes: MAX_PREDICATE_NODES,
        predicate_depth: MAX_PREDICATE_DEPTH,
        boolean_terms: MAX_BOOLEAN_TERMS,
        allowed_cross_joins: MAX_ALLOWED_CROSS_JOINS,
        order_terms: MAX_ORDER_TERMS,
        collection_order_terms: MAX_COLLECTION_ORDER_TERMS,
        output_name_bytes: MAX_OUTPUT_NAME_BYTES,
        semantic_id_bytes: MAX_SEMANTIC_ID_BYTES,
        diagnostic_bytes: MAX_DIAGNOSTIC_BYTES,
    };

    /// Return whether `actual` is within the selected-slot ceiling.
    pub const fn allows_selected_slots(self, actual: usize) -> bool {
        actual <= self.selected_slots
    }

    /// Return whether `actual` is within the binding ceiling.
    pub const fn allows_bindings(self, actual: usize) -> bool {
        actual <= self.bindings
    }

    /// Return whether `actual` is within the predicate-node ceiling.
    pub const fn allows_predicate_nodes(self, actual: usize) -> bool {
        actual <= self.predicate_nodes
    }

    /// Return whether `actual` is within the predicate-depth ceiling.
    pub const fn allows_predicate_depth(self, actual: usize) -> bool {
        actual <= self.predicate_depth
    }

    /// Return whether `actual` is within the diagnostic-byte ceiling.
    pub const fn allows_diagnostic_bytes(self, actual: usize) -> bool {
        actual <= self.diagnostic_bytes
    }
}

impl Default for StructuralLimits {
    fn default() -> Self {
        Self::CANONICAL
    }
}

/// Canonical structural limits used by every binding and provider.
pub const CANONICAL_STRUCTURAL_LIMITS: StructuralLimits = StructuralLimits::CANONICAL;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_slot_limit_is_exactly_sixteen() {
        let limits = StructuralLimits::CANONICAL;

        assert_eq!(limits.selected_slots, 16);
        assert!(limits.allows_selected_slots(16));
        assert!(!limits.allows_selected_slots(17));
    }

    #[test]
    fn fixed_ceilings_accept_the_boundary_and_reject_the_next_value() {
        let limits = StructuralLimits::CANONICAL;

        assert!(limits.allows_bindings(MAX_BINDINGS));
        assert!(!limits.allows_bindings(MAX_BINDINGS + 1));
        assert!(limits.allows_predicate_nodes(MAX_PREDICATE_NODES));
        assert!(!limits.allows_predicate_nodes(MAX_PREDICATE_NODES + 1));
        assert!(limits.allows_predicate_depth(MAX_PREDICATE_DEPTH));
        assert!(!limits.allows_predicate_depth(MAX_PREDICATE_DEPTH + 1));
        assert!(limits.allows_diagnostic_bytes(MAX_DIAGNOSTIC_BYTES));
        assert!(!limits.allows_diagnostic_bytes(MAX_DIAGNOSTIC_BYTES + 1));
    }

    #[test]
    fn default_is_the_canonical_protocol_value() {
        assert_eq!(StructuralLimits::default(), CANONICAL_STRUCTURAL_LIMITS);
    }
}
