# type-bridge compatibility packaging notice

This unofficial downstream package preserves the upstream TypeDB protocol
implementation and changes only package metadata/name so TypeDB protocol band
8 can coexist with other bands and pair with the type-bridge compatibility
driver package. The upstream project and original source remain **TypeDB**, from
[`typedb/typedb-protocol`](https://github.com/typedb/typedb-protocol). It is
based exactly on the crates.io `typedb-protocol` 3.11.0 archive:

This source checkout does not authorize registry publication. Any first
publication requires separate explicit TypeBridge owner authorization. If
distributed, this exact source-unmodified package is the authorized
compatibility artifact and must precede the paired driver package.

- Archive: <https://static.crates.io/crates/typedb-protocol/typedb-protocol-3.11.0.crate>
- SHA-256: `f051694ab18c9fb31f15e4567421b55a70e7dddbc1af60a6a1c4cf73ffe8d5e8`
- Upstream license retained: MPL-2.0
- Downstream package/version: `type-bridge-typedb-protocol-b8` 3.11.0

The generated Rust protocol source and `LICENSE` remain byte-identical to the
upstream archive. Only `Cargo.toml` package metadata and this disclosure differ.
The complete original TypeDB README follows below, unchanged. TypeDB is not
responsible for the downstream packaging changes.

---

# TypeDB Driver RPC (Remote Procedure Call) Protocol

A protocol for implementing a TypeDB driver, in many popular programming languages, using [GRPC (Google's Remote Procedure Call)](https://grpc.io) framework.
