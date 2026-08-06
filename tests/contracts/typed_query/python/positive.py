"""Positive Pyright fixtures for the installed public Query contract."""

from __future__ import annotations

from typing import assert_type

from tests.utils.handwritten import Entity, Relation, TypeFlags
from type_bridge.session import Database, TransactionContext
from type_bridge.typed import Page, Query, QuerySession


class Person(Entity):
    flags = TypeFlags(name="contract-person")


class Company(Entity):
    flags = TypeFlags(name="contract-company")


class Employment(Relation):
    flags = TypeFlags(name="contract-employment")


def assert_query_shapes(connection: Database | TransactionContext) -> None:
    session = QuerySession(connection)
    person1 = session.var(Person)
    person2 = session.var(Person)
    person3 = session.var(Person)
    person4 = session.var(Person)
    person5 = session.var(Person)
    person6 = session.var(Person)
    company1 = session.var(Company)
    company2 = session.var(Company)
    company3 = session.var(Company)
    company4 = session.var(Company)
    company5 = session.var(Company)
    employment1 = session.var(Employment)
    employment2 = session.var(Employment)
    employment3 = session.var(Employment)
    employment4 = session.var(Employment)
    employment5 = session.var(Employment)

    scalar = session.query(person1)
    assert_type(scalar, Query[Person])
    assert_type(scalar.one(), Person)
    assert_type(scalar.rows(limit=20), list[Person])
    assert_type(scalar.page_by(person1, limit=20), Page[Person])

    pair = session.query(person1, company1)
    assert_type(pair, Query[Person, Company])
    assert_type(pair.one(), tuple[Person, Company])
    assert_type(pair.rows(limit=20), list[tuple[Person, Company]])

    five = session.query(person1, employment1, company1, employment2, person2)
    assert_type(five, Query[Person, Employment, Company, Employment, Person])
    assert_type(
        five.one(),
        tuple[Person, Employment, Company, Employment, Person],
    )
    assert_type(
        five.rows(limit=20),
        list[tuple[Person, Employment, Company, Employment, Person]],
    )

    repeated = session.query(person1, person2)
    assert_type(repeated, Query[Person, Person])
    assert_type(repeated.one(), tuple[Person, Person])
    assert_type(repeated.rows(limit=20), list[tuple[Person, Person]])

    sixteen = session.query(
        person1,
        employment1,
        company1,
        person2,
        employment2,
        company2,
        person3,
        employment3,
        company3,
        person4,
        employment4,
        company4,
        person5,
        employment5,
        company5,
        person6,
    )
    assert_type(
        sixteen,
        Query[
            Person,
            Employment,
            Company,
            Person,
            Employment,
            Company,
            Person,
            Employment,
            Company,
            Person,
            Employment,
            Company,
            Person,
            Employment,
            Company,
            Person,
        ],
    )
    assert_type(
        sixteen.rows(limit=20),
        list[
            tuple[
                Person,
                Employment,
                Company,
                Person,
                Employment,
                Company,
                Person,
                Employment,
                Company,
                Person,
                Employment,
                Company,
                Person,
                Employment,
                Company,
                Person,
            ]
        ],
    )


def assert_database_connection_is_accepted(database: Database) -> None:
    assert_query_shapes(database)


def assert_borrowed_transaction_is_accepted(transaction: TransactionContext) -> None:
    assert_query_shapes(transaction)
