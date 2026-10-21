# Read and write data

Generated TypeBridge packages offer two complementary application query styles
over the same Rust ORM runtime.

## Model managers

Use each generated model's [manager](crud.md) for concise single-type insert,
put, update, delete, IID lookup, filtering, and terminal workflows. Managers
can own a transaction or reuse a caller-provided transaction.

Schema-defined [functions](functions.md) can be invoked through typed
function-query helpers.

## Immutable typed queries

Use package-local [immutable typed queries](typed-queries.md) for connected multi-model
matches, owner-aware fields and roles, exact or subtype selection, bounded
reachability, named pages, counts, existence checks, and one-exchange remote
execution.

Use a manager for one type and a query session when the result or predicate
spans types. The package-root raw `Query`/`QueryBuilder` compatibility facade is
separate from the generated query contract.

## Transactions and execution location

Python, Node, and Rust can reuse caller-owned transactions for related
operations. Queries execute either directly against TypeDB through the embedded
runtime or remotely through the [TypeBridge server](server-container.md).
Remote composition performs no I/O; one terminal operation performs one
caller-owned exchange.

For a write that must happen only if a condition holds, stage a typed plan in
a staged transaction: the check and the writes commit together or not at all.
[Concurrency and conditional writes](concurrency.md) shows the pattern and the
lock rules in every SDK, and [Checkout](checkout.md) applies it to a whole
shopping cart.
