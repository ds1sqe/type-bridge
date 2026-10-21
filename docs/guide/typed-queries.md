# Immutable generated queries

Each generated package exports a query facade bound to that package's verified
projection. It provides the same supported application outcomes in Python,
TypeScript/Node, and Rust without routing generated models through handwritten
model descriptors.

## Start a session

```python
from app_models import Age, Employment, Person

session = Person.query(db)
person = session.exact(Person)
employment = session.exact(Employment)
```

`Person.query(db)` and the package's `QuerySession(db)` constructor produce the
same package-scoped session. The connection decides where terminals run: a
direct `Database` or `TransactionContext`, or a proxy connection (see
[Proxy connections](#proxy-connections)). Composition is synchronous; every
terminal is a coroutine on every connection, so Python code awaits it.
TypeScript is the same: `new QuerySession(connection)` takes a `RustDatabase`,
a transaction, or a proxy connection, and every terminal returns a `Promise`.

Use `exact(Model)` for exactly one schema type and `subtypes(Model)` for a
polymorphic binding. Concrete result evidence determines the generated class
used for hydration.

## Match fields and roles

```python
adult = person.field(Person.age).gte(Age(18))
employee = employment.role(Employment.employee).connects(person)

query = session.query(person, employment).where(adult, employee)
```

Bindings, field tokens, role tokens, predicates, selections, and queries are
immutable and carry one projection identity. Cross-package composition fails
before execution.

## Low-level Query V2 plans

The generated facade is the usual application API. The separately retained
`@type-bridge/node/query-v2` entry point authors a complete prepared plan when
an application needs the low-level Query V2 vocabulary without model classes.
Rust owns its identities, validation, canonical bytes, and capabilities.

The equivalent Node authoring uses the same operation names and order. Keep the
low-level authority outside this builder function so the plan vocabulary cannot
silently replace the generated package's authority:

```typescript
import {
  QueryPlanBuilder,
  type AuthoredQueryInvocation,
  type QueryV2Authority,
} from "@type-bridge/node/query-v2";

function authorInvocation(authority: QueryV2Authority): AuthoredQueryInvocation {
  const builder = new QueryPlanBuilder(authority);
  const person = builder.binding("person");
  const name = builder.binding("name");
  const wanted = builder.input("wanted_name", "string", false);
  builder.match([
    builder.isa(person, "entity", "smoke-person", true),
    builder.has(person, name, "smoke-name"),
    builder.value(
      "equal",
      builder.bindingOperand(name),
      builder.inputOperand(wanted),
    ),
  ]);
  return builder.finalizeRows([person, name]).rows([["Alice"]]);
}
```

Builder transitions perform no provider or network I/O. Finalization is
terminal, and a rejected transition leaves the builder at its preceding valid
state. Explicit construction of `QueryV2Authority` remains a low-level API for
integrators that already own canonical declared-schema bytes and their trust
policy. `schema export-declared` is retained only as a debug/compatibility tool;
generated sessions do not call it, read its JSON, or use it as an authoring
input.

## Connectivity and explicit cross joins

Every selected binding must be connected by a predicate, match witness, or
bounded reachability path. An intentional Cartesian product must be declared:

```python
left = session.exact(Person)
right = session.exact(Person)

rows = await (
    session.query(left, right)
    .allow_cross_join(left, right)
    .rows(limit=10)
)
```

This prevents a missing join predicate from silently becoming an expensive
cross product.

## Bounded reachability

```python
source = session.exact(Person)
target = session.exact(Person)

path = session.reachable(
    source,
    target,
    NetworkLink,
    NetworkLink.origin,
    NetworkLink.destination,
    min_depth=1,
    max_depth=3,
)

connected = await session.query(source, target).where(path).rows(limit=100)
```

The relation and both role tokens are generated evidence. Depth bounds are
mandatory and validated before execution.

## Selection shapes

One binding produces one model; multiple bindings produce a tuple:

```python
one_person = await session.query(person).one()
pairs = await session.query(person, employment).where(employee).rows(limit=100)
```

Generated Python and TypeScript support positional selections through 16 slots.
Named selections map into an immutable declared row shape. Collection preserves
one root with zero or more related values:

```python
from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class EmploymentRow:
    person: Person
    employments: tuple[Employment, ...]


page = await (
    session.query_as(
        EmploymentRow,
        person=person,
        employments=employment.collect().distinct(),
    )
    .where(employee)
    .page_by(person, limit=25, include_total=True)
)
```

Collections may have their own generated-field ordering. `page_by` keeps root
pagination distinct from collection membership.

## Terminals

```python
await query.one()
await query.first(order_by=(person.field(Person.person_id).asc(),))
await query.rows(limit=100, offset=0)
await query.page_by(person, limit=25, include_total=True)
await query.count_by(person)
await query.exists_by(person)
```

`one` requires exactly one result. `first` returns `None` when empty. Materialized
rows and pages are bounded; unbounded implicit fetches are not part of the
generated contract.

## Direct reductions and grouping

```python
from app_models import aggregate

score = person.field(Person.score)
count, total, mean = await session.query(person).aggregate(
    person,
    aggregate.count(),
    aggregate.sum(score),
    aggregate.mean(score),
)

grouped = await (
    session.query(person, employment)
    .where(employee)
    .group_by(person, employment)
    .aggregate(aggregate.count(), aggregate.max(score))
)
```

Reducer inputs are owner/scalar checked. Grouping returns exact generated group
values plus typed reducer tuples.

## Proxy connections

A proxy connection runs the same session against a TypeBridge server instead of
TypeDB. Only the connection object changes; the query program is the same:

```python
from app_models import ExchangeConnection, Person, QueryExecutionResourceLimits, connect_proxy

# The built-in HTTP route to the server's /v2/capabilities and /v2/query.
session = Person.query(connect_proxy("https://typebridge.example"))

# Or a host-owned transport that routes and authenticates itself.
session = Person.query(
    ExchangeConnection(advertisement_bytes, exchange),
    resources=QueryExecutionResourceLimits(items=100, timeout_milliseconds=30_000),
)
person = session.exact(Person)
rows = await session.query(person).rows(limit=50)
```

Both connections reconstruct Query V2 authority from private, verified package
evidence; no authority file or low-level `QueryV2Authority` argument is
involved. `connect_proxy` fetches the advertisement over the configured TLS
connection, or takes a pinned `advertisement=`; `headers=` carries caller
authentication. `ExchangeConnection(advertisement, exchange)` takes
advertisement bytes and a coroutine that sends one request and returns its
reply; the caller owns transport, credentials, and retry policy. Either takes
`answer_limits=`, the ceiling every session on the connection runs under;
without it a proxy connection uses the proxy defaults (65,536 items,
32 MiB), lower than a direct connection's.

TypeScript passes the same connections to the same `QuerySession`:

```ts
import {
  ExchangeConnection,
  Person,
  QueryExecutionResourceLimits,
  QuerySession,
  connectProxy,
} from "app-models";

const session = new QuerySession(await connectProxy("https://typebridge.example"));

const hosted = new QuerySession(new ExchangeConnection(advertisementBytes, exchange), {
  resources: new QueryExecutionResourceLimits({ items: 100n, timeoutMilliseconds: 30_000n }),
});
const person = hosted.exact(Person);
const rows = await hosted.query(person).rows({ limit: 50n });
```

`connectProxy` takes `advertisement`, `headers`, `tls`, and `answerLimits`
options; `ExchangeConnection(advertisement, exchange, { answerLimits })` takes
a function that sends one request and resolves to its reply. Composition
remains local; one terminal performs one exchange and materializes through
the same package projection as direct execution.

Rust opens either connection from one policy with `SCHEMA.connect(policy)`
([Direct or proxy](rust.md#direct-or-proxy)); C runs each terminal on a proxy
connection with the same arguments as on a database
([Run a query on a database or a TypeBridge server](c.md#run-a-query-on-a-database-or-a-typebridge-server)).

Scripts without an event loop wrap a session in `type_bridge_core.blocking`,
which runs each terminal to completion on a private event-loop thread:

```python
from type_bridge_core import blocking

session = blocking(Person.query(db))
person = session.exact(Person)
rows = session.query(person).rows(limit=50)
```

Supported remote terminals are `one`, `first`, bounded `rows`, `page_by`,
`count_by`, `exists_by`, and typed `aggregate`/`group_by`, including exact/subtype
hydration, predicates, roles, reachability, explicit cross joins, selected
shapes, all reducers, and binding/field/tuple-field grouping. Reduction replies
echo the exact root, grouping, and reducer contract and are authenticated before
materialization through the same projection as direct execution.

### Staged transactions and plans in Python

A plan is one typed `query-plan/v3` pipeline: a match with comparisons and
`let_` values, then updates, inserts, puts and deletes. `Plan.builder()`
(`type_bridge.plan`) takes the generated model, field and role tokens and
forwards every call to the shared Rust builder, so the bytes equal the same
plan built in Rust, TypeScript or C. `where_` and `let_` carry a trailing
underscore because `where` and `let` read as keywords; comparisons are
`eq`, `ne`, `lt`, `le`, `gt` and `ge`, and `+ - * / %` and
`PlanExpr.{pow,abs,ceil,floor,round,min,max,length}` build expressions.
`bool`, `int`, `float`, `str`, `Decimal`, `date` and `datetime` literals are
typed automatically; `literal("duration", "P1D")` types the rest.

```python
from type_bridge.plan import Matched, Plan

from generated.python import Account, Item, Purchase, schema_authority


def purchase(order: str, item_id: str, account_id: str) -> Plan:
    plan = Plan.builder()
    item = plan.exact(Item)
    buyer = plan.exact(Account)
    order_input = plan.input("order_id", "string")
    item_input = plan.input("item_id", "string")
    account_input = plan.input("account_id", "string")
    stock = plan.field(item, Item.stock)
    price = plan.field(item, Item.price)
    balance = plan.field(buyer, Account.balance)
    plan.where_(plan.field(item, Item.item_id).eq(item_input))
    plan.where_(plan.field(buyer, Account.account_id).eq(account_input))
    plan.where_(stock.gt(0)).where_(balance.ge(price))
    plan.update(item, Item.stock, plan.let_("next_stock", stock - 1))
    plan.update(buyer, Account.balance, plan.let_("next_balance", balance - price))
    (
        plan.insert(Purchase)
        .field(Purchase.order_id, order_input)
        .player(Purchase.buyer, buyer)
        .player(Purchase.item, item)
    )
    plan.row([order_input.value(order), item_input.value(item_id), account_input.value(account_id)])
    return plan.build(schema_authority())


with db.staged_transaction() as tx:  # db: type_bridge.Database
    bought = tx.stage(purchase("O1", "I1", "A1"))
assert bought.result() in (Matched(1), Matched(0))
```

Calls never fail on content; `build` raises the first invalid call as a
`QueryV2Error` with `.code` (for example `query_plan_builder_duplicate_name`).
A handle from another builder raises `TypeError` at once. A generated
package's `schema_authority()` is its embedded, verified authority.

`Database.staged_transaction(kind=None, *, commit_on_exit=True)` (and
`RustDatabase.staged_transaction`) returns the core staged transaction:
`stage(plan)`, `commit()`, `rollback()` and `close()`. Nothing opens before `commit()`; an omitted kind
is inferred from the staged plans. `Pending.result()` answers `Rows(rows)`,
`Count(count)`, `Exists(exists)` or `Matched(count)`, raises the commit's
typed failure (`.code`, `.failure`), or raises `RuntimeError` before commit.
A `with` block commits on a clean exit unless `commit_on_exit=False`,
discards on an exception, and always closes.
`Database.transaction()` keeps its released eager context.

### Replacing `update_with`

`update_with(func)` reads every match, runs `func` in Python and writes each
instance back. It is deprecated since 2.3.0, warns with a
`DeprecationWarning`, and is removed in 3.0. Compute the new value in the
plan with `let_` instead, so the read and the write are one query:

```python
from type_bridge.plan import Plan

from generated.python import Account, schema_authority

# Before: accounts.filter(tier=Tier("gold")).update_with(add_bonus)
plan = Plan.builder()
account = plan.exact(Account)
tier = plan.input("tier", "string")
balance = plan.field(account, Account.balance)
plan.where_(plan.field(account, Account.tier).eq(tier))
plan.update(account, Account.balance, plan.let_("next_balance", balance + 10))
plan.row([tier.value("gold")])

with db.staged_transaction() as tx:
    updated = tx.stage(plan.build(schema_authority()))
```

### Staged transactions over the proxy

Writes cross the proxy as staged transactions. `transaction(kind)` on a proxy
connection (Rust `ProxyDatabase::transaction(options)`, Python and TypeScript
`ProxyConnection.transaction(kind)`) returns the core staged transaction: stage
plans (Rust and TypeScript `stage` and Python `stage` of a built `Plan`;
canonical `query-plan/v3` bytes also through Python `stage` and TypeScript
`stagePlan`), then `commit()`. The commit sends the whole stack as one
`typebridge.query-remote-request/v3` envelope to `POST /v3/staged`; the server
runs it on one TypeDB transaction and returns a signed reply that the SDK
verifies before resolving any answer. Answers and typed failures are the same
as direct execution, and `commit_outcome_unknown` stays an unknown outcome.

Each commit is one request under a fresh nonce, and the proxy keeps no
replies. A dropped or unverifiable reply, and a cancelled commit, leave the
outcome unknown (`commit_outcome_unknown`): the writes may have applied, and
the caller reads the database to find out, as after a lost direct commit.
Every other signed refusal keeps the server's code and applied nothing. Raw
TypeQL and typed match reads are not staged over the proxy; they are refused
at stage time.

The SDK verifies staged replies only under the signer it already trusts: the
`/v3/capabilities` advertisement must name the same executor and reply key as
the connection's advertisement, or the transaction fails with
`query_remote_v3_advertisement_mismatch`. It is fetched once per connection,
or pinned out of band (Rust `ProxyConnectionPolicy::staged_advertisement`,
Python `staged_advertisement=`, TypeScript `stagedAdvertisement`).

The server admits a staged write only from a caller an interceptor vouches for
(`on_v3_write_authority`), or from any caller when the operator sets
`[v2] allow_unauthenticated_staged_writes`; otherwise the commit fails with the
typed `staged_write_not_authorized` failure. The [staged remote envelope](../reference/query-plan-v3.md#staged-remote-envelope-v3)
defines the wire format.

## Query limits

A query is held to limits on its size and shape: encoded bytes, nesting depth,
bindings, predicate nodes, and so on. They default to fixed ceilings. A
session can lower them for an untrusted workload or raise them for a query the
defaults refuse, such as a predicate with a literal over 16 MiB:

```python
from app_models import Person, QueryLimits

session = Person.query(
    db,
    query_limits=QueryLimits(canonical_bytes=64 * 1024 * 1024),
)
```

```ts
import { QueryLimits, QuerySession } from "app-models";

const session = new QuerySession(database, {
  queryLimits: new QueryLimits({ canonical_bytes: 64n * 1024n * 1024n }),
});
```

```rust
use type_bridge::QueryLimits;

let session = db
    .query()?
    .with_query_limits(QueryLimits::from_overrides([("canonical_bytes", 64 << 20)])?)?;
```

Proxy sessions take the same option, since a proxy connection is passed to
the same session. C callers use
`<prefix>_query_session_open_with_limits` with an array of named overrides.

Limits are named identically in every language. The engine validates them, so
an unknown name or an unsupported value fails when the limits are built.

Raising a limit locally is not enough for a remote query: the executor
advertises the limits it enforces and a request is held to the stricter of the
two. A refusal names the `limit`, the `maximum` it was held to, and a `source`
of `default`, `configured`, or `advertised`, so it is clear whether changing
the session's limits can help. The
[typed-query contract](../development/typed-query-contract.md#query-limits)
lists every limit with its default.

## Safety contract

- Schema and projection fingerprints are verified at package installation.
- Only exact registered generated classes and tokens are accepted.
- Queries are immutable and package-scoped.
- Disconnected selections fail unless cross joins are explicit.
- Result evidence is revalidated before hydration.
- Remote limits and capability advertisements fail closed.

The normative cross-language behavior is maintained in the
[unified typed-query contract](../development/typed-query-contract.md).
