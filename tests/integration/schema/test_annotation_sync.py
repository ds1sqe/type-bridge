"""Integration tests for syncing schemas with annotations to TypeDB."""

from typing import ClassVar

import pytest

from type_bridge import (
    Database,
    Double,
    Entity,
    Flag,
    Integer,
    Key,
    SchemaManager,
    String,
    TypeFlags,
)


# Define attributes with annotations
class Name(String):
    pass


class Age(Integer):
    """Age with range constraint."""

    range_constraint: ClassVar[tuple[str | None, str | None]] = ("0", "150")


class Temperature(Double):
    """Temperature with range constraint."""

    range_constraint: ClassVar[tuple[str | None, str | None]] = ("-50.0", "50.0")


class Status(String):
    """Status with allowed values."""

    allowed_values: ClassVar[tuple[str, ...]] = ("active", "inactive", "pending")


class Email(String):
    """Email with regex constraint."""

    regex: ClassVar[str] = r"^[a-z]+@[a-z]+\.[a-z]+$"  # type: ignore[assignment]


# Define entities using the annotated attributes
class Person(Entity):
    flags = TypeFlags(name="annotation_test_person")
    name: Name = Flag(Key)
    age: Age | None = None


class Sensor(Entity):
    flags = TypeFlags(name="annotation_test_sensor")
    name: Name = Flag(Key)
    temperature: Temperature | None = None


class Task(Entity):
    flags = TypeFlags(name="annotation_test_task")
    name: Name = Flag(Key)
    status: Status | None = None


class User(Entity):
    flags = TypeFlags(name="annotation_test_user")
    name: Name = Flag(Key)
    email: Email | None = None


@pytest.fixture
def db():
    """Create a test database for annotation sync tests."""
    from tests.integration.conftest import TEST_DB_ADDRESS

    database = Database(address=TEST_DB_ADDRESS, database="test_annotation_sync")
    database.connect()

    # Clean up if exists
    if database.database_exists():
        database.delete_database()
    database.create_database()

    yield database

    # Cleanup
    database.delete_database()
    database.close()


@pytest.mark.integration
class TestSchemaAnnotationSync:
    """Test that schemas with annotations sync to TypeDB correctly."""

    def test_range_annotation_syncs_to_typedb(self, db: Database) -> None:
        """Verify @range is included when syncing schema to TypeDB."""
        schema_manager = SchemaManager(db)
        schema_manager.register(Person)

        # Generate schema and verify @range is included
        schema = schema_manager.generate_schema()
        assert "@range(0..150)" in schema

        # Sync to TypeDB - should not raise
        schema_manager.sync_schema()

    def test_typedb_enforces_range_constraint(self, db: Database) -> None:
        """Verify TypeDB enforces the @range constraint at database level."""
        schema_manager = SchemaManager(db)
        schema_manager.register(Person)
        schema_manager.sync_schema()

        manager = Person.manager(db)

        # Valid age should work
        person = Person(name=Name("ValidPerson"), age=Age(30))
        manager.insert(person)

        results = manager.get(name="ValidPerson")
        assert len(results) == 1
        assert results[0].age is not None
        assert results[0].age.value == 30

    def test_double_range_annotation_syncs(self, db: Database) -> None:
        """Verify @range for double type syncs correctly."""
        schema_manager = SchemaManager(db)
        schema_manager.register(Sensor)

        schema = schema_manager.generate_schema()
        assert "@range(-50.0..50.0)" in schema

        schema_manager.sync_schema()

        manager = Sensor.manager(db)
        sensor = Sensor(name=Name("TempSensor"), temperature=Temperature(25.5))
        manager.insert(sensor)

        results = manager.get(name="TempSensor")
        assert len(results) == 1
        assert results[0].temperature is not None
        assert results[0].temperature.value == 25.5

    def test_values_annotation_syncs_to_typedb(self, db: Database) -> None:
        """Verify @values annotation syncs correctly."""
        schema_manager = SchemaManager(db)
        schema_manager.register(Task)

        schema = schema_manager.generate_schema()
        assert '@values("active", "inactive", "pending")' in schema

        schema_manager.sync_schema()

        manager = Task.manager(db)
        task = Task(name=Name("MyTask"), status=Status("active"))
        manager.insert(task)

        results = manager.get(name="MyTask")
        assert len(results) == 1
        assert results[0].status is not None
        assert results[0].status.value == "active"

    def test_regex_annotation_syncs_to_typedb(self, db: Database) -> None:
        """Verify @regex annotation syncs correctly."""
        schema_manager = SchemaManager(db)
        schema_manager.register(User)

        schema = schema_manager.generate_schema()
        assert "@regex" in schema

        schema_manager.sync_schema()

        manager = User.manager(db)
        user = User(name=Name("TestUser"), email=Email("test@example.com"))
        manager.insert(user)

        results = manager.get(name="TestUser")
        assert len(results) == 1
        assert results[0].email is not None
        assert results[0].email.value == "test@example.com"


