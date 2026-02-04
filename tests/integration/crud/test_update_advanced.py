"""Integration tests for advanced entity update scenarios."""

import pytest
from type_bridge import (
    Card,
    Entity,
    Flag,
    Integer,
    Key,
    SchemaManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
@pytest.mark.order(160)
def test_update_entity_add_optional_attribute(clean_db):
    """Test adding value to optional attribute via update."""

    # Arrange
    class Name(String):
        pass

    class Email(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        email: Email | None = None

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)

    # Insert without email
    alice = Person(name=Name("Alice"), email=None)
    manager.insert(alice)

    # Act - Add email
    fetched = manager.get(name="Alice")[0]
    fetched.email = Email("alice@example.com")
    manager.update(fetched)

    # Assert
    updated = manager.get(name="Alice")[0]
    expected = "alice@example.com"
    actual = updated.email.value if updated.email else None
    assert expected == actual


@pytest.mark.integration
@pytest.mark.order(161)
def test_update_entity_modify_existing_attribute(clean_db):
    """Test modifying existing attribute value."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)
    bob = Person(name=Name("Bob"), age=Age(25))
    manager.insert(bob)

    # Act - Update age
    fetched = manager.get(name="Bob")[0]
    fetched.age = Age(26)
    manager.update(fetched)

    # Assert
    updated = manager.get(name="Bob")[0]
    expected = 26
    actual = updated.age.value
    assert expected == actual


@pytest.mark.integration
@pytest.mark.order(162)
def test_update_entity_with_multi_value_attribute_append(clean_db):
    """Test appending values to multi-value attribute."""

    # Arrange
    class Name(String):
        pass

    class Tag(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        tags: list[Tag] = Flag(Card(min=0))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)
    charlie = Person(name=Name("Charlie"), tags=[Tag("developer")])
    manager.insert(charlie)

    # Act - Add more tags
    fetched = manager.get(name="Charlie")[0]
    fetched.tags = [Tag("developer"), Tag("python"), Tag("senior")]
    manager.update(fetched)

    # Assert
    updated = manager.get(name="Charlie")[0]
    expected = 3
    actual = len(updated.tags)
    assert expected == actual


@pytest.mark.integration
@pytest.mark.order(163)
def test_update_preserves_other_attributes(clean_db):
    """Test that updating one attribute doesn't affect others."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Email(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age
        email: Email

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)
    diana = Person(name=Name("Diana"), age=Age(30), email=Email("diana@test.com"))
    manager.insert(diana)

    # Act - Update only age
    fetched = manager.get(name="Diana")[0]
    fetched.age = Age(31)
    manager.update(fetched)

    # Assert - Email should be preserved
    updated = manager.get(name="Diana")[0]
    expected_age = 31
    expected_email = "diana@test.com"
    assert expected_age == updated.age.value
    assert expected_email == updated.email.value


@pytest.mark.integration
@pytest.mark.order(164)
def test_update_with_inherited_attributes(clean_db):
    """Test updating entities with inherited attributes."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Salary(Integer):
        pass

    class BasePerson(Entity):
        flags = TypeFlags(name="base_person")
        name: Name = Flag(Key)
        age: Age

    class Employee(BasePerson):
        flags = TypeFlags(name="employee")
        salary: Salary

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(BasePerson, Employee)
    schema_manager.sync_schema(force=True)

    manager = Employee.manager(clean_db)
    eve = Employee(name=Name("Eve"), age=Age(28), salary=Salary(70000))
    manager.insert(eve)

    # Act - Update both inherited and own attribute
    fetched = manager.all()[0]
    fetched.age = Age(29)  # Inherited
    fetched.salary = Salary(75000)  # Own
    manager.update(fetched)

    # Assert
    updated = manager.all()[0]
    expected_age = 29
    expected_salary = 75000
    assert expected_age == updated.age.value
    assert expected_salary == updated.salary.value


@pytest.mark.integration
@pytest.mark.order(165)
def test_update_entity_remove_optional_attribute(clean_db):
    """Test setting optional attribute to None."""

    # Arrange
    class Name(String):
        pass

    class Notes(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        notes: Notes | None = None

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)
    frank = Person(name=Name("Frank"), notes=Notes("Some notes"))
    manager.insert(frank)

    # Act - Remove notes
    fetched = manager.get(name="Frank")[0]
    fetched.notes = None
    manager.update(fetched)

    # Assert
    updated = manager.get(name="Frank")[0]
    expected = None
    actual = updated.notes
    assert expected == actual


@pytest.mark.integration
@pytest.mark.order(166)
def test_update_multiple_entities_separately(clean_db):
    """Test updating multiple entities maintains independence."""

    # Arrange
    class Name(String):
        pass

    class Score(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        score: Score

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)
    people = [
        Person(name=Name("Grace"), score=Score(100)),
        Person(name=Name("Henry"), score=Score(200)),
    ]
    manager.insert_many(people)

    # Act - Update Grace only
    grace = manager.get(name="Grace")[0]
    grace.score = Score(150)
    manager.update(grace)

    # Assert - Henry unchanged, Grace updated
    updated_grace = manager.get(name="Grace")[0]
    updated_henry = manager.get(name="Henry")[0]

    expected_grace_score = 150
    expected_henry_score = 200
    assert expected_grace_score == updated_grace.score.value
    assert expected_henry_score == updated_henry.score.value


@pytest.mark.integration
@pytest.mark.order(167)
def test_update_entity_clears_multi_value_attribute(clean_db):
    """Test clearing all values from multi-value attribute."""

    # Arrange
    class Name(String):
        pass

    class Skill(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        skills: list[Skill] = Flag(Card(min=0))

    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    manager = Person.manager(clean_db)
    iris = Person(name=Name("Iris"), skills=[Skill("Python"), Skill("Java")])
    manager.insert(iris)

    # Act - Clear skills
    fetched = manager.get(name="Iris")[0]
    fetched.skills = []
    manager.update(fetched)

    # Assert
    updated = manager.get(name="Iris")[0]
    expected = 0
    actual = len(updated.skills)
    assert expected == actual
