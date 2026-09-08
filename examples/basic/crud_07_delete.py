"""Delete generated models by an explicit generated filter."""

from app_models import Person, PersonId

from type_bridge import Database


def main() -> None:
    db = Database(address="localhost:1729", database="typebridge-examples")
    db.connect()
    deleted = Person.manager(db).filter(person_id=PersonId("alan")).delete()
    print("deleted:", deleted)


if __name__ == "__main__":
    main()
