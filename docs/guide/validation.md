# Validation

Validation happens at four boundaries: canonical schema loading, generated
language types, generated runtime values, and provider execution.

## Schema validation

```bash
type-bridge --manifest typebridge.yaml schema check
```

The Rust schema engine rejects unknown keys, duplicate facts, invalid labels,
unsupported annotations, incompatible constraints, unresolved supertypes,
invalid role players, and cardinality conflicts. This check is offline and
performs no database mutation.

## Generated static validation

Generated packages encode constructor fields, optionality, collection shapes,
attribute scalars, reference keys, field ownership, and role-player types.

```python
person = Person(person_id=PersonId("ada"), age=Age(36))
```

```ts
const person = Person.create({
  personId: PersonId.create("ada"),
  age: Age.create(36n),
});
```

Pyright, TypeScript, and Rust reject wrong projected types before execution.
Generated field and role tokens are owner-aware, so a token from another model
or generated package cannot be composed into the query.

## Runtime validation

Runtime projection installation verifies canonical declared-schema and
projection fingerprints. Managers and query sessions then accept only the exact
classes installed for that package. A class with matching-looking attributes is
not a generated model and is rejected.

Value constructors enforce scalar domains and collection bounds. Native
lowering revalidates exact attribute wrappers, role players, IIDs, query
connectivity, limits, and result evidence before hydration.

## Provider validation

Migration planning checks the selected semantic profile and environment
capabilities. TypeDB remains authoritative for provider constraints and commit
conflicts. Treat a successful `schema generate` as code generation, not proof
that a migration was applied.

Stable diagnostics cross direct and remote boundaries. Handle them by category
or stable code rather than parsing incidental message text.
