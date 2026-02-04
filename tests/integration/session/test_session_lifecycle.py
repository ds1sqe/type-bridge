"""Integration tests for session and transaction lifecycle."""

import pytest
from type_bridge import (
    Database,
    Entity,
    Flag,
    Integer,
    Key,
    SchemaManager,
    String,
    TypeFlags,
)
from typedb.driver import TransactionType


# Attribute and entity types for session lifecycle tests
class SessionName(String):
    pass


class SessionAge(Integer):
    pass


class SessionTestPerson(Entity):
    flags = TypeFlags(name="session_test_person")
    name: SessionName = Flag(Key)
    age: SessionAge | None = None


@pytest.mark.integration
@pytest.mark.order(400)
class TestDatabaseLifecycle:
    """Tests for Database connection lifecycle."""

    def test_connect_creates_driver(self, clean_db):
        """connect() should create a driver instance."""
        # clean_db is already connected
        assert clean_db._driver is not None

    def test_close_destroys_driver(self, typedb_driver, test_database):
        """close() should destroy the driver instance."""
        from tests.integration.conftest import TEST_DB_ADDRESS

        db = Database(address=TEST_DB_ADDRESS, database=test_database)
        db.connect()
        assert db._driver is not None
        db.close()
        assert db._driver is None

    def test_context_manager_connect_close(self, test_database):
        """Database as context manager should connect and close."""
        from tests.integration.conftest import TEST_DB_ADDRESS

        with Database(address=TEST_DB_ADDRESS, database=test_database) as db:
            assert db._driver is not None
        assert db._driver is None

    def test_database_exists_true(self, clean_db, test_database):
        """database_exists() should return True for existing database."""
        assert clean_db.database_exists() is True

    def test_database_exists_false(self, typedb_driver):
        """database_exists() should return False for non-existing database."""
        from tests.integration.conftest import TEST_DB_ADDRESS

        db = Database(address=TEST_DB_ADDRESS, database="nonexistent_db_xyz")
        db.connect()
        try:
            assert db.database_exists() is False
        finally:
            db.close()


@pytest.mark.integration
@pytest.mark.order(401)
class TestDatabaseOperations:
    """Tests for database creation and deletion."""

    def test_create_database_when_not_exists(self, typedb_driver):
        """create_database() should create a new database."""
        from tests.integration.conftest import TEST_DB_ADDRESS

        db_name = "test_create_new_db"

        # Ensure database doesn't exist
        if typedb_driver.databases.contains(db_name):
            typedb_driver.databases.get(db_name).delete()

        db = Database(address=TEST_DB_ADDRESS, database=db_name)
        db.connect()
        try:
            db.create_database()
            assert typedb_driver.databases.contains(db_name) is True
        finally:
            db.close()
            # Cleanup
            if typedb_driver.databases.contains(db_name):
                typedb_driver.databases.get(db_name).delete()

    def test_create_database_idempotent(self, clean_db, test_database, typedb_driver):
        """create_database() should be idempotent (not error on existing)."""
        # Database already exists from clean_db fixture
        assert typedb_driver.databases.contains(test_database) is True
        # Should not raise
        clean_db.create_database()
        assert typedb_driver.databases.contains(test_database) is True

    def test_delete_database_when_exists(self, typedb_driver):
        """delete_database() should delete an existing database."""
        from tests.integration.conftest import TEST_DB_ADDRESS

        db_name = "test_delete_db"

        # Create database first
        if not typedb_driver.databases.contains(db_name):
            typedb_driver.databases.create(db_name)

        db = Database(address=TEST_DB_ADDRESS, database=db_name)
        db.connect()
        try:
            db.delete_database()
            assert typedb_driver.databases.contains(db_name) is False
        finally:
            db.close()

    def test_delete_database_idempotent(self, typedb_driver):
        """delete_database() should be idempotent (not error on non-existing)."""
        from tests.integration.conftest import TEST_DB_ADDRESS

        db_name = "test_delete_nonexistent"

        # Ensure database doesn't exist
        if typedb_driver.databases.contains(db_name):
            typedb_driver.databases.get(db_name).delete()

        db = Database(address=TEST_DB_ADDRESS, database=db_name)
        db.connect()
        try:
            # Should not raise
            db.delete_database()
            assert typedb_driver.databases.contains(db_name) is False
        finally:
            db.close()


