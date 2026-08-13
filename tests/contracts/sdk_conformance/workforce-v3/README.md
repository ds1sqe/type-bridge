# Workforce conformance v3

This directory is the additive four-binding data/model/runtime successor to
the preserved workforce V1 and V2 contracts. It freezes exactly 21 selected
proof rows for Python, Node, Rust, and C without changing either earlier wire.

The catalog is deliberately in `phase0_unfinalized` state. G01 changes the
canonical schema and every binding projection, so Phase 0 cannot truthfully
name their future digests. `expected_fingerprints` therefore remains `null`,
and the V3 comparator rejects report validation and fan-in with
`unfinalized_contract`. The dedicated V3 fixture keeps the preserved V1/V2
fixture unchanged while adding ordered-distinct `aliases` and `participant`
facts plus explicit range, regex, and allowed-value constraint anchors for
this campaign. After that fixture and all four emitters are
complete, one explicit authority update must freeze the real semantic and four
distinct projection fingerprints and change the state to `finalized` before
any report can be accepted.

The constraint observation distinguishes local validation from provider state:
scalar, cardinality, constructibility, identity, inheritance, and ordered-
distinct failures reject before I/O, while standalone `@unique` is retained in
the projection and enforced across owners by TypeDB.

The contract consists of:

- `catalog-v3.json`: the exact 21-row ledger, four live report-producer
  identities, the exact 17-case candidate transition, and the unfinalized
  fingerprint state;
- `journey-v3.json`: authored records, dependency-reversed cleanup order, and
  21 complete normalized observation objects;
- `report-schema-v3.json`: the closed four-binding, exact-21 report envelope;
- `proof-fragment-schema-v1.json`: the V3-specific same-run fragment wire;
- `proof-fragment-allowlist-v1.json`: exactly two committed fragment producers
  and seven deterministic lanes per binding; and
- `scripts/ci/compare_workforce_conformance_v3.py`: the independent
  fail-closed comparator.

The journey observations are closed semantic objects, not pass/fail markers.
They retain exact batch duplicate and rollback outcomes, borrowed transaction
visibility and poison state, six filter operators and strict-identity `first`,
canonical field/role/package identity, signed integer keys, unkeyed IID
lifecycle, ordered scalar/player duplicate identity, connection admission,
all eight resource dimensions, structured diagnostic classes, and complete
parent/child/result close behavior. The comparator rejects a changed top-level
shape even while the catalog remains unfinalized.

The 17 transition cases are C01, C03, C05, C07 through C12, C24 through C27,
C29, C30, G01, and G03, in that Plan05 order. G05, G06, G08, and G13 contribute
the selected data-plane cancellation, resource-limit, diagnostic, and close
rows but remain manifest gaps. The comparator derives pending promotions only
for the exact 17 and rejects any catalog that moves a broad evidence-only case
into that set.

## Live report authority

The exact V3 live producers are:

- `python.generated-data-model-runtime-v3-live` /
  `python.generated_data_model_runtime_v3_live`;
- `node.generated-data-model-runtime-v3-live` /
  `node.generated_data_model_runtime_v3_live`;
- `type-bridge-rust.generated-data-model-runtime-v3-live` /
  `rust_projection_live::generated_data_model_runtime_v3_live`; and
- `type-bridge-c.generated-data-model-runtime-v3-live` /
  `c_projection_live::generated_data_model_runtime_v3_live`.

Each runner receives a create-new destination through
`TYPE_BRIDGE_WORKFORCE_REPORT_V3`. It may publish one compact canonical JSON
report only after it has computed all 21 observations through its generated
public surface, verified cleanup, and matched the finalized source and
fingerprint authority. Expected journey objects may be compared after
observation; they may never seed, patch, or complete producer output.

## Deterministic proof fragments

Each binding has exactly two fragment producers. Its generated-package
producer owns three diagnostic lanes: projected constraint validation,
projection evidence integrity, and token package fencing. Its generated-data
producer owns four lanes: connection policy, data-operation cancellation,
data-operation resource limits, and the structured data-operation diagnostic.
All other selected observations must come from the live producer.

The shared projection-evidence diagnostic uses one raw-admission mutation that
exists before every binding-specific ledger is decoded: a missing semantic
schema fingerprint at canonical evidence index zero. Its occurrence counts are
therefore `1` to `0` in every binding; fixed-resource totals are deliberately
not compared across bindings because those ledgers have different cardinality.

The report job creates one fresh 64-lowercase-hex nonce and supplies it through
`TYPE_BRIDGE_WORKFORCE_V3_PROOF_RUN_NONCE`. Each fragment emitter receives one
create-new destination through `TYPE_BRIDGE_WORKFORCE_V3_PROOF_FRAGMENT`; the
report runner receives the resulting paths through
`TYPE_BRIDGE_WORKFORCE_V3_PROOF_FRAGMENTS`, using the platform path-list
separator. V3 formats, environment names, nonce, contract digests, producer
IDs, source identities, test IDs, and lane ownership are independent of V2.

The validator requires both committed producers exactly once, all seven lanes
exactly once, compact canonical JSON, regular non-symlink files, the fresh
nonce, and current raw digests for every listed source. A fragment carries a
whole observation computed by its named test; copying `journey-v3.json`'s
expected object into emitter logic is invalid evidence.

The Phase-0 allowlist names only source files that currently exist. If
implementation introduces a new semantic owner (including future C batch,
filter, or result modules), the allowlist source set must be intentionally
reviewed and updated before fragment emission. A stale Phase-0 source list or
digest cannot cross the V3 gate.

Final V3 acceptance does not make C supported or publishable. Plans06–08 and
#189 retain those decisions, and G05/G06/G08/G13 remain gaps until Plan08.
