import sys
import uuid

import pytest

from type_bridge import Entity, Flag, Integer, Key, String, TypeFlags
from type_bridge.attribute import AttributeFlags
from type_bridge.migration import author_migration
from type_bridge.migration.info import SchemaInfo
from type_bridge.migration.introspection import (
    IntrospectedAttribute,
    IntrospectedEntity,
    IntrospectedOwnership,
    IntrospectedSchema,
)
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


def test_generator_uses_snapshots():
    pytest.importorskip("type_bridge_core")

    # Target state has modified schema (added company, removed person)
    class Company(Entity):
        flags = TypeFlags(name="company")

    base = IntrospectedSchema(entities={"person": IntrospectedEntity(name="person")})
    target = SchemaInfo()
    target.entities = [Company]

    authored = author_migration(
        base.to_rust_schema_info(),
        target.to_rust_schema_info(),
        app_label="migrations",
        name="0002_swap",
        snapshot_version="v0002",
        previous_snapshot_version="v0001",
        generated_at="t",
    )

    assert authored is not None
    source = authored.python_source

    # Removals bind to the pre snapshot, additions to the post snapshot.
    assert "from migrations.snapshots.v0001 import Person" in source
    assert "from migrations.snapshots.v0002 import Company" in source

    # Rendered operations use class symbols
    assert "ops.RemoveEntity(Person)" in source
    assert "ops.AddEntity(Company)" in source


def test_generator_aliases_colliding_symbols():
    pytest.importorskip("type_bridge_core")

    class Name(String):
        flags = AttributeFlags(name="name")

    class Age(Integer):
        flags = AttributeFlags(name="age")

    # Target state: person owns name and age (nickname dropped, age added),
    # so the migration references Person on both the pre and post side.
    class Person(Entity):
        flags = TypeFlags(name="person")

        name: Name = Flag(Key)
        age: Age | None = None

    base = IntrospectedSchema(
        entities={"person": IntrospectedEntity(name="person")},
        attributes={
            "name": IntrospectedAttribute(name="name", value_type="string"),
            "nickname": IntrospectedAttribute(name="nickname", value_type="string"),
        },
        ownerships=[
            IntrospectedOwnership(
                owner_name="person",
                attribute_name="name",
                annotations=["@key"],
            ),
            IntrospectedOwnership(owner_name="person", attribute_name="nickname"),
        ],
    )
    target = SchemaInfo()
    target.entities = [Person]
    target.attribute_classes = {Name, Age}

    authored = author_migration(
        base.to_rust_schema_info(),
        target.to_rust_schema_info(),
        app_label="migrations",
        name="0002_add_age",
        snapshot_version="v0002",
        previous_snapshot_version="v0001",
        generated_at="t",
        before_schema=None,
    )

    assert authored is not None
    source = authored.python_source

    # v0001.Person should be aliased (e.g. PersonV0001) because both v0001 and v0002 define
    # Person and v0002 is the latest version so it gets the unaliased Person symbol.
    assert "Person as PersonV0001" in source
    v0002_import = next(
        line
        for line in source.splitlines()
        if line.startswith("from migrations.snapshots.v0002 import")
    )
    assert "Person" in v0002_import
    assert "Person as" not in v0002_import
    assert "ops.RemoveOwnership(PersonV0001" in source
    assert "ops.AddOwnership(Person" in source
