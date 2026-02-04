"""Integration tests for schema diff and comparison functionality."""

import pytest
from type_bridge import Entity, Flag, Integer, Key, Relation, Role, String, TypeFlags
from type_bridge.migration.diff import (
    AttributeFlagChange,
    EntityChanges,
    RelationChanges,
)
from type_bridge.migration.info import SchemaInfo


@pytest.mark.integration
@pytest.mark.order(110)
def test_schema_diff_no_changes(clean_db):
    """Test that comparing identical schemas shows no changes."""

    # Arrange
    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    schema1 = SchemaInfo()
    schema1.entities.append(Person)
    schema1.attribute_classes.add(Name)

    schema2 = SchemaInfo()
    schema2.entities.append(Person)
    schema2.attribute_classes.add(Name)

    # Act
    diff = schema1.compare(schema2)

    # Assert
    expected = False
    actual = diff.has_changes()
    assert expected == actual

    expected_summary = "No schema changes detected."
    actual_summary = diff.summary()
    assert expected_summary in actual_summary


@pytest.mark.integration
@pytest.mark.order(111)
def test_schema_diff_detect_added_entities(clean_db):
    """Test detecting newly added entities."""

    # Arrange
    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    class Company(Entity):
        flags = TypeFlags(name="company")
        name: Name = Flag(Key)

    schema1 = SchemaInfo()
    schema1.entities.append(Person)
    schema1.attribute_classes.add(Name)

    schema2 = SchemaInfo()
    schema2.entities.append(Person)
    schema2.entities.append(Company)  # New entity
    schema2.attribute_classes.add(Name)

    # Act
    diff = schema1.compare(schema2)

    # Assert
    expected = True
    actual = diff.has_changes()
    assert expected == actual

    expected_count = 1
    actual_count = len(diff.added_entities)
    assert expected_count == actual_count
    assert Company in diff.added_entities


