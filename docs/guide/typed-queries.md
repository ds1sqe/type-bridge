# Immutable generated queries

Each generated package exports a query facade bound to that package's verified
projection. It provides the same supported application outcomes in Python,
TypeScript/Node, and Rust without routing generated models through handwritten
model descriptors.

## Start a direct session

```python
from app_models import Age, Employment, Person

session = Person.query(db)
person = session.exact(Person)
employment = session.exact(Employment)
```

`Person.query(db)` and the package's `QuerySession` constructor produce the same
package-scoped direct session. A transaction can be supplied instead of a
database.

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

Builder transitions perform no provider or network I/O. Finalization is
terminal, and a rejected transition leaves the builder at its preceding valid
state.

## Connectivity and explicit cross joins

Every selected binding must be connected by a predicate, match witness, or
bounded reachability path. An intentional Cartesian product must be declared:

```python
left = session.exact(Person)
right = session.exact(Person)

rows = (
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

connected = session.query(source, target).where(path).rows(limit=100)
```

The relation and both role tokens are generated evidence. Depth bounds are
mandatory and validated before execution.

## Selection shapes

One binding produces one model; multiple bindings produce a tuple:

```python
one_person = session.query(person).one()
pairs = session.query(person, employment).where(employee).rows(limit=100)
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


page = (
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
query.one()
query.first(order_by=(person.field(Person.person_id).asc(),))
query.rows(limit=100, offset=0)
query.page_by(person, limit=25, include_total=True)
query.count_by(person)
query.exists_by(person)
```

`one` requires exactly one result. `first` returns `None` when empty. Materialized
rows and pages are bounded; unbounded implicit fetches are not part of the
generated contract.

## Direct reductions and grouping

```python
from app_models import aggregate

score = person.field(Person.score)
count, total, mean = session.query(person).aggregate(
    person,
    aggregate.count(),
    aggregate.sum(score),
    aggregate.mean(score),
)

grouped = (
    session.query(person, employment)
    .where(employee)
    .group_by(person, employment)
    .aggregate(aggregate.count(), aggregate.max(score))
)
```

Reducer inputs are owner/scalar checked. Grouping returns exact generated group
values plus typed reducer tuples.

## Remote queries

The generated package also exports `RemoteQuerySession`. The caller supplies a
validated Query V2 authority, exact advertisement bytes, a one-exchange
transport callback, and resource limits. Composition remains local; one terminal
performs one exchange and materializes through the same package projection as
direct execution.

Supported remote terminals are `one`, `first`, bounded `rows`, `page_by`,
`count_by`, and `exists_by`, including exact/subtype hydration, predicates,
roles, reachability, explicit cross joins, and selected shapes.

Remote reducers/grouping are currently native-only operations and fail with the
stable `query_remote_v2_native_only_operation` diagnostic before any exchange.
Remote mutations are not advertised by generated sessions.

## Safety contract

- Schema and projection fingerprints are verified at package installation.
- Only exact registered generated classes and tokens are accepted.
- Queries are immutable and package-scoped.
- Disconnected selections fail unless cross joins are explicit.
- Result evidence is revalidated before hydration.
- Remote limits and capability advertisements fail closed.

The normative cross-language behavior is maintained in the
[unified typed-query contract](../development/typed-query-contract.md).
