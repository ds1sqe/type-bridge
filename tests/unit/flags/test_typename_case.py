"""Tests for TypeNameCase formatting options for Entity and Relation types."""

from type_bridge import (
    Entity,
    Integer,
    Relation,
    Role,
    String,
    TypeFlags,
    TypeNameCase,
)


class Name(String):
    """Name attribute for testing."""

    pass


class Age(Integer):
    """Age attribute for testing."""

    pass


class Employee(Entity):
    """Employee entity for relation tests."""

    flags = TypeFlags(abstract=True)


def test_typename_case_classname_default():
    """Test that CLASS_NAME is the default case formatting."""

    class PersonName(Entity):
        # No explicit case parameter - should default to CLASS_NAME
        name: Name

    assert PersonName.get_type_name() == "PersonName"


def test_typename_case_lowercase_explicit():
    """Test explicit LOWERCASE case formatting."""

    class PersonName(Entity):
        flags = TypeFlags(case=TypeNameCase.LOWERCASE)
        name: Name

    assert PersonName.get_type_name() == "personname"


def test_typename_case_classname():
    """Test CLASS_NAME case formatting (keeps as-is)."""

    class PersonName(Entity):
        flags = TypeFlags(case=TypeNameCase.CLASS_NAME)
        name: Name

    assert PersonName.get_type_name() == "PersonName"


def test_typename_case_snake_case():
    """Test SNAKE_CASE formatting."""

    class PersonName(Entity):
        flags = TypeFlags(case=TypeNameCase.SNAKE_CASE)
        name: Name

    assert PersonName.get_type_name() == "person_name"


def test_typename_case_snake_case_complex():
    """Test SNAKE_CASE with complex class names."""

    class HTTPResponseData(Entity):
        flags = TypeFlags(case=TypeNameCase.SNAKE_CASE)
        name: Name

    assert HTTPResponseData.get_type_name() == "http_response_data"


def test_typename_explicit_takes_precedence():
    """Test that explicit type_name takes precedence over case formatting."""

    class PersonName(Entity):
        flags = TypeFlags(name="custom_name", case=TypeNameCase.SNAKE_CASE)
        name: Name

    # Should use explicit type_name, not apply case formatting
    assert PersonName.get_type_name() == "custom_name"


def test_relation_typename_case_lowercase():
    """Test LOWERCASE case formatting for relations."""

    class PersonEmployment(Relation):
        flags = TypeFlags(case=TypeNameCase.LOWERCASE)
        employee: Role[Employee] = Role("employee", Employee)

    assert PersonEmployment.get_type_name() == "personemployment"


def test_relation_typename_case_classname():
    """Test CLASS_NAME case formatting for relations."""

    class PersonEmployment(Relation):
        flags = TypeFlags(case=TypeNameCase.CLASS_NAME)
        employee: Role[Employee] = Role("employee", Employee)

    assert PersonEmployment.get_type_name() == "PersonEmployment"


def test_relation_typename_case_snake_case():
    """Test SNAKE_CASE formatting for relations."""

    class PersonEmployment(Relation):
        flags = TypeFlags(case=TypeNameCase.SNAKE_CASE)
        employee: Role[Employee] = Role("employee", Employee)

    assert PersonEmployment.get_type_name() == "person_employment"


def test_relation_typename_explicit_takes_precedence():
    """Test that explicit type_name takes precedence for relations."""

    class PersonEmployment(Relation):
        flags = TypeFlags(name="employment", case=TypeNameCase.SNAKE_CASE)
        employee: Role[Employee] = Role("employee", Employee)

    assert PersonEmployment.get_type_name() == "employment"


def test_schema_generation_with_snake_case():
    """Test that schema generation uses the formatted type name."""

    class PersonName(Entity):
        flags = TypeFlags(case=TypeNameCase.SNAKE_CASE)
        name: Name

    schema = PersonName.to_schema_definition()
    assert schema is not None
    assert "entity person_name" in schema


def test_schema_generation_with_classname():
    """Test that schema generation uses CLASS_NAME correctly."""

    class PersonName(Entity):
        flags = TypeFlags(case=TypeNameCase.CLASS_NAME)
        name: Name

    schema = PersonName.to_schema_definition()
    assert schema is not None
    assert "entity PersonName" in schema


def test_single_word_class_all_cases():
    """Test that single-word class names work correctly with all case options."""

    class Person(Entity):
        flags = TypeFlags(case=TypeNameCase.LOWERCASE)
        name: Name

    class Person2(Entity):
        flags = TypeFlags(case=TypeNameCase.CLASS_NAME)
        name: Name

    class Person3(Entity):
        flags = TypeFlags(case=TypeNameCase.SNAKE_CASE)
        name: Name

    assert Person.get_type_name() == "person"
    assert Person2.get_type_name() == "Person2"
    assert Person3.get_type_name() == "person3"


def test_acronym_handling():
    """Test handling of acronyms in class names."""

    class APIKey(Entity):
        flags = TypeFlags(case=TypeNameCase.SNAKE_CASE)
        name: Name

    # Should handle acronyms gracefully
    assert APIKey.get_type_name() == "api_key"


def test_multiple_consecutive_caps():
    """Test handling of multiple consecutive capital letters."""

    class XMLParser(Entity):
        flags = TypeFlags(case=TypeNameCase.SNAKE_CASE)
        name: Name

    assert XMLParser.get_type_name() == "xml_parser"


def test_typename_case_in_inheritance():
    """Test that child classes can have different case formatting than parents."""

    class BaseEntity(Entity):
        flags = TypeFlags(case=TypeNameCase.LOWERCASE, base=True)
        name: Name

    class PersonName(BaseEntity):
        flags = TypeFlags(case=TypeNameCase.SNAKE_CASE)
        age: Age

    assert PersonName.get_type_name() == "person_name"


def test_to_insert_query_uses_formatted_name():
    """Test that insert queries use the formatted type name."""

    class PersonName(Entity):
        flags = TypeFlags(case=TypeNameCase.SNAKE_CASE)
        name: Name

    person = PersonName(name=Name("Alice"))
    query = person.to_insert_query()
    assert "isa person_name" in query
