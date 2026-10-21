# Upgrading to 2.3

TypeBridge 2.3 runs every query, direct or through a TypeBridge server, on
one Rust pipeline, and adds staged transactions over typed `query-plan/v3`
programs in Python, TypeScript/Node, Rust and C. Most 2.2 application code
runs unchanged. This page lists what breaks, what is deprecated and what
replaces it, and the behaviour changes you may notice. The
[changelog](../changelog.md) has the complete list.

## Supported TypeDB versions

| Use | TypeDB |
| --- | --- |
| Queries and CRUD | 3.11.0 to 3.13.x; 3.14 and newer are refused |
| Recommended server, default in `docker-compose.yml` and `test.sh` | 3.13.6 |
| Staged plans with input rows (the `given` stage) | 3.12 or newer |
| Migrations on profile `typedb-3.12.1/v1` | exactly 3.12.3 |
| Migrations on profile `typedb-3.13.6/v1` | exactly 3.13.6 |

TypeDB 3.11 support is deprecated in 2.3 and removed in 2.4.0; move 3.11
servers to 3.12 or 3.13 before upgrading to 2.4.

The Python `typedb-driver` extra accepts `>=3.11,<3.14` (`>=3.12.3,<3.14` on
CPython 3.14). Existing workspaces keep the semantic profile they declare.
To move a migration history from 3.12.3 to 3.13.6, move the databases, set
`compatibility.semantic-profile` to `typedb-3.13.6/v1`, and run once per
environment:

```bash
type-bridge --manifest typebridge.yaml migration upgrade-profile \
  --environment production --to typedb-3.13.6/v1
```

