"""End-to-end tests for migration auto-generation.

These tests verify the full workflow:
1. Define Python models
2. Auto-generate migration from models
3. Apply migration to database
4. Verify schema was created correctly
5. Modify models and generate incremental migration
"""

from pathlib import Path

import pytest

from type_bridge import Entity, Flag, Integer, Key, Relation, Role, String, TypeFlags
from type_bridge.attribute import AttributeFlags
from type_bridge.migration import (
    MigrationExecutor,
    MigrationGenerator,
    ModelRegistry,
    SchemaIntrospector,
)
from type_bridge.migration.executor import MigrationError


# Test fixtures - Version 1: Basic schema
class Name(String):
    flags = AttributeFlags(name="name")


class Age(Integer):
    flags = AttributeFlags(name="age")


class Email(String):
    flags = AttributeFlags(name="email")


class PersonV1(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)


class CompanyV1(Entity):
    flags = TypeFlags(name="company")
    name: Name = Flag(Key)


# Version 2: Person with additional attribute
class PersonV2(Entity):
    flags = TypeFlags(name="person")
    name: Name = Flag(Key)
    age: Age | None = None


# Version 3: With relation
class EmploymentV3(Relation):
    flags = TypeFlags(name="employment")
    employee: Role[PersonV2] = Role("employee", PersonV2)
    employer: Role[CompanyV1] = Role("employer", CompanyV1)


@pytest.mark.integration
@pytest.mark.order(300)
def test_generate_initial_migration(clean_db, tmp_path: Path):
    """MigrationGenerator should create initial migration from models."""
    # Arrange
    migrations_dir = tmp_path / "migrations"
    generator = MigrationGenerator(clean_db, migrations_dir)

    # Act
    path = generator.generate(
        models=[PersonV1, CompanyV1],
        name="initial",
    )

    # Assert
    assert path is not None
    assert path.exists()
    assert "0001_initial.py" in str(path)

    # Check migration content
    content = path.read_text()
    assert "class InitialMigration(Migration)" in content
    # Should reference our model names
    assert "Person" in content or "person" in content
    assert "Company" in content or "company" in content


@pytest.mark.integration
@pytest.mark.order(301)
def test_apply_generated_migration(clean_db, tmp_path: Path):
    """Generated migration should successfully apply to database."""
    # Arrange
    migrations_dir = tmp_path / "migrations"
    generator = MigrationGenerator(clean_db, migrations_dir)
    executor = MigrationExecutor(clean_db, migrations_dir)

    # Generate migration
    path = generator.generate(
        models=[PersonV1, CompanyV1],
        name="initial",
    )
    assert path is not None

    # Act - Apply the migration
    result = executor.migrate()

    # Assert - Migration applied
    assert len(result) == 1
    assert result[0].success
    assert result[0].name == "0001_initial"

    # Verify schema in database (use introspect_for_models for TypeDB 3.x compat)
    introspector = SchemaIntrospector(clean_db)
    schema = introspector.introspect_for_models([PersonV1, CompanyV1])

    assert "person" in schema.get_entity_names()
    assert "company" in schema.get_entity_names()


@pytest.mark.integration
@pytest.mark.order(310)
def test_checksum_drift_fails_before_pending_schema_change(clean_db, tmp_path: Path):
    """A drifted applied file should stop execution before pending migrations run."""
    migrations_dir = tmp_path / "migrations"
    generator = MigrationGenerator(clean_db, migrations_dir)
    executor = MigrationExecutor(clean_db, migrations_dir)

    initial_path = generator.generate(
        models=[PersonV1],
        name="initial",
    )
    assert initial_path is not None
    result = executor.migrate()
    assert len(result) == 1
    assert result[0].success

    initial_path.write_text(initial_path.read_text() + "\n# checksum drift\n")
    (migrations_dir / "0002_drift_probe.py").write_text(
        f"""
from typing import ClassVar
from type_bridge.migration import Migration, operations as ops
from type_bridge.migration.operations import Operation


class DriftProbeMigration(Migration):
    dependencies: ClassVar[list[tuple[str, str]]] = [
        ({migrations_dir.name!r}, "0001_initial"),
    ]
    operations: ClassVar[list[Operation]] = [
        ops.RunTypeQL("define attribute drift-probe, value string;")
    ]
""".lstrip()
    )

    with pytest.raises(MigrationError, match="checksum drift"):
        executor.migrate()

    schema = SchemaIntrospector(clean_db).introspect()
    assert "drift-probe" not in schema.get_attribute_names()


