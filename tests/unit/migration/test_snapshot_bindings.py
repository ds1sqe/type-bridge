import sys
import uuid
from typing import Any, cast

import pytest

from type_bridge import Entity, TypeFlags
from type_bridge.migration import operations as ops
from type_bridge.migration.generator import MigrationGenerator
from type_bridge.migration.snapshots import (
    generate_snapshot,
    get_snapshot_metadata,
)
from type_bridge.models.registry import ModelRegistry


@pytest.fixture
def temp_migrations_dir(tmp_path):
    # Use a unique directory name for each test to avoid Python module import caching collisions
    unique_name = f"migrations_{uuid.uuid4().hex[:8]}"
    migrations_dir = tmp_path / unique_name
    migrations_dir.mkdir()
    yield migrations_dir
    # Clean up sys.modules to avoid polluting other tests
    for key in list(sys.modules.keys()):
        if key == unique_name or key.startswith(f"{unique_name}."):
            del sys.modules[key]


def test_generate_snapshot_and_metadata(temp_migrations_dir):
    schema_text = """
    define
    entity person,
        owns name @key;
    attribute name, value string;
    """

    snapshot_dir = generate_snapshot(
        migrations_dir=temp_migrations_dir,
        version="v0001",
        migration_name="0001_initial",
        schema_text=schema_text,
    )

    assert snapshot_dir.exists()
    assert (snapshot_dir / "__init__.py").exists()
    assert (snapshot_dir / "attributes.py").exists()
    assert (snapshot_dir / "entities.py").exists()
    assert (snapshot_dir / "registry.py").exists()
    assert (snapshot_dir / "schema.tql").exists()
    assert (snapshot_dir / "snapshot.json").exists()
    init_text = (snapshot_dir / "__init__.py").read_text()
    assert "from .attributes import Name" in init_text
    assert "from .entities import Person" in init_text

    metadata = get_snapshot_metadata(snapshot_dir)
    assert metadata is not None
    assert metadata["version"] == "v0001"
    assert metadata["source_migration"] == "0001_initial"
    assert "schema_hash" in metadata
    assert "file_hashes" in metadata
    assert "attributes.py" in metadata["file_hashes"]
    assert "entities.py" in metadata["file_hashes"]

    sys.path.insert(0, str(temp_migrations_dir.parent))
    try:
        import importlib

        snapshot_mod = importlib.import_module(f"{temp_migrations_dir.name}.snapshots.v0001")
        assert getattr(snapshot_mod, "Person").get_type_name() == "person"
        assert getattr(snapshot_mod, "Name").get_attribute_name() == "name"
    finally:
        try:
            sys.path.remove(str(temp_migrations_dir.parent))
        except ValueError:
            pass


def test_generate_snapshot_rejects_stale_existing_snapshot(temp_migrations_dir):
    schema_text_1 = """
    define
    entity person,
        owns name @key;
    attribute name, value string;
    """
    schema_text_2 = """
    define
    entity company,
        owns name @key;
    attribute name, value string;
    """

    generate_snapshot(
        migrations_dir=temp_migrations_dir,
        version="v0001",
        migration_name="0001_initial",
        schema_text=schema_text_1,
    )

    with pytest.raises(ValueError, match="schema hash mismatch"):
        generate_snapshot(
            migrations_dir=temp_migrations_dir,
            version="v0001",
            migration_name="0001_initial",
            schema_text=schema_text_2,
        )


def test_registry_isolation(temp_migrations_dir, monkeypatch):
    # Enable import by putting the temp directory in sys.path
    monkeypatch.syspath_prepend(temp_migrations_dir.parent)

    schema_text = """
    define
    entity isolating_person,
        owns name @key;
    attribute name, value string;
    """

    generate_snapshot(
        migrations_dir=temp_migrations_dir,
        version="v0001",
        migration_name="0001_initial",
        schema_text=schema_text,
    )

    # Clean resolution cache and registry to check isolation
    ModelRegistry.clear()

    # Import the snapshot class
    import importlib

    app_name = temp_migrations_dir.name
    entities_mod = importlib.import_module(f"{app_name}.snapshots.v0001.entities")

    # Check that it did NOT register in the global ModelRegistry
    assert ModelRegistry.get("isolating_person") is None

    # But it is a valid type-bridge Entity subclass
    PersonCls = getattr(entities_mod, "IsolatingPerson")
    assert issubclass(PersonCls, Entity)
    assert PersonCls.get_type_name() == "isolating_person"


