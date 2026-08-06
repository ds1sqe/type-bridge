# Python quick start

TypeBridge applications author schema in Split-YAML and import generated Python
bindings. The generated package owns attribute values, entity and relation
models, concise managers, and immutable query tokens.

## 1. Create the workspace

Create `typebridge.yaml`:

```yaml
format: typebridge.workspace/v1

schema:
  root: schema/schema.yaml
  ownership: exclusive
  managed-scope: quickstart

compatibility:
  semantic-profile: typedb-3.12.1/v1

migrations:
  directory: migrations/v2
  app-label: quickstart
  destructive: require-approval

bindings:
  python:
    output: app_models

environments:
  development:
    database: quickstart
    uri: localhost:1729
    tls: 'false'
    migrate: 'false'
    credential:
      username: env:TYPEDB_USERNAME
      password: env:TYPEDB_PASSWORD
```

Create `schema/schema.yaml`:

```yaml
format: typebridge.schema-set/v1
sources: [application.yaml]
```

Create `schema/application.yaml`:

```yaml
format: typebridge.schema/v2

attributes:
  person-id: {value: string}
  age: {value: integer}
  company-id: {value: string}

entities:
  person:
    owns:
      person-id: {key: true}
      age: {card: {min: 0, max: 1}}
  company:
    owns:
      company-id: {key: true}

relations:
  employment:
    relates:
      employee: {card: 1}
      employer: {card: 1}

plays:
  person:
    employment: [employee]
  company:
    employment: [employer]
```

## 2. Check, migrate, and generate

```bash
type-bridge --manifest typebridge.yaml schema check
type-bridge --manifest typebridge.yaml migration make --name initial
type-bridge --manifest typebridge.yaml migration apply --environment development
type-bridge --manifest typebridge.yaml schema generate
```

`schema check` is offline. Migration application is the explicit database
change; generation never mutates TypeDB. Commit the workspace and migration
history, and regenerate bindings after an accepted schema change.

## 3. Put and filter one type

```python
from app_models import Age, Person, PersonId
from type_bridge import Database

db = Database(address="localhost:1729", database="quickstart")
db.connect()

ada = Person(person_id=PersonId("ada"), age=Age(36))
Person.manager(db).put(ada)

adults = Person.manager(db).filter(age__gte=18).all()
for person in adults:
    print(person.person_id.value, person.age.value if person.age else None)
```

`put` is idempotent for the generated model's declared key. Managers also
provide `insert`, `insert_many`, `put_many`, `update`, `delete`, `get_by_iid`,
`all`, `first`, `count`, and `exists`. Pass a database for an owned transaction
or an existing transaction for an atomic multi-operation workflow.

## 4. Insert a relation

```python
from app_models import Company, CompanyId, Employment

acme = Company(company_id=CompanyId("acme"))
Company.manager(db).put(acme)

employment = Employment(employee=ada, employer=acme)
Employment.manager(db).insert(employment)
```

Generated relation constructors accept only projected player types allowed by
the Split-YAML `plays` facts. Hydrated relations preserve their concrete player
types and IIDs.

## 5. Query across types

```python
from app_models import Employment

session = Person.query(db)
person = session.exact(Person)
employment = session.exact(Employment)

employee = employment.role(Employment.employee).connects(person)
rows = (
    session.query(person, employment)
    .where(employee, person.field(Person.age).gte(Age(18)))
    .rows(limit=100)
)
```

Use `exact` for one generated type and `subtypes` for a polymorphic binding.
The package-local query API supports owner-aware fields and roles, Boolean and
string predicates, bounded reachability, explicit cross joins, positional or
named selections, collection, ordering, pages, counts, existence checks, and
direct reductions/grouping.

## Next steps

- [Model TypeDB data](../guide/models.md)
- [CRUD and transactions](../guide/crud.md)
- [Immutable generated queries](../guide/typed-queries.md)
- [Split-YAML reference](../guide/split-yaml-v1.md)
- [Schema and migration workflows](../guide/schema-workflows.md)
