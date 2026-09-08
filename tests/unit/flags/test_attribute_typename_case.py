"""Tests for TypeNameCase formatting options for Attribute types."""

from tests.utils.handwritten import (
    Boolean,
    DateTime,
    Double,
    Entity,
    Flag,
    Integer,
    Key,
    String,
    TypeFlags,
    TypeNameCase,
)


def test_attribute_typename_case_classname_default():
    """Test that CLASS_NAME is the default case formatting for attributes."""

    class PersonName(String):
        # No explicit case parameter - should default to CLASS_NAME
        pass

    assert PersonName.get_attribute_name() == "PersonName"


def test_attribute_typename_case_lowercase_explicit():
    """Test explicit LOWERCASE case formatting for attributes."""

    class PersonName(String):
        case = TypeNameCase.LOWERCASE

    assert PersonName.get_attribute_name() == "personname"


def test_attribute_typename_case_classname():
    """Test CLASS_NAME case formatting for attributes (keeps as-is)."""

    class PersonName(String):
        case = TypeNameCase.CLASS_NAME

    assert PersonName.get_attribute_name() == "PersonName"


def test_attribute_typename_case_snake_case():
    """Test SNAKE_CASE formatting for attributes."""

    class PersonName(String):
        case = TypeNameCase.SNAKE_CASE

    assert PersonName.get_attribute_name() == "person_name"


def test_attribute_typename_case_snake_case_complex():
    """Test SNAKE_CASE with complex attribute names."""

    class HTTPResponseCode(Integer):
        case = TypeNameCase.SNAKE_CASE

    assert HTTPResponseCode.get_attribute_name() == "http_response_code"


def test_attribute_explicit_name_takes_precedence():
    """Test that explicit attr_name takes precedence over case formatting."""

    class PersonName(String):
        attr_name = "full_name"
        case = TypeNameCase.SNAKE_CASE

    # Should use explicit attr_name, not apply case formatting
    assert PersonName.get_attribute_name() == "full_name"


def test_attribute_schema_generation_with_snake_case():
    """Test that schema generation uses the formatted attribute name."""

    class PersonName(String):
        case = TypeNameCase.SNAKE_CASE

    schema = PersonName.to_schema_definition()
    assert "attribute person_name" in schema
    assert "value string" in schema


def test_attribute_schema_generation_with_classname():
    """Test that schema generation uses CLASS_NAME correctly."""

    class PersonName(String):
        case = TypeNameCase.CLASS_NAME

    schema = PersonName.to_schema_definition()
    assert "attribute PersonName" in schema


def test_attribute_single_word_all_cases():
    """Test that single-word attribute names work correctly with all case options."""

    class Name(String):
        case = TypeNameCase.LOWERCASE

    class Name2(String):
        case = TypeNameCase.CLASS_NAME

    class Name3(String):
        case = TypeNameCase.SNAKE_CASE

    assert Name.get_attribute_name() == "name"
    assert Name2.get_attribute_name() == "Name2"
    assert Name3.get_attribute_name() == "name3"


def test_attribute_acronym_handling():
    """Test handling of acronyms in attribute class names."""

    class APIKey(String):
        case = TypeNameCase.SNAKE_CASE

    # Should handle acronyms gracefully
    assert APIKey.get_attribute_name() == "api_key"


def test_attribute_multiple_consecutive_caps():
    """Test handling of multiple consecutive capital letters."""

    class XMLData(String):
        case = TypeNameCase.SNAKE_CASE

    assert XMLData.get_attribute_name() == "xml_data"


def test_attribute_case_with_different_value_types():
    """Test that case formatting works with all value types."""

    class PersonAge(Integer):
        case = TypeNameCase.SNAKE_CASE

    class PersonScore(Double):
        case = TypeNameCase.SNAKE_CASE

    class IsActive(Boolean):
        case = TypeNameCase.SNAKE_CASE

    class CreatedAt(DateTime):
        case = TypeNameCase.SNAKE_CASE

    assert PersonAge.get_attribute_name() == "person_age"
    assert PersonScore.get_attribute_name() == "person_score"
    assert IsActive.get_attribute_name() == "is_active"
    assert CreatedAt.get_attribute_name() == "created_at"


def test_entity_owns_attribute_with_formatted_name():
    """Test that entities correctly own attributes with formatted names."""

    class PersonName(String):
        case = TypeNameCase.SNAKE_CASE

    class PersonAge(Integer):
        case = TypeNameCase.SNAKE_CASE

    class Person(Entity):
        flags = TypeFlags(case=TypeNameCase.SNAKE_CASE)
        name: PersonName = Flag(Key)
        age: PersonAge

    # Check schema generation includes formatted attribute names
    schema = Person.to_schema_definition()
    assert schema is not None
    assert "entity person" in schema  # Entity uses snake_case too
    assert "owns person_name @key" in schema
    assert "owns person_age" in schema


def test_attribute_insert_query_uses_formatted_name():
    """Test that insert queries use the formatted attribute name."""

    class PersonName(String):
        case = TypeNameCase.SNAKE_CASE

    class Person(Entity):
        name: PersonName = Flag(Key)

    person = Person(name=PersonName("Alice"))
    query = person.to_insert_query()
    assert "has person_name" in query


def test_mixed_case_attributes_in_entity():
    """Test that an entity can own attributes with different case settings."""

    class FirstName(String):
        case = TypeNameCase.SNAKE_CASE

    class LastName(String):
        case = TypeNameCase.CLASS_NAME

    class Age(Integer):
        # Default CLASS_NAME
        pass

    class Person(Entity):
        first_name: FirstName
        last_name: LastName
        age: Age

    schema = Person.to_schema_definition()
    assert schema is not None
    assert "owns first_name" in schema  # snake_case
    assert "owns LastName" in schema  # CLASS_NAME
    assert "owns Age" in schema  # CLASS_NAME (default)


def test_attribute_inheritance_preserves_case():
    """Test that child attribute classes can have different case than parents."""

    class BaseString(String):
        case = TypeNameCase.LOWERCASE

    class PersonName(BaseString):
        case = TypeNameCase.SNAKE_CASE

    # Child class should use its own case setting
    assert PersonName.get_attribute_name() == "person_name"


def test_attribute_explicit_name_validation():
    """Test that explicitly set attribute names are properly validated."""

    class Name(String):
        attr_name = "custom_name"

    assert Name.get_attribute_name() == "custom_name"

    schema = Name.to_schema_definition()
    assert "attribute custom_name" in schema
