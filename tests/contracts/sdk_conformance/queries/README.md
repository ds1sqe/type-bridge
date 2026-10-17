# SDK queries

This suite compares real Python, Node, Rust, and C typed-query producers
against one canonical set of 34 selected observations on each supported
TypeDB profile. It includes the former three-binding smoke rows in the
four-binding query journey.

Its exact17 transition cases are accepted live across all four bindings.
The G05, G06, G08, and G13 rows are evidence-only within this suite; their
broader cross-slice acceptance belongs to the artifacts suite.

The contract consists of:

- `catalog.json`: source identities, projection fingerprints, binding map,
  the exact selected proof ledger, and the strict subset of cases eligible for
  manifest transition;
- `journey.json`: deterministic authored records, operation order, cleanup
  order, and canonical observations;
- `report-schema.json`: the closed four-binding, 34-result report shape; and
- `proof-fragment-schema.json`: the closed same-run artifact used only when
  a selected fact requires a deterministic recording backend or authenticated
  hostile reply that a real TypeDB journey cannot produce; and
- `proof-fragment-allowlist.json`: the exact producer IDs, source-path sets,
  test IDs, and three deterministic observation lanes accepted from each
  binding; and
- `scripts/ci/sdk_queries_compare.py`: the fail-closed comparator.

Each live runner owns its output path through
`TYPE_BRIDGE_SDK_QUERIES_REPORT`. The parent directory must already exist, the
destination must not exist, and publication is one canonical atomic write only
after every selected observation and cleanup assertion succeeds.

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
`proof-fragment-schema.json`; it computes the observation from the exercised
API and must never copy `journey.json`'s expected object into the fragment.

The report job creates one fresh 64-hex-character run nonce and passes it to
both the fragment emitters and the final report runner through
`TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE`. Each emitter receives one
create-new destination through `TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENT`; a
report runner receives the resulting one-or-more paths through
`TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENTS`, encoded with the platform path-list
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

Queries is also the completed exact17 manifest transition gate. The accepted-live
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
