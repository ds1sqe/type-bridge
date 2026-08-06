# Generated Integration Coverage Inventory

This document records the generated-only application gates used by the current
repository. Split-YAML is the sole schema/model authority. The old handwritten
descriptor and model-manager suites are intentionally not part of this
inventory.

## Parity Authority

`tests/fixtures/generated-only-operation-parity-inventory.json` is the
executable operation inventory. Every accepted row names source anchors in a
clean generated Python, TypeScript/Node, or Rust package. The inventory test
rejects handwritten CRUD/query sources as successor evidence.

`tests/fixtures/handwritten-operation-removal-map.json` records the exact
pre-cutover test identities and their generated successor or retained-contract
disposition. It is historical removal evidence, not an executable compatibility
path.

## Generated Binding Gates

| Binding | Offline package gate | Live application gate |
| --- | --- | --- |
| Python | `crates/schema-codegen/tests/acceptance/check.py`, emitter tests, Pyright positive/negative fixtures | `tests/integration/schema/test_generated_projection_live.py` |
| TypeScript/Node | `crates/schema-codegen/tests/typescript_acceptance/check.mjs`, emitter tests, TypeScript compile fixtures | `crates/node/tests/projection-integration/generated-package-live.test.ts` via `npm run test:projection-integration` |
| Rust | schema-codegen Rust acceptance and an external generated consumer crate | `crates/schema-codegen/tests/rust_projection_live.rs` and its external consumer |

The three live applications cover model construction, scalar and multivalue
ownership, references, entity/relation CRUD, batch operations where advertised,
transactions, exact/subtype hydration, immutable queries, ordering, pages,
aggregates, grouping, reachability, IID predicates, and direct/remote result
materialization. Binding-specific operations such as Python filtered mutations
and hooks are required only in the binding that advertises them.

## Cross-Binding Gate

The CI job identity `cross-language-parity` is retained for branch protection.
It now runs:

- the generated operation inventory and source-removal map tests; and
- schema-codegen's `cross_binding` test over Python, TypeScript, and Rust
  projection output.

Live behavior is executed in each binding's integration job. CI requires all of
those jobs, so a binding cannot pass cross-language parity by being skipped.

## Retained Query and Runtime Coverage

Generated application tests are separate from retained compatibility contracts.
The following remain independently exercised:

- Python root `Query` / `QueryBuilder` and private execution dependencies;
- Node root `TypedQuery` / `TypedGroupByQuery`;
- Rust match/entity/relation/group-by query facades;
- low-level Query V2 request, validation, local execution, remote envelopes,
  hydration, and diagnostics;
- raw database/session/transaction lifecycle needed by generated packages; and
- read-only archive conversion, migration history, checksum, adoption, and
  recovery paths.

These retained tests may use private test-only model identities. They are not
schema-authoring examples and are not imported by generated packages.

## Provider and TLS Coverage

Ordinary live lanes exercise the retained TypeDB 3.11 and 3.12 provider window.
The 3.12.1 lane is the full generated-projection conformance baseline. Dedicated
TLS lanes run generated Python, Node, and Rust applications with a verified
custom root, alongside the retained low-level Query V2 transport probe.

Unsupported older providers are covered only by fail-closed version-window
tests. They are not active compatibility lanes.

## Exact Artifact Coverage

The release workflow prepares generated Python and compiled generated Node
fixtures in producer jobs. The `accept-live-artifact-parity` job then runs
those fixtures with:

- the exact Linux core wheel;
- the exact root Python wheel;
- the exact multi-platform npm tarball;
- the exact generated fixtures; and
- the immutable remote-query smoke server.

That consumer job performs no source generation, native build, or package
repack. It verifies that both generated Python journeys and the generated Node
application execute from the candidate artifacts before publication.

## Entry Points

Use these commands for the generated application surface:

```bash
uv run python type-bridge-core/crates/schema-codegen/tests/acceptance/check.py
node type-bridge-core/crates/schema-codegen/tests/typescript_acceptance/check.mjs
cargo test --manifest-path type-bridge-core/Cargo.toml \
  -p type-bridge-schema-codegen --test cross_binding
npm --prefix type-bridge-core/crates/node run typecheck:projection-integration
```

Live TypeDB coverage is included by `./test.sh`. The isolated Node generated
application can also be run with
`./scripts/run-node-projection-live.sh`.
