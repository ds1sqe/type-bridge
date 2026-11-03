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
Python attaches the input instance IID and refreshes that instance only when the
result contains exactly one root. For zero or several roots, the input instance
is unchanged and `items` contains the returned hydrated instances.

For Node, use `people.insert(input).item`, or iterate the returned `items`.
For Rust, retain the result with `let result = people.insert(input).await?;`,
then use `result.item()` to borrow the single model, or `result.into_items()` to
take ownership of all models. Rust exposes `count()`, `items()`, and `item()`
accessors.
