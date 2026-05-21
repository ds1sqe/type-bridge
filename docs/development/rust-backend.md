# Experimental Rust Backend

`TYPE_BRIDGE_BACKEND=rust` enables the Phase 2 development path where Python
Pydantic models register runtime descriptors with the shared Rust ORM and route
the supported CRUD subset through PyO3.

The default remains:

```bash
TYPE_BRIDGE_BACKEND=python
```

Phase 2 Rust backend scope:

- entity `insert`, `get`, `all`, `count`, and IID-based `delete`;
- relation `insert`, `count`, and IID-based `delete`;
- descriptor registration without an active manager or database.

Unsupported methods fail explicitly in Phase 2. This includes update, put,
insert-many, hooks, chainable query APIs, relation hydration, group-by, and
transaction-context parity.
