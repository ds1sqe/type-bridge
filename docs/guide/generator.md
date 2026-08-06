# Generate application bindings

`type-bridge schema generate` projects a checked Split-YAML workspace into the
configured Python, TypeScript/Node, and Rust outputs. It is the only supported
model-generation entry point.

## Configure outputs

```yaml
format: typebridge.workspace/v1

schema:
  root: schema/schema.yaml
  ownership: exclusive
  managed-scope: application

compatibility:
  semantic-profile: typedb-3.12.1/v1

migrations:
  directory: migrations/v2
  app-label: application
  destructive: require-approval

bindings:
  python:
    output: generated/python/app_models
  typescript:
    output: generated/typescript
  rust:
    output: generated/rust
```

Output paths are confined relative to the workspace and may not overlap the
schema, migration directory, or each other.

## Generate

```bash
type-bridge --manifest typebridge.yaml schema check
type-bridge --manifest typebridge.yaml schema generate
```

`schema check` and `schema generate` are offline. They do not connect to TypeDB
or apply a migration. Use the explicit [migration workflow](migrations.md) for
database changes.

## Generated package contract

Every target contains:

- exact attribute, entity, relation, and reference types;
- field and role tokens tied to their declaring model;
- concise single-type CRUD managers;
- immutable direct and remote query facades;
- canonical runtime-projection and schema fingerprints;
- target-language declarations suitable for Pyright, TypeScript, or Rust.

Python and TypeScript packages install their embedded projection into the native
runtime during import. Installation verifies exact generated classes and
fingerprints. Rust generated crates bind the equivalent `SchemaPackage` through
the public SDK.

Generated code is deterministic for the same workspace inputs and toolchain
version. Do not edit, subclass, or reconstruct it. Change Split-YAML, review the
schema/migration diff, regenerate, and run the target type checker.

## Use the outputs

=== "Python"

    ```python
    from app_models import Age, Person, PersonId

    ada = Person(person_id=PersonId("ada"), age=Age(36))
    Person.manager(db).put(ada)
    ```

=== "TypeScript"

    ```ts
    import { Age, Person, PersonId } from "./generated/typescript/dist/index.js";

    const ada = Person.create({
      personId: PersonId.create("ada"),
      age: Age.create(36n),
    });
    Person.manager(db).put(ada);
    ```

=== "Rust"

    ```rust
    let person = db.entities::<Person>().put(PersonCreate::new(
        PersonId::new("ada".to_owned()),
        Some(Age::new(36)),
    )).await?;
    ```

Exact constructor order and optional fields in Rust are schema-generated; use
the emitted API rather than copying this illustrative signature.

## CI check

Run generation in a clean checkout and fail when tracked generated artifacts
change unexpectedly, or generate into the build directory and compile the clean
consumer. TypeBridge's own acceptance suites generate fresh packages and reject
dependencies on handwritten authoring modules.

Programmatic TypeQL/TOML-to-model functions and target-language model factories
are not generator alternatives. Historical TOML is supported only through the
read-only conversion described in [TOML recovery](toml.md).
