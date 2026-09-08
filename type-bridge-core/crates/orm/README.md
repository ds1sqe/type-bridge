# type-bridge-orm

The shared asynchronous execution engine behind generated TypeBridge clients.
It provides database sessions, transactions, lifecycle hooks, prepared V2
query execution, and verified runtime projections. Public models are generated
from split YAML; handwritten model registration is not the application path.
Most applications should depend on [`type-bridge`](https://crates.io/crates/type-bridge).

## Dependency

```toml
[dependencies]
type-bridge-orm = "2.1.0"
```

Direct runtime integrations should begin with `Database` and the secure
connection types in the [crate API](https://docs.rs/type-bridge-orm/2.1.0), then
execute generated projections rather than handwritten descriptors.

## Feature flags

| Feature | Default | Effect |
| --- | --- | --- |
| `typedb` | via bands | Enables the TypeDB runtime |
| `band8` | yes | Enables TypeDB 3.11 provider support |
| `band9` | yes | Enables TypeDB 3.12 provider support |
| `integration-tests` | no | Exposes the repository's live parity test seam |

The crate is released in lockstep with TypeBridge 2.1.0, requires Rust 1.88+,
and supports TypeDB 3.11.x–3.12.x.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-orm/2.1.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
