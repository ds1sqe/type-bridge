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
