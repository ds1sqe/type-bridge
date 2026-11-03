# Upgrading to 3.0

## Single-item manager write results

Single-item `insert`, `put`, `update`, and `delete_returning` calls now return
`WriteResult`, including calls made in an explicit transaction. Node names the
last method `deleteReturning`. The result reports every root thing answered by
TypeDB, in answer order:

- `count`: the number of root things written;
- `items`: all hydrated root things;
- `item`: the single thing when `count == 1`, otherwise `None` / `null`.

Dynamic relation lookups using non-key fields can match two players and insert
two relations. Both appear in `items`; the call does not choose one or reject the
answer. Generated references remain key-only and cannot fan out by construction,
but use the same result shape. Repeated references to the same player follow
TypeQL behavior. Missing references retain their existing typed lookup failure.
No strict mode or additional pre-flight guard is added. The `_many` methods
continue to return lists.

For Python, change:

```python
person = people.insert(Person(name=Name("Ada")))
```

To:

```python
result = people.insert(Person(name=Name("Ada")))
person = result.item
```

Check `result.count` or iterate `result.items` when your references can fan out.
A single write that matches no root returns `count == 0`, empty `items`, and
`item is None`, including a missing referenced player or an absent update root.
It does not run a diagnostic lookup or raise the former missing-reference error.
The rare wide-row existence guard still prevents partially created relations.
Python attaches the input instance IID and refreshes that instance only when the
result contains exactly one root. For zero or several roots, the input instance
is unchanged and `items` contains the returned hydrated instances.

For Node, use `people.insert(input).item`, or iterate the returned `items`.
For Rust, retain the result with `let result = people.insert(input).await?;`,
then use `result.item()` to borrow the single model, or `result.into_items()` to
take ownership of all models. Rust exposes `count()`, `items()`, and `item()`
accessors.

## Core manager construction

Core descriptor managers require registry authority. Replace
`DynamicEntityManager::new` / `new_canonical` and
`DynamicRelationManager::new` / `new_canonical` with `::from_registry`.
Replace their `with_transaction` / `with_canonical_transaction` constructors
with `::with_registry_transaction`. Use `literal_values()` when the owning
provider requires literal transport.

The generic core `EntityManager<T>::new` and `RelationManager<R>::new`
constructors are replaced by `::from_registry` too. Their single writes and
the dynamic managers' single writes return `WriteResult`, including exact
put/update forms and returning deletes. Generic results contain hydrated models;
dynamic results contain hydrated rows. The many-result methods retain their
list contracts.
