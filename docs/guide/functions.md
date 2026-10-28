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

Declare exactly one return shape. For example, a scalar function uses
`returns: {scalar: integer}`, a composite single-row function uses
`returns: {tuple: [integer, string]}`, and a multi-row function uses
`returns: {stream: [event]}`. The generated immutable query vocabulary invokes
non-optional scalar functions; tuple and stream tokens remain available to
other typed result APIs and are rejected—not guessed—where unsupported.

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

## Execute a function

Use the generated function tokens in model-owned immutable queries for supported
scalar returns. For explicit transactions, build staged plans with the generated
package. Applications needing explicit TypeQL-shaped results can call a function
in raw TypeQL through `Database.execute_query`.

## Function shapes

TypeDB supports scalar, stream, parameterized, and composite returns. Use a
bounded limit for streams and test the exact function syntax against every
supported server line your deployment uses. TypeBridge validates binding/result
shape, but the TypeDB server executes the function body.

See [Split-YAML](split-yaml-v1.md), [generated bindings](generator.md), and
[queries](queries.md).
