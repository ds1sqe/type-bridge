"""Integration tests for Django-style lookup filters on EntityManager."""

import pytest

from type_bridge import Entity, Flag, Integer, Key, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(205)
def test_string_lookups(db_with_schema):
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    mgr = Person.manager(db_with_schema)

    people = [
        Person(name=Name("Alice"), age=Age(30)),
        Person(name=Name("Albert"), age=Age(25)),
        Person(name=Name("Bob"), age=Age(40)),
    ]
    mgr.insert_many(people)

    startswith_a = mgr.filter(name__startswith="Al").execute()
    assert {p.name.value for p in startswith_a} == {"Alice", "Albert"}

    contains_ice = mgr.filter(name__contains="ice").execute()
    assert {p.name.value for p in contains_ice} == {"Alice"}

    endswith_ce = mgr.filter(name__endswith="ce").execute()
    assert {p.name.value for p in endswith_ce} == {"Alice"}

    regex_b = mgr.filter(name__regex="^B.*").execute()
    assert {p.name.value for p in regex_b} == {"Bob"}


@pytest.mark.integration
@pytest.mark.order(206)
def test_numeric_lookups_and_in(db_with_schema):
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    mgr = Person.manager(db_with_schema)

    people = [
        Person(name=Name("Cara"), age=Age(20)),
        Person(name=Name("Dora"), age=Age(35)),
        Person(name=Name("Eli"), age=Age(42)),
    ]
    mgr.insert_many(people)

    over_30 = mgr.filter(age__gt=30).execute()
    assert {p.name.value for p in over_30} == {"Dora", "Eli"}

    in_filter = mgr.filter(name__in=["Cara", Name("Eli")]).execute()
    assert {p.name.value for p in in_filter} == {"Cara", "Eli"}


@pytest.mark.integration
@pytest.mark.order(207)
def test_isnull_lookup(db_with_schema):
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    mgr = Person.manager(db_with_schema)

    mgr.insert_many(
        [
            Person(name=Name("NullAge"), age=None),
            Person(name=Name("WithAge"), age=Age(50)),
        ]
    )

    missing_age = mgr.filter(age__isnull=True).execute()
    assert {p.name.value for p in missing_age} == {"NullAge"}

    present_age = mgr.filter(age__isnull=False).execute()
    assert {p.name.value for p in present_age} == {"WithAge"}


@pytest.mark.integration
@pytest.mark.order(208)
def test_comparison_lookups_gte_lt_lte(db_with_schema):
    """Test __gte, __lt, __lte comparison lookups."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age

    mgr = Person.manager(db_with_schema)

    people = [
        Person(name=Name("P20"), age=Age(20)),
        Person(name=Name("P30"), age=Age(30)),
        Person(name=Name("P40"), age=Age(40)),
        Person(name=Name("P50"), age=Age(50)),
    ]
    mgr.insert_many(people)

    # __gte: greater than or equal
    gte_30 = mgr.filter(age__gte=30).execute()
    assert {p.name.value for p in gte_30} == {"P30", "P40", "P50"}

    # __lt: less than
    lt_40 = mgr.filter(age__lt=40).execute()
    assert {p.name.value for p in lt_40} == {"P20", "P30"}

    # __lte: less than or equal
    lte_30 = mgr.filter(age__lte=30).execute()
    assert {p.name.value for p in lte_30} == {"P20", "P30"}


@pytest.mark.integration
@pytest.mark.order(209)
def test_exact_and_eq_lookups(db_with_schema):
    """Test __exact and __eq as aliases for exact match."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    mgr = Person.manager(db_with_schema)

    mgr.insert_many(
        [
            Person(name=Name("Alice"), age=Age(30)),
            Person(name=Name("Bob"), age=Age(25)),
        ]
    )

    # __exact with raw string
    exact_result = mgr.filter(name__exact="Alice").execute()
    assert len(exact_result) == 1
    assert exact_result[0].name.value == "Alice"

    # __eq with attribute instance
    eq_result = mgr.filter(name__eq=Name("Bob")).execute()
    assert len(eq_result) == 1
    assert eq_result[0].name.value == "Bob"


@pytest.mark.integration
@pytest.mark.order(210)
def test_combined_lookups(db_with_schema):
    """Test multiple lookups combined on different fields."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age

    mgr = Person.manager(db_with_schema)

    people = [
        Person(name=Name("Alice"), age=Age(25)),
        Person(name=Name("Albert"), age=Age(30)),
        Person(name=Name("Amy"), age=Age(35)),
        Person(name=Name("Bob"), age=Age(28)),
    ]
    mgr.insert_many(people)

    # Combined: name starts with "A" AND age between 25-32
    result = mgr.filter(name__startswith="A", age__gte=25, age__lt=32).execute()
    assert {p.name.value for p in result} == {"Alice", "Albert"}


@pytest.mark.integration
@pytest.mark.order(211)
def test_special_characters_in_string_patterns(db_with_schema):
    """Test that special regex characters are properly escaped."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    mgr = Person.manager(db_with_schema)

    # Insert entities with special regex characters
    mgr.insert_many(
        [
            Person(name=Name("John (Jr.)")),
            Person(name=Name("Jane [Sr]")),
            Person(name=Name("Bob $100")),
            Person(name=Name("Alice+Bob")),
        ]
    )

    # __contains with parentheses (regex special chars)
    result = mgr.filter(name__contains="(Jr.)").execute()
    assert len(result) == 1
    assert result[0].name.value == "John (Jr.)"

    # __contains with brackets
    result = mgr.filter(name__contains="[Sr]").execute()
    assert len(result) == 1
    assert result[0].name.value == "Jane [Sr]"

    # __startswith with special char
    result = mgr.filter(name__startswith="Bob $").execute()
    assert len(result) == 1
    assert result[0].name.value == "Bob $100"

    # __endswith with plus sign
    result = mgr.filter(name__endswith="+Bob").execute()
    assert len(result) == 1
    assert result[0].name.value == "Alice+Bob"


@pytest.mark.integration
@pytest.mark.order(212)
def test_lookup_empty_results(db_with_schema):
    """Test lookups return empty list when no matches."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age

    mgr = Person.manager(db_with_schema)

    mgr.insert(Person(name=Name("Alice"), age=Age(30)))

    # No matches for impossible age
    result = mgr.filter(age__gt=1000).execute()
    assert result == []

    # No matches for non-existent name pattern
    result = mgr.filter(name__startswith="ZZZ").execute()
    assert result == []


@pytest.mark.integration
@pytest.mark.order(213)
def test_range_query_same_field(db_with_schema):
    """Test range query using multiple lookups on same field."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age

    mgr = Person.manager(db_with_schema)

    people = [
        Person(name=Name("Teen"), age=Age(15)),
        Person(name=Name("Young"), age=Age(25)),
        Person(name=Name("Middle"), age=Age(45)),
        Person(name=Name("Senior"), age=Age(65)),
    ]
    mgr.insert_many(people)

    # Range: 18 <= age < 50 (adults under 50)
    result = mgr.filter(age__gte=18, age__lt=50).execute()
    assert {p.name.value for p in result} == {"Young", "Middle"}
