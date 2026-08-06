# Installation

Install the TypeBridge surface used by your application. Python, Node, Rust,
and the server are separate distribution identities backed by the same Rust
semantic engine.

## Python

Requirements:

- CPython 3.12–3.14
- TypeDB 3.11–3.12 for database operations

```bash
pip install type-bridge
```

Or with [uv](https://docs.astral.sh/uv/):

```bash
uv add type-bridge
```

The wheel includes the native runtime. Install the optional direct TypeDB
driver only if your application calls the driver API itself:

```bash
pip install "type-bridge[typedb-driver]"
```

## TypeScript / Node

The Node 18+ package includes prebuilt native modules for Linux glibc
(x64/arm64), macOS (x64/arm64), and Windows (x64/arm64):

```bash
npm install @type-bridge/node
```

Linux musl and other architectures are not prebuilt. See the
[TypeScript/Node packaging notes](../guide/typescript.md#packaging-note) before
selecting a deployment target.

## Rust

The TypeBridge Rust SDK requires Rust 1.88+. Releases starting with 2.0.1 are
distributed through crates.io:

```toml
[dependencies]
type-bridge = "2"
```

TypeBridge 2.0.0 predates Cargo distribution and remains available from the
exact source/Git revision recorded in its GitHub release.

Follow [Rust distribution](../guide/rust.md#distribution) for generated-crate
setup and the historical 2.0.0 Git declaration.

## CLI and code generation

The `type-bridge` command is installed with the Python package:

```bash
pip install type-bridge
type-bridge --help
```

It validates Split-YAML workspaces, creates and applies migrations, and
generates configured Python, TypeScript, and Rust projections. Split-YAML
workspace generation is the only active model-generation path.

## Server container

The standalone V2 query server is published separately:

```bash
docker pull ghcr.io/ds1sqe/type-bridge-server:2.1.0
```

Production deployments should use the immutable digest recorded in the release
notes. Follow the [server container guide](../guide/server-container.md) for
platforms, non-root execution, TLS, configuration, and supply-chain
verification.

## Install the source tree

```bash
git clone https://github.com/ds1sqe/type-bridge.git
cd type-bridge
PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 uv sync
```

`PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1` enables the current PyO3 release's
forward-compatible abi3 mode for CPython 3.14 source builds. Published wheels
do not require it.

For contributor dependencies and all-language checks, continue with
[Development setup](../development/setup.md).
