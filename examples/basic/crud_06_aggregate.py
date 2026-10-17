"""Aggregate through the generated immutable query facade."""

import asyncio

from app_models import Person, aggregate

from type_bridge import Database


async def main() -> None:
    db = Database(address="localhost:1729", database="typebridge-examples")
    db.connect()
    session = Person.query(db)
    person = session.exact(Person)
    age = person.field(Person.age)
    count, mean_age = await session.query(person).aggregate(
        person,
        aggregate.count(),
        aggregate.mean(age),
    )
    print("people:", count, "mean age:", mean_age)


if __name__ == "__main__":
    asyncio.run(main())
