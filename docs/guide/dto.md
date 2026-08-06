# Application DTOs

Generated TypeBridge models are persistence/query values bound to a canonical
schema projection. Keep transport-specific request and response DTOs in the
application boundary instead of using them as another schema authority.

## Recommended boundary

```python
from dataclasses import dataclass

from app_models import Age, Person, PersonId


@dataclass(frozen=True, slots=True)
class CreatePersonRequest:
    person_id: str
    age: int | None = None


def to_model(request: CreatePersonRequest) -> Person:
    return Person(
        person_id=PersonId(request.person_id),
        age=None if request.age is None else Age(request.age),
    )
```

This adapter makes validation ownership explicit:

- the API layer owns transport naming, omission, and authorization;
- generated attribute constructors own projected scalar/domain validation;
- generated model constructors own schema field and cardinality validation;
- managers and queries own exact projection/IID checks.

The workspace generator emits application model bindings, not configurable
Pydantic DTO hierarchies. If several services share a wire DTO, version that
wire contract separately and adapt it to the generated package at each service
boundary.

Do not subclass or edit generated model bases to add transport fields. Those
classes are regenerated and are accepted by native execution only as the exact
installed projection.
