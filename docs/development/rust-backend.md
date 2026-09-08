# Rust semantic backend

Rust is the only semantic execution path for generated Python, TypeScript/Node,
and Rust applications. Generated packages install canonical projection evidence
and route CRUD, immutable queries, hydration, transactions, migration, and
provider work through the shared engine.

There is no Python or Node ORM fallback. If `TYPE_BRIDGE_BACKEND` is supplied,
only `rust` is accepted.

The backend supports the exact operation set recorded in
`tests/fixtures/generated-only-operation-parity-inventory.json`, including:

- generated entity and relation CRUD, batches, IID/key fallback, and hooks;
- exact field, ownership, role-player, subtype, and scalar validation;
- concise filters and immutable multi-model direct/remote queries;
- rows, pages, count/existence, reducers, and grouping;
- caller-owned transactions and exact generated hydration;
- TypeDB 3.11/3.12 provider selection and fail-closed version checks.

Private dynamic managers and descriptors are execution machinery for verified
generated projections. They are not a public target-language declaration API.

Run focused Rust tests while changing the engine, then generated cross-language
acceptance and the live suite:

```bash
cd type-bridge-core
cargo test -p type-bridge-orm
cargo test -p type-bridge-schema-codegen
cd ..
./test.sh
```

Python `Database` and `TransactionContext` objects wrap Rust-owned handles. Raw
Python-driver transactions are separate and cannot be shared with generated
manager operations.
