# Cardinality, keys, uniqueness, and ordering

Declare multiplicity and annotations on the exact Split-YAML fact they govern.
The schema checker validates them once and every generated binding receives the
same contract.

## Cardinality forms

```yaml
entities:
  person:
    owns:
      person-id: {key: true}
      display-name: {card: 1}
      nickname: {card: {min: 0, max: 1}}
      aliases: {card: {min: 0, max: 3}}
      tags: {card: {min: 1}}
```

- `card: 1` means exactly one.
- `{min: 0, max: 1}` means optional scalar.
- A maximum above one produces a bounded collection.
- Omitting `max` means unbounded.
- `key: true` implies the provider key contract and is used by generated `put`.
- `unique: true` adds uniqueness without making the field the model key.

## Roles have two cardinality sides

```yaml
relations:
  employment:
    relates:
      employee: {card: 1}
      reviewer: {card: {min: 0, max: 3}}

plays:
  person:
    employment:
      employee: {card: {min: 0, max: 1}}
      reviewer: {card: {min: 0, max: 5}}
```

`relates` constrains one relation instance. `plays` constrains participation by
one player across relation instances. Do not substitute one for the other.

## Ordered and distinct collections

Ordering is a schema fact, not a property of the target-language container.
An unordered multi-value fact may be represented by a Python tuple or a
TypeScript array without promising stored order. Declare ordered/list semantics
explicitly in the supported Split-YAML form; `distinct` is valid only on an
ordered list.

Generated constructors enforce minimum/maximum bounds before execution. The
provider enforces the canonical schema again. Hydration preserves ordered
collections only when the schema and negotiated provider capability do.

## Generated shapes

| Schema multiplicity | Python | TypeScript | Rust |
| --- | --- | --- | --- |
| exactly one | value | value | value |
| zero or one | `Value | None` | optional/null form | `Option<Value>` |
| many | immutable tuple/sequence input | readonly array | generated collection |

Exact generated signatures are the source of truth for a particular schema;
run the target type checker after generation.