def test_resolve_subclasses_isolated(temp_migrations_dir, monkeypatch):
    monkeypatch.syspath_prepend(temp_migrations_dir.parent)

    schema_text = """
    define
    entity employee sub person;
    entity person,
        owns name @key;
    attribute name, value string;
    """

    generate_snapshot(
        migrations_dir=temp_migrations_dir,
        version="v0001",
        migration_name="0001_initial",
        schema_text=schema_text,
    )

    # Load snapshot classes
    import importlib

    app_name = temp_migrations_dir.name
    entities_mod = importlib.import_module(f"{app_name}.snapshots.v0001.entities")
    SnapshotPerson = getattr(entities_mod, "Person")
    SnapshotEmployee = getattr(entities_mod, "Employee")

    # Define an active app model class Person with the same TypeDB name but NOT a snapshot
    class ActivePerson(Entity):
        flags = TypeFlags(name="person")
        name: str

    class ActiveEmployee(ActivePerson):
        flags = TypeFlags(name="employee")

    # The active class register themselves in ModelRegistry
    assert ModelRegistry.get("person") == ActivePerson
    assert ModelRegistry.get("employee") == ActiveEmployee

    # Polymorphic resolution from ActivePerson should only resolve ActiveEmployee, NOT SnapshotEmployee
    res_active = ModelRegistry.resolve("employee", (ActivePerson,))
    assert res_active == ActiveEmployee

    # Polymorphic resolution from SnapshotPerson should only resolve SnapshotEmployee, NOT ActiveEmployee
    res_snap = ModelRegistry.resolve("employee", (SnapshotPerson,))
    assert res_snap == SnapshotEmployee


def test_generator_uses_snapshots(temp_migrations_dir, monkeypatch):
    monkeypatch.syspath_prepend(temp_migrations_dir.parent)

    # Pre-populate snapshot v0001
    schema_text_1 = """
    define
    entity person,
        owns name @key;
    attribute name, value string;
    """
    generate_snapshot(
        migrations_dir=temp_migrations_dir,
        version="v0001",
        migration_name="0001_initial",
        schema_text=schema_text_1,
    )

    # Target state has modified schema (added company, removed person)
    schema_text_2 = """
    define
    entity company,
        owns name @key;
    attribute name, value string;
    """
    generate_snapshot(
        migrations_dir=temp_migrations_dir,
        version="v0002",
        migration_name="0002_add_company",
        schema_text=schema_text_2,
    )

    # Let's instantiate generator
    class DummyDB:
        def database_exists(self):
            return True

        def transaction(self, mode):
            class DummyTx:
                def __enter__(self):
                    return self

                def __exit__(self, *args):
                    pass

                def execute(self, q):
                    return []

                def commit(self):
                    pass

            return DummyTx()

    generator = MigrationGenerator(cast(Any, DummyDB()), temp_migrations_dir)
    generator._pre_version = "v0001"
    generator._post_version = "v0002"

    # Define operations representing the changes
    from type_bridge.migration.ref import EntityRef

    remove_person = ops.RemoveEntity(EntityRef("person"))
    add_company = ops.AddEntity(EntityRef("company"))

    # Render operations
    generator._collect_imports_and_aliases([remove_person, add_company])
    rendered_ops = generator._render_operations([remove_person, add_company])
    rendered_imports = generator._generate_operations_imports([remove_person, add_company])

    # Check generated imports
    assert f"from {temp_migrations_dir.name}.snapshots.v0001 import Person" in rendered_imports
    assert f"from {temp_migrations_dir.name}.snapshots.v0002 import Company" in rendered_imports

    # Check rendered operations use class symbols
    assert "ops.RemoveEntity(Person)" in rendered_ops
    assert "ops.AddEntity(Company)" in rendered_ops


