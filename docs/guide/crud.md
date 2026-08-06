# CRUD and transactions

Every generated entity and relation class exposes a concise manager bound to
its verified package projection.

```python
from app_models import Age, Person, PersonId

ada = Person(person_id=PersonId("ada"), age=Age(36))
Person.manager(db).put(ada)
people = Person.manager(db).filter(age__gte=18).all()
```

No model registry or handwritten descriptor is involved. Native execution
accepts only the exact generated class and value wrappers installed by that
package.

## Entity manager operations

| Operation | Result |
| --- | --- |
| `insert(value)` | Insert one model and attach its IID. |
| `insert_many(values)` | Insert one homogeneous batch atomically. |
| `put(value)` | Idempotently match/insert by the declared key. |
| `put_many(values)` | Put one homogeneous batch atomically. |
| `update(value)` | Replace the exact IID-bearing Python model. |
| `delete(value_or_iid)` | Delete the exact generated model by IID. |
| `get_by_iid(iid)` | Return one hydrated model or `None`. |
| `filter(**lookups)` | Return a new immutable filtered manager. |
| `all()` / `first()` | Materialize all or the first match. |
| `count()` / `exists()` | Execute database-side terminals. |

```python
manager = Person.manager(db)

ada = manager.insert(Person(person_id=PersonId("ada"), age=Age(36)))
assert ada.iid is not None

ada.age = Age(37)
manager.update(ada)
assert manager.get_by_iid(ada.iid).age.value == 37

manager.delete(ada)
assert manager.get_by_iid(ada.iid) is None
```

`update` and `delete` require an attached canonical TypeDB IID. `put` requires
the generated model's projected key contract.

## Filters

Generated managers support `eq`, `ne`, `gt`, `gte`, `lt`, and `lte` suffixes.
No suffix means equality.

```python
adults = Person.manager(db).filter(age__gte=18)
assert adults.exists()
first = adults.first()
count = adults.count()
```

Pass the exact generated attribute wrapper or a compatible target-language
scalar. An exact wrapper is useful when the scalar domain could be ambiguous.

Generated field names may contain `__`. A complete field-name match wins unless
the key ends with a supported lookup whose prefix is also a field. Use an
explicit trailing `__eq` to select a field that collides with a lookup spelling:

```python
manager.filter(**{"foo__bar": FooBar(7)})
manager.filter(**{"score__gte__eq": ScoreGte(8)})
manager.filter(score__gte=Score(18))
```

The equivalent generated TypeScript filters use an object and generated target
names:

```ts
Person.manager(db).filter({ score__gte: Score.create(18n) }).all();
Person.manager(db).filter({ scoreGte__eq: ScoreGte.create(8n) }).all();
```

## Relation managers

Relations expose the same lifecycle and terminal operations. Their generated
constructor carries the role-player values:

```python
employment = Employment(employee=ada, employer=acme, since=Since(today))
Employment.manager(db).insert(employment)

stored = Employment.manager(db).get_by_iid(employment.iid)
Employment.manager(db).delete(stored)
```

Role players are lowered and hydrated through the installed projection. Exact
classes, reference forms, keys, and allowed `plays` facts are revalidated before
execution.

## Batches

```python
people = [
    Person(person_id=PersonId("ada"), age=Age(36)),
    Person(person_id=PersonId("grace"), age=Age(45)),
]
Person.manager(db).insert_many(people)
Person.manager(db).put_many(people)
```

A bulk call uses one transaction and returns values in input order. On supported
TypeDB 3.12/band-9 connections, eligible homogeneous entity inserts may use the
compiled `given`-stage fast path; fallback execution has the same result.

## Caller-owned transactions

Pass a transaction instead of a database to reuse it across generated managers
and query sessions:

```python
with db.transaction("write") as transaction:
    person_manager = Person.manager(transaction)
    employment_manager = Employment.manager(transaction)

    person = person_manager.put(
        Person(person_id=PersonId("ada"), age=Age(36))
    )
    employment_manager.insert(Employment(employee=person, employer=acme))
```

The context commits on normal exit and rolls back when an exception escapes.
A generated manager never commits a caller-owned transaction. Read transactions
can be shared with `Person.query(transaction)` for multiple terminal calls.

## Language differences

- Generated Python values are mutable; `update(value)` replaces the attached
  IID.
- Generated TypeScript values are immutable; use
  `update(iid, replacement)`.
- Generated Rust managers are async and use generated create/model/reference
  types; write transactions expose transaction-bound managers.

These are language-boundary differences. Entity/relation CRUD, IID behavior,
batch atomicity, filtering, terminals, and commit/rollback outcomes are covered
by the cross-binding generated-operation parity gate.

Use a package-local [immutable query](typed-queries.md) for joins, traversal,
selected shapes, ordering, pages, reductions, or grouping.
