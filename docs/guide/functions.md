# TypeDB Functions

TypeDB 3.x supports schema-defined functions using the `fun` keyword. TypeBridge provides the `FunctionQuery` class to generate TypeQL queries for calling these functions from Python.

## Overview

Functions in TypeDB are defined in the schema and executed as part of queries. TypeBridge's code generator can parse function definitions and generate Python wrapper functions that return `FunctionQuery` objects.

## Function Patterns

TypeDB functions support several patterns:

| Pattern | Schema Example | TypeQL Syntax |
|---------|---------------|---------------|
| Single value | `fun count() -> integer` | `let $x = count();` |
| Stream | `fun list-ids() -> { id }` | `let $x in list-ids();` |
| Parameterized | `fun get($id: string) -> entity` | `let $e = get("abc");` |
| Composite | `fun divide($a: int, $b: int) -> int, int` | `let ($q, $r) = divide(10, 3);` |

## Using FunctionQuery

### Basic Usage

```python
from type_bridge.expressions import FunctionQuery, ReturnType

# Define a function query
fn = FunctionQuery(
    name="count-artifacts",
    return_type=ReturnType(["integer"]),
)

# Generate the TypeQL query
query = fn.to_query()
# Output:
# match let $integer = count-artifacts();
# fetch { "integer": $integer };

# Execute against database
with db.transaction() as tx:
    results = tx.execute(query)
    count = results[0]["integer"]
```

### Stream Functions

Functions that return multiple rows use `{ }` syntax in TypeDB:

```python
# Schema: fun list-user-ids() -> { id }:
fn = FunctionQuery(
    name="list-user-ids",
    return_type=ReturnType(["id"], is_stream=True),
)

query = fn.to_query(limit=10)
# Output:
# match let $id in list-user-ids();
# limit 10;
# fetch { "id": $id };
```

### Parameterized Functions

Pass arguments to functions:

```python
# Schema: fun get-neighbors($target_id: string) -> { neighbor }:
fn = FunctionQuery(
    name="get-neighbors",
    args=[("$target_id", "art-001")],
    return_type=ReturnType(["neighbor"], is_stream=True),
)

query = fn.to_query()
# Output:
# match let $neighbor in get-neighbors("art-001");
# fetch { "neighbor": $neighbor };
```

### Query Modifiers

Add pagination, sorting, and other modifiers:

```python
fn = FunctionQuery(
    name="list-scores",
    return_type=ReturnType(["id", "score"], is_stream=True),
)

query = fn.to_query(
    limit=10,
    offset=20,
    sort_var="score",
    sort_order="desc",
)
# Output:
# match let ($id, $score) in list-scores();
# sort $score desc;
# offset 20;
# limit 10;
# fetch { "id": $id, "score": $score };
```

## FunctionQuery API

### Constructor

```python
FunctionQuery(
    name: str,                      # TypeDB function name
    return_type: ReturnType,        # Return type description
    args: list[tuple[str, Any]] = [],  # Function arguments
    docstring: str | None = None,   # Optional documentation
)
```

### ReturnType

```python
ReturnType(
    types: list[str],           # List of return type names
    is_stream: bool = False,    # True if returns multiple rows
    is_optional: list[bool] = [],  # Optional flags per type
)
```

### Methods

| Method | Description |
|--------|-------------|
| `to_call()` | Generate function call expression: `func-name(args)` |
| `to_match_let(result_vars)` | Generate match let clause |
| `to_fetch(result_vars, fetch_keys)` | Generate fetch clause |
| `to_query(limit, offset, sort_var, sort_order)` | Generate complete query |
| `to_reduce_query()` | Generate reduce query (non-stream only) |
| `with_args(**kwargs)` | Create copy with bound arguments |

### Properties

| Property | Description |
|----------|-------------|
| `return_type.is_stream` | True if function returns multiple rows |
| `return_type.is_composite` | True if function returns tuples |
| `return_type.is_single_value` | True if single non-stream value |

## Code Generation

When using the TypeBridge generator with a schema containing functions, Python wrapper functions are automatically generated:

### Schema (schema.tql)

```typeql
define
attribute artifact-id, value string;

entity artifact,
    owns artifact-id @key;

fun count-artifacts() -> integer:
    match $a isa artifact;
    return count($a);

fun list-artifact-ids() -> { artifact-id }:
    match $a isa artifact, has artifact-id $id;
    return { $id };

fun get-artifact-by-id($id: string) -> artifact:
    match $a isa artifact, has artifact-id $id;
    return first $a;
```

### Generated Code (functions.py)

```python
from typing import Iterator
from type_bridge.expressions import FunctionQuery, ReturnType


def count_artifacts() -> FunctionQuery[int]:
    """Call TypeDB function `count-artifacts`.

    Returns: integer
    """
    return FunctionQuery(
        name="count-artifacts",
        args=[],
        return_type=ReturnType(["integer"], is_stream=False),
    )


def list_artifact_ids() -> FunctionQuery[Iterator[str]]:
    """Call TypeDB function `list-artifact-ids`.

    Returns: stream of artifact-id
    """
    return FunctionQuery(
        name="list-artifact-ids",
        args=[],
        return_type=ReturnType(["artifact-id"], is_stream=True),
    )


def get_artifact_by_id(id: str | str) -> FunctionQuery[str]:
    """Call TypeDB function `get-artifact-by-id`.

    Returns: artifact
    """
    return FunctionQuery(
        name="get-artifact-by-id",
        args=[("$id", id)],
        return_type=ReturnType(["artifact"], is_stream=False),
    )
```

### Using Generated Functions

```python
from myschema.functions import count_artifacts, list_artifact_ids

# Simple count
fn = count_artifacts()
query = fn.to_query()
with db.transaction() as tx:
    results = tx.execute(query)
    total = results[0]["integer"]

# Stream with pagination
fn = list_artifact_ids()
query = fn.to_query(limit=100)
with db.transaction() as tx:
    results = tx.execute(query)
    ids = [r["artifact_id"] for r in results]
```

## Limitations

### Composite Stream Functions

TypeDB 3.x does not support destructuring tuples directly from stream functions. The syntax `let ($a, $b) in stream_func()` is not valid.

For functions returning streams of tuples, you may need to use a different approach or restructure the function.

### Runtime Execution

Functions are executed by TypeDB, not by TypeBridge. The `FunctionQuery` class only generates the TypeQL query string - actual execution happens when you run the query against the database.

## Best Practices

1. **Use the generator** - Let TypeBridge generate function wrappers from your schema
2. **Add pagination** - Use `limit` and `offset` for stream functions
3. **Handle errors** - Wrap execution in try/except for TypeDB errors
4. **Test against real DB** - Function syntax depends on TypeDB version

## See Also

- [Generator Documentation](generator.md) - Code generation from schemas
- [Queries Documentation](queries.md) - Query expressions and filtering
- [CRUD Operations](crud.md) - Working with entities and relations
