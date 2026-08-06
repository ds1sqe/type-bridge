# Queries and predicates

Use a generated manager for a concise one-type filter. Use the generated
package's immutable query session when predicates or results span fields,
roles, or model types.

## Single-type filters

```python
people = Person.manager(db).filter(age__gte=18).all()
```

Manager suffixes are `eq`, `ne`, `gt`, `gte`, `lt`, and `lte`. Multiple keyword
filters are combined with AND. See [CRUD](crud.md#filters) for double-underscore
field names and the explicit `__eq` escape.

## Owner-aware fields

```python
session = Person.query(db)
person = session.exact(Person)

age = person.field(Person.age)
name = person.field(Person.name)
```

`Person.age` is a generated field token. It can be bound only to a variable for
its projected owner. Tokens from another model or generated package are
rejected before query execution.

## Comparison and string predicates

```python
adult = age.gte(Age(18))
not_retired = age.lt(Age(65))
named_ada = name.eq(Name("Ada"))
prefix = name.starts_with(Name("A"))
substring = name.contains(Name("da"))
pattern = name.regex(Name("^A.*"))
```

Generated fields provide scalar-appropriate predicates. Equality can also
compare compatible bound fields:

```python
same_age = person.field(Person.age).eq_field(other.field(Person.age))
```

## Boolean composition

Predicates are immutable and compose with methods or Python operators:

```python
working_age = adult & not_retired
ada_or_grace = named_ada | name.eq(Name("Grace"))
not_ada = ~named_ada

rows = session.query(person).where(working_age, not_ada).rows(limit=100)
```

Each `where(...)` argument is ANDed. `and_`, `or_`, and `not_` are available
when operator syntax is inconvenient.

## Roles

```python
employment = session.exact(Employment)
employee = employment.role(Employment.employee).connects(person)

rows = session.query(person, employment).where(employee).rows(limit=100)
```

The generated role token proves its relation owner, declaring role, and allowed
player types.

## Ordering and windows

```python
rows = (
    session.query(person)
    .where(adult)
    .rows(
        limit=25,
        offset=50,
        order_by=(person.field(Person.person_id).asc(),),
    )
)
```

Rows are bounded. Ordering uses generated bound fields and explicit missing-
value policy where needed. `one`, `first`, `rows`, `page_by`, `count_by`, and
`exists_by` share the same immutable query.

## Raw compatibility builder

The package-root `Query`/`QueryBuilder` facade remains a raw TypeQL builder
separate from generated immutable queries. Its model helpers accept exact
installed generated classes:

```python
from type_bridge import Query, QueryBuilder

raw = QueryBuilder.match_entity(
    Person,
    "$person",
    person_id=PersonId("ada"),
).fetch("$person")

manual = Query().match("$person isa person").fetch("$person")
```

It produces query text; it does not recreate schema authority or provide typed
hydration. Prefer the generated query session for new application queries.
