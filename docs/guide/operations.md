# Operate and upgrade TypeBridge

Operational choices depend on the execution boundary and TypeDB version, not
on a separate semantic implementation.

## Run

- [Server container](server-container.md) covers immutable images, non-root
  execution, TLS, V2 authority configuration, health checks, and supply-chain
  verification.
- [Logging](logging.md) covers the Python logger hierarchy, query visibility,
  sensitive data, and debugging.
- [TypeDB compatibility](../development/typedb.md) records supported servers,
  driver bands, interpreters, native targets, and feature gates.

## Upgrade

Read [Upgrading to 2.1](upgrade-v2.md) before changing an existing 2.0.x
application. It separates the source-compatible package upgrade from later,
explicit adoption of V2 schema and query surfaces.

The [V2 deprecation inventory](v2-deprecations.md) is the exact removal
contract. A surface absent from its scheduled-removal list is not implicitly
scheduled for removal.

## Security boundaries

Use immutable release identities, provision credentials outside committed
configuration, authenticate remote capability advertisements, and bind V2
execution to the intended canonical schema. Detailed limits and trust inputs
remain in the server, typed-query, migration, and upgrade guides rather than
being duplicated here.
