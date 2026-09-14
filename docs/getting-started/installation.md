# Installation

**Security update:** The 2.2.1 prebuilt distributions use a rustls version
affected by [RUSTSEC-2026-0285](https://github.com/rustls/rustls/security/advisories/GHSA-2mjx-qc3c-rqvc).
A corrective release is in preparation; see the
[post-release notice](https://github.com/ds1sqe/type-bridge/releases/tag/v2.2.1).

Install the TypeBridge surface used by your application. Python, Node, Rust,
C, the CLI, and the server are separate distribution identities backed by the same Rust
semantic engine.

## Python

Requirements:

- CPython 3.12–3.14
- TypeDB 3.11–3.12 for generated CRUD/query operations; exactly 3.12.3 for
  connected V2 migration apply/verify/adopt

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

## C

The [2.2.1 GitHub release](https://github.com/ds1sqe/type-bridge/releases/tag/v2.2.1)
provides the ABI 1.6.0 shared runtime, standalone CLI, and generated C example
for Ubuntu 24.04 on x86_64 GNU/Linux. Verify the signed archives before
installation. C17 and C++17 applications use CMake or pkg-config; application
execution does not require Python or Cargo.

Follow the [C SDK guide](../guide/c.md) for archive names, generation, linking,
and the exact supported runtime matrix.

## CLI and code generation

The `type-bridge` command is installed with the Python package:

```bash
pip install type-bridge
type-bridge --help
```

It validates Split-YAML workspaces, creates and applies migrations, and
generates configured Python, TypeScript, Rust, and C projections. Split-YAML
workspace generation is the only active model-generation path. A standalone
CLI archive for Ubuntu 24.04 x86_64 is also available from the 2.2.1 release.

## Server container

The standalone V2 query server is published separately:

```bash
docker pull ghcr.io/ds1sqe/type-bridge-server:2.2.1
```

Production deployments should use the immutable digest recorded in the release
notes. Follow the [server container guide](../guide/server-container.md) for
platforms, non-root execution, TLS, configuration, and supply-chain
verification.

## Install the source tree

```bash
git clone https://github.com/ds1sqe/type-bridge.git
cd type-bridge
uv sync
```

PyO3 0.29 supports the CPython 3.12–3.14 source matrix without a forward-
compatibility override. Published wheels retain the abi3-py312 baseline.

For contributor dependencies and all-language checks, continue with
[Development setup](../development/setup.md).
