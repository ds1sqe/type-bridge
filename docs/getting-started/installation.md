# Installation

Install the TypeBridge surface used by your application. Python, Node, Rust,
and the server are separate distribution identities backed by the same Rust
semantic engine.

## Python

Requirements:

- CPython 3.12–3.14
- TypeDB 3.8–3.12 for database operations

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

The TypeBridge 2.0 Rust SDK requires Rust 1.88+ and is distributed from the
exact source/Git revision recorded in the matching GitHub release, not from
crates.io. The application and its generated schema crate resolve the same
immutable revision.

Follow [Rust distribution in 2.0](../guide/rust.md#distribution-in-20) for the
exact dependency and patch declarations.

## CLI and code generation

The `type-bridge` command is installed with the Python package:

```bash
pip install type-bridge
type-bridge --help
```

It validates Split-YAML workspaces, creates and applies migrations, and
generates configured Python, TypeScript, and Rust projections. The retained
single-file generator is also available through Python.

## Server container

The standalone V2 query server is published separately:

```bash
docker pull ghcr.io/ds1sqe/type-bridge-server:2.0.0
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
