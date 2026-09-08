# TypeDB retained-band compatibility packaging

TypeBridge's default/native band-9 path consumes the official upstream
`typedb-driver` and `typedb-protocol` crates directly. The source manifest is
exact-pinned to the newest non-yanked stable 3.12.x driver selected for the
release; currently that is 3.12.1, exercised against the TypeDB 3.12.1 server
baseline.
There is no active, consumed, or release-input TypeBridge band-9 fork.
The retained `typedb-driver-b9/` and `typedb-protocol-b9/` directories are
historical, `publish = false`, workspace-excluded quarantine snapshots. They
are forbidden for consumption and are not release inputs.

The band-8 trees are unofficial and source-unmodified. The TypeBridge owner
authorized their first publication for
Cargo distribution on 2026-08-03. These exact source-unmodified packages are
the authorized compatibility artifacts, with protocol preceding driver.
TypeBridge does not carry a terminal-close patch or any other behavioral driver
change. The TypeBridge package names exist solely so Cargo can resolve both
retained protocol bands in one native graph. TypeDB remains the upstream project
and original source; TypeDB is not responsible for the downstream package
names or packaging metadata.

| Band | Runtime packages | Upstream source | Registry disposition |
| --- | --- | --- | --- |
| 8 | `type-bridge-typedb-driver-b8` 3.11.5 and `type-bridge-typedb-protocol-b8` 3.11.0 | driver 3.11.5, protocol 3.11.0 | owner-authorized Cargo distribution uses the exact source-unmodified protocol-before-driver packages |
| 9 | official `typedb-driver` 3.12.1 and `typedb-protocol` 3.12.0 | official crates.io packages | consume upstream directly; never publish a TypeBridge fork |

Cargo treats the upstream 3.11 and 3.12 protocol requirements as one
semver-compatible package identity, so they cannot be resolved at different
exact versions in one graph. Renaming the band-8 protocol crate lets both
retained bands coexist without changing generated wire definitions or driver
behavior. Namespacing is a Cargo package-identity mechanism, not a behavioral
fork.

## Packaging-only differences

The `src/` trees and license bodies in every active compatibility package are
byte-identical to their corresponding official upstream archives. Differences
are confined to Cargo packaging metadata needed for the TypeBridge namespace,
same-band dependency aliases, workspace lint/doctest accommodation, and the
band-8 disclosure prepended to the otherwise preserved upstream README.
Terminal transaction close therefore has the exact behavior of the matching
upstream driver release.

## Licensing boundary

TypeBridge-authored crates and bindings remain MIT. Files derived from the
TypeDB drivers retain Apache-2.0, and files derived from the TypeDB protocols
retain MPL-2.0. Those licenses apply to their covered files; they do not
relicense the separate TypeBridge ORM, runtime, server, Python, or Node source.

## Immutable provenance

The compatibility trees originate from these exact upstream crates.io
archives:

```text
typedb-protocol 3.11.0
  https://static.crates.io/crates/typedb-protocol/typedb-protocol-3.11.0.crate
  sha256 f051694ab18c9fb31f15e4567421b55a70e7dddbc1af60a6a1c4cf73ffe8d5e8
  git tag 3.11.0, commit 1db5bdd6579352d31343da28be41844ed07da1b5

typedb-driver 3.11.5
  https://static.crates.io/crates/typedb-driver/typedb-driver-3.11.5.crate
  sha256 71c456fc6fb8f9112236fc088569cbe47f620443629ef8c81b1d79aec7b49fc6
  git tag 3.11.5, commit 7e669e41d9fee22fde8d5e60be7edbf00c6ec64b
```

Registry extraction metadata (`.cargo-ok` and `.cargo_vcs_info.json`) is not
copied into the source tree.

## Audit and refresh

The release identity gate downloads each archive with bounded, fail-closed
I/O, verifies its SHA-256 before reading the tarball, rejects unsafe archive
members, and compares it to the matching directory before any registry
mutation. Driver and protocol source path inventories and source bytes must
exactly match upstream. Only packaging metadata and the documented README
disclosure may differ. Protocol generated source and every retained license
body compare byte-for-byte. Local compatibility trees may contain only
`Cargo.toml`, `README.md`, `LICENSE`, and `src/`.

A compatibility-package refresh starts from a verified upstream archive, reapplies
only the namespaced packaging metadata and disclosure, then proves the source
trees remain upstream-identical. Band 9 changes only the official upstream
exact pin. Before the first immutable release-graph package is published, the
release workflow refuses publication unless that pin is the newest non-yanked
stable 3.12.x release. A retry after that cutoff retains and revalidates the
already-started exact graph even if a newer upstream patch appears. The
selected driver must also pass the TypeDB 3.12.1 conformance lane.
