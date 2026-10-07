# type-bridge

The public Rust client SDK for TypeBridge. Applications consume Rust models and
typed query surfaces generated from a split-YAML workspace, then use this crate
for direct TypeDB connections or proxy connections through a TypeBridge server. The
generated schema package—not handwritten entity or relation declarations—is
the application authority.

## Dependency

```toml
[dependencies]
type-bridge = "2.2.6"
```

Run `type-bridge schema generate`, import the generated package, and open a
connection with `SCHEMA.connect(policy)`: a `DirectConnectionPolicy` runs
queries against TypeDB, a `ProxyConnectionPolicy` through a TypeBridge
server, and the query code is the same for both. The
[crate API](https://docs.rs/type-bridge/2.2.6) documents transactions, generated
entity/relation managers, typed queries, hooks, and remote transport.
The generated crate embeds its verified authority and never reads the separate
generic-server authority artifact.

## Feature flags

| Feature | Default | Effect |
| --- | --- | --- |
| `typedb` | yes | Enables local TypeDB connections through the shared ORM |
| `band8` | yes | Enables TypeDB 3.11 provider support |
| `band9` | yes | Enables TypeDB 3.12 provider support |
| `test-harness` | no | Enables generated-package acceptance fixtures |

The SDK is released in lockstep with the TypeBridge 2.2.6 crate graph, requires
Rust 1.88+, and supports TypeDB 3.11.x–3.12.x.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge/2.2.6) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
