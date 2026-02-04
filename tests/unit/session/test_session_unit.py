"""Unit tests for session module classes (without TypeDB connection)."""

from unittest.mock import MagicMock

import pytest
from type_bridge.session import (
    Connection,
    ConnectionExecutor,
    Database,
    Transaction,
    TransactionContext,
)
from typedb.driver import TransactionType


class TestDatabaseConfiguration:
    """Tests for Database class configuration."""

    def test_default_address_and_database(self):
        """Database should have default address and database name."""
        db = Database()
        assert db.address == "localhost:1729"
        assert db.database_name == "typedb"

    def test_custom_address(self):
        """Database should accept custom address."""
        db = Database(address="192.168.1.1:1729")
        assert db.address == "192.168.1.1:1729"

    def test_custom_database_name(self):
        """Database should accept custom database name."""
        db = Database(database="my_database")
        assert db.database_name == "my_database"

    def test_custom_credentials(self):
        """Database should store username and password."""
        db = Database(username="admin", password="secret")
        assert db.username == "admin"
        assert db.password == "secret"

    def test_driver_none_before_connect(self):
        """Driver should be None before connection."""
        db = Database()
        assert db._driver is None

    def test_close_without_connect(self):
        """Close should be safe to call without prior connection."""
        db = Database()
        db.close()  # Should not raise
        assert db._driver is None


class TestTransactionContext:
    """Tests for TransactionContext class."""

    def test_context_init_stores_db_and_type(self):
        """TransactionContext should store database and transaction type."""
        db = Database()
        ctx = TransactionContext(db, TransactionType.READ)
        assert ctx.db is db
        assert ctx.tx_type == TransactionType.READ

    def test_context_init_with_write_type(self):
        """TransactionContext should accept WRITE type."""
        db = Database()
        ctx = TransactionContext(db, TransactionType.WRITE)
        assert ctx.tx_type == TransactionType.WRITE

    def test_context_init_with_schema_type(self):
        """TransactionContext should accept SCHEMA type."""
        db = Database()
        ctx = TransactionContext(db, TransactionType.SCHEMA)
        assert ctx.tx_type == TransactionType.SCHEMA

    def test_transaction_property_raises_before_enter(self):
        """Accessing transaction before entering context should raise."""
        db = Database()
        ctx = TransactionContext(db, TransactionType.READ)
        with pytest.raises(RuntimeError, match="TransactionContext not entered"):
            _ = ctx.transaction

    def test_database_property_returns_db(self):
        """Database property should return the Database."""
        db = Database()
        ctx = TransactionContext(db, TransactionType.READ)
        assert ctx.database is db


class TestConnectionExecutor:
    """Tests for ConnectionExecutor class."""

    def test_executor_with_database_stores_database(self):
        """Executor created with Database should store it."""
        db = Database()
        executor = ConnectionExecutor(db)
        assert executor._database is db
        assert executor._transaction is None

    def test_executor_with_transaction_stores_transaction(self):
        """Executor created with Transaction should store it."""
        # Create a mock TypeDB transaction
        mock_tx = MagicMock()
        tx = Transaction(mock_tx)
        executor = ConnectionExecutor(tx)
        assert executor._transaction is tx
        assert executor._database is None

    def test_executor_with_context_extracts_transaction(self):
        """Executor created with TransactionContext should extract transaction."""
        # Create mock context with transaction
        db = Database()
        ctx = TransactionContext(db, TransactionType.READ)
        # Manually set a mock transaction
        mock_tx = MagicMock()
        ctx._tx = Transaction(mock_tx)

        executor = ConnectionExecutor(ctx)
        assert executor._transaction is ctx._tx
        assert executor._database is None

    def test_has_transaction_property_true(self):
        """has_transaction should be True when using transaction."""
        mock_tx = MagicMock()
        tx = Transaction(mock_tx)
        executor = ConnectionExecutor(tx)
        assert executor.has_transaction is True

    def test_has_transaction_property_false(self):
        """has_transaction should be False when using database."""
        db = Database()
        executor = ConnectionExecutor(db)
        assert executor.has_transaction is False

    def test_database_property_when_using_database(self):
        """Database property should return Database when initialized with it."""
        db = Database()
        executor = ConnectionExecutor(db)
        assert executor.database is db

    def test_database_property_when_using_transaction(self):
        """Database property should return None when initialized with transaction."""
        mock_tx = MagicMock()
        tx = Transaction(mock_tx)
        executor = ConnectionExecutor(tx)
        assert executor.database is None

    def test_transaction_property_when_using_transaction(self):
        """Transaction property should return Transaction when initialized with it."""
        mock_tx = MagicMock()
        tx = Transaction(mock_tx)
        executor = ConnectionExecutor(tx)
        assert executor.transaction is tx

    def test_transaction_property_when_using_database(self):
        """Transaction property should return None when initialized with database."""
        db = Database()
        executor = ConnectionExecutor(db)
        assert executor.transaction is None


