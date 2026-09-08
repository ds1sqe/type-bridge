"""Generated constructors and managers preserve concrete model types."""

from typing import TYPE_CHECKING, assert_type

from app_models import Age, DisplayName, Person, PersonId, ProjectedModelManager

from type_bridge import Database

if TYPE_CHECKING:
    db = Database(address="localhost:1729", database="typebridge-examples")
    ada = Person(person_id=PersonId("ada"), display_name=DisplayName("Ada"), age=Age(36))
    assert_type(ada, Person)
    assert_type(Person.manager(db), ProjectedModelManager[Person])
    assert_type(Person.manager(db).filter(age__gte=Age(18)).all(), list[Person])
