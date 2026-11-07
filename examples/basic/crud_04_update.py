"""Fetch, modify, and update a generated entity."""

from app_models import Age, Person, PersonId

from type_bridge import Database


def main() -> None:
    db = Database(address="localhost:1729", database="typebridge-examples")
    db.connect()
    manager = Person.manager(db)
    person = manager.filter(person_id=PersonId("ada")).first()
    if person is None:
        raise RuntimeError("run crud.py first")
    person.age = Age(37)
    updated = manager.update(person)
    for item in updated.items:
        print(item.age.value if item.age else None)


if __name__ == "__main__":
    main()