class TestTransaction:
    """Tests for Transaction wrapper class."""

    def test_transaction_init_stores_raw_tx(self):
        """Transaction should store the raw TypeDB transaction."""
        mock_tx = MagicMock()
        tx = Transaction(mock_tx)
        assert tx._tx is mock_tx

    def test_commit_calls_underlying_commit(self):
        """commit() should call underlying transaction commit."""
        mock_tx = MagicMock()
        tx = Transaction(mock_tx)
        tx.commit()
        mock_tx.commit.assert_called_once()

    def test_rollback_calls_underlying_rollback(self):
        """rollback() should call underlying transaction rollback."""
        mock_tx = MagicMock()
        tx = Transaction(mock_tx)
        tx.rollback()
        mock_tx.rollback.assert_called_once()

    def test_is_open_delegates_to_underlying(self):
        """is_open should delegate to underlying transaction."""
        mock_tx = MagicMock()
        mock_tx.is_open.return_value = True
        tx = Transaction(mock_tx)
        assert tx.is_open is True
        mock_tx.is_open.assert_called_once()

    def test_close_when_open(self):
        """close() should close an open transaction."""
        mock_tx = MagicMock()
        mock_tx.is_open.return_value = True
        tx = Transaction(mock_tx)
        tx.close()
        mock_tx.close.assert_called_once()

    def test_close_when_already_closed(self):
        """close() should not call close on already closed transaction."""
        mock_tx = MagicMock()
        mock_tx.is_open.return_value = False
        tx = Transaction(mock_tx)
        tx.close()
        mock_tx.close.assert_not_called()


class TestDatabaseTransactionCreation:
    """Tests for Database.transaction() method."""

    def test_transaction_with_read_string(self):
        """transaction('read') should create READ transaction context."""
        db = Database()
        ctx = db.transaction("read")
        assert isinstance(ctx, TransactionContext)
        assert ctx.tx_type == TransactionType.READ

    def test_transaction_with_write_string(self):
        """transaction('write') should create WRITE transaction context."""
        db = Database()
        ctx = db.transaction("write")
        assert isinstance(ctx, TransactionContext)
        assert ctx.tx_type == TransactionType.WRITE

    def test_transaction_with_schema_string(self):
        """transaction('schema') should create SCHEMA transaction context."""
        db = Database()
        ctx = db.transaction("schema")
        assert isinstance(ctx, TransactionContext)
        assert ctx.tx_type == TransactionType.SCHEMA

    def test_transaction_default_is_read(self):
        """transaction() with no args should default to READ."""
        db = Database()
        ctx = db.transaction()
        assert ctx.tx_type == TransactionType.READ

    def test_transaction_with_transaction_type_enum(self):
        """transaction() should accept TransactionType enum directly."""
        db = Database()
        ctx = db.transaction(TransactionType.WRITE)
        assert ctx.tx_type == TransactionType.WRITE

    def test_transaction_invalid_string_defaults_to_read(self):
        """transaction() with invalid string should default to READ."""
        db = Database()
        ctx = db.transaction("invalid")
        assert ctx.tx_type == TransactionType.READ


