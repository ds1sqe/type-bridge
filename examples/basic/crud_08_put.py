"""Use generated key metadata for idempotent put operations."""

from app_models import Age, DisplayName, Person, PersonId

from type_bridge import Database


def main() -> None:
    db = Database(address="localhost:1729", database="typebridge-examples")
    db.connect()
    ada = Person(person_id=PersonId("ada"), display_name=DisplayName("Ada Lovelace"), age=Age(36))
    manager = Person.manager(db)
    first = manager.put(ada)
    second = manager.put(ada)
    print([item.iid for item in first.items], [item.iid for item in second.items])


if __name__ == "__main__":
    main()
