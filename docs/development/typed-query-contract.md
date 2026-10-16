# Generated Typed Query Contract

The additive read/write [query-plan V3 wire specification](../reference/query-plan-v3.md)
defines canonical bytes, expressions, write stages, operations and shared examples.
V1/V2 encodings and fingerprints described here remain frozen.

Status: implemented normative semantics for
[#170](https://github.com/ds1sqe/type-bridge/issues/170) through
[#177](https://github.com/ds1sqe/type-bridge/issues/177), consumed through
generated packages after the #189 cutover.

!!! note "Generated-package imports"

    `Query`, `QuerySession`, field tokens, role tokens, and model classes are
    emitted together inside each generated Python or TypeScript package. The
    base SDK supplies connections, the verified runtime projection, and
    low-level Query V2; it does not accept handwritten descriptors as schema
    authority.

This document fixes the public behavior shared by the Rust request, Python and
TypeScript facades, and TypeDB executor. When implementation details
conflict with this document, this contract wins unless a later accepted design
explicitly revises it.

## Public Surface and Compatibility Boundary

There is one new immutable public `Query`, whether it selects one model or many.
There are no separate public model-query, match-query, or fetch-query classes.

| Language | Generated-query import | Required connection |
| --- | --- | --- |
| Python | `from your_generated_package import Query, QuerySession` | `Database | TransactionContext` |
| TypeScript | `import { Query, QuerySession } from "./your-generated-package/index.js"` | `RustDatabase | RustTransactionContext` |

Python's package-root `Query` and `QueryBuilder` remain the mutable raw-TypeQL
builders. TypeScript's package-root `TypedQuery<T, Row>` remains the separately
unscheduled mutable compatibility query. Generated model managers provide the
concise single-type CRUD/query path; generated `QuerySession` provides immutable
multi-binding queries and is Python's only typed query API; the handwritten
`type_bridge.typed` facade was removed in 2.3.0. None of these retained
behaviors reopens model or schema declaration in the base SDK.

`QuerySession.var(Model)` returns one `BoundVar[Model]`. Two calls for the same
model have the same static type and different opaque runtime identities. New
bindings match their declared type exactly by default. Subtype-inclusive
matching must be requested explicitly on that binding.

`QuerySession.query(selection, ...)` requires between 1 and 16 selections. The
arguments declare the positional output in construction order. Selecting the
same binding handle twice is invalid; selecting two different handles for the
same model is valid. A seventeenth selection is rejected statically and by the
Rust validator. `Query.match(binding, ...)` adds hidden predicate witnesses
without changing the selected result type.

Every builder call returns a new query handle. Its base and sibling handles stay
unchanged. Python and TypeScript carry only language typing plus opaque Rust
handles; they do not hold a mirrored semantic plan or raw TypeQL.

## Complete Python Example

<!-- typed-query-example: python-models-and-session -->
```python
from dataclasses import dataclass
from typing import assert_type

from generated_v2 import (
    Container,
    Employment,
    Event,
    Identifier,
    Page,
    Person,
    Query,
    Score,
)

from type_bridge import Database

db = Database(address="localhost:1729", database="typed_query_example")
db.connect()
session = Person.query(db)
person = session.exact(Person)
event = session.exact(Event)
container = session.exact(Container)
employment = session.exact(Employment)

employee = employment.role(Employment.employee).connects(person)
subject = event.role(Event.subject).connects(person)
contained = container.role(Container.item).connects(event)
```

`generated_v2` is the checked fixture package in this repository. Applications
use the package name emitted for their own Split-YAML workspace; they never
write the classes shown above themselves.

The scalar, tuple, five-slot, and repeated-model forms retain their exact static
shapes. The five-slot graph below is connected through one container. The
repeated two-person output uses hidden bindings to connect its selected
variables.

<!-- typed-query-example: python-selection-arities -->
```python
one_person: Query[Person] = session.query(person)
two_slots: Query[Person, Event] = session.query(person, event).where(subject)

colleague = session.exact(Person)
other_event = session.exact(Event)
five_slots: Query[Person, Event, Container, Event, Person] = session.query(
    person,
    event,
    container,
    other_event,
    colleague,
).where(
    subject,
    contained,
    other_event.role(Event.subject).connects(colleague),
    container.role(Container.item).connects(other_event),
)

person_pair: Query[Person, Person] = (
    session.query(person, colleague)
    .match(event, other_event, container)
    .where(
        subject,
        contained,
        other_event.role(Event.subject).connects(colleague),
        container.role(Container.item).connects(other_event),
    )
)


# Terminals are coroutines on every connection; composition is synchronous.
async def selection_shapes() -> None:
    assert_type(await one_person.one(), Person)
    assert_type(await one_person.rows(limit=20), list[Person])
    assert_type(await two_slots.one(), tuple[Person, Event])
    assert_type(await two_slots.rows(limit=20), list[tuple[Person, Event]])
    assert_type(
        await five_slots.rows(limit=20),
        list[tuple[Person, Event, Container, Event, Person]],
    )
    assert_type(await person_pair.rows(limit=20), list[tuple[Person, Person]])


# Generated QuerySession.query has checked overloads through 16 selections. A
# seventeenth selection is a static diagnostic and Rust rejects forged input.
```

Fields and roles are bound through their owning variables. Python string
helpers take the corresponding typed string attribute while TypeScript helpers
take a string; only `regex` interprets that value as a regex. A disconnected
graph is invalid unless topology-level cross-join permission names its
components.

<!-- typed-query-example: python-topology-and-strings -->
```python
adults = one_person.where(
    person.field(Person.score).gte(Score(18)),
    person.field(Person.identifier).starts_with(Identifier("Al")),
    person.field(Person.identifier).contains(Identifier("Research")),
    person.field(Person.identifier).ends_with(Identifier("Labs")),
    person.field(Person.identifier).regex(Identifier(r"^A[[:alpha:]]+$")),
)

# Percent is literal input here; it has no SQL wildcard meaning.
literal_percent = one_person.where(person.field(Person.identifier).contains(Identifier("50%")))

# Cross joins are explicit topology permission, never a boolean predicate.
independent_pairs: Query[Person, Container] = session.query(person, container).allow_cross_join(
    person, container
)
```

Operation order is explicit at the terminal. `rows` returns distinct selected
identity tuples. `page_by`, `count_by`, and `exists_by` use distinct root
identity. Counts and existence do not inherit row/page order or windows.

<!-- typed-query-example: python-terminals-and-collections -->
```python
@dataclass(frozen=True, slots=True)
class PersonEvents:
    person: Person
    events: tuple[Event, ...]


work: Query[PersonEvents] = session.query_as(
    PersonEvents,
    person=person,
    events=event.collect().distinct(),
).where(subject)


async def terminals() -> None:
    ordered_people = await one_person.rows(
        limit=50,
        offset=0,
        order_by=(person.field(Person.identifier).asc(),),
    )
    assert_type(ordered_people, list[Person])

    person_count: int = await two_slots.count_by(person)
    any_person: bool = await two_slots.exists_by(person)
    assert_type((person_count, any_person), tuple[int, bool])

    page: Page[PersonEvents] = await work.page_by(
        person,
        limit=50,
        offset=0,
        order_by=(person.field(Person.identifier).asc(),),
        include_total=True,
    )
    assert_type(page.items, tuple[PersonEvents, ...])
    assert_type(page.total, int | None)
```

A database-backed terminal owns its read transaction. A caller-owned read
context remains usable after a terminal; the query neither commits, rolls back,
nor closes it.

<!-- typed-query-example: python-transaction-ownership -->
```python
async def ownership() -> None:
    # Owned: rows() opens one read transaction and closes it on success or error.
    owned = Person.query(db)
    owned_person = owned.exact(Person)
    assert_type(await owned.query(owned_person).rows(limit=10), list[Person])

    # Borrowed: the surrounding context owns lifecycle and may run more work.
    with db.transaction("read") as tx:
        borrowed = Person.query(tx)
        borrowed_person = borrowed.exact(Person)
        first_page = await borrowed.query(borrowed_person).rows(limit=10)
        second_page = await borrowed.query(borrowed_person).rows(limit=10, offset=10)
        assert_type(first_page + second_page, list[Person])
```

## Complete TypeScript Example

<!-- typed-query-example: typescript-models-and-session -->
```typescript
import {
  Container,
  Employment,
  Event,
  Identifier,
  Person,
  QuerySession,
  Score,
  type Page,
  type Query,
} from "./generated_v2/src/index.js";
import {
  RustDatabase,
  type RustTransactionContext,
} from "@type-bridge/node";

const db = RustDatabase.connect("localhost:1729", "typed_query_example");
const session = new QuerySession(db);
const person = session.exact(Person);
const event = session.exact(Event);
const container = session.exact(Container);
const employment = session.exact(Employment);

const employee = employment.role(Employment.employee).connects(person);
const subject = event.role(Event.subject).connects(person);
const contained = container.role(Container.item).connects(event);
```

TypeScript scalarizes a one-slot generated query and preserves a readonly tuple
for two or more slots.

<!-- typed-query-example: typescript-selection-arities -->
```typescript
const onePerson: Query<Person> = session.query(person);
const scalar: Person = await onePerson.one();
const scalarRows: readonly Person[] = await onePerson.rows({ limit: 20n });

const twoSlots: Query<readonly [Person, Event]> = session
  .query(person, event)
  .where(subject);
const pairRows: readonly (readonly [Person, Event])[] = await twoSlots.rows({
  limit: 20n,
});

const colleague = session.exact(Person);
const otherEvent = session.exact(Event);
const fiveSlots: Query<
  readonly [Person, Event, Container, Event, Person]
> = session
  .query(person, event, container, otherEvent, colleague)
  .where(
    subject,
    contained,
    otherEvent.role(Event.subject).connects(colleague),
    container.role(Container.item).connects(otherEvent),
  );

const personPair: Query<readonly [Person, Person]> = session
  .query(person, colleague)
  .match(event, otherEvent, container)
  .where(
    subject,
    contained,
    otherEvent.role(Event.subject).connects(colleague),
    container.role(Container.item).connects(otherEvent),
  );

void scalar;
void scalarRows;
void pairRows;
void fiveSlots;
void personPair;
// Generated query() accepts 1..16 selections. Seventeen is a tsc and Rust
// diagnostic.
```

<!-- typed-query-example: typescript-topology-and-strings -->
```typescript
const adults = onePerson.where(
  person.field(Person.score).gte(Score.create(18n)),
  person.field(Person.identifier).startsWith(Identifier.create("Al")),
  person.field(Person.identifier).contains(Identifier.create("Research")),
  person.field(Person.identifier).endsWith(Identifier.create("Labs")),
  person.field(Person.identifier).regex(Identifier.create(String.raw`^A[[:alpha:]]+$`)),
);

const literalPercent = onePerson.where(
  person.field(Person.identifier).contains(Identifier.create("50%")),
);

const independentPairs: Query<readonly [Person, Container]> = session
  .query(person, container)
  .allowCrossJoin(person, container);

void adults;
void literalPercent;
void independentPairs;
```

<!-- typed-query-example: typescript-terminals-and-collections -->
```typescript
const orderedPeople: readonly Person[] = await onePerson.rows({
  limit: 50n,
  offset: 0n,
  orderBy: [person.field(Person.identifier).asc()],
});

const personCount: bigint = await twoSlots.countBy(person);
const anyPerson: boolean = await twoSlots.existsBy(person);

const work: Query<Readonly<{
  person: Person;
  events: readonly Event[];
}>> = session.queryNamed({
  person,
  events: event.collect().distinct(),
}).where(subject);

const page: Page<Readonly<{
  person: Person;
  events: readonly Event[];
}>> = await work.pageBy(person, {
  limit: 50n,
  offset: 0n,
  orderBy: [person.field(Person.identifier).asc()],
  includeTotal: true,
});

void orderedPeople;
void personCount;
void anyPerson;
void page;
```

<!-- typed-query-example: typescript-transaction-ownership -->
```typescript
// Owned: rows() opens and closes one read transaction on every exit path.
const ownedSession = new QuerySession(db);
const ownedPerson = ownedSession.exact(Person);
const ownedRows = await ownedSession.query(ownedPerson).rows({ limit: 10n });

// Borrowed: only the caller closes the context, which remains reusable.
const tx: RustTransactionContext = db.transaction("read");
try {
  const borrowed = new QuerySession(tx);
  const borrowedPerson = borrowed.exact(Person);
  const firstPage = await borrowed.query(borrowedPerson).rows({ limit: 10n });
  const secondPage = await borrowed.query(borrowedPerson).rows({
    limit: 10n,
    offset: 10n,
  });
  void firstPage;
  void secondPage;
} finally {
  tx.close();
}

void employee;
void ownedRows;
```

## Row Shapes and Terminal Semantics

For selected arity `N`:

```text
RowOf[Query[T]]           = T
RowOf[Query[A, B, ...]]   = tuple[A, B, ...]              # Python
RowOf[Query<[A, B, ...]>] = readonly [A, B, ...]          # TypeScript
```

- `one()` requires exactly one distinct selected concept-identity tuple. Zero
  tuples raises cardinality code `no_result`; two or more raises cardinality
  code `not_unique`.
- `rows(limit, offset=0)` requires a positive limit and non-negative safe
  offset. It returns one item per distinct selected identity tuple.
- `page_by(root, ...)` pages distinct root concept identities and returns an
  immutable `Page[RowOf]`.
- `count_by(root)` and `exists_by(root)` use distinct matching root identities.
  They do not inherit fetch/page ordering, offset, or limit.
- One terminal call is one logical executor invocation. It may issue a bounded
  number of statements, all in the same read transaction.

The request model has a third row cardinality beside exactly-one and a bounded
window: `complete`. A complete fetch returns every distinct selected identity
tuple and is refused with `row_window_exceeded` when there are more than its
limit. Nothing is cut off, so it takes no offset and no order, and the selected
types need no unique key. Generated manager `all` and `first` run as complete
fetches; no query-builder terminal exposes it yet. A plan that uses it requires
the `query.operation.complete-rows` capability.

The Python page contract is a frozen envelope with tuple `items`, `offset`,
`limit`, and `total: int | None`. The TypeScript page is readonly and uses
`total: bigint | undefined`. `include_total`/`includeTotal` defaults to false,
so total is absent unless requested.

## Identity, Multiplicity, and Hydration

These identities are deliberately different:

| Concept | Used for | Rule |
| --- | --- | --- |
| Matching solution | Collection multiplicity | One complete positive assignment; satisfying overlapping `OR` branches does not clone it |
| Selected identity tuple | `one` and `rows` | Ordered identities of selected bindings; hidden witnesses do not duplicate it |
| Root identity | page/count/exists | The selected root concept, distinct before offset/limit |
| Collection concept identity | `CollectDistinct` | TypeDB concept identity, never model/value equality |

`x.collect()` preserves matching-solution multiplicity. If two different hidden
witnesses bind the same collected concept, it appears twice. `x.collect().distinct()`
deduplicates that output only by concept identity. Bindings never group,
deduplicate, or reconstruct collections.

Complete hydration covers declared selected concepts only, including the role
data required for selected relations. It does not recursively hydrate every
adjacent graph concept. Rust validates provider evidence before a binding may
construct a model or result container.

The normative identity example is
`tests/contracts/typed_query/expected-results-v1.json`. It includes duplicate
raw solutions, a selected-tuple result, distinct root results, multiplicity-
preserving collection output, and identity-distinct collection output. The ORM
recording-backend test loads those exact four solutions and verifies every
expected value through the production selected-result executor; it does not
reimplement the identity rules in a fixture interpreter.

## Graph Topology and Boolean Binding

- All selected and hidden positive bindings form one connected typed graph.
  Disconnected positive components fail unless `allow_cross_join` explicitly
  names the component connection.
- Every predicate binding is selected or attached through `match(...)`.
- Cycles are valid.
- Variables exported from `OR` are definitely bound in every branch. One
  complete assignment satisfying overlapping branches is still one solution.
- `NOT` may reference only bindings already positively bound outside it.
- Branch-local exports, correlated subqueries, recursive paths, and
  unconstrained cross joins are outside the initial contract.
- Cross-owner fields, invalid field-to-field comparisons, non-relation role
  sources, incompatible players, foreign-session handles, and unattached
  bindings fail before executor invocation.

## Output Shapes and Collections

Positional output preserves construction order. Named output is declared, not
an arbitrary dictionary projection. Python accepts a supported frozen dataclass
or `NamedTuple` whose field names and annotations exactly match its selections.
TypeScript infers a readonly object shape.

`BoundVar[T]` is a singular selection. `Collected[T]` selects a Python
`tuple[T, ...]` or readonly TypeScript array. An output containing a collection
is initially executable only with `page_by(root)`. The root is the only singular
page slot; every non-root output is collected. Collection output in `one()` or
`rows()` is invalid before data execution.

## Ordering and Bounds

Concept identity determines equality but is not assumed sortable.

- Every incomplete `rows` result requires total stable order across the
  selected tuple.
- Every incomplete root page requires total stable order on the root.
- Every collection requires deterministic binding-local order.
- The canonical validator extends public order with present, schema-declared
  unique scalar keys on the applicable binding. Missing, nullable, non-unique,
  or multi-valued keys fail closed.
- Public `rows` order may reference selected singular bindings only.
  Public `page_by` order may reference the root only.
- Sorting through multi-valued fields or roles requires an explicit reduction
  and is outside the initial scope.
- `limit` is a positive safe integer. `offset` is a non-negative safe integer.
  Oversize, overflow, NaN, infinity, fractional TypeScript numbers, or negative
  values fail before executor invocation.

## String Operators

`contains`, `starts_with`/`startsWith`, and `ends_with`/`endsWith` accept literal
text. Rust owns escaping. A percent sign is an ordinary percent sign, not a SQL
wildcard. `regex` is the only regex operator on the new facade. The new facade
does not expose `like`; legacy `like` behavior remains unchanged.

## TypeDB Parameter Transport

Predicate values remain typed Rust values through request validation and
compiler lowering. On a negotiated band-9 connection, the compiler emits
deterministic `$g0`, `$g1`, ... variables and the executor supplies one
`given` row through the driver's bounded streaming path. Values never cross a
Python or TypeScript raw-query boundary.

Pre-3.12 provider bands compile the same validated values through Rust's inline
literal renderer. Temporal, decimal, and duration operands also remain inline
on band 9 because the current portable row adapter cannot preserve their full
canonical TypeQL spelling surface. TypeDB requires the right operand of `like`
to be a string literal, so prefix, suffix, and regex predicates remain inline;
Rust still owns their escaping and validation. `contains` can use `given`,
retains TypeDB's Unicode case-folded substring semantics, and `regex` remains
the only raw regular expression.
The route is an internal provider choice and does not change the request shape,
statement ceiling, transaction snapshot, or public API.

## Transaction Ownership and Resources

With a database connection, a terminal opens one read transaction and closes it
on success, validation failure after opening, provider error, decode error,
timeout, and cancellation. With a caller-owned read `TransactionContext`, the
terminal neither commits, rolls back, closes, nor consumes it. Every internal
statement in the logical invocation uses that same transaction and snapshot.

Provider capability and schema-fingerprint checks happen before a data
statement. Provider/session hard ceilings cover processed rows, collected
concepts, response bytes, statement count, and transaction duration. A caller
may tighten but never raise those ceilings. Limits are enforced while
processing, never after unbounded materialization. Any violation fails the
whole operation; no partial row or page is exposed.

## Query Limits

A query is held to one set of limits from authoring to execution: the plan's
canonical encoding, its structure, and its remote request envelope. The limits
are a value, `QueryLimits`, not library constants. Defaults reproduce the
ceilings every earlier release enforced, so a caller who sets nothing sees
unchanged plan bytes, fingerprints, and refusals.

| Limit | Default | Supported maximum | Bounds |
| --- | ---: | ---: | --- |
| `canonical_bytes` | 16 MiB | none | One plan or invocation artifact; also one predicate literal and the lowered TypeQL |
| `canonical_depth` | 64 | 126 | Nesting of the canonical encoding |
| `canonical_collection_len` | 65,536 | none | Members of one encoded array or object |
| `canonical_string_bytes` | 1 MiB | none | One encoded string or key |
| `selected_slots` | 16 | 16 | Output slots, reducers, and grouping fields |
| `bindings` | 256 | 65,536 | Plan bindings and input columns |
| `predicate_nodes` | 4,096 | none | Total predicate nodes |
| `predicate_depth` | 64 | 126 | Predicate nesting |
| `boolean_terms` | 256 | none | Children of one conjunction, disjunction, or call |
| `allowed_cross_joins` | 1,024 | none | Explicit cross-join pairs |
| `order_terms` | 64 | none | Row or root ordering terms |
| `collection_order_terms` | 64 | none | Ordering terms of one collected slot |
| `input_rows` | 4,096 | none | Rows of one invocation |
| `input_bytes` | 4 MiB | none | Encoded bytes of one invocation's rows |
| `output_name_bytes` | 128 | none | One output or variable name |
| `semantic_id_bytes` | 512 | none | One semantic identity |
| `diagnostic_bytes` | 65,536 | none | One serialized diagnostic |
| `remote_request_bytes` | 32 MiB | none | One remote request envelope |

Every limit must be at least one. The three supported maxima are properties of
the implementation, not policy: the JSON parser decodes at most 127 nesting
levels and a request frames the plan in one, binding identities are 16 bits
wide, and generated tuple typing in every binding is sealed at sixteen
positions. `selected_slots` can therefore be lowered but not raised.

Rules that hold in every binding:

- Rust admits limits. A binding passes named overrides through and never
  validates, clamps, or defaults them. An unknown name or an unsupported value
  is refused before a session exists.
- A session's limits apply to every query it authors. Validation, V2
  adaptation, canonical encoding, and execution all read the limits the
  request was validated under.
- Repeated `where` clauses and chained `and`/`or` build one flat conjunction
  or disjunction. Nesting depth reflects only nesting the caller wrote.
- Default `predicate_depth` and default `canonical_depth` are both 64, and
  each predicate level costs two encoded levels, so a predicate nested past
  about thirty real levels is refused by `canonical_depth` until the caller
  raises it.
- Answer ceilings are separate. How much a query may return is the answer
  policy described under [Answer Limits](#answer-limits), not a query limit.

### Remote agreement

An executor states its query limits in its capability advertisement and
enforces exactly that statement: it decodes each request and validates each
plan under the advertised limits, whatever the client was configured with.
The advertisement names only limits that differ from the defaults, so a
default executor's advertisement bytes and fingerprint are unchanged. A client
built before query limits were configurable rejects an advertisement that
names any, which is the intended fail-closed outcome.

A client holds each request to the stricter of its session's limits and the
advertised limits and refuses before sending when the plan does not fit.

### Refusals

A query refused by a limit reports `resource_limit` with three details:

| Detail | Meaning |
| --- | --- |
| `limit` | The stable limit name from the table above |
| `maximum` | The effective value the query was held to |
| `source` | `default`, `configured` (the caller or host set it), or `advertised` (the remote executor's value was the stricter one) |

`source` tells the reader which setting to change: raising a session limit
does not help when the advertisement is the binding side. Diagnostics that
already carried their own details, such as `maximum_depth` or `actual_bytes`,
keep them.


## Answer Limits

How much a query may return is one policy on every transport,
`QueryExecutionResourceLimits`, with the same eight dimensions. What an unset
policy defaults to depends on the transport the session runs on: a direct
session starts from the 2.2 executor defaults, a proxy connection (and the
server) from what one remote envelope carries.
`QueryExecutionResourceLimits::default_for(AnswerTransport)` is the one place
an unset policy becomes a default; `Default` is the direct policy.

| Limit | Direct default | Proxy default | Supported maximum | Bounds |
| --- | ---: | ---: | ---: | --- |
| `timeout_milliseconds` | 30,000 | 30,000 | 86,400,000 | Duration of one terminal |
| `items` | 100,000 | 65,536 | none | Provider items read and rows returned; the scan and hydration both draw on it |
| `bytes` | 64 MiB | 32 MiB | none | Provider answer bytes, the reply envelope, and the projected result |
| `graph_nodes` | 1,000,000 | 65,536 | none | Hydrated things, including nested role players |
| `attribute_values` | 4,000,000 | 65,536 | none | Hydrated attribute values |
| `collection_members` | 1,000,000 | 65,536 | none | Collected identities, group keys, and reducer cells |
| `role_players` | 1,000,000 | 65,536 | none | Role-player references |
| `statements` | 3 | 3 | 3 | Provider statements of one terminal |

An answer between the two defaults is returned on a direct session and
refused through a proxy with `limit` naming the dimension and `source`
`default`; configure the proxy connection, and the executor, to return it.

Rules that hold in every binding:

- A caller may tighten any dimension and may raise one by configuring it. A
  value past a supported maximum is refused by name, never clamped.
- A session never widens the policy of the connection it runs on: each
  dimension is the stricter of the two.
- The executor, the reply codec, and projected materialization have no
  ceilings of their own. A result the policy admits is one every stage admits.
- A result past a ceiling is refused with `resource_limit` and the details
  `limit`, `maximum`, and `source`, as for query limits. `source` is
  `default` only when the value is the default of the session's transport.

### Remote agreement

An executor states the answer ceilings it enforces in its capability
advertisement, naming only those that differ from the proxy defaults, so a
default executor's advertisement bytes and fingerprint are unchanged. Each
request
carries the caller's budget. The client asks for no more than the executor
advertised, and the executor runs each request under the stricter of the
request's budget and its own ceilings, per dimension. The reply is encoded
and decoded under those budgets, so a reply over 32 MiB or 65,536 rows is
exchanged only between peers that both raised the limit.

The proxy keeps one bound of its own: a request lives at most five minutes,
so a longer `timeout_milliseconds` shortens to that through a proxy.

## Remote Envelope

One request format and one reply format cross the proxy boundary, whatever
the query: an authored low-level plan, a generated model query, or a
reduction.

| Wire value | Format |
| --- | --- |
| Request | `typebridge.query-remote-request/v2` |
| Response | `typebridge.query-remote-response/v2` |
| Failure | `typebridge.query-remote-failure/v2` |
| Signed outer reply | `typebridge.query-remote-signed-reply/v1` |
| Capability advertisement | `typebridge.query-remote-capabilities/v1` |

The request carries the exact canonical plan bytes (either released plan
format), the invocation, the result kind, the caller's answer budget, the
advertisement fingerprint, a nonce, and an absolute bounded expiry. Every
reply is signed with the executor's advertised key and binds the nonce, the
plan fingerprint, and the fingerprint of the whole request, so a replayed or
foreign reply is refused before any host object is built. A failure carries
the complete structured diagnostic: category, code, message, path, and
details.

An executor advertises the envelope with three capabilities, `query.plan.v2`,
`query.remote.envelope-v2`, and `query.remote.structured-diagnostic`; a client
refuses to prepare a request for an advertisement that lacks one with
`unsupported_required_capability` and the `missing` names. A request in
another format is refused by the executor with
`query_remote_v2_format_unsupported`, whose `supported` detail names the
request format it accepts.

## Stable Error Categories

| Category | Meaning | Executor invoked? | Data statements? |
| --- | --- | --- | --- |
| `invalid_plan` | Ownership, lineage, topology, shape, order, bound, or borrowed target is invalid | No; borrowed-target preflight has entered the executor | No |
| `cardinality` (`no_result`) | `one()` observed zero distinct selected tuples | Yes | Yes |
| `cardinality` (`not_unique`) | `one()` observed more than one distinct selected tuple | Yes | Yes, bounded to distinguish cardinality |
| `unsupported_capability` | Provider lacks a canonically required feature | Yes | No |
| `stale_schema` | Descriptor/schema fingerprint changed before lowering | Yes | No |
| `resource_limit` | A processing, collection, byte, statement, or duration ceiling was crossed | Yes | Possibly; never partial output |
| `cancelled` | Cooperative caller cancellation interrupted preparation, transaction open, or provider processing | Maybe | Possibly; never partial output |
| `provider` | TypeDB/provider execution failed | Yes | Possibly; never partial output |
| `result_decode` | Provider evidence/result did not match the validated request and shape | Yes | Possibly; never partial output |

Cancellation is distinct from timeout: an expired duration is a
`resource_limit`, while an observed caller cancellation is `cancelled` whether
it wins before transaction open or between provider items. In caller-transport
remote execution, cancellation can stop local preparation or reply decoding
and permits the caller to abort its own exchange; TypeBridge does not claim to
cancel a request after the caller has sent it.

No error becomes `None`, `null`, an empty result, a partial page, or a raw
provider exception. Rust error categories survive every generated binding
boundary.

Stable codes are case-specific and are not aliases for broad prose labels. For
example, disconnected topology is `disconnected_plan`, partial OR export is
`partial_or_binding`, missing provider support is
`missing_provider_capability` with the feature in structured details, and a
wrong result shape is `result_shape_mismatch`. Cross-owner fields, cross-owner
roles, and incompatible role players remain three separate diagnostics. The
versioned corpus pins the complete code table, including the distinct public-
boundary and canonical-request window failures. Absence of `like` is a static
API fact, not a fabricated runtime error.

## Legacy Compatibility Baseline

The #189 cutover removed the query facades' use as active schema/model
authoring authority, and 2.3.0 removed the handwritten Python typed facade.
The retained boundary is narrow:

- Python package-root raw-TypeQL `Query`/`QueryBuilder` imports, in-place
  mutation, and build output remain;
- generated Python managers retain concise single-type filters, ordering,
  terminals, aggregates, updates, deletes, and hydration;
- TypeScript package-root `TypedQuery<T, Row>` retains its two-parameter query
  behavior for released compatibility artifacts;
- low-level Python and Node Query V2 authority remains public;
- the former Node handwritten `/typed` registration surface and root model,
  field, role, attribute, flag, registry, and descriptor declarations are not
  active application paths.

No generated path may fall back to raw TypeQL, public strings or dictionaries
as schema meaning, `Any`, `unknown`, client-side filtering/grouping/
deduplication, or binding-side hydration.

## Fixture and Implementation Handoff

The versioned language-neutral corpus lives at
`tests/contracts/typed_query/corpus-v1.json`; its schema vocabulary is
`schema-v1.json`, and identity outcomes are in `expected-results-v1.json`.
Every case references paired Python and TypeScript example IDs from this
document. #172 consumed the plan/error/capability vocabulary; #174 activated
the facade checks; #176 and #177 proved the expected results against recording
and live providers. #189 moved those semantics into verified generated
packages. All ten marked examples are concatenated into checked Pyright and
`tsc` inputs after the canonical fixture package is emitted, so marker presence
alone is not accepted.

The exact logical IDs, duplicate hidden-witness solution, selected rows, and
collection ID sequences in `expected-results-v1.json` are recording-backed.
The clean generated-package live suites for Python, Node, and Rust execute the
same one-root page, collection, count, existence, grouping, direct, and remote
journeys through exact generated models and tokens. The generated-only
operation inventory maps those outcomes to their binding-specific evidence.

The compiler fixtures import the emitted package, not a handwritten authoring
barrel. Separately unscheduled query compatibility and read-only archive
baselines remain independent gates.

## Provider failures

The runtime classifies provider failures once. A `Failure::Rejected` contains
`phase` (`query` or `commit`), a value-free reason and, on staged execution, an
operation index. Reasons are conflict (with a reported lock kind), cardinality,
unique, another annotation constraint, or another validated provider code.
A `Failure::Indeterminate` means a commit may have applied. Its code is optional.
No winner identity, query value, database name or provider prose is retained.

Typed failures keep the released SDK diagnostic codes, categories and
details:

| Failure | Code | Released detail |
| --- | --- | --- |
| Commit rejection | `commit_definitely_aborted` | `commit_outcome: definitely_aborted` |
| Indeterminate commit | `commit_outcome_unknown` | `commit_outcome: unknown` |
| Query rejection | `provider_operation_failed` | `operation` |

Errors that already lowered to another released code, such as
`transaction_state_invalid`, keep it and gain the same typed details.

Typed diagnostics add `retry_safe` as a boolean, `code` as a validated TypeDB
identity and `rejection` (`conflict`, `cardinality`, `unique`, `constraint` or
`other`) for a rejection. Conflict diagnostics include `lock_kind`; annotation
constraints include `annotation`. These use bounded identity detail values.
The C ABI 1.6 projection omits them and keeps the released detail set
(`SdkExecutionDiagnostic::released_details`). A staged operation index
is an `Index` path segment; possible owner and attribute sites are `Type` and
`Field` path segments taken only from plan metadata. They are possible sites,
not evidence that a particular site or another transaction caused the failure.

New staged errors carry this classification structurally. Additive `.failure()`
accessors on released runtime/ORM/server error enums recover only the code and
fixed description written by the runtime; these released string carriers have
no operation index or plan-derived owner/attribute context. Their enum variants
remain unchanged. Arbitrary application messages are not parsed as failures.

A rejection is safe to retry only by rerunning the whole transaction against a
fresh snapshot, including its reads and conditions. It does not promise that
another attempt will succeed. Indeterminate commits require reconciliation
before retrying. TypeDB-origin ordinary error messages, debug output and source
chains contain a code plus fixed description. Runtime transport messages also
keep the operation name and a fixed transport kind, with the driver code when
available. Raw HTTP/TLS causes and endpoint values are discarded. Local
validation messages and application-owned transport errors retain their
existing contracts.
