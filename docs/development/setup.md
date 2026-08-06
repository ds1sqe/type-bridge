# Development setup

This page supplements the repository-owned
[`DEVELOPMENT.md`](https://github.com/ds1sqe/type-bridge/blob/master/DEVELOPMENT.md).
That file is the canonical product and verification boundary.

## Toolchain

- Python 3.12–3.14; `.python-version` selects 3.13 locally
- `uv`
- Rust 1.88 or newer
- Node 18 or newer; Node 20 is the primary development lane
- Podman or Docker for the default isolated TypeDB suite

Install the Python, native, and documentation dependencies:

```bash
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 uv sync --extra dev --extra docs
```

The PyO3 variable is needed only when compiling the current native crate on
CPython 3.14, but is harmless on 3.12–3.13. Published abi3 wheels do not need
it.

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
| `tests/fixtures/generated-only-operation-parity-inventory.json` | Cross-language operation acceptance authority |

Do not add target-language schema declarations or a facade-local semantic
implementation. Split-YAML is the only active authoring authority, and the Rust
engine owns lowering and validation.

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

## Temporary files

Put disposable probes and reports under `tmp/`; it is ignored. Generated
application output belongs at the path declared by a workspace manifest and
must not overlap schema or migration inputs.
