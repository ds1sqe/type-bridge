# Generated Typed Query Contract

The read/write [query-plan wire specification](../reference/query-plan.md)
defines canonical bytes, expressions, write stages, operations and shared examples.
Typed queries and manager terminals lower to this one native plan path.
Historical parity captures remain test oracles; retired query encodings are
not accepted execution APIs.

Status: implemented normative semantics for
[#170](https://github.com/ds1sqe/type-bridge/issues/170) through
[#177](https://github.com/ds1sqe/type-bridge/issues/177), consumed through
generated packages after the #189 cutover.

!!! note "Generated-package imports"

    `Query`, `QuerySession`, field tokens, role tokens, and model classes are
    emitted together inside each generated Python or TypeScript package. The
    base SDK supplies connections, the verified runtime projection, and
    typed staged plan execution; it does not accept handwritten descriptors as schema
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
| TypeScript | `import { Query, QuerySession } from "./your-generated-package/index.js"` | `Database | TransactionContext` |

Python's package-root `Query` and `QueryBuilder` are removed in 3.0; use
generated typed queries or `Database.execute_query` for raw TypeQL text.
TypeScript's package-root `TypedQuery<T, Row>` is also removed; generated
queries replace it. Generated model managers provide the
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
  Database,
  type TransactionContext,
} from "@type-bridge/node";

const db = Database.connect("localhost:1729", "typed_query_example");
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
const tx: TransactionContext = db.transaction("read");
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

Rust core callers can use `TransactionContext::execute_plans`,
`OwnedTransaction::execute_plans`, or `EagerTransaction::execute_plans` on a
caller-owned Read, Write, or Schema transaction. Read transactions refuse
write plans with `query_plan_read_only`. The stack shares one answer budget
and deadline and leaves commit, rollback, and close to the caller. A failed
or cancelled write makes the transaction rollback-only; a Schema transaction
can combine native data plans with schema statements before one commit.

Provider capability and schema-fingerprint checks happen before a data
statement. Provider/session hard ceilings cover processed rows, collected
concepts, response bytes, statement count, and transaction duration. A caller
may tighten but never raise those ceilings. Limits are enforced while
processing, never after unbounded materialization. Any violation fails the
whole operation; no partial row or page is exposed.

## Query Limits

A query is held to one set of limits from authoring to execution: the plan's
canonical encoding, its structure, and its remote request envelope. The limits
are a value, `QueryLimits`, not library constants. Small plans keep their
existing canonical bytes and fingerprints. In 3.0, typed predicate strings
also observe the default 1 MiB `canonical_string_bytes` ceiling; 2.x typed
predicates could reach the caller's artifact ceiling. Raise
`canonical_string_bytes` explicitly to retain that larger-value behavior,
subject to the artifact ceiling and the receiver's own limits. V3 requests
with actual strings above 1 MiB require `query.input.wide-strings`;
refusals name the configured `canonical_string_bytes` limit.

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
- A session's limits apply to every query it authors. Validation, native plan
  lowering, canonical encoding, and execution all read the limits the
  request was validated under.
- Repeated `where` clauses and chained `and`/`or` build one flat conjunction
  or disjunction. Nesting depth reflects only nesting the caller wrote.
- Default `predicate_depth` and default `canonical_depth` are both 64, and
  each predicate level costs two encoded levels, so a predicate nested past
  about thirty real levels is refused by `canonical_depth` until the caller
  raises it.
- Answer ceilings are separate. How much a query may return is the answer
  policy described under [Answer Limits](#answer-limits), not a query limit.

Manager `all()` authors its window from the caller's item policy. With no
caller override, that window remains 100,000 on both direct and proxy targets.
A proxy's transport default or a tighter server ceiling only bounds execution;
it never rewrites that window or its plan bytes. Equal caller policies therefore
produce equal plans. An answer exceeding the execution budget returns the
existing typed resource-limit refusal, without publishing a truncated list.

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
| `timeout_milliseconds` | 60,000 | 60,000 | none | Duration of one terminal; zero or negative binding input means no deadline |
| `items` | 100,000 | 65,536 | none | Provider items read and rows returned; the scan and hydration both draw on it |
| `bytes` | 64 MiB | 32 MiB | none | Provider answer bytes, the reply envelope, and the projected result |
| `graph_nodes` | 1,000,000 | 65,536 | none | Hydrated things, including nested role players |
| `attribute_values` | 4,000,000 | 65,536 | none | Hydrated attribute values |
| `collection_members` | 1,000,000 | 65,536 | none | Collected identities, group keys, and reducer cells |
| `role_players` | 1,000,000 | 65,536 | none | Role-player references |
| `statements` | unset (minimum 3) | unset (minimum 3) | 4,096 | Provider statements of one terminal |

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

Execution timeouts start when the call executes, not when its plan is built.
Rust represents them with `QueryTimeout`, either no deadline or a positive
millisecond duration. Python, Node and C accept signed 64-bit milliseconds;
zero and negative inputs select no deadline. A connection or server ceiling
still tightens an unbounded request. Commit is never interrupted: a deadline
observed before commit rolls back, and a dispatched commit runs to its outcome.

The proxy's `admission_window_milliseconds` is separate: it defaults to 60,000 ms
from request issue to admission. Configure it on the proxy connection (Node:
`admissionWindowMilliseconds`) and the server. The server's advertised ceiling
wins. It must be positive and finite; it protects request freshness and nonce
retention without shortening an admitted operation's execution time.

## Remote Envelope

All typed manager and query execution uses the staged envelope. Requests carry
canonical native plans, given rows, transaction kind, resource budgets, the
executor advertisement identity, a nonce and bounded admission expiry. Replies
bind the request and carry validated answers or structured execution failures.
The connection verifies the advertised reply key and complete request binding
before publishing values. See the [wire specification](../reference/query-plan.md#staged-remote-envelope-v3)
for exact fields, tags and signing rules.

`/v2/query` and `/v2/capabilities` are removed. Clients use `/v3/staged` and
`/v3/capabilities`; the old envelopes are not accepted by a compatibility route.

### Staged transactions

A transaction on a proxy connection is the same core `Transaction` as a
direct one, with a proxy backend. Staging stays local; `commit` serializes the
ordered stack into one `typebridge.query-remote-request/v3` envelope (exact
canonical V3 plan bytes, given rows, kind, limits, nonce and expiry) and
sends it to `POST /v3/staged`. The server runs the stack
on one TypeDB transaction and returns a `typebridge.query-remote-response/v3`
reply signed with the key from `GET /v3/capabilities`. The SDK verifies the
signature and the request binding, then resolves each answer through the same
plan answer path as direct execution; a failed reply becomes the same typed
failure, and `commit_outcome_unknown` stays indeterminate rather than "not
applied". Typed match reads compile to native plans in the same stack.
Raw TypeQL uses a separate explicit raw context and `/v3/raw`.

Each commit is one request under a fresh random nonce, and the server keeps
no replies. A lost or unverifiable reply, a cancelled commit, and the server's
own `commit_outcome_unknown` are indeterminate: the writes may have applied,
and the application reconciles by reading the database, as after a lost
direct commit. Every other verified signed refusal applied nothing and keeps
its code. TypeBridge reports outcomes; retries, locking and concurrency limits
are application policy. The staged advertisement is trusted only when its
executor binding and reply key equal the connection's advertisement
(`query_remote_advertisement_mismatch` otherwise); it is pinned or fetched
once per connection. The server admits writes only from a caller an
interceptor vouches for (`on_write_authority`), or from any caller under
`[staged] allow_unauthenticated_staged_writes`. The wire format and failure
vocabulary are in the
[staged remote envelope](../reference/query-plan.md#staged-remote-envelope-v3).

Fetch plans return a distinct `Documents` answer directly and remotely. The
shared Rust verifier checks the document against the plan's possible exact
types, owned attributes and listed roles. `AnswerLimits` bounds rows and
documents under one policy before commit. Python exposes `Documents.documents`,
TypeScript the `documents` answer variant, and Rust `PlanAnswer::Documents`.


### Hosting the staged executor in-process

A host that embeds `type-bridge-server` as a library runs the staged route
without its HTTP framing through `StagedExecutor`.
It takes the same `QueryPipeline` and `StagedQueryState` the server binary builds
and does everything the route does after the body is read:

```rust
use type_bridge_server::transport::staged::{StagedExecutor, StagedOutcomeKind};

let executor = Arc::new(StagedExecutor::new(pipeline, query_state));

// Admit the caller through the pipeline, as the route does for its headers.
let mut ctx = executor
    .pipeline()
    .request_context(metadata);
executor.pipeline().begin_request(&mut ctx).await?;

// Signed request bytes in, signed reply bytes out.
let outcome = executor
    .execute_staged_request(ctx, body, Instant::now() + execution_timeout)
    .await;
deliver(outcome.reply);                      // the bytes the client verifies
match outcome.kind {
    StagedOutcomeKind::Committed => {}       // code "ok"; the reply carries the answers
    StagedOutcomeKind::Failed => {}          // nothing applied; `outcome.code` says why
    StagedOutcomeKind::Unknown => {}         // a write may have applied: reconcile by reading
}
```

The hooks run in a fixed order, pinned by a test: the transport hook
(`on_transport`, during admission), `on_write_authority` for a write,
`on_plan` for every bound plan in index order before anything is
reserved or opened, then nonce reservation, staging and one commit,
`on_commit` once the outcome is known (observe-only, bounded by the audit
deadline), and the response policies (`on_response`) over the signed
reply. A refusal at any step before reservation leaves nothing reserved and
nothing open, so a retry is re-authorized rather than replayed. The outcome
is owned by a detached task once admitted: a host that stops awaiting
mid-commit does not drop the observers. The reply the host delivers is
already policed by its own pipeline's response hooks.

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

- Python package-root raw-TypeQL `Query`/`QueryBuilder` imports are removed
  in 3.0; generated typed queries and `Database.execute_query` replace them;
- generated Python managers retain concise single-type filters, ordering,
  terminals, aggregates, updates, deletes, and hydration;
- TypeScript generated queries replace the removed package-root
  `TypedQuery<T, Row>` facade;
- native staged plans replace low-level Python and Node Query V2 authority;
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
The C ABI 2.0 projection carries these typed failure details. A staged operation index
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

An unset statement budget remains `None` in Rust and Python, and `undefined`
in Node. At execution, a typed adapter resolves it to the larger of three
and its complete static stack size; user-authored plan execution resolves it
to three. An explicit value, including three, remains a strict ceiling.
Intersecting an unset policy with an explicit connection ceiling retains the
explicit ceiling. The existing C numeric limits struct stays unchanged:
a NULL descriptor leaves the budget unset; a supplied descriptor is explicit.
An advertisement omits an unset statement ceiling and retains an explicit
`statements: 3`; other default-valued answer-limit entries remain forbidden.
A 2.x peer refuses the explicit-three advertisement as noncanonical.


The core `build_plan_fetch_stack(registry, plan, model_slots)` constructor owns
native model projection for both released typed reads and generated plan
facades. It preserves scalar columns and duplicate selection rows, hydrates each
concrete model case, and returns one sealed model-tuple proof with core row/slot
ordinals. Generated bindings consume `selection()`, `request()`, `result()` and
`model_ordinal()` without reconstructing model graphs or joining IIDs. Unsupported
split shapes fail with `plan_returning_models_shape`, optional inputs with
`plan_returning_models_optional_input`, and distinct windows over multiple given
rows with `plan_returning_models_given_window`; model projection after writes
requires one complete model-only fetch or fails with
`plan_returning_models_after_write`. Shared staged plan carriers retain the
original canonical Arc allocation and refuse a source that changes its plan
identity after admission.

### Optional execution timelines

`QueryExecutionResourceLimits.collect_phase_timestamps` (Node:
`collectPhaseTimestamps`) defaults to `false`. Set it to `true` to retain a
full diagnostic phase timeline without a tracing subscriber. C uses the
`collect_phase_timestamps` word in its query execution limits descriptor:
0 disables collection, 1 enables it, and other values are refused. Rust
transactions also expose `TransactionOptions::with_collect_phase_timestamps`.
Python's `Database.staged_transaction` accepts keyword `timeout_milliseconds`
and `collect_phase_timestamps`. Node's `Database.stagedTransaction(kind, options)`
accepts `timeoutMilliseconds` (a signed bigint) and `collectPhaseTimestamps`.
Omitting these options preserves 60000 ms and collection off; zero or a negative
timeout disables the deadline. C's `*_transaction_with_options` factories
accept `type_bridge_staged_transaction_options_t`; a null options pointer
keeps those defaults. Explicit options require version 1 and the exact struct
size, a signed timeout, and collection 0 or 1.
This is a diagnostic option, not another resource ceiling or deadline.
With collection disabled and no enabled plan tracing span, phase observation
adds no heap allocation or per-phase clock reads. Full collection allocates
timeline storage and its growing stamp vector; its storage uses one additional
allocation to keep default manager futures small.

The option survives policy intersection when either side enables it. Proxy
requests carry it as an optional Boolean; omission means disabled. Timelines
contain one UTC anchor and monotonic offsets on that process's clock, with
statement indices and the failing phase. They remain diagnostic data and
never become answer content. Ordinary diagnostic details keep their existing
byte ceiling; timeline records use the request's existing statement bound
and the reply envelope ceiling.

Execution deadlines do not bound transport admission: the server's finite
admission window independently bounds transport policy, body collection,
preflight/decode, plan authorization and replay reservation, even when execution
is unbounded. Proxy connection setup also retains finite TCP, TLS-handshake and
capability-fetch timeouts. Raw and schema calls retain no implicit execution
deadline unless the caller configures one; the 60000 ms default applies to typed
plans and programs.

Collected timelines retain the first 8192 statement records and bounded
lifecycle/terminal records. Larger direct calls continue executing, but their
additional statement records are omitted. Invalid collected lifecycle data
produces a typed internal diagnostic rather than a reporting panic.
