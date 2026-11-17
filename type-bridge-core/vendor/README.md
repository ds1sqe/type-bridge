# TypeDB runtime provenance

The native runtime consumes the official crates.io `typedb-driver` and
`typedb-protocol` packages; currently that is 3.14.0, exercised against
TypeDB Server 3.12.3. There is no active, consumed, or release-input TypeBridge
band-9 fork. Historical band-9 trees are forbidden for consumption and are not
release inputs.

| Band | Runtime packages | Upstream source | Registry disposition |
| --- | --- | --- | --- |
| 9 | official `typedb-driver` and `typedb-protocol` 3.14.0 | official crates.io packages | consume upstream directly; never publish a TypeBridge fork |

TypeDB remains the upstream project and original source.

## Licensing boundary

TypeBridge-authored crates and bindings remain MIT. Files derived from the
TypeDB drivers retain Apache-2.0, and files derived from the TypeDB protocols
retain MPL-2.0. Those licenses apply to their covered files; they do not
relicense the separate TypeBridge ORM, runtime, server, Python, or Node source.
