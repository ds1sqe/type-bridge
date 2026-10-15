# SDK conformance suites

This directory is the executable acceptance authority for TypeBridge language
SDKs. `manifest.json` maps each capability to one Rust semantic owner, its
proof requirements, canonical cases, and the status of each binding.
`cases.json` retains the complete 44-case inventory. The generated-only parity
inventory is supporting evidence; it does not define FULL acceptance.

## Suites

| Suite | Concern | Main comparer |
| --- | --- | --- |
| `queries` | Typed queries, exact17 transitions, direct/remote observations and same-run proof fragments | `sdk_queries_compare.py` |
| `models` | Data operations, generated models, runtime and package integrity | `sdk_models_compare.py` |
| `administration` | Database administration, migration history, rollback and backfill | `sdk_administration_compare.py` |
| `serialization` | Canonical generated records, archives and lifecycle evidence | `sdk_serialization_compare.py` |
| `artifacts` | C distribution and terminal cross-slice acceptance, binding the other four suites | `sdk_artifacts_compare.py` |

Every suite has `catalog.json`, `journey.json`, `report-schema.json`, and
`profiles.json`. Additional evidence schemas, proof allowlists and workspaces
live with their suite without numeric filename suffixes. Shared scripts live
in `scripts/ci/`; `sdk_profile_matrix.py` owns the suite-to-directory table.
Report formats are `typebridge.sdk-<suite>-report/v1`, and producer variables
use `TYPE_BRIDGE_SDK_<SUITE>_*`.

The former three-binding smoke journey is folded into the four-binding queries
suite. It has no separate report, comparer or output. Queries preserves its
34 selected observations, including the smoke rows, across Python, Node,
Rust and C. Its cancellation, resource, diagnostic and close observations are
query-scoped evidence; broad transitions require the cross-slice artifacts
suite.

`run_sdk_conformance.py` selects a suite by name. Outputs are four binding
reports under `reports/<suite>/<binding>.json` when composed by
`run_full_c_artifact.py`. Artifacts predecessors are passed by suite, for
example `--predecessor queries=reports/queries/python.json`, and must cover
queries, models, administration and serialization for the same binding.

## TypeDB profiles

Each suite's `profiles.json` lists `typedb-3.12.1/v1` on TypeDB 3.12.3 and
`typedb-3.13.6/v1` on TypeDB 3.13.6. The catalog retains its frozen default
identity. Producers select `TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE` (or the
runner's `--semantic-profile`), otherwise that default. A comparison requires
one shared profile or exact server version and validates its matrix entry.

Queries and models pin semantic and per-binding projection fingerprints.
`schema-codegen/tests/sdk_profile_matrix.rs` reproduces every pinned entry
from the fixture source. Administration pins a native history digest per
profile: non-default per-run copies regenerate the same four migrations with
the existing engine. The committed base history remains unchanged, and bridge
replay has separate tests.

## Proofs and support boundaries

Every capability declares `compile_positive`, `compile_negative`,
`direct_runtime`, `remote_runtime`, `diagnostic`, `lifecycle`, and `artifact`
proofs as required or explicitly not applicable. Binding cells are
`accepted_offline`, `accepted_live`, `gap`, or `planned`. Uniform rejection is
not acceptance; a future target remains planned until its required proofs
execute through its public surface.

FULL concerns generated SDK behavior backed by the Rust semantic engine.
Legacy/raw query builders and language-specific conveniences are outside that
contract. The remote contract is query-only. Conformance evidence does not
itself grant release, publication, registry or public-support authority; those
require the separate release contracts and accepted artifacts.
