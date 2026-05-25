# Experimental Rust Backend

`TYPE_BRIDGE_BACKEND=rust` enables the Phase 3 development path where Python
Pydantic models register runtime descriptors with the shared Rust ORM and route
the supported CRUD subset through PyO3.

The default remains:

```bash
TYPE_BRIDGE_BACKEND=python
```

Current Rust backend scope:

- entity `insert`, `insert_many`, `update`, `update_many`, `put`, `put_many`, `get`,
  `get_by_iid`, `all`, `count`, chainable exact-match
  `filter(...).execute()/all()/first()/count()/delete()/update_with()/aggregate()`,
  comparison-expression filters, `limit()` / `offset()` slicing for supported
  filters, exact-match `group_by(...).aggregate()`, IID/key-based `delete`, and
  `delete_many`;
- relation `insert`, `insert_many`, `update`, `update_many`, `put`, `put_many`, `get`,
  `get_by_iid`, `all`, `count`, chainable exact-match
  `filter(...).execute()/all()/first()/count()/delete()/update_with()/aggregate()`,
  exact role-player filters, exact-match `group_by(...).aggregate()`, and
  IID-based `delete`;
- Rust-backed `Database.transaction("write")` / `"read"` contexts for the
  supported manager CRUD subset;
- Python-owned lifecycle hooks for supported `insert`, `insert_many`, `update`,
  `update_many`, `put`, `put_many`, and IID-based `delete` operations;
- descriptor registration without an active manager or database.

Phase 3 hardening status as of 2026-05-23:

- full Python unit suite passes: 1661 tests;
- full ruff check and format pass across the repository;
- full `type_bridge/` and `tests/` pyright checks pass with the local `.venv`
  site-packages path supplied explicitly to pyright;
- Rust workspace `cargo check --workspace`, `cargo test -p type-bridge-core-lib
  -p type-bridge-orm`, and `cargo doc -p type-bridge-core -p type-bridge-orm
  --no-deps` pass;
- existing integration smokes run through the backend selector by setting
  `TYPE_BRIDGE_BACKEND=python` or `TYPE_BRIDGE_BACKEND=rust`.

Relation `get`, `get_by_iid`, and `all` hydrate role players from Rust dynamic
relation rows, including repeated players for a role when TypeDB returns
multiple rows for one relation IID.

Rust backend validation should reuse the existing integration tests with only
the backend selector changed, instead of maintaining a separate backend corpus.
For focused CRUD coverage, run the existing CRUD integration smokes twice:

```bash
TYPE_BRIDGE_BACKEND=python uv run pytest tests/integration/crud/test_typedb_manager.py -m integration
TYPE_BRIDGE_BACKEND=rust uv run pytest tests/integration/crud/test_typedb_manager.py -m integration
```

Unsupported methods fail explicitly in Phase 3. This includes lookup filters,
ordering, boolean expression composition, role-player expression filters,
role-player aggregate filters, and raw Python driver `Transaction` handles. Python
`TransactionContext` instances select a Rust-owned transaction adapter when
`TYPE_BRIDGE_BACKEND=rust`; raw Python driver transactions are not shared with
Rust.
