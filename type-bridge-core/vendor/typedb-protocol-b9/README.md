# Historical quarantined snapshot — do not consume

This directory is retained only as historical audit material. Its manifest is
`publish = false`, it is excluded from the workspace, and it is forbidden for
TypeBridge consumption. The supported band-9 path consumes the official
upstream crates.io packages directly. This checkout is not a current/2.0
release input and must never be republished. The historical immutable registry
keys `type-bridge-typedb-driver-b9@3.12.0` and
`type-bridge-typedb-protocol-b9@3.12.0` already exist and must remain non-yanked
for released 1.5.x compatibility; this release graph must never republish or
yank them.

TypeDB remains the original upstream owner and source. The retained protocol
files remain under MPL-2.0; TypeBridge does not claim ownership of or relicense
them.

---

# TypeDB Driver RPC (Remote Procedure Call) Protocol

A protocol for implementing a TypeDB driver, in many popular programming languages, using [GRPC (Google's Remote Procedure Call)](https://grpc.io) framework.
