"""Test String attribute type."""

from tests.utils.handwritten import Entity, Flag, Key, String, TypeFlags


def test_string_creation():
    """Test creating String attribute."""

    class Name(String):
        pass

    name = Name("Alice")
    assert name.value == "Alice"
    assert isinstance(name, String)


def test_string_value_type():
    """Test that String has correct value_type for TypeDB."""

    class Name(String):
        pass

    assert Name.value_type == "string"


def test_string_in_entity():
    """Test using String in an entity."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    person = Person(name=Name("Alice"))
    assert person.name.value == "Alice"


def test_string_insert_query():
    """Test String formatting in insert queries."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name

    person = Person(name=Name("Alice"))
    query = person.to_insert_query()

    assert "$e isa person" in query
    assert 'has Name "Alice"' in query
    # Strings SHOULD be quoted
    assert '"Alice"' in query


def test_string_with_special_characters():
    """Test insert query generation with String containing special characters."""

    class Description(String):
        pass

    class Item(Entity):
        flags = TypeFlags(name="item")
        description: Description

    # Test with quotes in string - quotes should be escaped
    item = Item(description=Description('A "quoted" string'))
    query = item.to_insert_query()

    assert "$e isa item" in query
    assert "has Description" in query
    # Verify quotes are properly escaped
    assert r'has Description "A \"quoted\" string"' in query


def test_string_quote_escaping():
    """Test that double quotes are properly escaped in TypeQL queries."""

    class Text(String):
        pass

    class Message(Entity):
        flags = TypeFlags(name="message")
        text: Text

    # Test double quote escaping
    msg = Message(text=Text('He said "hello" to me'))
    query = msg.to_insert_query()

    # Should escape quotes with backslash
    assert r'has Text "He said \"hello\" to me"' in query
    # Should NOT have unescaped quotes
    assert 'has Text "He said "hello"' not in query


def test_string_backslash_escaping():
    """Test that backslashes are properly escaped in TypeQL queries."""

    class Path(String):
        pass

    class File(Entity):
        flags = TypeFlags(name="file")
        path: Path

    # Test backslash escaping
    file = File(path=Path("C:\\Users\\Documents"))
    query = file.to_insert_query()

    # Backslashes should be escaped
    assert r'has Path "C:\\Users\\Documents"' in query


def test_string_mixed_escaping():
    """Test escaping of both quotes and backslashes together."""

    class Description(String):
        pass

    class Item(Entity):
        flags = TypeFlags(name="item")
        description: Description

    # Test complex escaping with both quotes and backslashes
    item = Item(description=Description(r'Path: "C:\Program Files\App" is valid'))
    query = item.to_insert_query()

    # Both quotes and backslashes should be escaped
    assert r'has Description "Path: \"C:\\Program Files\\App\" is valid"' in query


def test_empty_string_insert_query():
    """Test insert query generation with empty string."""

    class Name(String):
        pass

    class Tag(Entity):
        flags = TypeFlags(name="tag")
        name: Name

    # Test with empty string
    tag = Tag(name=Name(""))
    query = tag.to_insert_query()

    assert "$e isa tag" in query
    assert 'has Name ""' in query


def test_string_optional_attribute():
    """Test String as optional attribute."""

    class Name(String):
        pass

    class Email(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        email: Email | None

    # Test with None
    person = Person(name=Name("Alice"), email=None)
    query = person.to_insert_query()
    assert 'has Name "Alice"' in query
    assert "has Email" not in query

    # Test with value
    person2 = Person(name=Name("Bob"), email=Email("bob@example.com"))
    query2 = person2.to_insert_query()
    assert 'has Email "bob@example.com"' in query2


def test_string_comparison():
    """Test String attribute comparison."""

    class Name(String):
        pass

    # Same value
    n1 = Name("Alice")
    n2 = Name("Alice")
    assert n1 == n2

    # Different values
    n3 = Name("Bob")
    assert n1 != n3


def test_string_string_representation():
    """Test string representation of String attributes."""

    class Name(String):
        pass

    name = Name("Alice")

    # Test __repr__
    assert "Name" in repr(name)
    assert "Alice" in repr(name)


def test_string_concatenation():
    """Test string concatenation operations."""

    class FirstName(String):
        pass

    class LastName(String):
        pass

    first = FirstName("Alice")
    last = LastName("Smith")

    # Test concatenation
    full = first + last
    assert full.value == "AliceSmith"


def test_string_with_newlines():
    """Test string with newline characters."""

    class Description(String):
        pass

    class Doc(Entity):
        flags = TypeFlags(name="doc")
        description: Description

    # Test with newlines
    doc = Doc(description=Description("Line 1\nLine 2\nLine 3"))
    query = doc.to_insert_query()

    assert "$e isa doc" in query
    assert "has Description" in query
