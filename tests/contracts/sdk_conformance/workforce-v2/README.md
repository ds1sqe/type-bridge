# Workforce conformance v2

This directory defines the four-binding typed-query successor to the preserved
`workforce-v1` checkpoint. It compares real Python, Node, Rust, and C live
producers against one exact TypeDB 3.12.1 fixture and one canonical set of 34
selected observations.

V2 does not replace or widen the V1 wire contract. V1 remains the three-binding,
nine-row Phase-1 regression. V2 adds C and the C13-C23, C28, C31, G02, G05,
G06, G08, and G13 typed-query evidence while retaining four non-overlapping V1
baseline rows. Its exact17 transition cases are accepted live across Python,
Node, Rust, and the internal C foundation. The G05, G06, G08, and G13 rows are
evidence-only and leave their broader capabilities as manifest gaps.

The contract consists of:

- `catalog-v2.json`: source identities, projection fingerprints, binding map,
  the exact selected proof ledger, and the strict subset of cases eligible for
  manifest transition;
- `journey-v2.json`: deterministic authored records, operation order, cleanup
  order, and canonical observations;
- `report-schema-v2.json`: the closed four-binding, 34-result report shape; and
- `proof-fragment-schema-v1.json`: the closed same-run artifact used only when
  a selected fact requires a deterministic recording backend or authenticated
  hostile reply that a real TypeDB journey cannot produce; and
- `proof-fragment-allowlist-v1.json`: the exact producer IDs, source-path sets,
  test IDs, and three deterministic observation lanes accepted from each
  binding; and
- `scripts/ci/compare_workforce_conformance_v2.py`: the fail-closed comparator.

Each live runner owns its output path through
`TYPE_BRIDGE_WORKFORCE_REPORT_V2`. The parent directory must already exist, the
destination must not exist, and publication is one canonical atomic write only
after every selected observation and cleanup assertion succeeds. V1 continues
to use `TYPE_BRIDGE_WORKFORCE_REPORT` independently.

Observations contain only stable authored identities and normalized semantic
values. They exclude TypeDB IIDs, cursors, ports, nonces, database names,
timestamps, process identities, provider text, and other run-specific data.
Direct and remote rows sharing an observation reference must be identical.
Cancellation and limit rows deliberately use lane-specific references where
provider effects differ; all four bindings must still agree within each lane.

## Deterministic proof fragments

A report producer may obtain a complete selected observation from a
deterministic binding-local test only when the live public provider cannot
honestly produce that condition, such as a latched in-flight cancellation or a
signed hostile remote reply. The test writes a fragment conforming to
`proof-fragment-schema-v1.json`; it computes the observation from the exercised
API and must never copy `journey-v2.json`'s expected object into the fragment.

The report job creates one fresh 64-hex-character run nonce and passes it to
both the fragment emitters and the final report runner through
`TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE`. Each emitter receives one
create-new destination through `TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENT`; a
report runner receives the resulting one-or-more paths through
`TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENTS`, encoded with the platform path-list
separator. Every fragment binds that nonce, the exact journey, allowlist, and
fragment-schema digests, its binding, its stable producer/test identity, and
the raw digests of all producer source files. Emitters write compact canonical
JSON with one terminal LF and no timestamps or machine identity. The final
runner accepts only regular non-symlink files no larger than 64 KiB, rejects
duplicate `(observation_ref, proof_kind)` lanes across fragments, recomputes
every bound source digest from the repository, and requires every committed
producer exactly once with its exact source-path set, test IDs, and complete
lane set. A missing, stale, foreign-binding, wrong-nonce, wrong-producer,
wrong-source, wrong-test, extra, or duplicate fragment prevents report
publication.

Fragments carry whole normalized observations, not JSON patches. A live check
may additionally gate the same semantics, but it cannot fill an expected field
that the fragment's named test did not observe. The report remains the only
published conformance artifact; fragments are task-owned intermediate evidence
and do not change the 34-row wire contract.

V2 is also the completed exact17 manifest transition gate. The accepted-live
subset is C02, C04, C06, C13-C23, C28, C31, and G02, assigned through one
dedicated four-live binding profile. In particular, C31 accepts remote reducers
and grouping and G02 accepts generated schema-function invocation from immutable
typed queries. The accepted checkpoint requires all four reports against the
transitioned manifest digest and an empty `pending_manifest_promotions` list.

The G05, G06, G08, and G13 selected rows remain query-scoped evidence for
cancellation, limits, structured diagnostics, and query-resource close. They
do not transition the broader runtime capabilities, which remain manifest gaps
until their connection, transaction, non-query, and native-resource lifecycle
scope is complete. No unselected or evidence-only case can use the exact17
transition path, and shared profiles are not widened as a shortcut. Exact17
acceptance does not imply that C is a supported or distributed SDK; C remains
an internal, unsupported foundation.

The comparator is evidence selection, not evidence inference. Compiler
positive/negative matrices, hostile ABI cases, package/install probes, and the
broader manifest proof ledger remain required even though they are not repeated
as report rows.
