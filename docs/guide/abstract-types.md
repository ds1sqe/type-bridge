# Abstract types

Abstract entities, relations, and attributes are declared in Split-YAML with
`abstract: true`. They define shared schema and polymorphic query boundaries but
are not concrete values to insert.

```yaml
attributes:
  identifier:
    abstract: true
    value: string
  employee-id:
    sub: identifier

entities:
  party:
    abstract: true
    owns:
      employee-id: {key: true}
  person:
    sub: party
  company:
    sub: party

relations:
  participation:
    abstract: true
    relates: [participant]
  employment:
    sub: participation
    relates:
      employee: {as: participant}
```

Generation emits type-safe tokens for abstract query roots and constructors only
for valid concrete model forms.

## Exact versus subtype queries

```python
session = Person.query(db)
only_people = session.exact(Person)
all_parties = session.subtypes(Party)

people = session.query(only_people).rows(limit=100)
parties = session.query(all_parties).rows(limit=100)
```

`exact` matches exactly the selected schema type. `subtypes` includes concrete
descendants and hydrates each result as its exact generated class. The same
distinction is available in generated Node and Rust queries.

Inherited ownership and roles are resolved by the Rust schema engine before
projection. Generated code does not recompute a hierarchy by inspecting target-
language inheritance.
