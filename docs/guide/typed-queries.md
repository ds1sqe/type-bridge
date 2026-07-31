# Immutable Typed Queries

TypeBridge's typed-query facade builds connected, multi-model TypeDB matches
without exposing TypeQL strings or untyped result dictionaries. It is additive:
the existing manager queries and package-root raw `Query` remain available.

Use the dedicated subpath:

```python
from type_bridge.typed import Page, Query, QuerySession
```

For Node, import the equivalent API from `@type-bridge/node/typed`.

## Two V2 Query Surfaces

The model-oriented facade in this guide is the usual application API. It uses
registered `Entity` and `Relation` classes, preserves direct terminal result
types, and hydrates concrete model instances. A separate low-level facade
authors the complete V2 plan vocabulary without model classes:

- Python: `type_bridge.query_v2`
- Node: `@type-bridge/node/query-v2`

Both low-level facades expose exactly `QueryV2Authority`,
`QueryPlanBuilder`, `AuthoredQueryPlan`, and `AuthoredQueryInvocation`.
The authority consumes a canonical declared-schema descriptor snapshot, not
raw TypeQL. Builder transitions are synchronous and perform no provider or
network I/O. Rust owns the binding identities, semantic checks, canonical
bytes, fingerprint, and capability set; Python and TypeScript never assemble
plan JSON.

This Python example authors a typed-input row plan:

```python
from pathlib import Path

from type_bridge.query_v2 import (
    AuthoredQueryInvocation,
    AuthoredQueryPlan,
    QueryPlanBuilder,
    QueryV2Authority,
)

declared_schema_bytes = Path("declared-schema.json").read_bytes()
authority = QueryV2Authority(
    declared_schema_bytes,
    "binding-smoke",
    "typedb-3.12.1/v1",
)
builder = QueryPlanBuilder(authority)
person = builder.binding("person")
name = builder.binding("name")
wanted = builder.input("wanted_name", "string", False)
builder.match(
    (
        builder.isa(person, "entity", "smoke-person", True),
        builder.has(person, name, "smoke-name"),
        builder.value(
            "equal",
            builder.binding_operand(name),
            builder.input_operand(wanted),
        ),
    )
)
plan: AuthoredQueryPlan = builder.finalize_rows((person, name))
invocation: AuthoredQueryInvocation = plan.rows((("Alice",),))
```

The equivalent Node authoring uses the same operation names and order:

```typescript
import { readFileSync } from "node:fs";
import {
  AuthoredQueryInvocation,
  AuthoredQueryPlan,
  QueryPlanBuilder,
  QueryV2Authority,
} from "@type-bridge/node/query-v2";

const declaredSchemaBytes = readFileSync("declared-schema.json");
const authority = new QueryV2Authority(
  declaredSchemaBytes,
  "binding-smoke",
  "typedb-3.12.1/v1",
);
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
const plan: AuthoredQueryPlan = builder.finalizeRows([person, name]);
const invocation: AuthoredQueryInvocation = plan.rows([["Alice"]]);
```

The same builder also covers boolean composition, schema and plan-local
functions, the Rust-owned `count`, `max`, `mean`, `median`, `min`, `std`, and
`sum` reducers, ordered windows, documents, and bounded reachability.
Finalization is terminal. Every rejected mutating transition leaves the
builder at its preceding valid state, so callers may correct that transition
without rebuilding earlier handles.

## Start a Query Session

A query session owns the native descriptor registry and the opaque identities of
every variable it creates. Pass either a connected `Database` or an active read
`TransactionContext`:

```python
session = QuerySession(db)
person = session.var(Person)
employment = session.var(Employment)
company = session.var(Company)
```

Each `var()` call creates a different runtime variable, even when the model is
the same. Matching is exact by default:

```python
exact_person = session.var(Person)
person_or_subtype = session.var(Person, subtypes=True)
```

The first variable matches only concepts whose concrete type is `person`. The
second may materialize a registered concrete subclass such as `Employee`.
Python discovers loaded subclasses recursively. Node applications register
constructors that JavaScript cannot discover before requesting `"subtypes"`:

```typescript
const session = new QuerySession(db).registerModels(Employee, Contractor);
const person = session.var(Person, "subtypes");
```

## Owner-Aware Fields and Roles

Fields are resolved from an attribute class through the model already bound to
the variable. Roles remain owner-aware relation descriptors. Neither surface
accepts public strings, and Rust rejects incompatible owners, players, or
sessions before execution.

