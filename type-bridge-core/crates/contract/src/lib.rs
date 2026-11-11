//! Versioned, binding-neutral contract primitives for TypeBridge.
//!
//! This public, release-lockstep supporting crate sits at the bottom of the
//! workspace dependency graph. It deliberately contains no parser, query,
//! migration, provider, or binding dependencies.

#![deny(missing_docs)]

/// Open capability identifiers and deterministic sets.
pub mod capability;
/// Canonical JSON encoding, decoding, and format versions.
pub mod codec;
/// Canonical TypeDB decimal parsing and comparison.
pub mod decimal;
/// Stable binding-neutral structured diagnostics.
pub mod diagnostic;
/// One parser per closed enum and the `unknown_enum_value` diagnostic.
pub mod enum_spelling;
/// Diagnostic-only execution phase timelines.
pub mod execution_timeline;
/// Value-only typed provider failures.
pub mod failure;
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
/// Closed binding-neutral migration backfill plans and fingerprints.
pub mod migration_backfill;
/// Atomic migration data programs.
pub mod migration_data;
pub use migration_assertion::migration_assertion_capability_vocabulary;
pub use query_plan::query_given_rows_capability;
mod migration_assertion_wire;
/// Target-independent canonical generated-model records and ordered archives.
pub mod projected_record;
/// Binding-target configuration and reproducible projection fingerprints.
pub mod projection;
/// Fail-closed canonical wire decoding for runtime projections.
pub mod projection_wire;
pub mod query_model_evidence;
pub mod query_model_limits;
/// Canonical typed read/write plans and their shared vocabulary.
pub mod query_plan;
/// Staged transaction envelopes and authenticated replies.
pub mod query_remote;
/// Bounded structured diagnostics shared by remote plan transports.
pub mod query_remote_diagnostic;
pub mod reserved;

mod query_plan_wire;

mod declared_schema_wire;
pub mod schema;
/// Trusted, reversible schema transitions over one durable managed scope.
pub mod schema_delta;
mod schema_delta_wire;
/// Domain-safe schema fingerprint wrappers.
pub mod schema_fingerprint;
pub mod schema_lowering;
/// Versioned in-memory diagnostics for binding-neutral SDK execution.
pub mod sdk_diagnostic;
/// Versioned server-semantic defaults shared by resolution and fingerprints.
pub mod semantic_profile;
/// The core batch split rule shared by every binding and the proxy.
pub mod split;
/// Canonical temporal component values.
pub mod temporal;
/// Domain-tagged canonical scalar, cardinality, and annotation values.
pub mod value;
