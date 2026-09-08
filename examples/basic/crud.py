"""Complete generated Python CRUD journey.

Generate ``app_models`` from ``examples/typebridge.yaml`` before running this
script. Split-YAML—not these Python values—is the schema authority.
"""

from app_models import (
    Age,
    Aliases,
    Company,
    CompanyId,
    DisplayName,
    Employment,
    Industry,
    Person,
    PersonId,
    Position,
    Salary,
)

from type_bridge import Database


def main() -> None:
    db = Database(address="localhost:1729", database="typebridge-examples")
    db.connect()

    ada = Person(
        person_id=PersonId("ada"),
        display_name=DisplayName("Ada Lovelace"),
        age=Age(36),
        aliases=[Aliases("A. A. Lovelace")],
    )
    acme = Company(
        company_id=CompanyId("analytical-engines"),
        display_name=DisplayName("Analytical Engines"),
        industry=[Industry("computing")],
    )

    Person.manager(db).put(ada)
    Company.manager(db).put(acme)
    Employment.manager(db).insert(
        Employment(
            employee=ada,
            employer=acme,
            position=Position("programmer"),
            salary=Salary(100_000),
        )
    )

    adults = Person.manager(db).filter(age__gte=Age(18)).all()
    for person in adults:
        print(person.person_id.value, person.display_name.value)


if __name__ == "__main__":
    main()
