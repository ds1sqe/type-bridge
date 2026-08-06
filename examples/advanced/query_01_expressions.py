"""Compose a generated multi-type query with field and role tokens."""

from app_models import Age, Employment, Person

from type_bridge import Database


def main() -> None:
    db = Database(address="localhost:1729", database="typebridge-examples")
    db.connect()
    session = Person.query(db)
    person = session.exact(Person)
    employment = session.exact(Employment)
    employee = employment.role(Employment.employee).connects(person)
    adults = person.field(Person.age).gte(Age(18))
    rows = session.query(person, employment).where(employee, adults).rows(limit=100)
    for employee_model, employment_model in rows:
        print(employee_model.person_id.value, employment_model.position.value)


if __name__ == "__main__":
    main()
