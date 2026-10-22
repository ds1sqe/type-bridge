# Testing TypeBridge

Current FULL-SDK acceptance is governed by the machine-readable capability
manifest and versioned sdk contracts. A clean Split-YAML workspace must
generate Python, TypeScript/Node, and Rust packages that perform every accepted
operation for that binding. The old operation inventory is retained cutover
evidence and a manifest seed, not current acceptance authority. Handwritten
schema declarations are not test fixtures for active application behavior.

## Test tiers

### Fast offline tests

```bash
uv run pytest
```

The default marker expression excludes live integration, proxy, and benchmark
tests. It includes Python unit tests plus compatibility, artifact, release,
inventory, and generated-package contract tests.

### Generated-package acceptance

The schema-codegen suite emits fresh packages and checks their target-language
types and runtime behavior:

```bash
cargo test -p type-bridge-schema-codegen
```

Key evidence:

- `crates/schema-codegen/tests/acceptance/` — generated Python type/runtime checks
- `crates/schema-codegen/tests/typescript_acceptance/` — generated TypeScript checks
- `crates/schema-codegen/tests/rust_acceptance.rs` — external generated Rust crate
- `crates/schema-codegen/tests/c_emitter.rs` — deterministic internal C package
  and strict installed-compiler checks
- `tests/contracts/sdk_conformance/manifest.json` — capability,
  proof-profile, binding-state, and transition authority
- `tests/contracts/sdk_conformance/<suite>/` — executable catalogs,
  journeys, report schemas, and evidence contracts
- `tests/fixtures/generated-only-operation-parity-inventory.json` — retained
  generated-only cutover evidence and manifest seed
- `tests/fixtures/handwritten-operation-removal-map.json` — frozen mapping from
  removed test families to generated successors or separately retained query
  contracts

The five suite directories are `queries`, `models`, `administration`,
`serialization`, and `artifacts`. Queries includes the former smoke rows.
Each suite selects a TypeDB profile; administration regenerates a native
four-migration copy for non-default profiles. Suite producers, report formats
and predecessor rules are documented in
`tests/contracts/sdk_conformance/README.md`.

### Node package checks

```bash
cd type-bridge-core/crates/node
npm run build
npm run test:unit
npm run test:dts
npm run smoke:package
```

The package smoke validates the packed tarball, not only the source tree. The
public package must contain generated-runtime, query, connection, and native
surfaces and reject descriptor/model factory payloads.

### Rust workspace checks

```bash
cd type-bridge-core
cargo fmt --all -- --check
cargo test --workspace
```

Generated projection live acceptance additionally exercises a dependency-
isolated consumer crate, preventing success through workspace-private paths.

### Internal C foundation checks

```bash
./scripts/check.sh c
```

This source-only gate emits a generated C package, installs the private runtime
and generated package into an isolated prefix, discovers both with CMake, and
compiles and runs a clean C17 consumer on the current native host. It also
compile-checks the installed public headers as C++17. Its platform branches
fail closed unless GCC/g++ and Clang/clang++ are available on Linux,
Clang/clang++ are available on macOS, or both MSVC and clang-cl are available
on Windows. Current accepted evidence covers the native-host run; the
configured hosted macOS and Windows lanes remain unverified and cannot support
platform claims. Linux additionally runs address/undefined-behavior sanitizers
and checks the Rust-owned C boundary on MSRV 1.88. ABI 1.6 coverage includes
verified flat and chunked schema packages, projected values/models, synchronous
runtime, policy-aware database and distinct read/write transactions,
cancellation, classified commit outcomes, parent/child ownership, generated
nominal exact entity and relation CRUD/count, homogeneous atomic mutation
batches, and closed role-player unions. It also covers bounded generated create
arguments and the native streaming create builder, plus the generated nominal
query, reduction, schema-function, caller-owned remote-transport, and ordered
field-token manager-filter facades. Manager-filter coverage includes immutable
siblings, strict identity-first validation, and reusable borrowed reads without
adding C runtime exports.

The full integration suite runs the ordinary generated C17 CRUD and typed-query
consumer and strict generated C17/C++17 ABI-1.6 successor consumers on the
3.12.3, 3.13.6 and 3.14.0 lanes. A focused four-binding lane compares the same generated
Python, Node, Rust, and C manager-filter observation. The successor lane leaves
ordered attributes and ordered role-player lists empty and therefore does not
claim live list-instance evidence. These checks do not make C a supported SDK
or release artifact.

Run the focused generated manager-filter parity lane with:

```bash
uv run python scripts/ci/run_generated_live.py manager
```

### Live integration

```bash
./test.sh                          # live tier on TypeDB 3.14.0 (default line)
./test.sh --typedb 3.12.3          # live tier on one supported line
./test.sh --typedb all             # every line, then the two-server upgrade lane
./test.sh --typedb all --tls       # the same, with the TLS lane on each line
```

