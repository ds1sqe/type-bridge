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
- `tests/contracts/sdk_conformance/manifest-v1.json` — capability,
  proof-profile, binding-state, and transition authority
- `tests/contracts/sdk_conformance/sdk-v*/` — executable catalogs,
  journeys, report schemas, and evidence contracts
- `tests/fixtures/generated-only-operation-parity-inventory.json` — retained
  generated-only cutover evidence and manifest seed
- `tests/fixtures/handwritten-operation-removal-map.json` — frozen mapping from
  removed test families to generated successors or separately retained query
  contracts

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
consumer and strict generated C17/C++17 ABI-1.6 successor consumers against
exact TypeDB 3.12.3. A focused four-binding lane compares the same generated
Python, Node, Rust, and C manager-filter observation. The successor lane leaves
ordered attributes and ordered role-player lists empty and therefore does not
claim live list-instance evidence. These checks do not make C a supported SDK
or release artifact.

Run the focused generated manager-filter parity lane with:

```bash
uv run python scripts/ci/run_manager_filter_live.py
```

### Live integration

```bash
./test.sh
```

The default lane creates and removes an isolated TypeDB. `--no-integration`
runs offline Rust, Python, Node, and internal C-foundation tiers.
`--no-isolated` uses an existing server. The retained live matrix covers TypeDB
3.11 and 3.12 provider paths; 3.12.3 is the V2 conformance baseline.

On the exact 3.12.3 lane, a compiled generated C17 consumer independently
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
hand, or widen generated values to `object`/`Any`. The retained V1/raw query
facades may use private handwritten fixtures only in tests explicitly scoped to
those unscheduled contracts.

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
