# Cargo package index

TypeBridge 2.1 publishes 17 first-party Rust packages as one lockstep product.
Two source-unmodified TypeDB compatibility packages remain public under their
fixed versions. This page is the maintained index for those 19 public Cargo
packages; private Python and Node native-binding crates are deliberately not
part of the crates.io surface.

Most application authors need only [`type-bridge`](#primary-entry-points).
Generated Rust bindings depend on that SDK facade and should not assemble the
supporting layers directly.

## Package layers

```text
generated Rust application packages
                 |
                 v
 type-bridge SDK | type-bridge-cli | type-bridge-server
                 |
                 v
 ORM | query | migration | workspace | schema code generation
                 |
                 v
 schema | schema compatibility | TypeDB runtime
                 |
                 v
 core library | wire contract | retained TypeDB compatibility packages
```

Arrows mean “uses a lower-level responsibility”, not that every package has a
direct Cargo dependency on every package below it. The package manifests remain
the exact dependency authority.

## Primary entry points

| Package | Responsibility | Use it directly when |
| --- | --- | --- |
| [`type-bridge`](https://crates.io/crates/type-bridge/2.1.0) · [rustdoc](https://docs.rs/type-bridge/2.1.0) | Generated Rust SDK runtime and public client facade | Building an application from generated Rust bindings |
| [`type-bridge-cli`](https://crates.io/crates/type-bridge-cli/2.1.0) · [rustdoc](https://docs.rs/type-bridge-cli/2.1.0) | Reusable CLI library plus the thin `type-bridge` binary | Embedding or installing Split-YAML, generation, and migration commands |
| [`type-bridge-server`](https://crates.io/crates/type-bridge-server/2.1.0) · [rustdoc](https://docs.rs/type-bridge-server/2.1.0) | Generic query-server library and production binary | Serving generated V2 query authority over the supported transport |

## Supporting packages

Supporting crates are public so generated packages, integrations, and advanced
Rust consumers can use a stable boundary without depending on private native
bindings. They are not separate products with independent version lines.

| Package | Layer responsibility | Intended direct consumers |
| --- | --- | --- |
| [`type-bridge-contract`](https://crates.io/crates/type-bridge-contract/2.1.0) · [rustdoc](https://docs.rs/type-bridge-contract/2.1.0) | Binding-neutral identifiers, values, diagnostics, and wire contracts | Alternative bindings and protocol tooling |
| [`type-bridge-core-lib`](https://crates.io/crates/type-bridge-core-lib/2.1.0) · [rustdoc](https://docs.rs/type-bridge-core-lib/2.1.0) | Shared compatibility engine retained below public language surfaces | TypeBridge adapters that need the established core implementation |
| [`type-bridge-schema`](https://crates.io/crates/type-bridge-schema/2.1.0) · [rustdoc](https://docs.rs/type-bridge-schema/2.1.0) | Lossless Split-YAML schema documents, normalization, and safety classification | Schema tools that operate before provider execution |
| [`type-bridge-query`](https://crates.io/crates/type-bridge-query/2.1.0) · [rustdoc](https://docs.rs/type-bridge-query/2.1.0) | Immutable provider-neutral query plans and validation | Generated query facades and alternate executors |
| [`type-bridge-schema-migration`](https://crates.io/crates/type-bridge-schema-migration/2.1.0) · [rustdoc](https://docs.rs/type-bridge-schema-migration/2.1.0) | Provider-neutral schema migration manifests, profiles, and lowering | Migration planners and provider adapters |
| [`type-bridge-toml-transpiler`](https://crates.io/crates/type-bridge-toml-transpiler/2.1.0) · [rustdoc](https://docs.rs/type-bridge-toml-transpiler/2.1.0) | TOML recovery-schema to TypeQL transpilation | Recovery and compatibility tooling only |
| [`type-bridge-schema-compat`](https://crates.io/crates/type-bridge-schema-compat/2.1.0) · [rustdoc](https://docs.rs/type-bridge-schema-compat/2.1.0) | One-way compatibility parsers into the V2 schema fact graph | Importers for retained released schema formats |
| [`type-bridge-schema-codegen`](https://crates.io/crates/type-bridge-schema-codegen/2.1.0) · [rustdoc](https://docs.rs/type-bridge-schema-codegen/2.1.0) | Python, TypeScript, and Rust source projection from compiled schema authority | Generators and build integrations |
| [`type-bridge-orm-derive`](https://crates.io/crates/type-bridge-orm-derive/2.1.0) · [rustdoc](https://docs.rs/type-bridge-orm-derive/2.1.0) | Procedural derives for Rust ORM model metadata | Generated Rust model crates through re-exported derives |
| [`type-bridge-typedb-runtime`](https://crates.io/crates/type-bridge-typedb-runtime/2.1.0) · [rustdoc](https://docs.rs/type-bridge-typedb-runtime/2.1.0) | TypeDB driver-band selection and provider execution primitives | TypeDB-backed executors and server integrations |
| [`type-bridge-orm`](https://crates.io/crates/type-bridge-orm/2.1.0) · [rustdoc](https://docs.rs/type-bridge-orm/2.1.0) | Async model CRUD, transactions, hooks, and query execution | Generated Rust models and generated runtime integrations |
| [`type-bridge-migration`](https://crates.io/crates/type-bridge-migration/2.1.0) · [rustdoc](https://docs.rs/type-bridge-migration/2.1.0) | Migration authoring, planning, archives, and execution orchestration | CLI and migration automation |
| [`type-bridge-schema-migration-typedb`](https://crates.io/crates/type-bridge-schema-migration-typedb/2.1.0) · [rustdoc](https://docs.rs/type-bridge-schema-migration-typedb/2.1.0) | TypeDB lowering and execution adapter for provider-neutral schema migrations | TypeDB migration runners |
| [`type-bridge-workspace`](https://crates.io/crates/type-bridge-workspace/2.1.0) · [rustdoc](https://docs.rs/type-bridge-workspace/2.1.0) | Split-YAML workspace discovery, configuration, and generated artifact paths | CLI and workspace-aware build tooling |

## Retained compatibility packages

These packages preserve exact, source-unmodified TypeDB components needed by
the supported compatibility band. They keep their upstream-derived licensing
and fixed versions; they do not follow the TypeBridge 2.1 version line.

| Package | Fixed version | Purpose |
| --- | --- | --- |
| [`type-bridge-typedb-protocol-b8`](https://crates.io/crates/type-bridge-typedb-protocol-b8/3.11.0) · [rustdoc](https://docs.rs/type-bridge-typedb-protocol-b8/3.11.0) | 3.11.0 | Namespaced protocol package for the retained band-8 driver graph |
| [`type-bridge-typedb-driver-b8`](https://crates.io/crates/type-bridge-typedb-driver-b8/3.11.5) · [rustdoc](https://docs.rs/type-bridge-typedb-driver-b8/3.11.5) | 3.11.5 | Namespaced TypeDB driver package for the retained band-8 runtime |

## Release contract

- All 17 first-party packages use Rust 1.88 or newer and release in dependency
  order under one TypeBridge version.
- Every first-party library builds independently with all features and strict
  rustdoc coverage; changing a package's publish metadata cannot remove it from
  the inventory-driven gate.
- The two retained compatibility packages are verified as preexisting,
  immutable inputs rather than republished as TypeBridge-authored code.
- See the [Rust client guide](rust.md) for application usage and the
  [development internals](../development/internals.md) for release validation.
