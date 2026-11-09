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

Single writes return `WriteResult` with `count`, `items`, and `item`. `item`
is present only when exactly one root was written. Python attaches the input
facade's IID only in that case; zero or multiple roots leave it unchanged.
Generated Python managers use the same native runtime for ordered and unordered
schemas. They hydrate and stage publication before completing the write.
Regenerate older packages to replace their V1 manager resource.
Ordered and unordered Node packages now expose the same manager operations;
regenerate older Node packages to use that surface.
A failed owned write restores the input facade; a materialization failure in a
borrowed write makes that transaction rollback-only.

## Entity manager operations

| Operation | Result |
| --- | --- |
| `insert(value)` | Insert one model and attach its IID. |
| `insert_many(values)` | Insert one homogeneous batch atomically. |
| `put(value)` | Idempotently match/insert by the declared key; keyless models use TypeQL `put` of the given values. |
| `put_many(values)` | Put one homogeneous batch atomically. |
| `update(value)` | Update by attached canonical TypeDB IID. |
| `delete(value_or_iid)` | Delete by attached canonical TypeDB IID or an `Iid` value. |
| `get_by_iid(iid)` | Return one hydrated model or `None`. |
| `filter(**lookups)` | Return a new immutable filtered manager. |
| `where(field, comparison, value)` | Start an immutable exact field-token filter. |
| `all()` / `first()` | Materialize all or the first match. |
| `count()` / `exists()` | Execute database-side terminals. |

```python
manager = Person.manager(db)

ada = manager.insert(Person(person_id=PersonId("ada"), age=Age(36))).item
assert ada is not None
assert ada.iid is not None

ada.age = Age(37)
manager.update(ada)
assert manager.get_by_iid(ada.iid).age.value == 37

manager.delete(ada)
assert manager.get_by_iid(ada.iid) is None
```

Python `update`, `update_many`, and model-valued deletion require attached
canonical TypeDB IIDs. They do not look up detached models or discard inputs.
Read the desired models first, then pass them to the explicit IID operations.
A keyed `put` looks up the declared key and replaces the other fields. A
keyless `put` is TypeQL `put` on the exact model type: it returns an instance
holding every given value and role player, or inserts the pattern; it never
updates, does not match subtype instances, and fails with
`put_ambiguous_match` when several instances match. See
[Keyless put](upgrade-3.0.md#keyless-put).

The callback `update_with` and no-argument/filtered `delete()` conveniences
are removed. Use an authored update/delete plan staged in one transaction
for a database-side filtered mutation. For application-computed values,
read the models, change them, then call `update_many` with their attached IIDs.

## Filters

Generated managers retain the compatibility `filter(...)` surface for IID,
raw-scalar and rich-operator read workflows. Its scalar lookup
suffixes are `eq`, `ne`, `gt`, `gte`, `lt`, and `lte`; no suffix means
equality.

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

For an exact generated-field contract, use the distinct immutable `where`
filter. It accepts an issued field token, one of the closed six comparisons,
and the field's exact generated attribute value. Ordered and unordered packages
expose the same native manager surface.

```python
from app_models import ProjectedManagerComparison

root = Person.manager(db).where()
adult = root.where(Person.age, ProjectedManagerComparison.GTE, Age(18))
assert root.count() >= adult.count()  # the parent remains reusable

ada = Person.manager(db).where(
    Person.person_id,
    ProjectedManagerComparison.EQ,
    PersonId("ada"),
).first()
```

```ts
const root = Person.manager(db).where();
const adult = root.where(Person.fields.age, "gte", Age.create(18n));
const ada = Person.manager(db)
  .where(Person.fields.personId, "eq", PersonId.create("ada"))
  .first();
```

Canonical filters expose `where`, `all`, `first`, `count`, and `exists`.
Python filters also expose `delete()` and `update(replacement)`; these mutations
act on every match and do not run instance CRUD hooks. Python
`filter(iid=Iid("0x1234"))` retains the IID selection for those terminals.
Use `iid__in` for a list or iterable of `Iid` values; an empty list matches
nothing without a provider request. String IID arguments are refused.
Their `first` is identity-strict: supply a single IID or one semantically
consistent equality predicate for every effective reference-key field. An unkeyed, partial, conflicting, or otherwise
nonsingular filter fails before provider I/O. Compatibility-manager `first()`
retains its released arbitrary-first behavior.

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
result = Person.manager(db).insert_many(people)
assert result.count == len(result.items) == len(result.input_indexes)
Person.manager(db).put_many(people)
```

Python, Rust, and Node batch writes return one `BatchWriteResult`. Its `items`
contain every root in input order; `input_indexes` (`inputIndexes` in Node)
identify their original input rows. A missing input contributes no item, and
several roots from one input repeat that input's index. Rust exposes `items()`
and `input_indexes()` and can consume the models with `into_items()`.

A batch call compiles one plan and runs every statement in the same transaction.
Python preserves an input model's identity only when that input produces exactly
one root. Zero or multiple roots leave it unchanged. A failure during hydration
or publication rolls back the owned write or makes a borrowed write rollback-only.

## Caller-owned transactions

Pass a transaction instead of a database to reuse it across generated managers
and query sessions:

```python
with db.transaction("write") as transaction:
    person_manager = Person.manager(transaction)
    employment_manager = Employment.manager(transaction)

    person = person_manager.put(
        Person(person_id=PersonId("ada"), age=Age(36))
    ).item
    assert person is not None
    employment_manager.insert(Employment(employee=person, employer=acme))
```

The context commits on normal exit and rolls back when an exception escapes.
A generated manager never commits a caller-owned transaction. Read transactions
can be shared with `Person.query(transaction)` for multiple terminal calls.
Canonical manager filters can borrow that same read transaction; each terminal
leaves it active for sibling filters and typed queries.

## Atomic conditional writes

Generated managers write without a condition. When a write must happen only
if a condition holds (enough stock, enough balance), build it as one typed
plan and run it in a staged transaction: the check and the writes run in one
TypeDB transaction, and a plan that matches no row writes nothing.

```python
from type_bridge.plan import Matched

with db.staged_transaction("write") as tx:
    bought = tx.stage(plan)  # one plan: match, let, update, insert
# Committed on exit. Matched(0): the condition did not hold, nothing written.
ok = bought.result() == Matched(1)
```

A lost race fails the commit with `CommitConflict`; run the whole transaction
again. Reading a value does not protect it from a concurrent change, so write
what the rule depends on. [Concurrency and conditional writes](concurrency.md)
has the purchase plan in every SDK, the lock rules, retries and typed failures.
[Checkout: a whole cart in one query](checkout.md) checks and writes a whole
shopping cart, stock and balance, in one plan.

## Language differences

- Generated Python values are mutable; `update(value)` replaces the attached
  IID. Canonical filters accept only field tokens issued by that generated
  Python package instance.
- Generated TypeScript values are immutable; use
  `update(iid, replacement)`. Canonical filters require the package-local
  token identity retained by the generated runtime.
- Generated Rust managers are async and use generated create/model/reference
  types; write transactions expose transaction-bound managers. Rust rejects
  foreign generated token types statically and revalidates token metadata at
  runtime.

These are language-boundary differences. Entity/relation CRUD, IID behavior,
batch atomicity, filtering, terminals, and commit/rollback outcomes are covered
by the cross-binding generated-operation parity gate.

Use a package-local [immutable query](typed-queries.md) for joins, traversal,
selected shapes, ordering, pages, reductions, or grouping.
