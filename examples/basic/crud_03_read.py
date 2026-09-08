"""Read generated models with concise single-type manager terminals."""

from app_models import Person

from type_bridge import Database


def main() -> None:
    db = Database(address="localhost:1729", database="typebridge-examples")
    db.connect()
    manager = Person.manager(db)
    first = manager.first()
    print("count:", manager.count())
    print("first:", first)
    if first is not None and first.iid is not None:
        print("by IID:", manager.get_by_iid(first.iid))


if __name__ == "__main__":
    main()
