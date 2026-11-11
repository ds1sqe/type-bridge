# type-bridge-query

Schema-aware validation and lowering for TypeBridge migration assertion plans. It
derives result shapes and rejects plans outside the exact managed schema,
semantic profile, capability set, or structural limits. This supporting crate
is primarily for TypeBridge engines and transports; applications should prefer
the generated query API exposed through
[`type-bridge`](https://crates.io/crates/type-bridge).

## Dependency

```toml
[dependencies]
type-bridge-query = "3.0.0"
```

Begin with [`validate_migration_assertion_plan`](https://docs.rs/type-bridge-query/3.0.0/type_bridge_query/fn.validate_migration_assertion_plan.html)
and carry the returned validated value into lowering or execution. Never send
an unvalidated contract plan directly to a provider. Typed query plans are
validated by native plan construction in `type-bridge-contract`.

This crate has no optional features. It is released in lockstep with TypeBridge
3.0.0 and requires Rust 1.88+. Its semantic contracts target TypeDB
3.12.x–3.14.x, with 3.13.6 as the conformance baseline.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-query/3.0.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
