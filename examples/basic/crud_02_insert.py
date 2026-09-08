"""Insert generated entities in one batch."""

from app_models import Age, DisplayName, Person, PersonId

from type_bridge import Database


def main() -> None:
    db = Database(address="localhost:1729", database="typebridge-examples")
    db.connect()
    people = [
        Person(person_id=PersonId("grace"), display_name=DisplayName("Grace Hopper"), age=Age(85)),
        Person(person_id=PersonId("alan"), display_name=DisplayName("Alan Turing"), age=Age(41)),
    ]
    inserted = Person.manager(db).insert_many(people)
    print([person.iid for person in inserted])


if __name__ == "__main__":
    main()
