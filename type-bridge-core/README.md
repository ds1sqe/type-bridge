# TypeBridge shared engine

Rust-owned semantic engine and native product workspace for **TypeBridge**.
It implements schema, query, migration, validation, code generation, ORM, and
provider behavior shared by the Python, TypeScript/Node, generated Rust, CLI,
and server surfaces.

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
    ├── python/, node/                      # native bindings
    └── core/, migration/, toml-transpiler/ # released engines and converters
```

## Crate groups

The workspace has 19 first-party crates. `contract`, `schema`, `query`, and the
schema-migration crates own the canonical V2 semantics; `schema-compat` and
`schema-codegen` project those semantics into released and generated surfaces;
`typedb-runtime`, `orm`, and `orm-derive` own provider execution; and
`workspace`, `cli`, `server`, `rust`, `python`, and `node` expose the product
surfaces. The released core, migration reader, and TOML converter remain
separate compatibility boundaries.

## Rust publication boundary

The nine V2 implementation crates are workspace-internal: `contract`,
`schema`, `query`, `schema-migration`, `schema-migration-typedb`,
`schema-codegen`, `schema-compat`, `workspace`, and `cli`. Their first-party
versions track the repository release identity (currently `2.0.1`). The
public Rust SDK closure is published to crates.io in dependency order;
`type-bridge-core` and `type-bridge-node` remain native binding crates for the
Python and Node products, respectively, and are not crates.io identities.

The release-identity gate requires every public SDK dependency to be present
on crates.io before it can publish the root crate. It leaves native binding and
OCI server products on their own distribution surfaces.

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
./scripts/check.sh           # Both
```

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
