"""Fetch filtered models, then update and delete them in bulk."""

from app_models import Age, Aliases, Person

from type_bridge import Database


def add_adult_alias(person: Person) -> None:
    person.aliases = (*person.aliases, Aliases("adult"))


def main() -> None:
    db = Database(address="localhost:1729", database="typebridge-examples")
    db.connect()
    manager = Person.manager(db)
    adults = manager.filter(age__gte=Age(18)).all()
    for person in adults:
        add_adult_alias(person)
    updated = manager.update_many(adults)
    minors = manager.filter(age__lt=Age(18)).all()
    deleted = manager.delete_many(minors)
    print("updated:", updated.count, "deleted:", len(deleted))


if __name__ == "__main__":
    main()