See [Move a history to a new TypeDB line](migrations.md#move-a-history-to-a-new-typedb-line).

## Before you upgrade

1. **Upgrade clients and servers together.** A 2.3 server refuses a V1-format
   query envelope on its V2 query route, a 2.3 client sends authored plans
   only to a server that advertises the V2 envelope, and a 2.2 client cannot
   decode the advertisement of a server configured with non-default limits.
2. **Regenerate generated packages.** A package without embedded schema
   authority is refused with `schema_authority_required` (every generator
   since 2.1.0 embeds it). Older Python stubs still run, but a type checker
   reports their descriptor import until they are regenerated.
3. **Surface deprecations in tests**, for example with
   `python -W error::DeprecationWarning -m pytest`, Rust `#![deny(deprecated)]`,
   or the `@deprecated` hints in a TypeScript editor.

## What breaks

### Generated query terminals are asynchronous

Every terminal (`one`, `first`, `rows`, `page_by`, `count_by`, `exists_by`,
`aggregate`) is a coroutine in Python and returns a `Promise` in TypeScript,
direct connections included. Building the query stays synchronous.

```python
# 2.2
rows = session.query(person).rows(limit=50)

# 2.3
rows = await session.query(person).rows(limit=50)

# 2.3, a script without an event loop
from type_bridge_core import blocking

session = blocking(Person.query(db))
rows = session.query(session.exact(Person)).rows(limit=50)
```

### One session type for direct and proxy

`QuerySession` takes the connection first and runs on a database, a
transaction, or a proxy connection. The handwritten `type_bridge.typed`
facade is gone; import the session from your generated package.

```python
# 2.2
from type_bridge.typed import QuerySession
remote = RemoteQuerySession(advertisement, exchange, RemoteQueryLimits(max_items=100))

# 2.3
from app_models import (
    ExchangeConnection,
    QueryExecutionResourceLimits,
    QuerySession,
    connect_proxy,
)

remote = QuerySession(
    ExchangeConnection(advertisement, exchange),
    resources=QueryExecutionResourceLimits(items=100),
)
built_in = QuerySession(connect_proxy("https://typebridge.example"))
```

TypeScript is the same: `new QuerySession(new ExchangeConnection(advertisement,
exchange), { resources })` or `new QuerySession(await connectProxy(url))`.

### Rust `connect` returns a `Connection`

`SchemaPackage::connect` takes a direct or proxy policy and returns
`Connection<S>`. Match `Connection::Direct` to reach CRUD managers:

```rust
// 2.2: let db: Database<AppSchema> = SCHEMA.connect(direct_policy).await?;
let db = match SCHEMA.connect(direct_policy).await? {
    Connection::Direct(db) => db,
    Connection::Proxy(_) => unreachable!("a direct policy opens a direct connection"),
};
```

### Limits and refusals come from the core

- A predicate nested deeper than the session's `QueryLimits` allow is refused
  with `canonical_json_too_deep`, direct and remote alike. Raise the
  session's `query_limits` (`queryLimits`, `with_query_limits`) to run it.
- `QueryExecutionResourceLimits` keeps the values you pass instead of
  clamping them, and refuses a timeout over one day or more than 3
  statements.
- Window, selection, aggregate, group-by and reachability refusals, member
  cardinality checks, and unknown enum spellings now come from the Rust core
  with one structured code in every language. Python raises
  `InvalidQueryRequestError`, `ProjectedCardinalityError` and
  `UnknownEnumValueError`, all still `ValueError`s. TypeScript raises the
  structured error with `category` and `code` instead of `RangeError` or
  `TypeError`; switch on `code`.

### Rust remote hosts use the V2 envelope

Only `typebridge.query-remote-request/v2` is accepted. Code that hosted the
proxy route itself with the V1 or version-dispatching items moves to the
`*_v2` items:

```rust
// 2.2: AdmittedRemoteRequestEnvelope / RemoteRejectionEnvelope
let admitted = preflight_remote_request_versioned(bytes, &context, &advertisement, ceilings)?;
let reply = execute_admitted_remote_request_versioned(admitted, &mut tx, &signer).await;
// 2.3: AdmittedRemoteRequestV2 / RemoteRejectionV2, same arguments
let admitted = preflight_remote_request_v2(bytes, &context, &advertisement, ceilings)?;
let reply = execute_admitted_remote_request_v2(admitted, &mut tx, &signer).await;
```

The changelog's "Breaking changes" section lists every renamed constant,
removed item, and changed refusal code.

## Deprecated and their replacements

Each deprecated name keeps working and warns (Python `DeprecationWarning`,
TypeScript `@deprecated`, Rust `#[deprecated]`). All are removed in 3.0.

| Deprecated | Use instead |
| --- | --- |
| Python `update_with(func)` | An update plan that computes the value with `let_()`, staged in a transaction |
| Python `type_bridge.session.Transaction` | `Database.transaction()` |
| Python and TypeScript `RemoteQuerySession`, `RemoteQueryLimits`, `RemoteQuery`, `RemoteGroupedQuery` | `QuerySession(ExchangeConnection(...))`, `QueryExecutionResourceLimits`, `Query`, `GroupedQuery` |
| Rust `RemoteDatabase`, `RemoteQueryTransport`, `RemoteConnectionOptions`, `RemoteQueryLimits`, `DirectTls` | `ProxyDatabase`, `ProxyExchange`, `ProxyConnectionPolicy`, `QueryExecutionResourceLimits`, `TlsPolicy` |
| Rust `MAX_QUERY_*` constants | `DEFAULT_PROXY_QUERY_*` (same values) |
| Rust `type_bridge_orm::Transaction` | `OwnedTransaction` |
| Rust `require_supported_migration_server` | `require_supported_migration_server_for_profile` |
| Legacy query nodes (`Clause`, `Pattern`, `Statement`, ...) and String compilers (`QueryCompiler::compile`, Python `QueryCompiler.compile`, `compile_batch`, `parse_typeql_query`) | Generated typed query plans; `try_compile` / `try_compile_node` when bridging old nodes |
| Rust String schema-generation helpers | `SchemaInfo::to_typeql` |
| The schema archive reader (`_ArchivedTypeSchema.from_typeql`) | Split-YAML schema authority |

The `update_with` replacement reads and writes in one query, so no other
transaction can change the row between the read and the write:

```python
# 2.2
accounts.filter(tier=Tier("gold")).update_with(add_bonus)

# 2.3
from type_bridge.plan import Plan

from app_models import Account, schema_authority

plan = Plan.builder()
account = plan.exact(Account)
tier = plan.input("tier", "string")
balance = plan.field(account, Account.balance)
plan.where_(plan.field(account, Account.tier).eq(tier))
plan.update(account, Account.balance, plan.let_("next_balance", balance + 10))
plan.row([tier.value("gold")])

with db.staged_transaction() as tx:
    tx.stage(plan.build(schema_authority()))
```

Not deprecated in 2.3: `Database.transaction()`, TypeScript
`RustDatabase.transaction()`, Rust `ReadTransaction` and `WriteTransaction`,
the generated managers, and the C ABI 1.6 functions. The staged transaction
does not carry managers or typed queries yet, so these remain the way to use
them.

## Behaviour changes you may notice

- **Rollback keeps the transaction.** After a successful `rollback()`, the
  same transaction accepts more work and a later commit; `close()` ends it.
  A Python `with` block commits work done after an explicit rollback when it
  exits normally. Rust `WriteTransaction::rollback(self)` and the C ABI 1.6
  rollback still consume the transaction; use `rollback_keep`.
- **TypeDB error text is redacted.** Errors that come from TypeDB show a
  bracketed code and a fixed description instead of the server's message,
  so they never contain query values or credentials. Do not parse messages;
  read the code or the typed `failure`.
- **Commit failures are typed.** Python commit errors are
  `CommitRejected`, `CommitConflict` or `CommitOutcomeUnknown`, all
  `RuntimeError` subclasses, so `except RuntimeError` still catches them.
  Every SDK exposes `failure` with `retry_safe`. When `retry_safe` is true,
  rerun the whole transaction from the start; reconcile an unknown outcome
  before retrying.
  See [Concurrency and conditional writes](concurrency.md).
- **A close failure after a successful commit** is logged, not raised.
- **Generated TypeQL text is formatted differently.** Schema, migration and
  query text is rendered by the official TypeQL AST, so whitespace, stage
  separators and number spelling change. Plan bytes, recorded migrations and
  their checksums keep their bytes. Do not compare rendered TypeQL byte for
  byte.
- **Generated writes run as plans where the package allows it.** Rust single
  writes and every C write run as `query-plan/v3` write plans. Python and
  TypeScript writes (except a TypeScript single `delete`), and Rust
  batches, do so only in packages generated with ordered collections
  (projection handler version 2); a package without them keeps its
  released write path in 2.3, because the handler version pins its
  behaviour. Relation puts, and single inserts, updates and puts on models
  with ordered fields or roles, keep the released path everywhere. The
  next two changes apply only to writes that run as plans.
- **Relation references resolve inside the write.** A polymorphic reference
  that matches several things, or two references of one role that resolve to
  one player, are now refused after the write instead of before it; a
  borrowed transaction becomes rollback-only.
- **Large batches split** into chunks of up to 4,096 rows on the same
  transaction instead of failing; the batch is still all-or-nothing.
- **C direct answer defaults are looser.** A C database, transaction, call
  or batch without a limits descriptor now gets the direct defaults the
  other SDKs use (100,000 items, 64 MiB) instead of 65,536 items. A proxy
  connection gets the wire limits (65,536 items, 32 MiB).

## New in 2.3

- Staged transactions: `Database.staged_transaction()` (Python),
  `RustDatabase.stagedTransaction()` (TypeScript), `Database::transaction`
  (Rust) and `type_bridge_database_transaction` (C ABI 1.7). Nothing reaches
  TypeDB before `commit`, and each staged answer resolves after it.
- A plan builder in all four languages that gives the same canonical bytes
  in each; see the [Query-plan V3 reference](../reference/query-plan-v3.md).
- Staged transactions over a TypeBridge server (`POST /v3/staged`), one
  signed request per commit.
- C ABI 1.7. Existing 1.6 consumers keep working against the 1.7 runtime;
  see [Require ABI 1.7](c.md#require-abi-17).

Language guides: [Python typed queries](typed-queries.md),
[TypeScript](typescript.md), [Rust](rust.md), [C](c.md).
