# TypeDB functions

Functions are schema facts. Declare them in Split-YAML and generate the same
signature identity for every configured binding.

## Declare a function

```yaml
functions:
  find-events:
    parameters:
      - {name: event, type: event}
    returns:
      stream: [event]
    body:
      typeql: |-
        match
          $event isa event;
        return { $event };
```

Parameters and return values are structured schema types. `body.typeql` is the
provider-owned function body. The workspace checker validates the declaration
shape and selected semantic profile; provider execution remains capability-
gated.

```bash
type-bridge --manifest typebridge.yaml schema check
type-bridge --manifest typebridge.yaml migration make --name add-find-events
type-bridge --manifest typebridge.yaml migration apply --environment development
type-bridge --manifest typebridge.yaml schema generate
```

Generated packages expose an exact `FunctionRef`/function token with the
projected parameter and return types. For example, generated Python typing
preserves `FunctionRef[[Event], Iterator[Event]]`. Application code imports the
token from its generated package; it does not rebuild the signature in Python
or Node.

## Retained raw Python function queries

The separately retained raw query facade can call a known TypeDB function when
an application needs explicit TypeQL-shaped results:

```python
from type_bridge.expressions import FunctionQuery, ReturnType

query = FunctionQuery(
    name="find-events",
    args=[("$event", "0x123")],
    return_type=ReturnType(["event"], is_stream=True),
).to_query(limit=100)

with db.transaction("read") as tx:
    rows = tx.execute(query)
```

`FunctionQuery` is query construction only. It is not schema or model
authority, and it does not install a generated projection. Prefer generated
model-owned immutable queries when the result should hydrate as generated
models.

## Function shapes

TypeDB supports scalar, stream, parameterized, and composite returns. Use a
bounded limit for streams and test the exact function syntax against every
supported server line your deployment uses. TypeBridge validates binding/result
shape, but the TypeDB server executes the function body.

See [Split-YAML](split-yaml-v1.md), [generated bindings](generator.md), and
[queries](queries.md).