class TestConnectionTypeAlias:
    """Tests for Connection type alias."""

    def test_connection_accepts_database(self):
        """Connection type should accept Database."""
        db = Database()
        # Type checking - Connection should accept Database
        conn: Connection = db
        assert isinstance(conn, Database)

    def test_connection_accepts_transaction(self):
        """Connection type should accept Transaction."""
        mock_tx = MagicMock()
        tx = Transaction(mock_tx)
        # Type checking - Connection should accept Transaction
        conn: Connection = tx
        assert isinstance(conn, Transaction)

    def test_connection_accepts_transaction_context(self):
        """Connection type should accept TransactionContext."""
        db = Database()
        ctx = TransactionContext(db, TransactionType.READ)
        # Type checking - Connection should accept TransactionContext
        conn: Connection = ctx
        assert isinstance(conn, TransactionContext)


class TestDriverInjection:
    """Tests for external driver injection feature (issue #85)."""

    def test_driver_none_by_default(self):
        """Database should have no driver by default."""
        db = Database()
        assert db._driver is None
        assert db._owns_driver is True  # Will own any driver it creates

    def test_injected_driver_stored(self):
        """Database should store injected driver."""
        mock_driver = MagicMock()
        db = Database(driver=mock_driver)
        assert db._driver is mock_driver
        assert db._owns_driver is False  # Does not own injected driver

    def test_connect_skips_when_driver_injected(self):
        """connect() should be a no-op when driver is injected."""
        mock_driver = MagicMock()
        db = Database(driver=mock_driver)

        # connect() should not modify the driver
        db.connect()
        assert db._driver is mock_driver
        assert db._owns_driver is False

    def test_close_clears_reference_but_does_not_close_injected_driver(self):
        """close() should clear reference but not close injected driver."""
        mock_driver = MagicMock()
        db = Database(driver=mock_driver)

        db.close()

        # Reference should be cleared
        assert db._driver is None
        # But close() should NOT have been called on the driver
        mock_driver.close.assert_not_called()

    def test_close_closes_owned_driver(self):
        """close() should close driver when Database owns it."""
        mock_driver = MagicMock()
        db = Database()
        # Simulate connect() creating a driver
        db._driver = mock_driver
        db._owns_driver = True

        db.close()

        # Driver should be closed
        mock_driver.close.assert_called_once()
        assert db._driver is None

    def test_driver_property_returns_injected_driver(self):
        """Driver property should return injected driver without connecting."""
        mock_driver = MagicMock()
        db = Database(driver=mock_driver)

        # Accessing driver property should return the injected driver
        assert db.driver is mock_driver
        # connect() should not have been called (no new driver created)
        assert db._owns_driver is False

    def test_context_manager_with_injected_driver(self):
        """Context manager should work with injected driver."""
        mock_driver = MagicMock()

        with Database(driver=mock_driver) as db:
            assert db._driver is mock_driver

        # After exit, reference cleared but driver not closed
        assert db._driver is None
        mock_driver.close.assert_not_called()

    def test_multiple_databases_share_driver(self):
        """Multiple Database instances can share the same driver."""
        mock_driver = MagicMock()

        db1 = Database(database="db1", driver=mock_driver)
        db2 = Database(database="db2", driver=mock_driver)

        assert db1._driver is mock_driver
        assert db2._driver is mock_driver
        assert db1._owns_driver is False
        assert db2._owns_driver is False

        # Close both - driver should NOT be closed
        db1.close()
        db2.close()
        mock_driver.close.assert_not_called()

    def test_database_exists_with_injected_driver(self):
        """database_exists() should work with injected driver."""
        mock_driver = MagicMock()
        mock_driver.databases.contains.return_value = True

        db = Database(database="test_db", driver=mock_driver)

        assert db.database_exists() is True
        mock_driver.databases.contains.assert_called_with("test_db")
