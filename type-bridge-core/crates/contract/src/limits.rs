//! Canonical implementation-independent codec ceilings.

/// Maximum canonical artifact size: 16 MiB.
pub const MAX_CANONICAL_BYTES: usize = 16 * 1024 * 1024;
/// Maximum JSON nesting depth, counting the root as one.
pub const MAX_CANONICAL_DEPTH: usize = 64;
/// Maximum direct members in one array or object.
pub const MAX_CANONICAL_COLLECTION_LEN: usize = 65_536;
/// Maximum UTF-8 byte length of one string or object key: 1 MiB.
pub const MAX_CANONICAL_STRING_BYTES: usize = 1024 * 1024;

/// The complete canonical JSON structural limit set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecLimits {
    /// Maximum encoded or decoded bytes.
    pub max_bytes: usize,
    /// Maximum nested value depth.
    pub max_depth: usize,
    /// Maximum direct collection members.
    pub max_collection_len: usize,
    /// Maximum bytes in one string or key.
    pub max_string_bytes: usize,
}

impl CodecLimits {
    /// Canonical Phase 1 codec limits.
    pub const CANONICAL: Self = Self {
        max_bytes: MAX_CANONICAL_BYTES,
        max_depth: MAX_CANONICAL_DEPTH,
        max_collection_len: MAX_CANONICAL_COLLECTION_LEN,
        max_string_bytes: MAX_CANONICAL_STRING_BYTES,
    };
}

impl Default for CodecLimits {
    fn default() -> Self { Self::CANONICAL }
}

/// Canonical limits used by every contract codec consumer.
pub const CANONICAL_CODEC_LIMITS: CodecLimits = CodecLimits::CANONICAL;

/// Maximum number of selected output slots in one typed plan.
pub const MAX_SELECTED_SLOTS: usize = 16;
/// Maximum number of bindings in one typed plan.
pub const MAX_BINDINGS: usize = 256;
/// Maximum number of nodes in one predicate tree.
pub const MAX_PREDICATE_NODES: usize = 4_096;
/// Maximum predicate nesting depth, counting a root as depth one.
pub const MAX_PREDICATE_DEPTH: usize = 64;
/// Maximum number of children in one boolean expression.
pub const MAX_BOOLEAN_TERMS: usize = 256;
/// Maximum number of explicitly allowed cross-join binding pairs.
pub const MAX_ALLOWED_CROSS_JOINS: usize = 1_024;
/// Maximum number of public row/root ordering terms.
pub const MAX_ORDER_TERMS: usize = 64;
/// Maximum number of ordering terms for one collected output slot.
pub const MAX_COLLECTION_ORDER_TERMS: usize = 64;
/// Maximum UTF-8 bytes in one output or query-variable name.
pub const MAX_OUTPUT_NAME_BYTES: usize = 128;
/// Maximum UTF-8 bytes in one serialized semantic identity.
pub const MAX_SEMANTIC_ID_BYTES: usize = 512;
/// Maximum bytes in one canonical serialized diagnostic.
pub const MAX_DIAGNOSTIC_BYTES: usize = 65_536;

/// Shared implementation-independent typed-plan structural ceilings.
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

    /// Return whether `actual` fits the selected-slot ceiling.
    pub const fn allows_selected_slots(self, actual: usize) -> bool {
        actual <= self.selected_slots
    }

    /// Return whether `actual` fits the binding ceiling.
    pub const fn allows_bindings(self, actual: usize) -> bool {
        actual <= self.bindings
    }

    /// Return whether `actual` fits the predicate-node ceiling.
    pub const fn allows_predicate_nodes(self, actual: usize) -> bool {
        actual <= self.predicate_nodes
    }

    /// Return whether `actual` fits the predicate-depth ceiling.
    pub const fn allows_predicate_depth(self, actual: usize) -> bool {
        actual <= self.predicate_depth
    }

    /// Return whether `actual` fits the diagnostic-byte ceiling.
    pub const fn allows_diagnostic_bytes(self, actual: usize) -> bool {
        actual <= self.diagnostic_bytes
    }
}

impl Default for StructuralLimits {
    fn default() -> Self {
        Self::CANONICAL
    }
}

/// Canonical structural limits shared by V1 and V2 typed plans.
pub const CANONICAL_STRUCTURAL_LIMITS: StructuralLimits = StructuralLimits::CANONICAL;
