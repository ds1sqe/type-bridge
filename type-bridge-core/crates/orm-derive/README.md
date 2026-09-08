# type-bridge-orm-derive

Procedural derives used by generated TypeBridge Rust query packages. The crate
currently provides `#[derive(SelectedRow)]`, which builds a declaration-ordered
named selection constructor. It is a supporting code-generation dependency;
most applications receive it transitively through generated code and
[`type-bridge`](https://crates.io/crates/type-bridge).

## Dependency

```toml
[dependencies]
type-bridge-orm-derive = "2.1.0"
```

Use the derive only on the generated selection shapes described in the
[crate API](https://docs.rs/type-bridge-orm-derive/2.1.0). Handwritten models
are not a TypeBridge schema-authoring path.

This crate has no optional features. It is released in lockstep with TypeBridge
2.1.0 and requires Rust 1.88+. Generated clients target the TypeBridge 2.1
TypeDB range of 3.11.x–3.12.x.

[Repository](https://github.com/ds1sqe/type-bridge) ·
[API documentation](https://docs.rs/type-bridge-orm-derive/2.1.0) ·
[MIT license](https://github.com/ds1sqe/type-bridge/blob/master/LICENSE)
