# Entities

Declare entity types, ownership, and inheritance in Split-YAML. Generation
produces exact application classes and query tokens for each target language.

## Declare an entity

```yaml
attributes:
  person-id: {value: string}
  name: {value: string}
  age: {value: integer}
  aliases: {value: string}

entities:
  person:
    doc: A person in the application domain.
    owns:
      person-id: {key: true}
      name: {card: 1}
      age: {card: {min: 0, max: 1}}
      aliases: {card: {min: 0, max: 3}}
```

`key: true` identifies the idempotent `put` key. An exact `card: 1` field is
required, `0..1` is optional, and a maximum above one generates a collection.

## Construct generated entities

=== "Python"

    ```python
    from app_models import Age, Name, Person, PersonId

    ada = Person(
        person_id=PersonId("ada"),
        name=Name("Ada"),
        age=Age(36),
    )
    ```

=== "TypeScript"

    ```ts
    const ada = Person.create({
      personId: PersonId.create("ada"),
      name: Name.create("Ada"),
      age: Age.create(36n),
    });
    ```

=== "Rust"

    ```rust
    let ada = PersonCreate::new(
        PersonId::new("ada".to_owned()),
        Name::new("Ada".to_owned()),
        Some(Age::new(36)),
    );
    ```

Target-language names are generated deterministically from schema labels. The
canonical TypeDB label remains embedded in the verified projection.

## Inheritance and abstract entities

```yaml
entities:
  party:
    abstract: true
    owns:
      party-id: {key: true}
  person:
    sub: party
    owns:
      age: {card: {min: 0, max: 1}}
  employee:
    sub:
      type: person
      doc: A working person.
    owns:
      rank: {card: 1}
```

Construct concrete generated types. Query an exact type with `exact(Person)` or
the full concrete hierarchy with `subtypes(Party)`. Hydration returns the exact
generated subtype proven by the query result.

## CRUD

```python
Person.manager(db).put(ada)
stored = Person.manager(db).get_by_iid(ada.iid)
adults = Person.manager(db).filter(age__gte=18).all()
```

The manager is bound to the generated model's installed projection; arbitrary
classes cannot register themselves as entities. See [CRUD and transactions](crud.md).