@pytest.mark.integration
@pytest.mark.order(402)
class TestTransactionTypes:
    """Tests for different transaction types."""

    def test_read_transaction(self, clean_db):
        """Read transaction should allow queries."""
        # Setup schema first
        schema_manager = SchemaManager(clean_db)
        schema_manager.register(SessionTestPerson)
        schema_manager.sync_schema(force=True)

        with clean_db.transaction(TransactionType.READ) as tx:
            # Should be able to read (even if empty)
            results = tx.execute("match $p isa session_test_person; fetch { $p.* };")
            assert results == []

    def test_write_transaction(self, clean_db):
        """Write transaction should allow inserts."""
        # Setup schema first
        schema_manager = SchemaManager(clean_db)
        schema_manager.register(SessionTestPerson)
        schema_manager.sync_schema(force=True)

        with clean_db.transaction(TransactionType.WRITE) as tx:
            tx.execute('insert $p isa session_test_person, has SessionName "Alice";')
            # Commit happens automatically on exit

        # Verify insert persisted
        with clean_db.transaction(TransactionType.READ) as tx:
            results = tx.execute("match $p isa session_test_person; fetch { $p.* };")
            assert len(results) == 1

    def test_schema_transaction(self, clean_db):
        """Schema transaction should allow schema changes."""
        with clean_db.transaction(TransactionType.SCHEMA) as tx:
            tx.execute("define attribute lifecycle_test_attr, value string;")
            # Commit happens automatically on exit

        # Verify schema change persisted
        schema = clean_db.get_schema()
        assert "lifecycle_test_attr" in schema


@pytest.mark.integration
@pytest.mark.order(403)
class TestTransactionOperations:
    """Tests for transaction operations."""

    def test_execute_returns_results(self, clean_db):
        """execute() should return query results."""
        # Setup schema and data
        schema_manager = SchemaManager(clean_db)
        schema_manager.register(SessionTestPerson)
        schema_manager.sync_schema(force=True)

        # Insert test data
        with clean_db.transaction(TransactionType.WRITE) as tx:
            tx.execute('insert $p isa session_test_person, has SessionName "Bob";')

        # Execute read query
        with clean_db.transaction(TransactionType.READ) as tx:
            results = tx.execute("match $p isa session_test_person; fetch { $p.* };")
            assert len(results) == 1
            # Result should contain the person data
            assert any("Bob" in str(r) for r in results)

    def test_is_open_property(self, clean_db):
        """is_open should reflect transaction state."""
        # Setup schema
        schema_manager = SchemaManager(clean_db)
        schema_manager.register(SessionTestPerson)
        schema_manager.sync_schema(force=True)

        with clean_db.transaction(TransactionType.READ) as tx:
            assert tx.transaction.is_open is True

    def test_explicit_commit(self, clean_db):
        """Explicit commit should persist changes."""
        schema_manager = SchemaManager(clean_db)
        schema_manager.register(SessionTestPerson)
        schema_manager.sync_schema(force=True)

        with clean_db.transaction(TransactionType.WRITE) as tx:
            tx.execute('insert $p isa session_test_person, has SessionName "Charlie";')
            tx.commit()

        # Verify persisted
        with clean_db.transaction(TransactionType.READ) as tx:
            results = tx.execute("match $p isa session_test_person; fetch { $p.* };")
            assert len(results) == 1


@pytest.mark.integration
@pytest.mark.order(404)
class TestExecuteQuery:
    """Tests for Database.execute_query() convenience method."""

    def test_execute_query_read(self, clean_db):
        """execute_query with read type should return results."""
        schema_manager = SchemaManager(clean_db)
        schema_manager.register(SessionTestPerson)
        schema_manager.sync_schema(force=True)

        results = clean_db.execute_query(
            "match $p isa session_test_person; fetch { $p.* };", transaction_type="read"
        )
        assert results == []

    def test_execute_query_write_commits(self, clean_db):
        """execute_query with write type should commit changes."""
        schema_manager = SchemaManager(clean_db)
        schema_manager.register(SessionTestPerson)
        schema_manager.sync_schema(force=True)

        clean_db.execute_query(
            'insert $p isa session_test_person, has SessionName "Dave";', transaction_type="write"
        )

        # Verify committed
        results = clean_db.execute_query(
            "match $p isa session_test_person; fetch { $p.* };", transaction_type="read"
        )
        assert len(results) == 1

    def test_execute_query_schema_commits(self, clean_db):
        """execute_query with schema type should commit changes."""
        clean_db.execute_query(
            "define attribute execute_query_test_attr, value string;", transaction_type="schema"
        )

        # Verify committed
        schema = clean_db.get_schema()
        assert "execute_query_test_attr" in schema


@pytest.mark.integration
@pytest.mark.order(405)
class TestGetSchema:
    """Tests for Database.get_schema() method."""

    def test_get_schema_returns_string(self, clean_db):
        """get_schema() should return schema as string."""
        schema = clean_db.get_schema()
        assert isinstance(schema, str)

    def test_get_schema_includes_defined_types(self, clean_db):
        """get_schema() should include defined types."""
        schema_manager = SchemaManager(clean_db)
        schema_manager.register(SessionTestPerson)
        schema_manager.sync_schema(force=True)

        schema = clean_db.get_schema()
        assert "session_test_person" in schema
        assert "SessionName" in schema
