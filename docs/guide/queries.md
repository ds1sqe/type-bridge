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

For the binding-neutral exact subset, build a canonical manager filter with an
issued field token and exact projected value. It is immutable and exposes
`all`, identity-strict `first`, `count`, `exists`, `update`, and `delete`; joins, roles, ordering,
pagination, and rich string predicates remain on the typed-query surface below.

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

rows = await session.query(person).where(working_age, not_ada).rows(limit=100)
```

Each `where(...)` argument is ANDed. `and_`, `or_`, and `not_` are available
when operator syntax is inconvenient.

## Roles

```python
employment = session.exact(Employment)
employee = employment.role(Employment.employee).connects(person)

rows = await session.query(person, employment).where(employee).rows(limit=100)
```

The generated role token proves its relation owner, declaring role, and allowed
player types.

## Ordering and windows

```python
rows = await (
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
`exists_by` share the same immutable query. Each terminal is awaited, on a
direct or a proxy connection alike (see
[Typed queries](typed-queries.md#start-a-session)).

## Raw TypeQL execution

TypeQL in, the driver's output out, the same pair in every SDK, and no promise: it runs any TypeQL in any transaction kind, schema changes that break the generated models included, without TypeBridge checks, hooks or limits.

The pair is spelled per SDK:

| SDK | Open a raw context | Execute TypeQL |
| --- | --- | --- |
| Python | `raw_transaction` | `execute_query` |
| TypeScript | `rawTransaction` | `executeQuery` |
| Rust | `raw_transaction` | `execute_query` |
| C | `type_bridge_database_raw_transaction` or `type_bridge_proxy_connection_raw_transaction` | `type_bridge_raw_transaction_execute_query` |

Direct contexts support several statements in one provider
transaction. Proxy contexts use the opt-in `/v3/raw` endpoint, which is off
by default. Use generated typed queries and plan builders when you need
schema validation and generated results.

The result is the complete HTTP query-answer dictionary; `answers` contains
its rows or documents. Python integers preserve the provider's full precision.
Use an explicit raw context to group direct statements:

```python
with db.raw_transaction("write") as tx:
    tx.execute_query('insert $p isa person, has name "Ada";')
    tx.execute_query('insert $p isa person, has name "Grace";')
    tx.rollback()
```

Clean exit commits unless `commit_on_exit=False` or the caller already completed
the context. An exception escaping the `with` block closes without committing.
If you catch an exception inside the block and exit normally, a direct context
commits its other successful queries. Call `rollback()` or set
`commit_on_exit=False` to discard pending writes.
On a proxy connection a raw
transaction holds one query, because `/v3/raw` runs each request as its own
transaction. A second query raises `RawQueryError` with code
`raw_transaction_single_query`; rollback after a proxy write raises
`raw_transaction_already_committed`. A disabled host raises `raw_disabled`.
Provider errors retain `code`, `message`, `status`, `provider_code` and
`commit_outcome`; `commit_outcome_unknown` means durability must be checked.
