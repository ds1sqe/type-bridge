# Installation

Install the TypeBridge surface used by your application. Python, Node, Rust,
C, the CLI, and the server are separate distribution identities backed by the same Rust
semantic engine.

## TypeDB versions

Run the newest supported TypeDB, currently 3.14.0. Supported lines are the
measured 3.12–3.14 lines with profile patch floors. General version admission
accepts >=3.12.0. Newer lines are admitted with one warning per connection:
"not tested or supported by this TypeBridge release". They reuse the newest
measured profile and protocol assumptions without a compatibility guarantee.
Verified tags, protocol bands, and migration pairs are in
[TypeDB integration](../development/typedb.md#server-and-driver-compatibility).

| TypeBridge | TypeDB servers | Protocol bands |
| --- | --- | --- |
| 3.0.0 | Admits >=3.12.0; supported lines 3.12–3.14; newer lines warn | 9 |
| 2.3.3 | Admits >=3.11.0; supported lines 3.11–3.14; newer lines warn | 8, 9 (newer lines reuse measured assumptions) |
| 2.3.2 | Admits >=3.11.0; supported lines 3.11–3.14; newer lines warn | 8, 9 (newer lines reuse measured assumptions) |
| 2.3.1 | 3.11–3.14 | 8, 9 (3.14 shares profile `typedb-3.13.6/v1`) |
| 2.3.0 | 3.11–3.13 | 8, 9 |
| 2.1.0–2.2.6 | 3.11–3.12 | 8, 9 (3.8 and 3.10 removed in 2.1.0) |
| 2.0.0–2.0.2 | 3.8–3.12 | 7, 8, 9 (3.8 and 3.10 with a deprecation notice) |
| 1.5.8–1.5.11 | 3.8–3.12 | 7, 8, 9 |
| 1.5.0–1.5.6 | 3.8.0–3.11 | 7, 8 |
| Older releases | See the [changelog](../changelog.md) | |

## Python

Requirements:

- CPython 3.12–3.14
- TypeDB >=3.12.0 is admitted; supported lines are the measured 3.12–3.14
  lines. Connected migrations also enforce the workspace profile floors:
  3.12.3+ on 3.12, or 3.13.6+/3.14.0+ and newer lines on the newest profile.

Here, `+` admits later patches on the same minor line; it does not admit a new minor line.

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

The TypeBridge Rust SDK requires Rust 1.88+ and is distributed through
crates.io:

```toml
[dependencies]
type-bridge = "3"
```

Follow [Rust distribution](../guide/rust.md#distribution) for generated-crate
setup.

## C

TypeBridge 3.0 uses the ABI 2.0 shared runtime. Install the runtime and CLI
from the same product release, then regenerate your C package. C17 and C++17
applications use CMake or pkg-config; application execution does not require
Python or Cargo. The 3.0 release is in preparation.

Follow the [C SDK guide](../guide/c.md) for generation, linking, and the
supported runtime matrix, and [upgrade to 3.0](../guide/upgrade-3.0.md) for
changes from ABI 1.x.

## CLI and code generation

The `type-bridge` command is installed with the Python package:

```bash
pip install type-bridge
type-bridge --help
```

It validates Split-YAML workspaces, creates and applies migrations, and
generates configured Python, TypeScript, Rust, and C projections. Split-YAML
workspace generation is the only active model-generation path. A standalone
CLI archive for Ubuntu 24.04 x86_64 is also published with each release.

## Server container

The standalone staged query server is published separately:

```bash
docker pull ghcr.io/ds1sqe/type-bridge-server:3.0.0
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
