# type-bridge-contract

Versioned, binding-neutral contract primitives for TypeBridge. This supporting
crate defines canonical identifiers, codecs, diagnostics, fingerprints, schema
facts, query plans, migration values, and generated-projection contracts. It is
intended for TypeBridge engine and binding authors; most applications should
depend on [`type-bridge`](https://crates.io/crates/type-bridge) instead.

## Dependency

```toml
[dependencies]
type-bridge-contract = "2.1.0"
```

Start with the [crate API](https://docs.rs/type-bridge-contract/2.1.0): create
values through their validated constructors and use the canonical codec APIs
when bytes cross a process or persistence boundary. Do not invent a parallel
serialization for these contracts.

The default feature set is empty. `serde-backend-conformance` enables strict
Serde JSON backend checks and is not required by normal consumers.

All first-party TypeBridge Rust crates are released in version lockstep and
require Rust 1.88+. Database-facing TypeBridge 2.1 components support TypeDB
3.11.x–3.12.x, with 3.12.1 as the V2 semantic baseline.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-contract/2.1.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