Each live lane creates and removes its own isolated TypeDB and runs under that
line's semantic profile: 3.11.5 with `typedb-3.11.5/v1`, 3.12.3 with
`typedb-3.12.1/v1`, and 3.13.6 and 3.14.0 with `typedb-3.13.6/v1`. The offline tiers run
once. With `--typedb all`, each line is its own compose project
(`tb-<worktree>-typedb-<line>`), and the summary lists every lane.
`--no-integration` runs offline Rust, Python, Node, and internal C-foundation
tiers. `--no-isolated` uses an existing server; its version picks the lane.

| Live step | 3.11.5 | 3.12.3 | 3.13.6 | 3.14.0 |
| --- | --- | --- | --- | --- |
| Rust ORM, production V2 server, generated Rust | yes | yes | yes | yes |
| Python and Node integration, generated projections | yes | yes | yes | yes |
| SDK report fan-in, projected exact-live parity | — | yes | yes | — |
| C runtime and generated C CRUD live | — | yes | yes | — |
| CLI workspace and migration recovery lifecycles | — | yes | yes | yes |
| Unsupported-server migration refusal | yes | — | — | — |
| TLS lane (`--tls`) | — | yes | yes | — |

Steps bound to the SDK conformance contracts run where the lane's profile is in
the SDK version matrix and pairs with the lane's server; the matrix pairs
`typedb-3.13.6/v1` with 3.13.6, so the 3.14.0 lane skips them. Connected migrations refuse TypeDB 3.11, so that lane
proves the refusal instead of the lifecycles. Its TLS lane is skipped too: the
TLS steps assert the band-9 driver and apply a workspace migration.

The upgrade lane starts a 3.12.3 and a 3.13.6 server together and moves a
migration history across a profile bridge
(`live_profile_upgrade` and the CLI `upgrade-profile` test). The tests move
the databases between the servers with the Rust TypeDB driver's database
export and import, so the lane needs no TypeDB console and always runs under
`./test.sh --typedb all`.

On the 3.12.3, 3.13.6 and 3.14.0 lanes, a compiled generated C17 consumer independently
verifies the detected server version and exercises read close, write commit,
rollback, active-write close, and parent-in-use behavior. A separate generated
C17 consumer exercises the exact Person and Membership database/read/write
CRUD paths, immutable typed queries, reductions and grouping, schema-function
calls, caller-owned remote transport, structured diagnostics, cancellation,
limits, and explicit query-resource close; it ends with every created resource
deleted. The ABI-1.6 successor lane compiles one generated consumer as both C17
and C++17, exercises both package-admission forms, policy entries, keyed entity
and relation batches, unkeyed IID lifecycles, and later-row rollback, then
removes its isolated database before evaluating process assertions. These
remain internal foundation evidence, not a public C support or distribution
claim.

Both server bands run the same generated application assertions. The 3.11.5
lane emits from `schema-3.11.5.yaml` and defines `provider-3.11.5.tql`; an
offline guard proves those fixtures differ from the 3.12.1 pair only by the
removal of 3.12-only plays-side documentation annotations.

The generated live journeys cover, where advertised by each binding:

- exact scalar, optional, multivalue, reference, and role-player construction;
- entity and relation insert/put/read/update/delete, batches, and atomicity;
- concise filters, including double-underscore field names and explicit lookup
  disambiguation;
- hooks, key fallback, IID predicates, subtype hydration, and transactions;
- immutable owner-aware field/role queries, rows/pages/count/existence,
  aggregation/grouping, and remote materialization.

## Writing tests after the cutover

For an application operation:

1. Add the schema fact to a Split-YAML fixture.
2. Generate or use immutable generated evidence.
3. Exercise exact generated classes/tokens through the public binding.
4. Update the capability manifest and the applicable sdk catalog,
   journey, report schema, and evidence producers when the accepted contract
   expands; do not treat the retained operation inventory as mutable authority.
5. Run the equivalent bindings that advertise the operation.

Do not subclass private query-engine classes, build runtime descriptors by
hand, or widen generated values to `object`/`Any`. The retained raw query
builders may use private handwritten fixtures only in tests explicitly scoped
to those contracts.

Historical migration and TOML fixtures are read-only evidence. Tests may load,
verify, convert, and adopt them, but must not use them as active authoring
authority or generate new root Python/JSON histories.

## Release and artifact gates

Source tests do not prove publication parity. Release acceptance separately
builds and inspects:

- Python facade wheel/sdist and native wheels;
- the npm tarball and its platform binaries;
- the exact 18-crate Cargo archive set;
- native notices and provider provenance;
- the server container and registry identities.

Hostile artifact tests inject removed modules, provider-band payloads, symlink
escapes, version drift, and missing graph members and require the validators to
reject them.

## Quality and documentation

```bash
uv run ruff check .
uv run ruff format --check .
uv run pyright type_bridge/
uv run pyright tests/
uv run --extra docs mkdocs build --strict
```

When a generated API changes, update the generated acceptance evidence,
declaration baselines, user guide, capability manifest, and applicable
sdk contracts together. Keep the old operation inventory as retained
cutover evidence.
