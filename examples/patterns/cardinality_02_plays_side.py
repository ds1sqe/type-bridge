"""Plays-side role cardinality.

This example shows the difference between:

- relates-side cardinality: how many players fill a role in one relation
- plays-side cardinality: how many relation instances a player may play in a role

It only generates schema text, so it does not require a running TypeDB server.
"""

from type_bridge import Card, Entity, Relation, Role, TypeFlags
from type_bridge.migration.info import SchemaInfo


class Person(Entity):
    flags = TypeFlags(name="person")


class Company(Entity):
    flags = TypeFlags(name="company")


class Employment(Relation):
    flags = TypeFlags(name="employment")

    employee: Role[Person] = Role("employee", Person)
    employer: Role[Company] = Role(
        "employer",
        Company,
        cardinality=Card(1, 1),
        plays_cardinality=Card(0, 1),
    )


def generated_schema() -> str:
    schema = SchemaInfo()
    schema.entities.extend([Person, Company])
    schema.relations.append(Employment)
    return schema.to_typeql()


def main() -> None:
    typeql = generated_schema()
    print(typeql)

    assert "relates employer @card(1..1)" in typeql
    assert "company plays employment:employer @card(0..1);" in typeql


if __name__ == "__main__":
    main()
