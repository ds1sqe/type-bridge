//! Versioned, binding-neutral contract primitives for TypeBridge.
//!
//! This unpublished `0.x` crate sits at the bottom of the workspace dependency
//! graph. It deliberately contains no schema, query, migration, provider, or
//! binding envelopes.

#![warn(missing_docs)]

/// Open capability identifiers and deterministic sets.
pub mod capability;
/// Canonical JSON encoding, decoding, and format versions.
pub mod codec;
/// Canonical TypeDB decimal parsing and comparison.
pub mod decimal;
/// Stable binding-neutral structured diagnostics.
pub mod diagnostic;
/// Domain-separated fingerprint algorithms and values.
pub mod fingerprint;
/// Validated typed identifier primitives.
pub mod id;
/// Canonical codec structural limits.
pub mod limits;
/// Canonical temporal component values.
pub mod temporal;
/// Domain-tagged canonical scalar, cardinality, and annotation values.
pub mod value;
