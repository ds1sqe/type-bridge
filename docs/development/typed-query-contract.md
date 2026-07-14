# Unified Typed Query Contract

Status: implemented normative contract for
[#170](https://github.com/ds1sqe/type-bridge/issues/170) through
[#177](https://github.com/ds1sqe/type-bridge/issues/177).

!!! note "Additive typed-query imports"

    The immutable facade is available from `type_bridge.typed` and
    `@type-bridge/node/typed`. Existing package-root query APIs remain available
    and unchanged; importing the new subpath is an explicit migration choice.

This document fixes the public behavior shared by the Rust request, Python and
TypeScript facades, and TypeDB executor. When implementation details
conflict with this document, this contract wins unless a later accepted design
explicitly revises it.

## Public Surface and Compatibility Boundary

There is one new immutable public `Query`, whether it selects one model or many.
There are no separate public model-query, match-query, or fetch-query classes.

| Language | Typed-query import | Required connection |
| --- | --- | --- |
| Python | `from type_bridge.typed import Query, QuerySession` | `Database | TransactionContext` |
| TypeScript | `import { Query, QuerySession } from "@type-bridge/node/typed"` | `RustDatabase | RustTransactionContext` |

Python's package-root `Query` and `QueryBuilder` remain the mutable raw-TypeQL
builders. TypeScript's package-root `TypedQuery<T, Row>` remains the current
mutable two-parameter manager query. Python manager queries remain mutable too.
Their imports, ignored-return mutation, generic arity, filters, ordering,
terminals, aggregates, updates, deletes, and hydration do not migrate as part of
#170.

`QuerySession.var(Model)` returns one `BoundVar[Model]`. Two calls for the same
model have the same static type and different opaque runtime identities. New
bindings match their declared type exactly by default. Subtype-inclusive
matching must be requested explicitly on that binding.

`QuerySession.query(selection, ...)` requires between 1 and 16 selections. The
arguments declare the positional output in construction order. Selecting the
same binding handle twice is invalid; selecting two different handles for the
same model is valid. `Query.match(binding, ...)` adds hidden predicate witnesses
without changing the selected result type.

Every builder call returns a new query handle. Its base and sibling handles stay
unchanged. Python and TypeScript carry only language typing plus opaque Rust
handles; they do not hold a mirrored semantic plan or raw TypeQL.

## Complete Python Example

<!-- typed-query-example: python-models-and-session -->
```python
from dataclasses import dataclass
from typing import assert_type

from type_bridge import (
    Database,
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    String,
    TypeFlags,
)
from type_bridge.typed import BoundRole, Page, Query, QuerySession, RoleRef


class Name(String):
    pass


class Age(Integer):
    pass


class Industry(String):
    pass


class Position(String):
    pass


class Person(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age


class Company(Entity):
    flags = TypeFlags(name="company")
    name: Name = Flag(Key)
    industry: Industry


class Employment(Relation):
    flags = TypeFlags(name="employment")
    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role("employer", Company)
    position: Position


# Attribute classes are owner-derived field tokens. The bound variable supplies
# the model owner; no descriptor cast or public string is part of the API.
person_name = Name
person_age = Age
company_name = Name
company_industry = Industry

db = Database(address="localhost:1729", database="typed_query_example")
db.connect()
session = QuerySession(db)
person = session.var(Person)
employment = session.var(Employment)
company = session.var(Company)
assert_type(Employment.employee, RoleRef[Person, Employment])
assert_type(Employment.employer, RoleRef[Company, Employment])
assert_type(employment.role(Employment.employee), BoundRole[Person])
```

The scalar, tuple, five-slot, and repeated-model forms retain their exact static
shapes. The five-slot graph below is connected through one company. The repeated
two-person output uses hidden bindings to connect its selected variables.

<!-- typed-query-example: python-selection-arities -->
```python
one_person: Query[Person] = session.query(person)
assert_type(one_person.one(), Person)
assert_type(one_person.rows(limit=20), list[Person])

two_slots: Query[Person, Company] = (
    session.query(person, company)
    .match(employment)
    .where(
        employment.role(Employment.employee).is_(person),
        employment.role(Employment.employer).is_(company),
    )
)
assert_type(two_slots.one(), tuple[Person, Company])
assert_type(two_slots.rows(limit=20), list[tuple[Person, Company]])

colleague = session.var(Person)
other_employment = session.var(Employment)
five_slots: Query[Person, Employment, Company, Employment, Person] = session.query(
    person,
    employment,
    company,
    other_employment,
    colleague,
).where(
    employment.role(Employment.employee).is_(person),
    employment.role(Employment.employer).is_(company),
    other_employment.role(Employment.employee).is_(colleague),
    other_employment.role(Employment.employer).is_(company),
)
assert_type(
    five_slots.rows(limit=20),
    list[tuple[Person, Employment, Company, Employment, Person]],
)

person_pair: Query[Person, Person] = (
    session.query(person, colleague)
    .match(
        employment,
        other_employment,
        company,
    )
    .where(
        employment.role(Employment.employee).is_(person),
        employment.role(Employment.employer).is_(company),
        other_employment.role(Employment.employee).is_(colleague),
        other_employment.role(Employment.employer).is_(company),
    )
)
assert_type(person_pair.rows(limit=20), list[tuple[Person, Person]])

# QuerySession.query has checked overloads through 16 selections. A seventeenth
# selection is a static diagnostic and Rust rejects forged unchecked requests.
```

Fields and roles are bound through their owning variables. Python string
helpers take the corresponding typed string attribute while TypeScript helpers
take a string; only `regex` interprets that value as a regex. A disconnected
graph is invalid unless topology-level cross-join permission names its
components.

<!-- typed-query-example: python-topology-and-strings -->
```python
adults_in_ai = two_slots.where(
    person.field(person_age).gte(Age(18)),
    company.field(company_industry).eq(Industry("AI")),
    person.field(person_name).starts_with(Name("Al")),
    company.field(company_name).contains(Name("Research")),
    company.field(company_name).ends_with(Name("Labs")),
    person.field(person_name).regex(Name(r"^A[[:alpha:]]+$")),
)

# Percent is literal input here; it has no SQL wildcard meaning.
literal_percent = one_person.where(person.field(person_name).contains(Name("50%")))

# Cross joins are explicit topology permission, never a boolean predicate.
independent_pairs: Query[Person, Company] = session.query(person, company).allow_cross_join(
    person, company
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
    order_by=(person.field(person_name).asc(),),
)

person_count: int = two_slots.count_by(person)
any_person: bool = two_slots.exists_by(person)

@dataclass(frozen=True, slots=True)
class PersonWork:
    person: Person
    employments: tuple[Employment, ...]
    companies: tuple[Company, ...]


work: Query[PersonWork] = session.query_as(
    PersonWork,
    person=person,
    employments=employment.collect(),
    companies=company.collect().distinct(),
).where(
    employment.role(Employment.employee).is_(person),
    employment.role(Employment.employer).is_(company),
)

page: Page[PersonWork] = work.page_by(
    person,
    limit=50,
    offset=0,
    order_by=(person.field(person_name).asc(),),
    include_total=True,
)
assert_type(page.items, tuple[PersonWork, ...])
assert_type(page.total, int | None)
```

A database-backed terminal owns its read transaction. A caller-owned read
context remains usable after a terminal; the query neither commits, rolls back,
nor closes it.

<!-- typed-query-example: python-transaction-ownership -->
```python
# Owned: rows() opens one read transaction and closes it on success or error.
owned = QuerySession(db)
owned_person = owned.var(Person)
owned_rows = owned.query(owned_person).rows(limit=10)

# Borrowed: the surrounding context owns lifecycle and may run more work.
with db.transaction("read") as tx:
    borrowed = QuerySession(tx)
    borrowed_person = borrowed.var(Person)
    first_page = borrowed.query(borrowed_person).rows(limit=10)
    second_page = borrowed.query(borrowed_person).rows(limit=10, offset=10)
```

## Complete TypeScript Example

<!-- typed-query-example: typescript-models-and-session -->
```typescript
import {
  Entity,
  Key,
  Relation,
  RustDatabase,
  attr,
  field,
  role,
  type RustTransactionContext,
} from "@type-bridge/node";
import {
  QuerySession,
  references,
  type Page,
  type Query,
} from "@type-bridge/node/typed";

class Name extends attr.String("name") {}
class Age extends attr.Integer("age") {}
class Industry extends attr.String("industry") {}
class Position extends attr.String("position") {}

class Person extends Entity("person", {
  name: field(Name, Key),
  age: field(Age),
}) {}

class Company extends Entity("company", {
  name: field(Name, Key),
  industry: field(Industry),
}) {}

class Employment extends Relation("employment", {
  employee: role(Person),
  employer: role(Company),
  position: field(Position),
}) {}

const db = RustDatabase.connect("localhost:1729", "typed_query_example");
const session = new QuerySession(db);
const person = session.var(Person);
const employment = session.var(Employment);
const company = session.var(Company);
const personRefs = references(Person);
const companyRefs = references(Company);
const employmentRefs = references(Employment);
```

TypeScript uses one readonly tuple parameter because it has no variadic generic
parameter list. `QueryRow<Slots>` scalarizes a one-slot tuple and preserves a
readonly tuple for two or more slots.

<!-- typed-query-example: typescript-selection-arities -->
```typescript
const onePerson: Query<readonly [Person]> = session.query(person);
const scalar: Person = onePerson.one();
const scalarRows: readonly Person[] = onePerson.rows({ limit: 20 });

const twoSlots: Query<readonly [Person, Company]> = session
  .query(person, company)
  .match(employment)
  .where(
    employment.role(employmentRefs.roles.employee).is(person),
    employment.role(employmentRefs.roles.employer).is(company),
  );
const pairRows: readonly (readonly [Person, Company])[] = twoSlots.rows({
  limit: 20,
});

const colleague = session.var(Person);
const otherEmployment = session.var(Employment);
const fiveSlots: Query<
  readonly [Person, Employment, Company, Employment, Person]
> = session
  .query(person, employment, company, otherEmployment, colleague)
  .where(
    employment.role(employmentRefs.roles.employee).is(person),
    employment.role(employmentRefs.roles.employer).is(company),
    otherEmployment.role(employmentRefs.roles.employee).is(colleague),
    otherEmployment.role(employmentRefs.roles.employer).is(company),
  );

const personPair: Query<readonly [Person, Person]> = session
  .query(person, colleague)
  .match(employment, otherEmployment, company)
  .where(
    employment.role(employmentRefs.roles.employee).is(person),
    employment.role(employmentRefs.roles.employer).is(company),
    otherEmployment.role(employmentRefs.roles.employee).is(colleague),
    otherEmployment.role(employmentRefs.roles.employer).is(company),
  );

void scalar;
void scalarRows;
void pairRows;
void fiveSlots;
void personPair;
// query() accepts 1..16 selections. Seventeen is a tsc and Rust diagnostic.
```

<!-- typed-query-example: typescript-topology-and-strings -->
```typescript
const adultsInAi = twoSlots.where(
  person.field(personRefs.fields.age).gte(new Age(18n)),
  company.field(companyRefs.fields.industry).eq(new Industry("AI")),
  person.field(personRefs.fields.name).startsWith("Al"),
  company.field(companyRefs.fields.name).contains("Research"),
  company.field(companyRefs.fields.name).endsWith("Labs"),
  person.field(personRefs.fields.name).regex(String.raw`^A[[:alpha:]]+$`),
);

const literalPercent = onePerson.where(
  person.field(personRefs.fields.name).contains("50%"),
);

const independentPairs: Query<readonly [Person, Company]> = session
  .query(person, company)
  .allowCrossJoin(person, company);

void adultsInAi;
void literalPercent;
void independentPairs;
```

<!-- typed-query-example: typescript-terminals-and-collections -->
```typescript
const orderedPeople: readonly Person[] = onePerson.rows({
  limit: 50,
  offset: 0,
  orderBy: [person.field(personRefs.fields.name).asc()],
});

const personCount: bigint = twoSlots.countBy(person);
const anyPerson: boolean = twoSlots.existsBy(person);

const work: Query<readonly [Readonly<{
  person: Person;
  employments: readonly Employment[];
  companies: readonly Company[];
}>]> = session.queryNamed({
  person,
  employments: employment.collect(),
  companies: company.collect().distinct(),
}).where(
  employment.role(employmentRefs.roles.employee).is(person),
  employment.role(employmentRefs.roles.employer).is(company),
);

const page: Page<Readonly<{
  person: Person;
  employments: readonly Employment[];
  companies: readonly Company[];
}>> = work.pageBy(person, {
  limit: 50,
  offset: 0,
  orderBy: [person.field(personRefs.fields.name).asc()],
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
const ownedPerson = ownedSession.var(Person);
const ownedRows = ownedSession.query(ownedPerson).rows({ limit: 10 });

// Borrowed: only the caller closes the context, which remains reusable.
const tx: RustTransactionContext = db.transaction("read");
try {
  const borrowed = new QuerySession(tx);
  const borrowedPerson = borrowed.var(Person);
  const firstPage = borrowed.query(borrowedPerson).rows({ limit: 10 });
  const secondPage = borrowed.query(borrowedPerson).rows({
    limit: 10,
    offset: 10,
  });
  void firstPage;
  void secondPage;
} finally {
  tx.close();
}

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

The following current-major behavior remains fixed while the new subpaths are
added:

- Python package-root raw-TypeQL `Query`/`QueryBuilder` imports, in-place
  mutation, and build output;
- Python manager-query ignored-return mutation for `filter`, `limit`, and
  `offset`, plus `all`, `first`, `execute`, `count`, `exists`, aggregate,
  group-by, update, and delete;
- dictionary/Django-style filters, string ordering, polymorphic managers, and
  #117 cross-type `has` behavior;
- Pydantic class-level descriptors and instance model values;
- TypeScript package-root `TypedQuery<T, Row>` export, two-parameter generic,
  mutation, terminals, aggregate, and group-by;
- root package model, field, role, and attribute declarations.

No new path may fall back to raw TypeQL, strings, dictionaries, `Any`,
`unknown`, client-side filtering/grouping/deduplication, or binding-side
hydration.

## Fixture and Implementation Handoff

The versioned language-neutral corpus lives at
`tests/contracts/typed_query/corpus-v1.json`; its schema vocabulary is
`schema-v1.json`, and identity outcomes are in `expected-results-v1.json`.
Every case references paired Python and TypeScript example IDs from this
document. #172 consumed the plan/error/capability vocabulary; #174 activated
the public facade checks; #176 and #177 prove the expected results against
recording and live providers. All ten marked examples are concatenated into
checked Pyright and `tsc` inputs, so marker presence alone is not accepted.

The exact logical IDs, duplicate hidden-witness solution, selected rows, and
collection ID sequences in `expected-results-v1.json` are recording-backed.
The separate live TypeDB dataset intentionally has different employment and
company IDs. Its source Python, extracted-wheel Python, and packed-Node readers
execute an exact one-root page plus the larger collection page, count, and
exists, then compare a normalized projection derived from the normative
manifest: distinct logical roots, the one-root page envelope, collection and
collection-distinct cardinalities, count, and existence. This is live-equivalent
semantic evidence; it is not a claim that the live dataset contains every exact
manifest identity.

The compiler fixtures import the real public typed subpaths, while the legacy
artifact baselines remain permanent.
