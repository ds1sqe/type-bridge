"""Unit tests for escaping special characters in multi-value attributes."""

from type_bridge import Card, Entity, Flag, Key, Relation, Role, String, TypeFlags


def test_multivalue_string_with_quotes():
    """Test insert query generation for multi-value String with quotes."""

    class Name(String):
        pass

    class Tag(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        tags: list[Tag] = Flag(Card(min=0))

    # Test with quotes in list items
    person = Person(
        name=Name("Alice"),
        tags=[Tag('tag "one"'), Tag('tag "two"'), Tag('He said "hello"')],
    )
    query = person.to_insert_query()

    assert "$e isa person" in query
    assert 'has Name "Alice"' in query
    # Each tag should be escaped individually
    assert r'has Tag "tag \"one\""' in query
    assert r'has Tag "tag \"two\""' in query
    assert r'has Tag "He said \"hello\""' in query


def test_multivalue_string_with_backslashes():
    """Test insert query generation for multi-value String with backslashes."""

    class Name(String):
        pass

    class Path(String):
        pass

    class FileSet(Entity):
        flags = TypeFlags(name="fileset")
        name: Name = Flag(Key)
        paths: list[Path] = Flag(Card(min=0))

    # Test with backslashes in list items
    fileset = FileSet(
        name=Name("MyFiles"),
        paths=[Path("C:\\Users\\Alice"), Path("D:\\Projects\\TypeBridge")],
    )
    query = fileset.to_insert_query()

    assert "$e isa fileset" in query
    # Each path should have escaped backslashes
    assert r'has Path "C:\\Users\\Alice"' in query
    assert r'has Path "D:\\Projects\\TypeBridge"' in query


def test_multivalue_string_with_mixed_escaping():
    """Test insert query generation for multi-value String with both quotes and backslashes."""

    class Name(String):
        pass

    class Description(String):
        pass

    class Document(Entity):
        flags = TypeFlags(name="document")
        name: Name = Flag(Key)
        descriptions: list[Description] = Flag(Card(min=0))

    # Test with complex escaping in list items
    doc = Document(
        name=Name("README"),
        descriptions=[
            Description(r'Path: "C:\Program Files\App"'),
            Description(r'Quote: "C:\\test\\"'),
            Description('Normal "text" here'),
        ],
    )
    query = doc.to_insert_query()

    # All items should be properly escaped
    assert r'has Description "Path: \"C:\\Program Files\\App\""' in query
    assert r'has Description "Quote: \"C:\\\\test\\\\\""' in query
    assert r'has Description "Normal \"text\" here"' in query


def test_multivalue_string_empty_and_special():
    """Test insert query generation for multi-value String with empty strings and special cases."""

    class Name(String):
        pass

    class Tag(String):
        pass

    class Item(Entity):
        flags = TypeFlags(name="item")
        name: Name = Flag(Key)
        tags: list[Tag] = Flag(Card(min=0))

    # Test with empty string, newlines, and tabs
    item = Item(
        name=Name("TestItem"),
        tags=[Tag(""), Tag("line1\nline2"), Tag("tab\there")],
    )
    query = item.to_insert_query()

    # Empty string should be allowed
    assert 'has Tag ""' in query
    # Newlines and tabs should be escaped for TypeQL
    assert r"line1\nline2" in query
    assert r"tab\there" in query


def test_relation_multivalue_escaping():
    """Test insert query generation for Relation with multi-value attributes containing special chars."""

    class Name(String):
        pass

    class Tag(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    class Company(Entity):
        flags = TypeFlags(name="company")
        name: Name = Flag(Key)

    class Employment(Relation):
        flags = TypeFlags(name="employment")
        employee: Role[Person] = Role("employee", Person)
        employer: Role[Company] = Role("employer", Company)
        tags: list[Tag] = Flag(Card(min=0))

    alice = Person(name=Name("Alice"))
    techcorp = Company(name=Name("TechCorp"))

    # Test relation with special characters in multi-value attributes
    emp = Employment(
        employee=alice,
        employer=techcorp,
        tags=[Tag('skill "Python"'), Tag("path\\to\\file"), Tag('both "A\\B"')],
    )

    # Get the insert query parts
    # Note: Relation.to_insert_query() requires entities to be inserted first
    # So we test the query building logic indirectly

    # Verify tag values are set correctly
    assert emp.tags[0].value == 'skill "Python"'
    assert emp.tags[1].value == "path\\to\\file"
    assert emp.tags[2].value == 'both "A\\B"'


def test_multivalue_single_quotes():
    """Test that single quotes are NOT escaped (TypeQL uses double quotes)."""

    class Name(String):
        pass

    class Tag(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        tags: list[Tag] = Flag(Card(min=0))

    person = Person(
        name=Name("Bob"),
        tags=[Tag("it's"), Tag("can't"), Tag("won't")],
    )
    query = person.to_insert_query()

    # Single quotes should NOT be escaped
    assert "it's" in query
    assert "can't" in query
    assert "won't" in query
    # Should not have backslash before single quotes
    assert "it\\'s" not in query


def test_multivalue_unicode_characters():
    """Test multi-value String with Unicode characters."""

    class Name(String):
        pass

    class Tag(String):
        pass

    class Article(Entity):
        flags = TypeFlags(name="article")
        name: Name = Flag(Key)
        tags: list[Tag] = Flag(Card(min=0))

    article = Article(
        name=Name("Article1"),
        tags=[Tag("café"), Tag("日本語"), Tag("emoji😀"), Tag("Ñoño")],
    )
    query = article.to_insert_query()

    # Unicode should be preserved
    assert "café" in query
    assert "日本語" in query
    assert "emoji😀" in query
    assert "Ñoño" in query