@pytest.mark.integration
class TestTypeDBEnforcesConstraints:
    """Test that TypeDB enforces constraints at the database level."""

    def test_typedb_rejects_out_of_range_via_raw_query(self, db: Database) -> None:
        """Verify TypeDB rejects out-of-range values via raw TypeQL."""
        from typedb.driver import TransactionType

        schema_manager = SchemaManager(db)
        schema_manager.register(Person)
        schema_manager.sync_schema()

        # Try to insert out-of-range value directly via TypeQL
        # This bypasses Python validation to test TypeDB enforcement
        insert_query = """
        insert $p isa annotation_test_person,
            has Name "DirectInsert",
            has Age 200;
        """

        with pytest.raises(Exception) as exc_info:
            with db.driver.transaction(  # type: ignore[attr-defined]
                db.database_name, TransactionType.WRITE
            ) as tx:
                tx.query(insert_query).resolve()
                tx.commit()

        # TypeDB should reject the value
        assert "200" in str(exc_info.value) or "range" in str(exc_info.value).lower()

    def test_typedb_rejects_invalid_values_via_raw_query(self, db: Database) -> None:
        """Verify TypeDB rejects invalid @values via raw TypeQL."""
        from typedb.driver import TransactionType

        schema_manager = SchemaManager(db)
        schema_manager.register(Task)
        schema_manager.sync_schema()

        # Try to insert invalid status directly via TypeQL
        insert_query = """
        insert $t isa annotation_test_task,
            has Name "DirectTask",
            has Status "invalid_status";
        """

        with pytest.raises(Exception) as exc_info:
            with db.driver.transaction(  # type: ignore[attr-defined]
                db.database_name, TransactionType.WRITE
            ) as tx:
                tx.query(insert_query).resolve()
                tx.commit()

        # TypeDB should reject the value
        assert "invalid_status" in str(exc_info.value) or "values" in str(exc_info.value).lower()

    def test_typedb_rejects_invalid_regex_via_raw_query(self, db: Database) -> None:
        """Verify TypeDB rejects values not matching @regex via raw TypeQL."""
        from typedb.driver import TransactionType

        schema_manager = SchemaManager(db)
        schema_manager.register(User)
        schema_manager.sync_schema()

        # Try to insert an invalid email directly via TypeQL
        # "not-an-email" doesn't match the regex pattern
        insert_query = """
        insert $u isa annotation_test_user,
            has Name "DirectUser",
            has Email "not-an-email";
        """

        with pytest.raises(Exception) as exc_info:
            with db.driver.transaction(  # type: ignore[attr-defined]
                db.database_name, TransactionType.WRITE
            ) as tx:
                tx.query(insert_query).resolve()
                tx.commit()

        # TypeDB should reject the value due to regex mismatch
        assert "not-an-email" in str(exc_info.value) or "regex" in str(exc_info.value).lower()


# Define independent attribute for testing
class Language(String):
    """Language attribute that can exist without owners."""

    independent = True


class Document(Entity):
    """Document entity that owns Language."""

    flags = TypeFlags(name="annotation_test_document")
    name: Name = Flag(Key)
    language: Language | None = None


@pytest.mark.integration
class TestIndependentAnnotationSync:
    """Test @independent annotation syncs and works correctly."""

    def test_independent_annotation_syncs_to_typedb(self, db: Database) -> None:
        """Verify @independent is included when syncing schema to TypeDB."""
        schema_manager = SchemaManager(db)
        schema_manager.register(Document)

        # Generate schema and verify @independent is included
        schema = schema_manager.generate_schema()
        assert "@independent" in schema
        assert "attribute Language @independent, value string;" in schema

        # Sync to TypeDB - should not raise
        schema_manager.sync_schema()

    def test_independent_attribute_can_be_inserted_standalone(self, db: Database) -> None:
        """Verify independent attributes can be inserted without an owner."""
        from typedb.driver import TransactionType

        schema_manager = SchemaManager(db)
        schema_manager.register(Document)
        schema_manager.sync_schema()

        # Insert an independent attribute directly without an owner
        insert_query = """
        insert $lang isa Language "English";
        """

        with db.driver.transaction(  # type: ignore[attr-defined]
            db.database_name, TransactionType.WRITE
        ) as tx:
            tx.query(insert_query).resolve()
            tx.commit()

        # Verify the attribute exists
        with db.driver.transaction(  # type: ignore[attr-defined]
            db.database_name, TransactionType.READ
        ) as tx:
            result = tx.query('match $lang isa Language "English"; select $lang;').resolve()
            answers = list(result.as_concept_rows())
            assert len(answers) == 1

    def test_independent_attribute_with_entity(self, db: Database) -> None:
        """Verify independent attributes work normally when owned by an entity."""
        schema_manager = SchemaManager(db)
        schema_manager.register(Document)
        schema_manager.sync_schema()

        manager = Document.manager(db)
        doc = Document(name=Name("README"), language=Language("English"))
        manager.insert(doc)

        results = manager.get(name="README")
        assert len(results) == 1
        assert results[0].language is not None
        assert results[0].language.value == "English"

    def test_is_independent_method(self, db: Database) -> None:
        """Verify the is_independent() method returns correct values."""
        assert Language.is_independent() is True
        assert Name.is_independent() is False