@pytest.mark.integration
@pytest.mark.order(112)
def test_schema_diff_detect_removed_entities(clean_db):
    """Test detecting removed entities."""

    # Arrange
    class Name(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    class Company(Entity):
        flags = TypeFlags(name="company")
        name: Name = Flag(Key)

    schema1 = SchemaInfo()
    schema1.entities.append(Person)
    schema1.entities.append(Company)
    schema1.attribute_classes.add(Name)

    schema2 = SchemaInfo()
    schema2.entities.append(Person)  # Company removed
    schema2.attribute_classes.add(Name)

    # Act
    diff = schema1.compare(schema2)

    # Assert
    expected = True
    actual = diff.has_changes()
    assert expected == actual

    expected_count = 1
    actual_count = len(diff.removed_entities)
    assert expected_count == actual_count
    assert Company in diff.removed_entities


@pytest.mark.integration
@pytest.mark.order(113)
def test_schema_diff_detect_added_attributes(clean_db):
    """Test detecting newly added attribute types."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    schema1 = SchemaInfo()
    schema1.entities.append(Person)
    schema1.attribute_classes.add(Name)

    schema2 = SchemaInfo()
    schema2.entities.append(Person)
    schema2.attribute_classes.add(Name)
    schema2.attribute_classes.add(Age)  # New attribute type

    # Act
    diff = schema1.compare(schema2)

    # Assert
    expected = True
    actual = diff.has_changes()
    assert expected == actual

    expected_count = 1
    actual_count = len(diff.added_attributes)
    assert expected_count == actual_count
    assert Age in diff.added_attributes


@pytest.mark.integration
@pytest.mark.order(114)
def test_schema_diff_detect_removed_attributes(clean_db):
    """Test detecting removed attribute types."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    schema1 = SchemaInfo()
    schema1.entities.append(Person)
    schema1.attribute_classes.add(Name)
    schema1.attribute_classes.add(Age)

    schema2 = SchemaInfo()
    schema2.entities.append(Person)
    schema2.attribute_classes.add(Name)  # Age removed

    # Act
    diff = schema1.compare(schema2)

    # Assert
    expected = True
    actual = diff.has_changes()
    assert expected == actual

    expected_count = 1
    actual_count = len(diff.removed_attributes)
    assert expected_count == actual_count
    assert Age in diff.removed_attributes


@pytest.mark.integration
@pytest.mark.order(115)
def test_schema_diff_detect_added_relations(clean_db):
    """Test detecting newly added relations."""

    # Arrange
    class Name(String):
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

    schema1 = SchemaInfo()
    schema1.entities.append(Person)
    schema1.entities.append(Company)
    schema1.attribute_classes.add(Name)

    schema2 = SchemaInfo()
    schema2.entities.append(Person)
    schema2.entities.append(Company)
    schema2.relations.append(Employment)  # New relation
    schema2.attribute_classes.add(Name)

    # Act
    diff = schema1.compare(schema2)

    # Assert
    expected = True
    actual = diff.has_changes()
    assert expected == actual

    expected_count = 1
    actual_count = len(diff.added_relations)
    assert expected_count == actual_count
    assert Employment in diff.added_relations


@pytest.mark.integration
@pytest.mark.order(116)
def test_schema_diff_detect_removed_relations(clean_db):
    """Test detecting removed relations."""

    # Arrange
    class Name(String):
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

    schema1 = SchemaInfo()
    schema1.entities.append(Person)
    schema1.entities.append(Company)
    schema1.relations.append(Employment)
    schema1.attribute_classes.add(Name)

    schema2 = SchemaInfo()
    schema2.entities.append(Person)
    schema2.entities.append(Company)
    # Employment removed
    schema2.attribute_classes.add(Name)

    # Act
    diff = schema1.compare(schema2)

    # Assert
    expected = True
    actual = diff.has_changes()
    assert expected == actual

    expected_count = 1
    actual_count = len(diff.removed_relations)
    assert expected_count == actual_count
    assert Employment in diff.removed_relations


@pytest.mark.integration
@pytest.mark.order(117)
def test_schema_diff_summary_formatting(clean_db):
    """Test that schema diff summary formats correctly."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    class Company(Entity):
        flags = TypeFlags(name="company")
        name: Name = Flag(Key)

    schema1 = SchemaInfo()
    schema1.entities.append(Person)
    schema1.attribute_classes.add(Name)

    schema2 = SchemaInfo()
    schema2.entities.append(Person)
    schema2.entities.append(Company)  # Added entity
    schema2.attribute_classes.add(Name)
    schema2.attribute_classes.add(Age)  # Added attribute

    # Act
    diff = schema1.compare(schema2)
    summary = diff.summary()

    # Assert
    assert "Schema Comparison Summary" in summary
    assert "Added Entities" in summary
    assert "Company" in summary
    assert "Added Attributes" in summary
    assert "age" in summary.lower() or "Age" in summary


@pytest.mark.integration
@pytest.mark.order(118)
def test_entity_changes_has_changes(clean_db):
    """Test EntityChanges.has_changes() method."""
    # Arrange & Act
    # Test 1: No changes
    changes_empty = EntityChanges()
    expected = False
    actual = changes_empty.has_changes()
    assert expected == actual

    # Test 2: Added attributes
    changes_added = EntityChanges(added_attributes=["age"])
    expected = True
    actual = changes_added.has_changes()
    assert expected == actual

    # Test 3: Removed attributes
    changes_removed = EntityChanges(removed_attributes=["email"])
    expected = True
    actual = changes_removed.has_changes()
    assert expected == actual

    # Test 4: Modified attributes
    flag_change = AttributeFlagChange(name="age", old_flags="@card(0..1)", new_flags="@card(1..1)")
    changes_modified = EntityChanges(modified_attributes=[flag_change])
    expected = True
    actual = changes_modified.has_changes()
    assert expected == actual


@pytest.mark.integration
@pytest.mark.order(119)
def test_relation_changes_has_changes(clean_db):
    """Test RelationChanges.has_changes() method."""
    # Arrange & Act
    # Test 1: No changes
    changes_empty = RelationChanges()
    expected = False
    actual = changes_empty.has_changes()
    assert expected == actual

    # Test 2: Added roles
    changes_roles = RelationChanges(added_roles=["manager"])
    expected = True
    actual = changes_roles.has_changes()
    assert expected == actual

    # Test 3: Removed roles
    changes_removed_roles = RelationChanges(removed_roles=["employee"])
    expected = True
    actual = changes_removed_roles.has_changes()
    assert expected == actual

    # Test 4: Added attributes
    changes_attrs = RelationChanges(added_attributes=["salary"])
    expected = True
    actual = changes_attrs.has_changes()
    assert expected == actual

    # Test 5: Removed attributes
    changes_removed_attrs = RelationChanges(removed_attributes=["position"])
    expected = True
    actual = changes_removed_attrs.has_changes()
    assert expected == actual
