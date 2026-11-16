# type-bridge-core (Python bindings)

PyO3 bindings that expose the **type-bridge** Rust core to Python.

## Overview

This crate wraps [`type-bridge-core-lib`](../core/) and exposes it as a native
Python extension module via [PyO3](https://pyo3.rs) and
[maturin](https://www.maturin.rs).

Applications use generated models and the `type_bridge` query API. This
extension provides native projection, plan execution, diagnostics and canonical
value boundaries. Generated models own schema validation and value conversion.
Legacy validators, scalar formatters and query-node wrappers were removed in 3.0.

## Building

```bash
cd type-bridge-core

# Development build (editable)
maturin develop

# Release build
maturin build --release
```

The patched PyO3 0.29 binding supports CPython 3.12–3.14 directly and retains
the abi3-py312 wheel baseline and GIL requirement.

## Usage from Python

```python
from type_bridge_core import Iid, canonical_enum_spelling

iid = Iid("0x1e00000000000000000001")
assert str(iid) == "0x1e00000000000000000001"
assert canonical_enum_spelling("manager_lookup", "exact") == "eq"
```

## Staged remote execution

Generated query sessions and typed plans use the shared staged runtime for
both direct and proxy execution. Connect through the generated package's
`connect_proxy` or `ExchangeConnection`, then use the same queries and
transaction `stage`/`commit`/pending `result` operations as on a direct database.

The proxy uses `/v3/staged` and `/v3/capabilities`. Authenticate the executor's
advertisement over trusted TLS or pin its exact bytes out of band. Replies are
signed and bound to the request and executor identity before typed results are
constructed. Raw TypeQL uses its separate explicit raw transaction context.

## License

MIT
