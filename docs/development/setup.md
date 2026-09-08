# Development setup

This page supplements the repository-owned
[`DEVELOPMENT.md`](https://github.com/ds1sqe/type-bridge/blob/master/DEVELOPMENT.md).
That file is the canonical product and verification boundary.

## Toolchain

- Python 3.12–3.14; `.python-version` selects 3.13 locally
- `uv`
- Rust 1.88 or newer
- Node 18 or newer; Node 20 is the primary development lane
- CMake 3.20+ plus C17 and C++17 compilers when working on the internal C
  foundation; the native-host gate requires GCC and Clang on Linux, Clang on
  macOS, or MSVC and clang-cl on Windows; Unix C checks also require
  `pkg-config`
- Podman or Docker for the default isolated TypeDB suite

`./scripts/check.sh c` validates only the native host on which it runs. Current
accepted evidence covers that native-host gate; the configured hosted macOS and
Windows lanes remain unverified and are not platform-support evidence.

When validating on Windows, run the internal C check from a Visual Studio
developer shell with both MSVC and clang-cl available, together with the MSVC
linker and binary-inspection tools.

Install the Python, native, and documentation dependencies:

```bash
uv sync --extra dev --extra docs
```

The patched PyO3 0.29 binding supports the declared CPython 3.12–3.14 matrix
directly, preserving abi3-py312 and the GIL-required module contract. No
forward-compatibility override is required. Rust scope checks also require
cargo-audit 0.22.2 and run the shared fresh-database dependency gate.

The Python facade and native core are one release unit and use the same exact
version. The optional Python `typedb-driver` dependency exists for direct-driver
tests and calls; generated managers use the embedded Rust runtime.

## Generated example workspace

The repository examples are generated-only:

```bash
type-bridge --manifest examples/typebridge.yaml schema check
type-bridge --manifest examples/typebridge.yaml schema generate
export PYTHONPATH="$PWD/examples/generated/python${PYTHONPATH:+:$PYTHONPATH}"
uv run python examples/basic/crud.py
```

Applying the example schema to TypeDB is a separate, explicit migration step:

```bash
type-bridge --manifest examples/typebridge.yaml migration make --name initial
type-bridge --manifest examples/typebridge.yaml migration apply --environment development
```

Application examples import model values and managers from the generated
`app_models` package. They never declare schema in Python.

## Focused development checks

Use the smallest relevant command while iterating:

```bash
uv run pytest tests/unit/compat/test_generated_only_python_root.py
cargo test -p type-bridge-schema-codegen
npm run test:unit --prefix type-bridge-core/crates/node
./scripts/check.sh c
```

Before handoff, run the scope-level and full checks described in
[Testing](testing.md):

```bash
uv run ruff check .
uv run ruff format --check .
cargo fmt --all -- --check --manifest-path type-bridge-core/Cargo.toml
./scripts/check.sh all
./test.sh
uv run --extra docs mkdocs build --strict
```

`./test.sh` creates an isolated TypeDB by default. Select a container engine
with `CONTAINER_TOOL=podman` or `CONTAINER_TOOL=docker`. Use `--no-isolated`
only when intentionally targeting an existing server.

## Source boundaries

| Path | Responsibility |
| --- | --- |
| `type_bridge/` | Python connection/query facade and archive recovery readers |
| `type-bridge-core/crates/schema*` | Split-YAML resolution, projection, compatibility, and generation |
| `type-bridge-core/crates/orm/` | Shared generated-projection ORM execution |
| `type-bridge-core/crates/python/` | PyO3 generated-runtime boundary |
| `type-bridge-core/crates/node/` | N-API and public TypeScript runtime boundary |
| `type-bridge-core/crates/rust/` | Public generated Rust client |
| `type-bridge-core/crates/c/` | Private C projected-value and provider-lifecycle ABI foundation under development |
| `tests/contracts/sdk_conformance/manifest-v1.json` | Capability, proof-profile, binding-state, and transition authority |
| `tests/contracts/sdk_conformance/sdk-v*/` | Versioned executable conformance catalogs, journeys, report schemas, and evidence contracts |
| `tests/fixtures/generated-only-operation-parity-inventory.json` | Retained generated-only cutover evidence and manifest seed, not current acceptance authority |

Do not add target-language schema declarations or a facade-local semantic
implementation. Split-YAML is the only active authoring authority, and the Rust
engine owns lowering and validation.

The generated C package and ABI 1.6 currently carry verified flat/chunked
schema/projection evidence, projected values/models, synchronous runtime,
exact-3.12.3 policy-aware database/read/write transaction and cancellation
handles, plus generated nominal exact single-entity and single-relation
CRUD/count, homogeneous atomic mutation batches, and closed typed role-player
unions. The internal ABI also carries generated nominal typed-query, reduction,
schema-function, and caller-owned remote-transport wrappers without duplicating
Rust-owned query semantics. Ordered C-v3 packages compose those entries into
generated nominal field-token manager filters for database and borrowed-read
execution, with no additional native exports. Chunked package resources and a
streaming create builder bound large generated objects to the C11
hosted-implementation portability floors. Exact TypeDB 3.12.3 acceptance
leaves ordered values empty and makes no live list-instance evidence claim. This remains an internal
development target, not a supported SDK or published artifact; multi-endpoint
connection policy and a C distribution contract remain future work.

## Logging and debugging

Python uses standard module logging. Enable the retained connection/query
facade when debugging:

```python
import logging

logging.basicConfig(level=logging.DEBUG)
logging.getLogger("type_bridge").setLevel(logging.DEBUG)
```

For native failures, keep the focused command and add `RUST_BACKTRACE=1`. For
test output, use `uv run pytest -vv -s --log-cli-level=DEBUG`.

The generated Python and Node packages install immutable projection evidence at
import time. When registration fails, compare the generated package version,
declared-schema fingerprint, and runtime version before investigating data
operations.

The internal C probe verifies the same identities when it opens a generated
schema-package descriptor through the Rust-owned ABI.

## Temporary files

Put disposable probes and reports under `tmp/`; it is ignored. Generated
application output belongs at the path declared by a workspace manifest and
must not overlap schema or migration inputs.