```python
person_name = person.field(Name)
company_name = company.field(Name)
employment_code = employment.field(Code)
employee_edge = employment.role(Employment.employee).is_(person)
employer_edge = employment.role(Employment.employer).is_(company)
```

Static analyzers infer `Employment.employee` as
`RoleRef[Person, Employment]` and the bound role as `BoundRole[Person]`. No
consumer cast or type-checker suppression is required.

Use `is_()` when expressing a role-player edge and `eq_field()` when making an
explicit field-to-field comparison. The older `connects()` and overloaded
`eq()` spellings remain compatible aliases.

Predicates are immutable and composable:

```python
alice_or_bob = person_name.eq(Name("Alice")) | person_name.eq(Name("Bob"))
not_archived = ~person_name.starts_with(Name("Archived"))

query = session.query(person).where(alice_or_bob & not_archived)
```

Equality, ordered comparisons, field-to-field comparisons, string operators,
and `AND`/`OR`/`NOT` are checked against descriptor types before execution.

On a negotiated TypeDB 3.12 band-9 connection, supported predicate values are
compiled as deterministic parameters and travel in one `given` row, separately
from the TypeQL statement. Older provider bands execute the same validated plan
with Rust's inline literal renderer, so no userspace API or compatibility branch
is required. Temporal, decimal, and duration values remain safely inlined
because the current row adapter cannot preserve their complete canonical TypeQL
spelling surface. Prefix, suffix, and regex operands also remain inline because
TypeDB requires a literal on the right side of `like`; Rust still owns their
escaping and validation.

`contains` follows TypeDB's Unicode case-folded matching, so
`name.contains(Name("LIC"))` matches `"Alice"`. Prefix, suffix, and regex
matching remain case-sensitive unless the explicit regex requests otherwise.

## Bounded Reachability

`QuerySession.reachable()` adds one directed, bounded graph predicate without
changing the query's inferred result type:

```python
root = session.var(Person)
descendant = session.var(Person, subtypes=True)

within_three = session.reachable(
    root,
    descendant,
    Composition,
    Composition.parent,
    Composition.child,
    min_depth=1,
    max_depth=3,
)

rows: list[Person] = session.query(descendant).match(root).where(
    root.field(Name).eq(Name("Alice")),
    within_three,
).rows(limit=50, order_by=(descendant.field(Name).asc(),))
```

The bounds are inclusive. One positive hop is one instance of the exact
relation type, traversed from the first role to the second. A zero-hop branch
means that the two endpoint variables bind the same TypeDB concept. Endpoint
variables retain their own exact or subtype-inclusive match mode, and inherited
owner-aware role references are accepted for a relation subtype.

Positive paths are finite walks: cycles, repeated vertices, and repeated
relation instances are allowed. The predicate is existential, so multiple
proof paths or multiple admitted depths do not duplicate an otherwise equal
selected identity tuple. They also do not establish an ordering; stable result
order still comes from explicit order terms plus the engine's validated
identity extension.

Malformed, reversed, excessive, or structurally over-budget bounds fail while
the predicate is constructed, before any provider I/O. The implementation
unrolls the finite depth range into one provider query; it never performs a
client-side traversal or one request per hop.

Node exposes the same operation with a bounds object:

```typescript
const withinThree = session.reachable(
  root,
  descendant,
  Composition,
  compositionRefs.roles.parent,
  compositionRefs.roles.child,
  { minDepth: 1, maxDepth: 3 },
);
```

## Select One or Many Models

`query()` accepts 1 through 16 selections. Construction order is result order,
and repeated model types remain distinct:

```python
single: Query[Person] = session.query(person)

pair: Query[Person, Company] = session.query(person, company).match(
    employment
).where(employee_edge, employer_edge)

colleague = session.var(Person)
other_employment = session.var(Employment)

five: Query[Person, Employment, Company, Employment, Person] = session.query(
    person,
    employment,
    company,
    other_employment,
    colleague,
).where(
    employee_edge,
    employer_edge,
    other_employment.role(Employment.employee).is_(colleague),
    other_employment.role(Employment.employer).is_(company),
)
```

`match()` adds hidden witnesses without adding output slots. Hidden witnesses
do not duplicate an otherwise identical selected identity tuple. Every selected
or hidden positive variable must form one connected graph. For an intentional
cross join, grant topology permission explicitly:

```python
independent = session.query(person, company).allow_cross_join(person, company)
```

Every builder returns a new immutable query; its parent and siblings are
unchanged.

## `one()` and Bounded `rows()`