def test_generator_aliases_colliding_symbols(temp_migrations_dir, monkeypatch):
    monkeypatch.syspath_prepend(temp_migrations_dir.parent)

    # Pre-populate v0001: person owns name
    schema_text_1 = """
    define
    entity person,
        owns name @key;
    attribute name, value string;
    """
    generate_snapshot(
        migrations_dir=temp_migrations_dir,
        version="v0001",
        migration_name="0001_initial",
        schema_text=schema_text_1,
    )

    # Pre-populate v0002: person owns name and age
    schema_text_2 = """
    define
    entity person,
        owns name @key,
        owns age;
    attribute name, value string;
    attribute age, value integer;
    """
    generate_snapshot(
        migrations_dir=temp_migrations_dir,
        version="v0002",
        migration_name="0002_add_age",
        schema_text=schema_text_2,
    )

    # Generator setup
    class DummyDB:
        def database_exists(self):
            return True

    generator = MigrationGenerator(cast(Any, DummyDB()), temp_migrations_dir)
    generator._pre_version = "v0001"
    generator._post_version = "v0002"

    from type_bridge.migration.ref import AttributeRef, EntityRef

    # We remove age from person of v0001, and add age to person of v0002
    op_remove = ops.RemoveOwnership(EntityRef("person"), AttributeRef("name"))
    op_add = ops.AddOwnership(EntityRef("person"), AttributeRef("age"))

    # Render
    generator._collect_imports_and_aliases([op_remove, op_add])
    rendered_ops = generator._render_operations([op_remove, op_add])
    rendered_imports = generator._generate_operations_imports([op_remove, op_add])

    # v0001.Person should be aliased (e.g. PersonV0001) because both v0001 and v0002 define Person
    # and v0002 is the latest version so it gets the unaliased Person symbol.
    assert "Person as PersonV0001" in rendered_imports
    assert f"from {temp_migrations_dir.name}.snapshots.v0002 import Age, Person" in rendered_imports
    assert "ops.RemoveOwnership(PersonV0001" in rendered_ops
    assert "ops.AddOwnership(Person" in rendered_ops


def test_generator_aliases_copy_attribute_owner(temp_migrations_dir, monkeypatch):
    monkeypatch.syspath_prepend(temp_migrations_dir.parent)

    schema_text_1 = """
    define
    entity person,
        owns name @key;
    attribute name, value string;
    """
    schema_text_2 = """
    define
    entity person,
        owns name @key,
        owns age;
    attribute name, value string;
    attribute age, value integer;
    """
    generate_snapshot(
        migrations_dir=temp_migrations_dir,
        version="v0001",
        migration_name="0001_initial",
        schema_text=schema_text_1,
    )
    generate_snapshot(
        migrations_dir=temp_migrations_dir,
        version="v0002",
        migration_name="0002_add_age",
        schema_text=schema_text_2,
    )

    class DummyDB:
        def database_exists(self):
            return True

    generator = MigrationGenerator(cast(Any, DummyDB()), temp_migrations_dir)
    generator._pre_version = "v0001"
    generator._post_version = "v0002"

    from type_bridge.migration.ref import AttributeRef, EntityRef

    operations = [
        ops.CopyAttribute(owner=EntityRef("person"), source="name", dest="age"),
        ops.AddOwnership(EntityRef("person"), AttributeRef("age")),
    ]

    generator._collect_imports_and_aliases(operations)
    rendered_ops = generator._render_operations(operations)
    rendered_imports = generator._generate_operations_imports(operations)

    assert f"from {temp_migrations_dir.name}.snapshots.v0001 import Person as PersonV0001" in (
        rendered_imports
    )
    assert f"from {temp_migrations_dir.name}.snapshots.v0002 import Age, Person" in (
        rendered_imports
    )
    assert "ops.CopyAttribute(owner=PersonV0001" in rendered_ops
    assert "ops.AddOwnership(Person, Age)" in rendered_ops
