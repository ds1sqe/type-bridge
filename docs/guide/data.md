# Read and write data

TypeBridge offers two complementary application query styles over the same
Rust ORM runtime.

## Model managers

Use [CRUD managers](crud.md) for model-centered insert, put, update, delete,
filter, ordering, aggregation, and transaction workflows. The
[query-expression guide](queries.md) covers predicates, Boolean composition,
pagination, grouping, raw queries, and TypeDB 3.12 given-stage parameters.

Schema-defined [functions](functions.md) can be invoked through typed
function-query helpers.

## Immutable typed queries

Use [immutable typed queries](typed-queries.md) for connected multi-model
matches, owner-aware fields and roles, exact or subtype selection, bounded
reachability, named pages, counts, existence checks, and one-exchange remote
execution.

The immutable facade is additive. Existing managers and the package-root raw
query APIs remain available, so an application can adopt it per query.

## Transactions and execution location

Python, Node, and Rust can reuse caller-owned transactions for related
operations. Queries execute either directly against TypeDB through the embedded
runtime or remotely through the [TypeBridge server](server-container.md).
Remote composition performs no I/O; one terminal operation performs one
caller-owned exchange.
