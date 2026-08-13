# FULL SDK conformance contract

This directory is the machine-readable acceptance authority for TypeBridge
language SDKs. It is intentionally broader than
`tests/fixtures/generated-only-operation-parity-inventory.json`: that inventory
records evidence retained through the 2.1 generated-only cutover, including
binding-specific conveniences and known uniform omissions. It is evidence
input, not the definition of FULL support.

`manifest-v1.json` maps granular canonical capabilities to exactly one Rust
semantic owner, canonical cases, proof applicability, and one status for every
current and planned binding. A workflow may compose several capabilities; a
large historical operation row must not become one ambiguous capability with
several owners.

## Frozen decisions and Phase-1 baseline

- C is the first reference implementation. Kotlin/JVM, Haskell, Go, and .NET
  follow in that order against the same manifest.
- FULL is the generated-only V2 contract. Handwritten declaration APIs, V1/raw
  query builders, archive recovery internals, and target-specific conveniences
  are not conformance capabilities.
- Bulk entity and relation update/delete with all-or-nothing behavior is common
  SDK behavior. Its absence in Node is a parity gap, not permission to omit it
  from C.
- Single-type filtering is common behavior; Python/TypeScript `__` lookup
  spellings are representation details and are not copied into C.
- Direct and remote reducers and grouping are common behavior. Generated
  schema-function invocation is likewise part of the immutable typed-query
  contract. The `workforce-v2` exact17 transition accepts the selected direct
  and remote cases live across Python, Node, Rust, and the internal C
  foundation.
- The remote contract is query-only. Remote mutation is explicitly outside the
  canonical contract rather than an unsupported capability that every SDK must
  implement.
- Lifecycle hooks, Python filtered callbacks, Python cross-type owner lookup,
  Python detached-key update/delete fallback, and the retained root
  `QueryBuilder` are non-normative conveniences.
- `compat.legacy-root-surfaces` in the older typed-query corpus is compatibility
  evidence and cannot satisfy a canonical V2 proof.

The Phase-1 workforce checkpoint executes the exact selected proof rows in
`workforce-v1/catalog-v1.json` through generated Python, Node, and Rust
applications on TypeDB 3.12.1. Its catalog pins the semantic fingerprint and
each target projection fingerprint; matching report strings without those
fixture-owned identities are rejected. The checkpoint is deliberately narrow
and does not promote any capability or future binding.

The successor `workforce-v2` checkpoint assigns `accepted_live` through a
dedicated four-binding profile only to the exact17 transition cases: C02, C04,
C06, C13-C23, C28, C31, and G02. This acceptance does not widen a shared
profile or imply that C is a supported or distributed SDK; C remains an
internal, unsupported foundation until its full application contract is
complete.

## Remaining shared gaps

The manifest retains these broader gaps until their complete public lifecycle
evidence exists:

- ordered/list schema facts documented by the FULL journey but not represented
  by the current Split-YAML/schema IR;
- G05 cancellation, G06 timeout and resource limits, G08 structured diagnostics
  across all workflows, and G13 explicit close across connection, transaction,
  query, and native-resource lifecycles; their `workforce-v2` selected rows are
  query-scoped retained evidence, not broad-capability transitions;
- a public V2 rollback command, binding-neutral data-backfill operation, and
  runtime migration facade;
- standalone CLI installation evidence independent of a language SDK;
- canonical generated-model serialization/deserialization.

A gap cannot be converted to accepted by prose, by an implementation shared
only privately, or by a test that never crosses the advertised public surface.

## Proof kinds and states

Every capability maps all of these proof kinds either to `required` or to an
explicit `not_applicable` rationale:

- `compile_positive`
- `compile_negative`
- `direct_runtime`
- `remote_runtime`
- `diagnostic`
- `lifecycle`
- `artifact`

Every binding cell is exactly one of:

- `accepted_offline`
- `accepted_live`
- `gap`
- `planned`

Uniform rejection is not acceptance. `planned` is used for a new target until
the required proof set is executable; it is never a public support claim.
