# Generated Typed Query Contract

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
multi-binding queries. None of these retained behaviors reopens model or schema
declaration in the base SDK.

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
assert_type(one_person.one(), Person)
assert_type(one_person.rows(limit=20), list[Person])

two_slots: Query[Person, Event] = session.query(person, event).where(subject)
assert_type(two_slots.one(), tuple[Person, Event])
assert_type(two_slots.rows(limit=20), list[tuple[Person, Event]])

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
assert_type(
    five_slots.rows(limit=20),
    list[tuple[Person, Event, Container, Event, Person]],
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
assert_type(person_pair.rows(limit=20), list[tuple[Person, Person]])

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
ordered_people = one_person.rows(
    limit=50,
    offset=0,
    order_by=(person.field(Person.identifier).asc(),),
)

person_count: int = two_slots.count_by(person)
any_person: bool = two_slots.exists_by(person)

@dataclass(frozen=True, slots=True)
class PersonEvents:
    person: Person
    events: tuple[Event, ...]


work: Query[PersonEvents] = session.query_as(
    PersonEvents,
    person=person,
    events=event.collect().distinct(),
).where(subject)

page: Page[PersonEvents] = work.page_by(
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
# Owned: rows() opens one read transaction and closes it on success or error.
owned = Person.query(db)
owned_person = owned.exact(Person)
owned_rows = owned.query(owned_person).rows(limit=10)

# Borrowed: the surrounding context owns lifecycle and may run more work.
with db.transaction("read") as tx:
    borrowed = Person.query(tx)
    borrowed_person = borrowed.exact(Person)
    first_page = borrowed.query(borrowed_person).rows(limit=10)
    second_page = borrowed.query(borrowed_person).rows(limit=10, offset=10)
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
const scalar: Person = onePerson.one();
const scalarRows: readonly Person[] = onePerson.rows({ limit: 20n });

const twoSlots: Query<readonly [Person, Event]> = session
  .query(person, event)
  .where(subject);
const pairRows: readonly (readonly [Person, Event])[] = twoSlots.rows({
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
const orderedPeople: readonly Person[] = onePerson.rows({
  limit: 50n,
  offset: 0n,
  orderBy: [person.field(Person.identifier).asc()],
});

const personCount: bigint = twoSlots.countBy(person);
const anyPerson: boolean = twoSlots.existsBy(person);

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
}>> = work.pageBy(person, {
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
const ownedRows = ownedSession.query(ownedPerson).rows({ limit: 10n });

// Borrowed: only the caller closes the context, which remains reusable.
const tx: RustTransactionContext = db.transaction("read");
try {
  const borrowed = new QuerySession(tx);
  const borrowedPerson = borrowed.exact(Person);
  const firstPage = borrowed.query(borrowedPerson).rows({ limit: 10n });
  const secondPage = borrowed.query(borrowedPerson).rows({
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

## Stable Error Categories

| Category | Meaning | Executor invoked? | Data statements? |
| --- | --- | --- | --- |
| `invalid_plan` | Ownership, lineage, topology, shape, order, bound, or borrowed target is invalid | No; borrowed-target preflight has entered the executor | No |
| `cardinality` (`no_result`) | `one()` observed zero distinct selected tuples | Yes | Yes |
| `cardinality` (`not_unique`) | `one()` observed more than one distinct selected tuple | Yes | Yes, bounded to distinguish cardinality |
| `unsupported_capability` | Provider lacks a canonically required feature | Yes | No |
| `stale_schema` | Descriptor/schema fingerprint changed before lowering | Yes | No |
| `resource_limit` | A processing, collection, byte, statement, or duration ceiling was crossed | Yes | Possibly; never partial output |
| `provider` | TypeDB/provider execution failed | Yes | Possibly; never partial output |
| `result_decode` | Provider evidence/result did not match the validated request and shape | Yes | Possibly; never partial output |

No error becomes `None`, `null`, an empty result, a partial page, or a raw
provider exception. Rust error categories survive Python and TypeScript
boundaries.

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

The #189 cutover does not schedule the existing query facades themselves for
removal. It does remove their use as active schema/model authoring authority.
The retained boundary is therefore narrow:

- Python package-root raw-TypeQL `Query`/`QueryBuilder` imports, in-place
  mutation, and build output remain;
- generated Python managers retain concise single-type filters, ordering,
  terminals, aggregates, updates, deletes, and hydration;
- TypeScript package-root `TypedQuery<T, Row>` retains its two-parameter query
  behavior for released compatibility artifacts;
- low-level Python and Node Query V2 authority remains public;
- the Python `type_bridge.typed` implementation remains isolated compatibility
  machinery, but generated applications import their package-local facade;
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
