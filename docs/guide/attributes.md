# Attributes

Attributes are independent schema types. Declare them once in Split-YAML, then
use their generated value classes wherever an entity or relation owns them.

## Declare attribute types

```yaml
format: typebridge.schema/v2

attributes:
  person-id:
    value: string
  age:
    value:
      type: integer
      range: {min: 0, max: 150}
  email:
    value:
      type: string
      regex: '^[^@]+@[^@]+$'
  status:
    value:
      type: string
      values: [active, inactive]
  balance:
    value: decimal
  birthday:
    value: date
  seen-at:
    value: datetime-tz
  session-length:
    value: duration
  audit-note:
    independent: true
    value: string
```

The built-in value types are `string`, `integer`, `double`, `decimal`,
`boolean`, `date`, `datetime`, `datetime-tz`, and `duration`. `regex`, `range`,
and `values` constrain the value declaration. `doc` and `meta` attach provider
documentation and metadata. `independent: true` permits an attribute instance
to exist without an owner.

Run `type-bridge schema check` before generation. Invalid combinations,
unsupported annotations, and scalar constraint mismatches fail in the Rust
schema engine.

## Generated values

=== "Python"

    ```python
    from app_models import Age, Balance, Email
    from decimal import Decimal

    age = Age(36)
    email = Email("ada@example.com")
    balance = Balance(Decimal("12.50"))

    assert age.value == 36
    ```

=== "TypeScript"

    ```ts
    import { Age, Balance, Email } from "./generated/models/index.js";

    const age = Age.create(36n);
    const email = Email.create("ada@example.com");
    const balance = Balance.create("12.50");
    ```

=== "Rust"

    ```rust
    let age = Age::new(36);
    let email = Email::new("ada@example.com".to_owned());
    ```

Integer values use Python `int`, JavaScript `bigint`, and the generated Rust
integer type. Decimal and duration boundaries remain lossless strings or native
language values according to the generated target API.

Generated attribute classes are projection values, not declaration bases. To
rename an attribute or change its scalar/constraint contract, edit Split-YAML,
review a migration, and regenerate.

## Ownership is separate

An attribute declaration does not imply an owner. Add it under an entity or
relation's `owns` facts:

```yaml
entities:
  person:
    owns:
      person-id: {key: true}
      age: {card: {min: 0, max: 1}}
      email: {unique: true, card: {min: 0, max: 1}}
```

The generated model constructor and field token are derived from these facts.
See [cardinality](cardinality.md), [entities](entities.md), and
[generated queries](typed-queries.md).