One selected slot is scalar. Two or more slots produce a typed tuple:

```python
alice: Person = single.where(person_name.eq(Name("Alice"))).one()

rows: list[tuple[Person, Company]] = pair.rows(
    limit=25,
    offset=0,
    order_by=(person_name.asc(),),
)
```

`one()` is defined over distinct selected identity tuples. Zero rows raises
error code `no_result`; more than one raises `not_unique`. `rows()` requires a
positive integer limit and a non-negative bounded offset. Rust extends public
ordering with validated unique scalar keys so incomplete results are stable;
nullable, multi-valued, or otherwise non-total ordering fails closed.

## Named and Collected Pages

Python named output uses a frozen dataclass or `NamedTuple` with exact names and
annotations. Collections execute only through a page rooted at the query's one
singular selection:

```python
from dataclasses import dataclass


@dataclass(frozen=True, slots=True)
class PersonWork:
    person: Person
    employments: tuple[Employment, ...]
    companies: tuple[Company, ...]


work: Query[PersonWork] = session.query_as(
    PersonWork,
    person=person,
    employments=employment.collect().order_by(
        employment_code.asc()
    ),
    companies=company.collect().distinct().order_by(
        company_name.asc()
    ),
).where(employee_edge, employer_edge)

page: Page[PersonWork] = work.page_by(
    person,
    limit=20,
    offset=0,
    order_by=(person_name.asc(),),
    include_total=True,
)
```

`page.items` and collected slots are tuples. `Page` is frozen; `total` is an
`int` only when requested, otherwise `None`.

Paging selects distinct root identities before offset and limit, then rebinds
and hydrates exactly those roots in the same transaction snapshot.
`collect()` preserves matching-solution multiplicity. `distinct()` changes only
that collection slot and deduplicates by TypeDB concept identity.

Python typed queries currently reject a relation model whose role player is
another relation while descriptors are being planned, before TypeDB I/O. The
legacy `Role[Relation]` declaration and CRUD surface remain supported; typed
recursive relation hydration will require a separate cycle-safe result
contract. The released TypeScript binding continues to expose its existing
nonrecursive `ShallowRelationInstance` result for this shape.

## Count and Existence

Counts and existence use distinct root identity and do not inherit a row/page
window or ordering:

```python
count: int = work.count_by(person)
exists: bool = work.exists_by(person)
```

## Transaction Ownership

With a `Database`, one terminal owns one read transaction and closes it on every
exit path. With an active caller-owned read context, terminals use that exact
snapshot and never close, commit, roll back, or consume it:

```python
with db.transaction("read") as tx:
    borrowed = QuerySession(tx)
    person = borrowed.var(Person, subtypes=True)
    query = borrowed.query(person)

    first = query.rows(limit=10)
    second = query.count_by(person)  # The same context remains usable.
```

Write, schema, inactive, and already-consumed contexts are rejected.

## Remote Model Queries

`RemoteQuerySession` reuses the complete model grammar above but replaces the
direct terminal with one caller-supplied asynchronous exchange. Composition is
immutable, synchronous, and side-effect free: `var()`, `query()`, `match()`,
`where()`, `allow_cross_join()`, and `reachable()` make no transport call.
Direct terminals remain synchronous and source-compatible; remote terminals
are always awaitable.

This Python fragment is extracted verbatim from the live local/remote parity
test. Its surrounding declared-model fixture supplies `declared`, `SCOPE`,
`PROFILE`, `advertisement`, `exchange`, and `_query`. The overloaded helper
preserves the three-binding result type, so the awaitable needs no `Any`,
cast, or ignore directive:

```python
remote_session = RemoteQuerySession(
    QueryV2Authority(declared, SCOPE, PROFILE),
    advertisement,
    exchange,
    RemoteQueryLimits(
        max_items=10,
        max_bytes=1 << 20,
        max_collection_members=30,
        max_graph_nodes=30,
        max_attribute_values=30,
        max_role_players=30,
        deadline_ms=30_000,
    ),
)
remote_query, remote_employee = _query(remote_session)
assert exchanges == 0
remote_rows = asyncio.run(
    remote_query.rows(
        limit=10,
        order_by=(remote_employee.field(ParityPersonName).asc(),),
    )
)
```

The equivalent fragment is extracted verbatim from the Node live parity test.
Its surrounding fixture supplies the declared models, owner-aware references,
advertisement, and exchange. The awaited terminal is inferred as
`Promise<readonly (readonly [ParityPerson, ParityProject,
ParityAssignment])[]>`; explicit registration lets the base `ParityPerson`
selection hydrate a concrete `ParityEmployee`:

```typescript
const remoteSession = new RemoteQuerySession(
  new QueryV2Authority(declared, SCOPE, PROFILE),
  advertisement,
  postRemote,
  {
    maxItems: 10n,
    maxBytes: 1n << 20n,
    maxCollectionMembers: 30n,
    maxGraphNodes: 30n,
    maxAttributeValues: 30n,
    maxRolePlayers: 30n,
    deadlineMs: 30_000n,
  },
).registerModels(
  ParityEmployee,
  ParityProject,
  ParityAssignment,
);
const remoteEmployee = remoteSession.var(ParityPerson, "subtypes");
const remoteProject = remoteSession.var(ParityProject);
const remoteAssignment = remoteSession.var(ParityAssignment);
const remoteQuery = remoteSession
  .query(remoteEmployee, remoteProject, remoteAssignment)
  .where(
    remoteAssignment
      .role(assignmentRefs.roles.employee)
      .connects(remoteEmployee),
    remoteAssignment
      .role(assignmentRefs.roles.project)
      .connects(remoteProject),
  );
assert.equal(exchanges, 0);
const remoteRows = await remoteQuery.rows({
  limit: 10,
  orderBy: [remoteEmployee.field(personRefs.fields.name).asc()],
});
```

The caller owns capability discovery, authentication, TLS, HTTP or other
transport, application headers, status handling, and retry policy. TypeBridge
does not fetch `/v2/capabilities`, choose credentials, create an HTTP client,
or attach application context. The supplied advertisement is a trust input:
authenticate it over the intended server channel or pin it out of band before
constructing the session.

One awaited terminal prepares one nonce-bound request, calls `exchange`
exactly once, and consumes exactly one returned byte buffer. TypeBridge does
not retry, join, traverse, or issue a second hydration query. A caller may
invoke a new terminal under its own retry policy, which creates a fresh
request. Preparation failures make zero exchange calls; transport failures
are not decoded or retried; replayed, stale, foreign-owner, forged, or
over-budget replies fail before any model constructor runs.

The six response/hydration ceilings and optional deadline are snapshotted when
the session is created and may only be tightened by the remote contract.
Validated remote rows preserve direct ordering and output shape. A
subtype-inclusive base binding materializes its registered concrete subtype
(`ParityEmployee` in the Node example) after the complete authenticated hydration
graph passes validation, without client-side TypeDB I/O.

## Errors and Resource Safety

There are two intentionally distinct structured error contracts:

- Model-query planning, direct execution, and result materialization use
  Python `MatchRequestError` or Node `TypedMatchError`. Their categories are
  `invalid_plan`, `cardinality`, `unsupported_capability`, `stale_schema`,
  `resource_limit`, `provider`, and `result_decode`.
- Low-level plan authoring and the V2 remote envelope use `QueryV2Error`.
  Import it from `type_bridge_core` in Python or from the
  `@type-bridge/node` package root in Node.
  Its categories are `invalid_contract`, `unsupported_capability`,
  `resource_limit`, and `integrity`. Remote authentication, nonce, replay,
  ownership, and request-correlation failures therefore report `integrity`;
  they do not use the model-query `invalid_plan` taxonomy.

Both contracts preserve a stable `category`, `code`, human-readable message,
ordered typed `path`, and deterministically keyed typed `details`. Python
exposes the human-readable field as `.message`. Node `TypedMatchError` uses
`.message`; Node `QueryV2Error` uses `.diagnosticMessage` while its inherited
`.message` remains the conventional `"code: message"` exception text.
Remote authentication and request-correlation failures take precedence over
any untrusted payload diagnostic.

Validation and capability failures that can be known in advance execute no data
statement. Provider, decode, timeout, cancellation, and resource-limit failures
return no partial row or page. The executor bounds processed rows, collection
members, response bytes, statements, and duration while consuming the result.

## Coexisting with Legacy Queries

The model-oriented immutable facade lives under `type_bridge.typed` and
`@type-bridge/node/typed`; complete low-level plan authoring lives under the
separate query-v2 paths listed above. Existing package-root raw-TypeQL builders
and mutable manager queries keep their current mutation, terminal, pagination,
aggregation, update, and delete behavior. Applications can migrate
query-by-query without renaming or converting unrelated APIs.

For the complete cross-language semantics, identity rules, and stable error
table, see the [unified typed-query contract](../development/typed-query-contract.md).
