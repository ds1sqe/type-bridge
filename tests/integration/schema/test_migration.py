"""Integration tests for schema migration functionality."""

import pytest

from type_bridge import (
    AttributeFlags,
    Entity,
    Flag,
    Integer,
    Key,
    MigrationManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
@pytest.mark.order(100)
def test_migration_manager_initialization(clean_db):
    """Test that MigrationManager initializes correctly."""
    # Arrange & Act
    manager = MigrationManager(clean_db)

    # Assert
    expected_migrations = []
    assert expected_migrations == manager.migrations
    assert clean_db == manager.db


@pytest.mark.integration
@pytest.mark.order(101)
def test_add_migration(clean_db):
    """Test adding migrations to the manager."""
    # Arrange
    manager = MigrationManager(clean_db)
    migration_name = "add_person_entity"
    migration_schema = "define\nentity person, owns name;"

    # Act
    manager.add_migration(migration_name, migration_schema)

    # Assert
    expected = [(migration_name, migration_schema)]
    assert expected == manager.migrations


@pytest.mark.integration
@pytest.mark.order(102)
def test_add_multiple_migrations(clean_db):
    """Test adding multiple migrations in order."""
    # Arrange
    manager = MigrationManager(clean_db)

    # Act
    manager.add_migration("migration1", "define\nattribute name, value string;")
    manager.add_migration("migration2", "define\nentity person, owns name;")
    manager.add_migration("migration3", "define\nattribute age, value integer;")

    # Assert
    expected = 3
    actual = len(manager.migrations)
    assert expected == actual

    # Verify order is preserved
    expected_first_name = "migration1"
    actual_first_name = manager.migrations[0][0]
    assert expected_first_name == actual_first_name

    expected_last_name = "migration3"
    actual_last_name = manager.migrations[2][0]
    assert expected_last_name == actual_last_name


@pytest.mark.integration
@pytest.mark.order(103)
def test_create_attribute_migration(clean_db):
    """Test generating attribute migration TypeQL."""
    # Arrange
    manager = MigrationManager(clean_db)

    # Act
    actual = manager.create_attribute_migration("email", "string")

    # Assert
    expected = "define\nattribute email, value string;"
    assert expected == actual


@pytest.mark.integration
@pytest.mark.order(104)
def test_create_entity_migration(clean_db):
    """Test generating entity migration TypeQL."""
    # Arrange
    manager = MigrationManager(clean_db)

    # Act
    actual = manager.create_entity_migration("person", ["name", "age", "email"])

    # Assert
    expected = "define\nentity person\n    owns name\n    owns age\n    owns email\n;"
    assert expected == actual


@pytest.mark.integration
@pytest.mark.order(105)
def test_create_entity_migration_no_attributes(clean_db):
    """Test generating entity migration without attributes."""
    # Arrange
    manager = MigrationManager(clean_db)

    # Act
    actual = manager.create_entity_migration("company", [])

    # Assert
    expected = "define\nentity company\n;"
    assert expected == actual


@pytest.mark.integration
@pytest.mark.order(106)
def test_create_relation_migration(clean_db):
    """Test generating relation migration TypeQL."""
    # Arrange
    manager = MigrationManager(clean_db)
    roles = [("employee", "person"), ("employer", "company")]
    attributes = ["position", "salary"]

    # Act
    actual = manager.create_relation_migration("employment", roles, attributes)

    # Assert
    expected = (
        "define\n"
        "relation employment\n"
        "    relates employee\n"
        "    relates employer\n"
        "    owns position\n"
        "    owns salary\n"
        ";\n"
        "\n"
        "person plays employment:employee;\n"
        "company plays employment:employer;"
    )
    assert expected == actual


@pytest.mark.integration
@pytest.mark.order(107)
def test_create_relation_migration_no_attributes(clean_db):
    """Test generating relation migration without owned attributes."""
    # Arrange
    manager = MigrationManager(clean_db)
    roles = [("parent", "person"), ("child", "person")]

    # Act
    actual = manager.create_relation_migration("parentship", roles, None)

    # Assert
    expected = (
        "define\n"
        "relation parentship\n"
        "    relates parent\n"
        "    relates child\n"
        ";\n"
        "\n"
        "person plays parentship:parent;\n"
        "person plays parentship:child;"
    )
    assert expected == actual


@pytest.mark.integration
@pytest.mark.order(108)
def test_apply_migrations_single(clean_db):
    """Test applying a single migration to database."""
    # Arrange
    manager = MigrationManager(clean_db)

    # Create attribute and entity migration
    attr_migration = manager.create_attribute_migration("name", "string")
    entity_migration = manager.create_entity_migration("person", ["name"])

    manager.add_migration("add_name_attribute", attr_migration)
    manager.add_migration("add_person_entity", entity_migration)

    # Act
    manager.apply_migrations()

    # Assert - Verify schema was applied by using TypeBridge entity manager
    # Define the entity class to match the schema
    # Note: AttributeFlags(name="name") to match migration-created "name" attribute
    class Name(String):
        flags = AttributeFlags(name="name")

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    # Try to create manager (will fail if schema doesn't exist)
    person_mgr = Person.manager(clean_db)

    # Verify we can query (should return empty list, not error)
    result = person_mgr.all()
    expected = 0
    actual = len(result)
    assert expected == actual

    # Migration list should still be intact (migrations are not cleared)
    expected_count = 2
    actual_count = len(manager.migrations)
    assert expected_count == actual_count


@pytest.mark.integration
@pytest.mark.order(109)
def test_apply_migrations_complex_schema(clean_db):
    """Test applying multiple migrations creating complex schema."""
    # Arrange
    manager = MigrationManager(clean_db)

    # Build complete schema through migrations
    manager.add_migration(
        "attributes",
        """define
attribute name, value string;
attribute age, value integer;
attribute position, value string;""",
    )

    manager.add_migration(
        "entities",
        """define
entity person, owns name, owns age;
entity company, owns name;""",
    )

    manager.add_migration(
        "relations",
        """define
relation employment,
    relates employee,
    relates employer,
    owns position;

person plays employment:employee;
company plays employment:employer;""",
    )

    # Act
    manager.apply_migrations()

    # Assert - Verify all types exist by using TypeBridge managers
    # Test 1-4: Can insert and query actual data using TypeBridge
    class Name(String):
        flags = AttributeFlags(name="name")

    class Age(Integer):
        flags = AttributeFlags(name="age")

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age

    manager_person = Person.manager(clean_db)
    alice = Person(name=Name("Alice"), age=Age(30))
    manager_person.insert(alice)

    # Fetch and verify
    results = manager_person.all()
    expected = 1
    actual = len(results)
    assert expected == actual
    assert "Alice" == results[0].name.value
