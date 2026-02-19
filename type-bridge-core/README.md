# type-bridge-core

Rust core for the **type-bridge** TypeDB ORM — high-performance AST, schema parser, query compiler, validation engine, and value coercer.

## Workspace structure

```
type-bridge-core/
├── Cargo.toml          # Workspace root
└── crates/
    ├── core/           # type-bridge-core-lib  (pure Rust, no runtime deps)
    ├── python/         # type-bridge-core      (PyO3 bindings → Python)
    └── server/         # type-bridge-server    (query pipeline + HTTP API)
```

## Crates

| Crate | Description |
|-------|-------------|
| [`type-bridge-core-lib`](crates/core/) | Pure-Rust TypeQL AST, schema parser, query compiler, validation engine, and value coercer |
| [`type-bridge-core`](crates/python/) | PyO3 bindings exposing the Rust core to Python via serde-tagged-enum dicts |
| [`type-bridge-server`](crates/server/) | Transport-agnostic query pipeline with validation, interceptors, and HTTP API |

## Building

```bash
# Check all crates (requires PYO3 compat flag on Python ≥ 3.14)
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 cargo check --all-targets

# Build the Python extension
cd type-bridge-core
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 maturin develop

# Run tests
cargo test -p type-bridge-core-lib -p type-bridge-server

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

MIT
