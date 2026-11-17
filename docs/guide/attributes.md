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
    let age = Age::new(36)?;
    let email = Email::new("ada@example.com".to_owned())?;
    ```

Integer values use Python `int`, JavaScript `bigint`, and the generated Rust
integer type. Decimal and duration boundaries remain lossless strings or native
language values according to the generated target API.

Generated attribute classes are projection values, not declaration bases. To
rename an attribute or change its scalar/constraint contract, edit Split-YAML,
review a migration, and regenerate.

## Python value ranges

| Schema value | Python output and exact range |
| --- | --- |
| `integer` | `int`, −2⁶³ through 2⁶³−1 |
| `double` | Finite binary64 `float` |
| `decimal` | `Decimal`, −9223372036854775808 through 9223372036854775807.9999999999999999999, at most 19 fractional digits; exponent-form inputs are accepted |
| `boolean`, `string` | `bool`, Unicode `str` (default maximum 1 MiB of UTF-8) |
| `date` | `date`, years 1–9999 in the native view; canonical text supports years −262143 through 262142 |
| `datetime`, `datetime-tz` | `datetime`, years 1–9999 and microsecond precision in the native view; canonical text retains nanoseconds, fixed offsets, and IANA zones |
| `duration` | `timedelta` with no months and microsecond precision in the native view; canonical text retains months, days, and nanoseconds |

Temporal attribute constructors accept native values or canonical strings.
Every temporal wrapper exposes read-only `.canonical`. Hydration retains that
text privately, so passing an unchanged attribute to `put` or `update` preserves
its exact value. Assign a newly constructed attribute to change the value.

A temporal value outside the native view's range still hydrates with its model.
Accessing that attribute's `.value` raises `ValueError` pointing to `.canonical`;
other fields remain accessible. For example, `SeenAt("2024-01-01T00:00:00.123456789+09:00[Asia/Seoul]")`
retains the zone and all nine fractional digits through `.canonical`.

Scalar-only query results have no wrapper. Errors name the scalar domain and
zero-based column ordinal when a value exceeds the native range or precision; select the attribute or model to access `.canonical`.
Timezone-aware scalar results carry the resolved instant.

Duration strings use canonical `P<months>M<days>DT<seconds>[.<fraction>]S`
components with zero components omitted (`PT0S` for zero), at most nine
fractional digits, and no redundant trailing fractional zeros. Python and Node
accept the same canonical string grammar. Provider writes require nonnegative
values, months and days fitting `u32`, and total time nanoseconds fitting `u64`.

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
