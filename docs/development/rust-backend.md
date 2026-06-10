# Rust Backend

The Rust backend is the default ORM execution path. Python Pydantic models
register runtime descriptors with the shared Rust ORM and route manager CRUD,
query, hydration, and transaction operations through PyO3.

The Python ORM backend fallback was removed in #125 Phase 4. If
`TYPE_BRIDGE_BACKEND` is set, only `rust` is accepted.

Current Rust backend scope:

- entity `insert`, `insert_many`, `update`, `update_many`, `put`, `put_many`,
  `get`, `get_by_iid`, `all`, `count`, chainable
  `filter(...).execute()/all()/first()/count()/delete()/update_with()/aggregate()`,
  lookup filters, boolean/string/comparison expressions, ordering, pagination,
  `group_by(...).aggregate()`, IID/key-based `delete`, and `delete_many`;
- relation `insert`, `insert_many`, `update`, `update_many`, `put`, `put_many`,
  `get`, `get_by_iid`, `all`, `count`, chainable
  `filter(...).execute()/all()/first()/count()/delete()/update_with()/aggregate()`,
  relation attribute filters, exact role-player filters, role-player
  expressions, ordering, pagination, `group_by(...).aggregate()`, and delete;
- Rust-backed `Database.transaction("write")` / `"read"` contexts for the
  manager CRUD/query surface;
- Python-owned lifecycle hooks for supported `insert`, `insert_many`, `update`,
  `update_many`, `put`, `put_many`, and delete operations;
- descriptor registration without an active manager or database.

Phase 4 status as of 2026-06-04:

- full Rust backend integration wrapper passes:
  `TYPE_BRIDGE_BACKEND=rust UV_CACHE_DIR=/tmp/uv-cache ./test.sh --no-isolated`;
- unit suite passes with the Rust backend as the selector default;
- `typedb-driver` is no longer a default dependency. It remains in the
  `dev` and `typedb-driver` extras for tests and direct Python driver APIs.

Relation `get`, `get_by_iid`, and `all` hydrate role players from Rust dynamic
relation rows, including repeated players for a role when TypeDB returns
multiple rows for one relation IID.

Rust backend validation should reuse the existing integration tests. The wrapper
defaults to Rust:

```bash
./test.sh --no-isolated
```

Python `TransactionContext` instances select a Rust-owned transaction adapter
by default; raw Python driver transactions are not shared with Rust.
