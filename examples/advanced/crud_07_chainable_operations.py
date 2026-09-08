"""Run generated filtered updates and deletes."""

from app_models import Age, Aliases, Person

from type_bridge import Database


def add_adult_alias(person: Person) -> None:
    person.aliases = (*person.aliases, Aliases("adult"))


def main() -> None:
    db = Database(address="localhost:1729", database="typebridge-examples")
    db.connect()
    adults = Person.manager(db).filter(age__gte=Age(18))
    updated = adults.update_with(add_adult_alias)
    deleted = Person.manager(db).filter(age__lt=Age(18)).delete()
    print("updated:", len(updated), "deleted:", deleted)


if __name__ == "__main__":
    main()
