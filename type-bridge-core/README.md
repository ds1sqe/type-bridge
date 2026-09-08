# TypeBridge shared engine

Rust-owned semantic engine and native product workspace for **TypeBridge**.
It implements schema, query, migration, validation, code generation, ORM, and
provider behavior shared by the Python, TypeScript/Node, generated Rust, CLI,
and server surfaces. An internal generated C ABI 1.4 foundation covers verified
flat and chunked schema packages, projected values/models, policy-aware
synchronous database/transaction lifecycle, nominal exact single-entity and
single-relation CRUD/count, homogeneous atomic mutation batches, and closed
typed role-player unions. It also carries a generated nominal typed-query facade
over the compact Rust-owned generic ABI, schema-function calls, reductions, and
caller-owned remote transport. Chunked package resources and a streaming create
builder keep generated objects within the hosted C11 portability floors. Exact
TypeDB 3.12.1 acceptance deliberately leaves ordered values empty and does not
claim live list-instance evidence; C is not yet a supported SDK.

## Workspace structure

```
type-bridge-core/
├── Cargo.toml          # Workspace root
└── crates/
    ├── contract/, schema/, query/          # canonical V2 contracts and engines
    ├── schema-migration*/                  # offline and TypeDB migration execution
    ├── schema-compat/, schema-codegen/     # compatibility input and projections
    ├── typedb-runtime/, orm/, orm-derive/  # provider bands and ORM
    ├── workspace/, cli/, server/, rust/    # workspace, server, and Rust SDK
    ├── python/, node/, c/                  # private native bindings
    └── core/, migration/, toml-transpiler/ # released engines and converters
```

## Crate groups

The workspace has 20 first-party crates. `contract`, `schema`, `query`, and the
schema-migration crates own the canonical V2 semantics; `schema-compat` and
`schema-codegen` project those semantics into released and generated surfaces;
`typedb-runtime`, `orm`, and `orm-derive` own provider execution; and
`workspace`, `cli`, `server`, `rust`, `python`, and `node` expose the supported
product surfaces. The private `c` crate exposes the internal projected-value,
runtime, exact-3.12.1 database, transaction, cancellation, and exact
single-entity and single-relation CRUD ABI foundation, with generated nominal
wrappers and closed typed role-player unions. ABI 1.4 additionally provides
typed flat/chunked package admission, policy-aware database and transaction
entries, generated atomic mutation batches, and the internal generated
typed-query, reduction, function, and remote-transport foundation.
The released core, migration reader, and TOML converter remain separate
compatibility boundaries.

## Rust publication boundary

The 17 first-party Rust crates are published to crates.io in dependency order
and share the repository release identity (currently `2.2.0`). Supporting
engine crates remain available for integrators, while most Rust applications
should depend on the `type-bridge` SDK. `type-bridge-server` is distributed as
both a Cargo crate and an OCI image. `type-bridge-core`, `type-bridge-node`, and
`type-bridge-c` remain private native binding crates. The C crate is not
published or packaged for users while its support and distribution contracts
remain incomplete.

The release-identity gate requires every public dependency to be present on
crates.io before it publishes each downstream crate in the lockstep graph.
Native Python and Node bindings retain their own distribution surfaces. The C
foundation has no release distribution surface.

## Building

```bash
# Check all crates (requires PYO3 compat flag on Python ≥ 3.14)
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 cargo check --all-targets

# Build the Python extension
cd type-bridge-core
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 maturin develop

# Run tests
cargo test --workspace

# Generate docs
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 cargo doc --no-deps --open
```

## Local CI mirror

Use the project-level check script to mirror CI locally:

```bash
./scripts/check.sh rust      # Rust checks only
./scripts/check.sh python    # Python checks only
./scripts/check.sh node      # Node checks only
./scripts/check.sh c         # Internal C foundation checks
./scripts/check.sh           # All source checks
```

The internal C gate requires CMake 3.20+ and C17/C++17 compilers; Unix runs
also require `pkg-config`.

## License

TypeBridge-authored code is MIT licensed. The native distributions also embed
Apache-2.0 TypeDB driver code, MPL-2.0 TypeDB protocol code, and the
BSD-3-Clause `ed25519-dalek`/`curve25519-dalek` reply-authentication
implementation. Legacy bands 7 and 8 are explicitly disclosed, namespaced
packaging-only packages with
upstream-identical Rust source behavior; their names exist solely for Cargo
multi-band coexistence, and they contain no downstream close patch. The default
band-9 path uses official upstream packages and exact-pins the latest
non-yanked stable 3.12.x driver at release cutoff (currently 3.12.1). See
[`vendor/README.md`](vendor/README.md) and the packaged
`THIRD_PARTY_NOTICES.md` for exact versions, immutable source, and license
texts.
