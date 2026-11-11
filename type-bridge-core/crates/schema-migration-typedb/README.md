# type-bridge-schema-migration-typedb

The TypeDB-backed provider and fenced execution store for canonical TypeBridge
schema migrations. This leaf crate joins provider-neutral migration plans to
the TypeDB runtime, control schema, journal, live observation, and managed
fence. Most applications should invoke these capabilities through the
`type-bridge` CLI. The generic TypeBridge server does not invoke migration
execution.

## Dependency

```toml
[dependencies]
type-bridge-schema-migration-typedb = "3.0.0"
```

Embedders should start with the fully bound `TypeDbMigrationRunner` from the
[crate API](https://docs.rs/type-bridge-schema-migration-typedb/3.0.0). It
constructs one execution binding and reuses it for every provider/store
component it composes.

Lower-level embedders must do the same explicitly:

```rust
# use std::sync::Arc;
# use type_bridge_contract::diagnostic::Diagnostic;
# use type_bridge_orm::Database;
# use type_bridge_schema::ManagedDeltaContext;
# use type_bridge_schema_migration::VerifiedMigrationApplyPlan;
# use type_bridge_schema_migration_typedb::{
#     TypeDbExecutionBinding, TypeDbMigrationProvider, TypeDbMigrationStore,
#     VerifiedMigrationCatalog,
# };
# fn compose<'a>(
#     managed: Arc<Database>,
#     journal: Arc<Database>,
#     context: ManagedDeltaContext,
#     catalog: VerifiedMigrationCatalog<'a>,
#     plan: &VerifiedMigrationApplyPlan,
# ) -> Result<(), Diagnostic> {
let binding = TypeDbExecutionBinding::new(managed, journal, context)?;
let store = TypeDbMigrationStore::new(&binding, catalog)?;
let provider = TypeDbMigrationProvider::new(&binding, plan.schema_authorities())?;
# let _ = (store, provider);
# Ok(())
# }
```

`TypeDbExecutionBinding` validates the exact managed/journal pair, the TypeDB
server line and patch floor the semantic profile requires, and
`ManagedDeltaContext`. The store binds each lease
it issues to this process-local identity; the store and provider reject an
unbound lease or a lease from any independently constructed binding before
opening a transaction. Constructing a second binding from otherwise identical
handles and context deliberately creates a different identity, so reuse or
clone the first binding. This local identity is not serialized and does not
change migration journal wire records.

Catalogs, plans, and transaction-group states from another scope or semantic
profile are also rejected before mutation. Run only verified plans and retain
the active managed fence for the full mutation; never write the control schema
or journal ad hoc.

This crate has no optional features. It is released in lockstep with TypeBridge
3.0.0 and requires Rust 1.88+. Connected migrations run on exactly two
profile/server pairs: `typedb-3.12.1/v1` with TypeDB 3.12.3 and
`typedb-3.13.6/v1` with TypeDB 3.13.6 or 3.14.0.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-schema-migration-typedb/3.0.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
