"""Inspect generated wrapper values and hydrated model identities."""

from app_models import Age, DisplayName, Person, PersonId

ada = Person(person_id=PersonId("ada"), display_name=DisplayName("Ada Lovelace"), age=Age(36))
print("model:", ada)
print("identifier:", ada.person_id.value)
print("age:", ada.age.value if ada.age else None)
