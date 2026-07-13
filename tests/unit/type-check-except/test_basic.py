"""Basic tests for the new Attribute-based API."""

from type_bridge import (
    Card,
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    String,
    TypeFlags,
    Unique,
)


def test_attribute_creation():
    """Test creating attribute types."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    assert Name.get_attribute_name() == "Name"  # Default CLASS_NAME case
    assert Name.get_value_type() == "string"
    assert Age.get_attribute_name() == "Age"  # Default CLASS_NAME case
    assert Age.get_value_type() == "integer"


def test_flag_annotation():
    """Test Flag annotation system for Key, Unique, and Card."""

    class Email(String):
        pass

    # Test Key flag
    key_flag = Flag(Key)
    assert key_flag.is_key is True
    assert key_flag.to_typeql_annotations() == [
        "@key"
    ]  # @key implies @card(1,1), no need to output it

    # Test Unique flag
    unique_flag = Flag(Unique)
    assert unique_flag.is_unique is True
    assert unique_flag.to_typeql_annotations() == [
        "@unique"
    ]  # Bare @unique does not imply a cardinality

    # Test combined Key + Unique flags
    combined_flag = Flag(Key, Unique)
    assert combined_flag.is_key is True
    assert combined_flag.is_unique is True
    assert combined_flag.to_typeql_annotations() == ["@key", "@unique"]  # @key implies @card(1,1)

    # Test Card flag
    card_min_only = Flag(Card(min=2))
    assert card_min_only.card_min == 2
    assert card_min_only.card_max is None
    assert card_min_only.to_typeql_annotations() == ["@card(2..)"]  # Unbounded max

    card_min_max = Flag(Card(1, 5))
    assert card_min_max.card_min == 1
    assert card_min_max.card_max == 5
    assert card_min_max.to_typeql_annotations() == ["@card(1..5)"]  # Range syntax

    # Test Key + Card combination
    # Note: In TypeDB, @key always implies @card(1,1), so explicit Card with Key is redundant
    # The @card annotation is never output when @key is present
    key_card = Flag(Key, Card(min=1))
    assert key_card.is_key is True
    assert key_card.card_min == 1
    assert key_card.card_max is None
    assert key_card.to_typeql_annotations() == [
        "@key"
    ]  # @key always implies exactly one, @card is omitted


def test_card_with_list_types():
    """Test Card API with list type annotations."""

    class Name(String):
        pass

    class Tag(String):
        pass

    class Job(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)  # @key @card(1,1)
        tags: list[Tag] = Flag(Card(min=2))  # @card(2,∞)
        jobs: list[Job] = Flag(Card(1, 5))  # @card(1,5)

    owned = Person.get_owned_attributes()

    # Check that list types with Card work correctly
    assert owned["tags"].flags.card_min == 2
    assert owned["tags"].flags.card_max is None
    assert owned["tags"].flags.has_explicit_card is True

    assert owned["jobs"].flags.card_min == 1
    assert owned["jobs"].flags.card_max == 5
    assert owned["jobs"].flags.has_explicit_card is True

    # Check schema generation
    schema = Person.to_schema_definition()
    assert isinstance(schema, str)
    assert "@card(2..)" in schema  # Unbounded max
    assert "@card(1..5)" in schema  # Range syntax


def test_card_validation_rejects_non_list():
    """Test that Card with non-list types raises TypeError."""
    import pytest

    class Age(Integer):
        pass

    class Phone(String):
        pass

    # Should raise TypeError when Card is used on non-list type
    with pytest.raises(
        TypeError, match="Flag\\(Card\\(...\\)\\) can only be used with list\\[Type\\]"
    ):

        class InvalidPerson(Entity):
            flags = TypeFlags(name="invalid_person")
            age: Age = Flag(Card(min=0, max=1))  # Should fail!

    # Should also reject Card on union types with None
    with pytest.raises(
        TypeError, match="Flag\\(Card\\(...\\)\\) can only be used with list\\[Type\\]"
    ):

        class InvalidPerson2(Entity):
            flags = TypeFlags(name="invalid_person2")
            phone: Phone | None = Flag(Card(min=0, max=1))  # Should fail! Use Optional instead

    # Verify that Phone | None works WITHOUT Card
    class ValidPerson(Entity):
        flags = TypeFlags(name="valid_person")
        phone: Phone | None  # This is fine - treated as @card(0,1)

    owned = ValidPerson.get_owned_attributes()
    assert owned["phone"].flags.card_min == 0
    assert owned["phone"].flags.card_max == 1
    assert owned["phone"].flags.has_explicit_card is False


def test_list_requires_card():
    """Test that list[Type] annotations must have Flag(Card(...))."""
    import pytest

    class Tag(String):
        pass

    # Should raise TypeError when list[Type] is used without Flag(Card(...))
    with pytest.raises(
        TypeError, match="list\\[Type\\] annotations must use Flag\\(Card\\(...\\)\\)"
    ):

        class InvalidPerson(Entity):
            flags = TypeFlags(name="invalid_person")
            tags: list[Tag]  # Should fail! Must use Flag(Card(...))

    # Should also fail without any default value
    with pytest.raises(
        TypeError, match="list\\[Type\\] annotations must use Flag\\(Card\\(...\\)\\)"
    ):

        class InvalidPerson2(Entity):
            flags = TypeFlags(name="invalid_person2")
            tags: list[Tag] = Flag(Key)  # Should fail! Key doesn't provide Card

    # Verify that list[Type] with Flag(Card(...)) works
    class ValidPerson(Entity):
        flags = TypeFlags(name="valid_person")
        tags: list[Tag] = Flag(Card(min=1))  # This is correct

    owned = ValidPerson.get_owned_attributes()
    assert owned["tags"].flags.card_min == 1
    assert owned["tags"].flags.has_explicit_card is True


def test_entity_creation():
    """Test creating entities with owned attributes using Optional and Card."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Tag(String):
        pass

    class Email(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)  # @key @card(1,1)
        age: Age | None  # @card(0,1) - using Optional
        email: Email | None  # @card(0,1) - using union syntax
        tags: list[Tag] = Flag(Card(min=2))  # @card(2,∞)

    # Check owned attributes
    owned = Person.get_owned_attributes()
    assert "name" in owned
    assert "age" in owned
    assert "email" in owned
    assert "tags" in owned

    # Check Key flag
    assert owned["name"].flags.is_key is True
    assert owned["age"].flags.is_key is False
    assert owned["email"].flags.is_key is False

    # Check cardinality - both Optional and Union syntax work
    assert owned["name"].flags.card_min == 1
    assert owned["name"].flags.card_max == 1
    assert owned["age"].flags.card_min == 0
    assert owned["age"].flags.card_max == 1
    assert owned["email"].flags.card_min == 0
    assert owned["email"].flags.card_max == 1
    assert owned["tags"].flags.card_min == 2
    assert owned["tags"].flags.card_max is None

    # Check type name
    assert Person.get_type_name() == "person"


