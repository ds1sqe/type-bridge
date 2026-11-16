# Relations

Relations are generated from Split-YAML `relations` and `plays` facts. Each
generated role token preserves both its declaring relation and permitted player
types.

## Declare roles and players

```yaml
attributes:
  person-id: {value: string}
  company-id: {value: string}
  since: {value: date}

entities:
  person:
    owns: {person-id: {key: true}}
  company:
    owns: {company-id: {key: true}}

relations:
  employment:
    owns:
      since: {card: {min: 0, max: 1}}
    relates:
      employee: {card: 1}
      employer: {card: 1}

plays:
  person:
    employment:
      employee: {card: {min: 0, max: 1}}
  company:
    employment: [employer]
```

Relates-side cardinality constrains players on one relation instance. Plays-side
cardinality constrains how often a player participates in that role.

An unordered role is a set: a repeated player counts once toward its
relates-side cardinality. Insert, put, batch put, and update normalize equivalent
player references before checking that cardinality. Model normalization keeps
positions and repetitions in ordered roles; `distinct: true` rejects repetitions.
Supported TypeDB providers currently refuse writes with populated ordered roles.

## Construct and write a generated relation

```python
from app_models import Company, CompanyId, Employment, Person, PersonId, Since
from datetime import date

ada = Person.manager(db).put(Person(person_id=PersonId("ada"))).item
acme = Company.manager(db).put(Company(company_id=CompanyId("acme"))).item
assert ada is not None and acme is not None

employment = Employment(
    employee=ada,
    employer=acme,
    since=Since(date(2026, 1, 1)),
)
Employment.manager(db).insert(employment)
```

Generated constructors reject a player from the wrong package, type, role, or
projection. A generated reference can be used when an IID/key is already known
and a complete hydrated model is unnecessary.

## Query through roles

```python
session = Employment.query(db)
person = session.exact(Person)
employment = session.exact(Employment)

employee = employment.role(Employment.employee).links(person)
rows = await session.query(person, employment).where(employee).rows(limit=100)
```

Role tokens are owner-aware: `Employment.employee` cannot be applied to a
binding of another relation, and the connected player binding must be allowed
by the canonical `plays` facts.

## Relation inheritance and specialization

```yaml
relations:
  participation:
    abstract: true
    relates: [participant]
  employment:
    sub: participation
    relates:
      employee: {as: participant, card: 1}
```

`as` specializes a parent role. Generated constructors expose the effective
child role, while exact/subtype queries preserve the canonical declaring-role
identity required by TypeDB. Specialization is supported in the first
migration and when adding new relation types. Changing specialization on an
existing relation uses a no-instances assertion, including its subtypes;
populated relations require an explicit data migration before that change.
A specializing role must satisfy both its local cardinality (including the
default) and the parent's cardinality. Generated create inputs use their
intersection, so an inherited minimum of one makes the specializing player
required even when the child omits `card`.


Ordered/list roles are declared with the Split-YAML ordering facts supported by
the selected semantic profile. Provider capability checks remain authoritative;
generation never claims an operation the provider runtime cannot execute.
