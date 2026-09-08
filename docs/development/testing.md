# Testing TypeBridge

The cutover acceptance rule is observable operation parity: a clean Split-YAML
workspace must generate Python, TypeScript/Node, and Rust packages that perform
every operation retained for that binding. Handwritten schema declarations are
not test fixtures for active application behavior.

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
- `tests/fixtures/generated-only-operation-parity-inventory.json` — exact
  operation/variant/evidence map
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

### Live integration

```bash
./test.sh
```

The default lane creates and removes an isolated TypeDB. `--no-integration`
runs offline Rust, Python, and Node tiers. `--no-isolated` uses an existing
server. The retained live matrix covers TypeDB 3.11 and 3.12 provider paths;
3.12.1 is the V2 conformance baseline.

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
4. Add the operation and variants to the parity inventory when it expands the
   supported contract.
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
declaration baselines, user guide, and parity inventory together.
