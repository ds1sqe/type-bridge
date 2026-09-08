"""Test Integer attribute type."""

from tests.utils.handwritten import Entity, Flag, Integer, Key, String, TypeFlags


def test_integer_creation():
    """Test creating Integer attribute."""

    class Age(Integer):
        pass

    age = Age(30)
    assert age.value == 30
    assert isinstance(age, Integer)


def test_integer_value_type():
    """Test that Integer has correct value_type for TypeDB."""

    class Quantity(Integer):
        pass

    assert Quantity.value_type == "integer"


def test_integer_in_entity():
    """Test using Integer in an entity."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age

    person = Person(name=Name("Alice"), age=Age(30))
    assert person.age.value == 30


def test_integer_insert_query():
    """Test Integer formatting in insert queries."""

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        age: Age

    person = Person(age=Age(30))
    query = person.to_insert_query()

    assert "$e isa person" in query
    assert "has Age 30" in query
    # Integers should NOT be quoted
    assert '"30"' not in query


def test_integer_edge_cases():
    """Test insert query generation with Integer edge cases."""

    class Total(Integer):
        pass

    class Counter(Entity):
        flags = TypeFlags(name="counter")
        count: Total

    # Test with zero
    counter_zero = Counter(count=Total(0))
    query_zero = counter_zero.to_insert_query()
    assert "has Total 0" in query_zero

    # Test with negative number
    counter_negative = Counter(count=Total(-42))
    query_negative = counter_negative.to_insert_query()
    assert "has Total -42" in query_negative

    # Test with large number
    counter_large = Counter(count=Total(999999999))
    query_large = counter_large.to_insert_query()
    assert "has Total 999999999" in query_large


def test_integer_optional_attribute():
    """Test Integer as optional attribute."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    # Test with None
    person = Person(name=Name("Alice"), age=None)
    query = person.to_insert_query()
    assert 'has Name "Alice"' in query
    assert "has Age" not in query

    # Test with value
    person2 = Person(name=Name("Bob"), age=Age(25))
    query2 = person2.to_insert_query()
    assert "has Age 25" in query2


def test_integer_comparison():
    """Test Integer attribute comparison."""

    class Amount(Integer):
        pass

    # Same value
    c1 = Amount(42)
    c2 = Amount(42)
    assert c1 == c2

    # Different values
    c3 = Amount(100)
    assert c1 != c3


def test_integer_string_representation():
    """Test string representation of Integer attributes."""

    class Age(Integer):
        pass

    age = Age(30)

    # Test __repr__
    assert "Age" in repr(age)
    assert "30" in repr(age)