def test_entity_instance():
    """Test creating entity instances."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name
        age: Age

    # NOTE: Intentionally using raw values to test Pydantic's type coercion
    alice = Person(name="Alice", age=30)
    # Strict type safety: attributes are wrapped instances
    assert isinstance(alice.name, Name)
    assert not isinstance(alice.name, str)  # Not a raw str
    assert alice.name != "Alice"  # Not equal to raw value
    assert alice.name.value == "Alice"  # Access raw value via .value
    assert isinstance(alice.age, Age)
    assert not isinstance(alice.age, int)  # Not a raw int
    assert alice.age != 30  # Not equal to raw value
    assert alice.age.value == 30  # Access raw value via .value


def test_entity_schema_generation():
    """Test generating entity schema with generic cardinality types."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Email(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)  # @key with default card(1,1)
        age: Age | None  # @card(0,1) - Optional syntax
        email: Email | None  # @card(0,1) - Union syntax

    schema = Person.to_schema_definition()
    assert isinstance(schema, str)
    assert "entity person" in schema
    assert "owns Name @key" in schema  # Name uses CLASS_NAME default, @key implies @card(1,1)
    assert "owns Age @card(0..1)" in schema  # Age uses CLASS_NAME default
    assert "owns Email @card(0..1)" in schema  # Email uses CLASS_NAME default


def test_entity_insert_query():
    """Test generating insert query."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name
        age: Age

    # NOTE: Intentionally using raw values to test Pydantic's type coercion
    alice = Person(name="Alice", age=30)
    query = alice.to_insert_query()

    assert "$e isa person" in query
    assert 'has Name "Alice"' in query  # Name uses CLASS_NAME default
    assert "has Age 30" in query  # Age uses CLASS_NAME default


def test_relation_creation():
    """Test creating relations."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name

    class Friendship(Relation):
        flags = TypeFlags(name="friendship")

        friend1: Role[Person] = Role("friend", Person)
        friend2: Role[Person] = Role("friend", Person)

    # Check roles
    assert "friend1" in Friendship._roles
    assert "friend2" in Friendship._roles
    assert Friendship._roles["friend1"].role_name == "friend"


def test_relation_with_attributes():
    """Test relations with owned attributes."""

    class Name(String):
        pass

    class SinceYear(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name

    class Friendship(Relation):
        flags = TypeFlags(name="friendship")

        friend1: Role[Person] = Role("friend", Person)
        friend2: Role[Person] = Role("friend", Person)

        since_year: SinceYear  # default card(1,1)

    # Check owned attributes
    owned = Friendship.get_owned_attributes()
    assert "since_year" in owned
    assert owned["since_year"].typ.get_attribute_name() == "SinceYear"  # CLASS_NAME default
    assert owned["since_year"].flags.card_min == 1
    assert owned["since_year"].flags.card_max == 1


def test_relation_schema_generation():
    """Test generating relation schema."""

    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name

    class Friendship(Relation):
        flags = TypeFlags(name="friendship")

        friend1: Role[Person] = Role("friend", Person)
        friend2: Role[Person] = Role("friend", Person)

    schema = Friendship.to_schema_definition()
    assert isinstance(schema, str)
    assert "relation friendship" in schema
    assert "relates friend" in schema


def test_attribute_schema_generation():
    """Test generating attribute schema."""

    class Name(String):
        pass

    class Age(Integer):
        pass

    name_schema = Name.to_schema_definition()
    age_schema = Age.to_schema_definition()

    assert "attribute Name, value string;" in name_schema  # CLASS_NAME default
    assert "attribute Age, value integer;" in age_schema  # CLASS_NAME default
