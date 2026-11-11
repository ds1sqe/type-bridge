# TypeBridge shared engine

Rust-owned semantic engine and native product workspace for **TypeBridge**.
It implements schema, query, migration, validation, code generation, ORM, and
provider behavior shared by the Python, TypeScript/Node, generated Rust, C,
CLI, and server surfaces. The generated C SDK targets the ABI 2.0 shared
runtime on the verified Ubuntu 24.04 x86_64 matrix; see the
[C SDK guide](../docs/guide/c.md).

## Workspace structure

```
type-bridge-core/
├── Cargo.toml          # Workspace root
└── crates/
    ├── contract/, schema/, query/          # canonical contracts and engines
    ├── schema-migration*/                  # offline and TypeDB migration execution
    ├── schema-compat/, schema-codegen/     # compatibility input and projections
    ├── typedb-runtime/, orm/, orm-derive/  # TypeDB runtime and ORM
    ├── workspace/, cli/, server/, rust/    # workspace, server, and Rust SDK
    ├── python/, node/, c/                  # private native bindings
    ├── perf/, perf-stages/                 # private performance tooling
    └── core/                               # shared core library
```

## Crate groups

The workspace has 20 first-party crates. `contract`, `schema`, `query`, and the
schema-migration crates own the canonical semantics; `schema-compat` and
`schema-codegen` project those semantics into released and generated surfaces;
`typedb-runtime`, `orm`, and `orm-derive` own provider execution; and
`workspace`, `cli`, `server`, `rust`, `python`, `node`, and `c` expose the
supported product surfaces. `perf` and `perf-stages` are private tooling.

## Rust publication boundary

The 15 first-party Rust crates are published to crates.io in dependency order
and share the repository release identity (currently `3.0.0`). Supporting
engine crates remain available for integrators, while most Rust applications
should depend on the `type-bridge` SDK. `type-bridge-server` is distributed as
both a Cargo crate and an OCI image. `type-bridge-core`, `type-bridge-node`, and
`type-bridge-c` remain private native binding crates.

The release-identity gate requires every public dependency to be present on
crates.io before it publishes each downstream crate in the lockstep graph.
Native Python and Node bindings retain their own distribution surfaces. The C
runtime ships as signed GitHub release archives.

## Building

```bash
# Check all crates
cargo check --all-targets

# Build the Python extension
cd type-bridge-core
maturin develop

# Run tests
cargo test --workspace

# Generate docs
cargo doc --no-deps --open
```

## Local CI mirror

Use the project-level check script to mirror CI locally:

```bash
./scripts/check.sh rust      # Rust checks only
./scripts/check.sh python    # Python checks only
./scripts/check.sh node      # Node checks only
./scripts/check.sh c         # C SDK and runtime checks
./scripts/check.sh           # All source checks
```

The C gate requires CMake 3.20+ and C17/C++17 compilers; Unix runs also
require `pkg-config`.

## License

TypeBridge-authored code is MIT licensed. The native distributions also embed
Apache-2.0 TypeDB driver code, MPL-2.0 TypeDB protocol code, and the
BSD-3-Clause `ed25519-dalek`/`curve25519-dalek` reply-authentication
implementation. The runtime consumes the official crates.io `typedb-driver`
and `typedb-protocol` packages, exact-pinned (currently 3.14.0). See
[`vendor/README.md`](vendor/README.md) and the packaged
`THIRD_PARTY_NOTICES.md` for exact versions, immutable source, and license
texts.
