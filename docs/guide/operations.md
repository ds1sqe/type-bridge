# Operate and upgrade TypeBridge

Operational choices depend on the execution boundary and TypeDB version, not
on a separate semantic implementation.

## Run

- [Server container](server-container.md) covers immutable images, non-root
  execution, TLS, staged authority configuration, health checks, and supply-chain
  verification.
- [Logging](logging.md) covers the Python logger hierarchy, query visibility,
  sensitive data, and debugging.
- [TypeDB compatibility](../development/typedb.md) records supported servers,
  driver bands, interpreters, native targets, and feature gates.

## Upgrade

Read [Upgrading to 3.0](upgrade-3.0.md) before moving a 2.3 application to
3.0. Its checklist lists every 3.0 removal with its replacement. The
historical [2.3](upgrade-2.3.md) and [2.1](upgrade-v2.md) guides cover earlier
upgrades.

## Security boundaries

Use immutable release identities, provision credentials outside committed
configuration, authenticate remote capability advertisements, and bind staged
execution to the intended canonical schema. Detailed limits and trust inputs
remain in the server, typed-query, migration, and upgrade guides rather than
being duplicated here.

Python provider failures expose an immutable `error.failure` dataclass with
`phase`, `reason`, `lock_kind`, `annotation`, `op_index`, `retry_safe` and
`code`. Ordinary commit errors remain catchable as `RuntimeError` and use
`CommitRejected`, its `CommitConflict` subclass, or `CommitOutcomeUnknown`,
exported by `type_bridge`. Existing structured-query exception ancestry stays
unchanged. A rejection permits rerunning the entire transaction on a fresh
snapshot; an indeterminate commit must be reconciled before retrying.