@pytest.mark.integration
@pytest.mark.order(302)
def test_incremental_migration_detects_new_attribute(clean_db, tmp_path: Path):
    """Generator should detect and create migration for new attribute ownership."""
    # Arrange - First create initial schema
    migrations_dir = tmp_path / "migrations"
    generator = MigrationGenerator(clean_db, migrations_dir)
    executor = MigrationExecutor(clean_db, migrations_dir)

    # Generate and apply initial migration
    generator.generate(models=[PersonV1], name="initial")
    executor.migrate()

    # Act - Generate migration for updated model (with age)
    path = generator.generate(
        models=[PersonV2],
        name="add_age",
    )

    # Assert
    assert path is not None
    assert "0002_add_age.py" in str(path)

    content = path.read_text()
    # Should have RunTypeQL operations for adding attribute and ownership
    assert "ops.RunTypeQL" in content
    assert "attribute age" in content  # defines the age attribute
    assert "person owns age" in content  # adds ownership


@pytest.mark.integration
@pytest.mark.order(303)
def test_incremental_migration_detects_new_entity(clean_db, tmp_path: Path):
    """Generator should detect new entity type."""
    # Arrange
    migrations_dir = tmp_path / "migrations"
    generator = MigrationGenerator(clean_db, migrations_dir)
    executor = MigrationExecutor(clean_db, migrations_dir)

    # Generate and apply initial migration
    generator.generate(models=[PersonV1], name="initial")
    executor.migrate()

    # Act - Add Company
    path = generator.generate(
        models=[PersonV1, CompanyV1],
        name="add_company",
    )

    # Assert
    assert path is not None
    content = path.read_text()
    # Should have RunTypeQL for adding entity
    assert "ops.RunTypeQL" in content
    # Should reference company
    assert "company" in content.lower()


@pytest.mark.integration
@pytest.mark.order(304)
def test_incremental_migration_detects_new_relation(clean_db, tmp_path: Path):
    """Generator should detect new relation type."""
    # Arrange
    migrations_dir = tmp_path / "migrations"
    generator = MigrationGenerator(clean_db, migrations_dir)
    executor = MigrationExecutor(clean_db, migrations_dir)

    # Generate and apply initial migration with entities
    generator.generate(models=[PersonV2, CompanyV1], name="initial")
    executor.migrate()

    # Act - Add Employment relation
    path = generator.generate(
        models=[PersonV2, CompanyV1, EmploymentV3],
        name="add_employment",
    )

    # Assert
    assert path is not None
    content = path.read_text()
    # Should have RunTypeQL for adding relation
    assert "ops.RunTypeQL" in content
    assert "employment" in content.lower()


@pytest.mark.integration
@pytest.mark.order(305)
def test_no_changes_returns_none(clean_db, tmp_path: Path):
    """Generator should return None when no changes detected."""
    # Arrange
    migrations_dir = tmp_path / "migrations"
    generator = MigrationGenerator(clean_db, migrations_dir)
    executor = MigrationExecutor(clean_db, migrations_dir)

    # Generate and apply initial migration
    generator.generate(models=[PersonV1], name="initial")
    executor.migrate()

    # Act - Try to generate again with same models
    path = generator.generate(
        models=[PersonV1],
        name="no_changes",
    )

    # Assert - Should return None (no changes)
    assert path is None


@pytest.mark.integration
@pytest.mark.order(306)
def test_model_registry_discover(clean_db, tmp_path: Path):
    """ModelRegistry.discover should find models in a module."""
    # Create a test module
    test_module = tmp_path / "test_models.py"
    test_module.write_text("""
from type_bridge import Entity, String, Flag, Key, TypeFlags
from type_bridge.attribute import AttributeFlags

class TestName(String):
    flags = AttributeFlags(name="test_name")

class TestPerson(Entity):
    flags = TypeFlags(name="test_person")
    name: TestName = Flag(Key)
""")

    # Add tmp_path to Python path
    import sys

    sys.path.insert(0, str(tmp_path))

    try:
        # Clear registry
        ModelRegistry.clear()

        # Act
        models = ModelRegistry.discover("test_models")

        # Assert
        assert len(models) == 1
        assert models[0].__name__ == "TestPerson"
        assert ModelRegistry.is_registered(models[0])

    finally:
        sys.path.remove(str(tmp_path))
        ModelRegistry.clear()


