# type-bridge-typedb-runtime

The shared asynchronous TypeDB driver layer for TypeBridge. It owns secure
connection preparation, provider-band negotiation, transaction execution,
bounded answers, and exact value conversion. Most applications should use the
generated client through [`type-bridge`](https://crates.io/crates/type-bridge)
instead of depending on this runtime directly.

## Dependency

```toml
[dependencies]
type-bridge-typedb-runtime = "2.1.0"
```

Start with the secure connection types in the
[crate API](https://docs.rs/type-bridge-typedb-runtime/2.1.0) and keep the
resulting prepared trust material alive for the connection lifetime. Use
bounded answer limits for all provider reads.

## Feature flags

| Feature | Default | Provider |
| --- | --- | --- |
| `band8` | yes | TypeDB driver 3.11.5; supports TypeDB 3.11 and safe discovery of 3.12 |
| `band9` | yes | Official TypeDB driver 3.12.1; supports TypeDB 3.12 |

At least one band must be enabled. The crate is released in lockstep with
TypeBridge 2.1.0, requires Rust 1.88+, and supports TypeDB 3.11.x–3.12.x.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-typedb-runtime/2.1.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
