# type-bridge

The public Rust client SDK for TypeBridge. Applications consume Rust models and
typed query surfaces generated from a split-YAML workspace, then use this crate
for local TypeDB sessions or authenticated remote V2 query execution. The
generated schema package—not handwritten entity or relation declarations—is
the application authority.

## Dependency

```toml
[dependencies]
type-bridge = "2.1.0"
```

Run `type-bridge schema generate`, import the generated package, and bind its
`SchemaPackage` to a `Database` or `RemoteDatabase`. The
[crate API](https://docs.rs/type-bridge/2.1.0) documents transactions, generated
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

The SDK is released in lockstep with the TypeBridge 2.1.0 crate graph, requires
Rust 1.88+, and supports TypeDB 3.11.x–3.12.x.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge/2.1.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
