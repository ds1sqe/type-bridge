# Core engine

Rust is the only semantic execution path for generated Python, TypeScript/Node,
Rust, and C applications. Generated packages install canonical projection evidence
and route CRUD, immutable queries, hydration, transactions, migration, and
provider work through the shared engine.

The engine supports the exact operation set recorded in
`tests/fixtures/generated-only-operation-parity-inventory.json`, including:

- generated entity and relation CRUD, batches, IID/key fallback, and hooks;
- exact field, ownership, role-player, subtype, and scalar validation;
- concise filters and immutable multi-model direct/remote queries;
- rows, pages, count/existence, reducers, and grouping;
- caller-owned transactions and exact generated hydration;
- TypeDB provider selection: a 3.12.0 admission floor, measured 3.12–3.14
  lines, and a warning for newer lines.

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

Python `Database` and `TransactionContext` objects wrap Rust-owned handles. Raw contexts are explicit unchecked provider transactions and cannot be
shared with generated manager operations.

The ORM also provides staged execution through
`Database::transaction(session::staged::TransactionOptions)`. Stage TypeQL
with `session::staged::StagedQuery`, then call `commit()` to execute the
ordered statements in one provider transaction. Pending answers become
readable only after the entire execution succeeds. Rollback discards queued
work; successful commits leave an empty queue ready for reuse. Close ends
the logical transaction. Dropping it never commits.

`Transaction` at the ORM and Rust SDK roots names the staged transaction.
`OwnedTransaction` names the eager owner used by retained internal machinery. Internal eager execution supports
migration assertions and legacy prerequisite reads. Its `internal-eager`
feature is restricted to the ORM and schema migration provider by
`scripts/ci/check_internal_eager.py`. The export is doc-hidden, not isolated
from Cargo feature unification: linking the migration provider can expose
`session::eager` elsewhere in the graph. The probe also rejects references
to that module outside the allowlist.

Owned ORM transactions and shared contexts retain their provider after a
successful rollback. A context reports `RolledBack` until new work resumes;
new work is permitted and can commit. Call close to release it explicitly.
Rollback failures and cancelled rollback futures still make it terminal.
