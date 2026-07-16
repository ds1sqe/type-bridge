//! Versioned, binding-neutral contract primitives for TypeBridge.
//!
//! This unpublished `0.x` crate sits at the bottom of the workspace dependency
//! graph. It deliberately contains no parser, query, migration, provider, or
//! binding dependencies.

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
/// Durable managed-scope identities, bindings, and profile fingerprints.
pub mod managed_scope;
/// Context-free canonical migration identities, fingerprints, and schema steps.
pub mod migration;
/// Canonical typed migration assertion syntax and fingerprints.
pub mod migration_assertion;
pub use migration_assertion::migration_assertion_capability_vocabulary;
pub use query_plan::query_plan_capability_vocabulary;
mod migration_assertion_wire;
/// Binding-target configuration and reproducible projection fingerprints.
pub mod projection;
/// Fail-closed canonical wire decoding for runtime projections.
pub mod projection_wire;
/// Versioned schema identities, facts, provenance, and fingerprints.
/// Reusable typed query plans and the first public V2 read vocabulary.
pub mod query_plan;

mod query_plan_wire;

pub mod schema;
mod declared_schema_wire;
pub mod schema_lowering;
/// Trusted, reversible schema transitions over one durable managed scope.
pub mod schema_delta;
mod schema_delta_wire;
/// Domain-safe schema fingerprint wrappers.
pub mod schema_fingerprint;
/// Versioned server-semantic defaults shared by resolution and fingerprints.
pub mod semantic_profile;
/// Canonical temporal component values.
pub mod temporal;
/// Domain-tagged canonical scalar, cardinality, and annotation values.
pub mod value;