@pytest.mark.integration
@pytest.mark.order(307)
def test_full_workflow_with_registry(clean_db, tmp_path: Path):
    """Full workflow: register models -> generate -> apply -> verify."""
    # Arrange
    migrations_dir = tmp_path / "migrations"

    # Clear and register models
    ModelRegistry.clear()
    ModelRegistry.register(PersonV1, CompanyV1)

    # Act
    generator = MigrationGenerator(clean_db, migrations_dir)
    executor = MigrationExecutor(clean_db, migrations_dir)

    # Generate from registered models
    models = ModelRegistry.get_all()
    path = generator.generate(models=models, name="initial")
    assert path is not None

    # Apply migration
    result = executor.migrate()
    assert len(result) == 1
    assert result[0].success

    # Verify schema (use introspect_for_models for TypeDB 3.x compat)
    introspector = SchemaIntrospector(clean_db)
    schema = introspector.introspect_for_models(models)

    assert "person" in schema.get_entity_names()
    assert "company" in schema.get_entity_names()

    # Check ownerships
    person_ownerships = {o.attribute_name for o in schema.get_ownerships_for("person")}
    company_ownerships = {o.attribute_name for o in schema.get_ownerships_for("company")}

    assert "name" in person_ownerships
    assert "name" in company_ownerships

    # Cleanup
    ModelRegistry.clear()


@pytest.mark.integration
@pytest.mark.order(308)
def test_empty_migration_creation(clean_db, tmp_path: Path):
    """Generator should create empty migration when requested."""
    # Arrange
    migrations_dir = tmp_path / "migrations"
    generator = MigrationGenerator(clean_db, migrations_dir)

    # Act
    path = generator.generate(
        models=[],
        name="custom_changes",
        empty=True,
    )

    # Assert
    assert path is not None
    assert path.exists()
    content = path.read_text()
    assert "operations: ClassVar[list[Operation]] = []" in content


@pytest.mark.integration
@pytest.mark.order(309)
def test_model_file_edit_workflow(clean_db, tmp_path: Path):
    """Test realistic workflow using versioned fixture files.

    fixtures/v1/models.py has Person with name → 0001_init.py
    fixtures/v2/models.py has Person with nickname → 0002_add_nickname.py

    This simulates editing the same model file over time.
    """
    # Import v1 models from fixtures
    from tests.integration.migration.fixtures.v1 import models as v1_models_module

    migrations_dir = tmp_path / "migrations"

    try:
        # ========== Step 1: V1 models (Person with name only) ==========
        ModelRegistry.clear()
        v1_person = v1_models_module.Person

        assert v1_person.__name__ == "Person"
        assert "name" in v1_person.__annotations__
        assert "nickname" not in v1_person.__annotations__

        generator = MigrationGenerator(clean_db, migrations_dir)
        path1 = generator.generate(models=[v1_person], name="init")

        assert path1 is not None
        assert "0001_init.py" in str(path1)

        # Apply initial migration
        executor = MigrationExecutor(clean_db, migrations_dir)
        results = executor.migrate()

        assert len(results) == 1
        assert results[0].success

        # Verify initial schema (use introspect_for_models for TypeDB 3.x compat)
        introspector = SchemaIntrospector(clean_db)
        schema = introspector.introspect_for_models([v1_person])

        assert "person" in schema.get_entity_names()
        person_attrs = {o.attribute_name for o in schema.get_ownerships_for("person")}
        assert "name" in person_attrs
        assert "nickname" not in person_attrs

        # ========== Step 2: V2 models (Person with name + nickname) ==========
        from tests.integration.migration.fixtures.v2 import models as v2_models_module

        v2_person = v2_models_module.Person

        # Verify v2 model has nickname attribute
        assert "name" in v2_person.__annotations__
        assert "nickname" in v2_person.__annotations__

        # Generate incremental migration
        path2 = generator.generate(models=[v2_person], name="add_nickname")

        assert path2 is not None
        assert "0002_add_nickname.py" in str(path2)

        # Verify migration content
        content = path2.read_text()
        assert "nickname" in content.lower()

        # Apply incremental migration
        results2 = executor.migrate()

        assert len(results2) == 1
        assert results2[0].success
        assert results2[0].name == "0002_add_nickname"

        # Verify final schema (use introspect_for_models for TypeDB 3.x compat)
        schema2 = introspector.introspect_for_models([v2_person])
        person_attrs2 = {o.attribute_name for o in schema2.get_ownerships_for("person")}
        assert "name" in person_attrs2
        assert "nickname" in person_attrs2  # Now added!

    finally:
        ModelRegistry.clear()
