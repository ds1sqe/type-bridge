"""Filter one generated type with exact wrapper and lookup keys."""

from app_models import Age, DisplayName, Person

from type_bridge import Database


def main() -> None:
    db = Database(address="localhost:1729", database="typebridge-examples")
    db.connect()
    manager = Person.manager(db)
    adults = manager.filter(age__gte=Age(18)).all()
    named_a = manager.filter(display_name__startswith=DisplayName("A")).all()
    print("adults:", len(adults), "names starting A:", len(named_a))


if __name__ == "__main__":
    main()
