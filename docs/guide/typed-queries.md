# Immutable Typed Queries

TypeBridge's typed-query facade builds connected, multi-model TypeDB matches
without exposing TypeQL strings or untyped result dictionaries. It is additive:
the existing manager queries and package-root raw `Query` remain available.

Use the dedicated subpath:

```python
from type_bridge.typed import Page, Query, QuerySession
```

For Node, import the equivalent API from `@type-bridge/node/typed`.

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

## Errors and Resource Safety

Native match failures expose a stable `category` and `code`. The categories are
`invalid_plan`, `cardinality`, `unsupported_capability`, `stale_schema`,
`resource_limit`, `provider`, and `result_decode`.

Validation and capability failures that can be known in advance execute no data
statement. Provider, decode, timeout, cancellation, and resource-limit failures
return no partial row or page. The executor bounds processed rows, collection
members, response bytes, statements, and duration while consuming the result.

## Coexisting with Legacy Queries

The immutable facade lives only under `type_bridge.typed` and
`@type-bridge/node/typed`. Existing package-root raw-TypeQL builders and mutable
manager queries keep their current mutation, terminal, pagination, aggregation,
update, and delete behavior. Applications can migrate query-by-query without
renaming those APIs.

For the complete cross-language semantics, identity rules, and stable error
table, see the [unified typed-query contract](../development/typed-query-contract.md).
