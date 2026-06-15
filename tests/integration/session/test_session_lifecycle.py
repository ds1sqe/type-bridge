"""Integration tests for session and transaction lifecycle."""

import pytest
from typedb.driver import TransactionType

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

    def test_connect_creates_rust_handle(self, clean_db):
        """connect() should create a Rust database handle."""
        # clean_db is already connected
        assert getattr(clean_db, "_rust_backend_database", None) is not None

    def test_close_destroys_rust_handle(self, test_database):
        """close() should clear the Rust database handle."""
        from tests.integration.conftest import TEST_DB_ADDRESS

        db = Database(address=TEST_DB_ADDRESS, database=test_database)
        db.connect()
        assert getattr(db, "_rust_backend_database", None) is not None
        db.close()
        assert getattr(db, "_rust_backend_database", None) is None

    def test_context_manager_connect_close(self, test_database):
        """Database as context manager should connect and close."""
        from tests.integration.conftest import TEST_DB_ADDRESS

        with Database(address=TEST_DB_ADDRESS, database=test_database) as db:
            assert getattr(db, "_rust_backend_database", None) is not None
        assert getattr(db, "_rust_backend_database", None) is None

    def test_database_exists_true(self, clean_db, test_database):
        """database_exists() should return True for existing database."""
        assert clean_db.database_exists() is True

    def test_database_exists_false(self):
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

    def test_create_database_when_not_exists(self):
        """create_database() should create a new database."""
        from tests.integration.conftest import TEST_DB_ADDRESS

        db_name = "test_create_new_db"

        db = Database(address=TEST_DB_ADDRESS, database=db_name)
        db.connect()
        try:
            if db.database_exists():
                db.delete_database()
            db.create_database()
            assert db.database_exists() is True
        finally:
            if db.database_exists():
                db.delete_database()
            db.close()

    def test_create_database_idempotent(self, clean_db, test_database):
        """create_database() should be idempotent (not error on existing)."""
        # Database already exists from clean_db fixture
        assert clean_db.database_exists() is True
        # Should not raise
        clean_db.create_database()
        assert clean_db.database_exists() is True

    def test_delete_database_when_exists(self):
        """delete_database() should delete an existing database."""
        from tests.integration.conftest import TEST_DB_ADDRESS

        db_name = "test_delete_db"

        db = Database(address=TEST_DB_ADDRESS, database=db_name)
        db.connect()
        try:
            db.create_database()
            assert db.database_exists() is True
            db.delete_database()
            assert db.database_exists() is False
        finally:
            db.close()

    def test_delete_database_idempotent(self):
        """delete_database() should be idempotent (not error on non-existing)."""
        from tests.integration.conftest import TEST_DB_ADDRESS

        db_name = "test_delete_nonexistent"

        db = Database(address=TEST_DB_ADDRESS, database=db_name)
        db.connect()
        try:
            if db.database_exists():
                db.delete_database()
            # Should not raise
            db.delete_database()
            assert db.database_exists() is False
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
