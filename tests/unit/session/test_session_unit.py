"""Unit tests for session module classes (without TypeDB connection)."""

from unittest.mock import MagicMock

import pytest

from type_bridge._native import database_from_core
from type_bridge.session import (
    Connection,
    Database,
    TransactionContext,
)
from type_bridge.typedb_driver import TransactionType


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

    def test_package_owned_rust_database_is_registered_and_closes_once(self):
        """The generated facade owns one native handle and close is idempotent."""
        native = MagicMock()
        database = database_from_core("localhost:1729", "generated", native)

        assert database.address == "localhost:1729"
        assert database.database_name == "generated"
        assert getattr(database, "_native_database") is native
        assert database._transport_committed is True

        database.close()
        database.close()

        native.close.assert_called_once_with()
        assert not hasattr(database, "_native_database")
        assert database._transport_committed is False

    def test_package_owned_rust_database_exposes_pair_administration(self):
        """The generated facade preserves native pair-safe administration."""
        native = MagicMock()
        native.inspect_database_pair.return_value = "owned_pair"
        native.inspect_database_pair_controlled.return_value = "standalone_managed"
        deletion = object()
        controlled_deletion = object()
        native.plan_database_delete.return_value = deletion
        native.plan_database_delete_controlled.return_value = controlled_deletion
        cancellation = object()
        database = database_from_core("localhost:1729", "generated", native)

        assert database.inspect_database_pair() == "owned_pair"
        assert (
            database.inspect_database_pair_controlled(
                timeout_milliseconds=25,
                cancellation=cancellation,
            )
            == "standalone_managed"
        )
        assert database.plan_database_delete() is deletion
        assert (
            database.plan_database_delete_controlled(
                timeout_milliseconds=50,
                cancellation=cancellation,
            )
            is controlled_deletion
        )

        native.inspect_database_pair.assert_called_once_with()
        native.inspect_database_pair_controlled.assert_called_once_with(
            timeout_milliseconds=25,
            cancellation=cancellation,
        )
        native.plan_database_delete.assert_called_once_with()
        native.plan_database_delete_controlled.assert_called_once_with(
            timeout_milliseconds=50,
            cancellation=cancellation,
        )

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

    def test_close_preserves_released_falsey_injected_driver_behavior(self):
        """The released truthiness seam leaves a falsey injected driver attached."""

        class FalseyDriver:
            def __init__(self) -> None:
                self.close_calls = 0

            def __bool__(self) -> bool:
                return False

            def close(self) -> None:
                self.close_calls += 1

        driver = FalseyDriver()
        db = Database(driver=driver)  # type: ignore[arg-type]
        setattr(db, "_native_database", object())

        db.close()

        assert db._driver is driver
        assert driver.close_calls == 0
        assert not hasattr(db, "_native_database")

    def test_close_preserves_released_truth_probe_error_order(self):
        """A driver truth-probe error precedes every close-time mutation."""

        class ExplodingDriver:
            def __bool__(self) -> bool:
                raise ValueError("driver truth probe failed")

        driver = ExplodingDriver()
        rust_database = object()
        db = Database(driver=driver)  # type: ignore[arg-type]
        setattr(db, "_native_database", rust_database)

        with pytest.raises(ValueError, match="driver truth probe failed"):
            db.close()

        assert db._driver is driver
        assert getattr(db, "_native_database") is rust_database

    def test_close_closes_cached_rust_database_before_releasing_it(self):
        """close() should explicitly stop a cached Rust provider connection."""
        db = Database()
        rust_database = MagicMock()
        setattr(db, "_native_database", rust_database)
        retained_reference = rust_database

        db.close()

        retained_reference.close.assert_called_once_with()
        assert not hasattr(db, "_native_database")

    def test_close_releases_legacy_rust_database_double_without_close(self):
        """Older Rust-handle stand-ins remain compatible with close()."""
        db = Database()
        setattr(db, "_native_database", object())

        db.close()

        assert not hasattr(db, "_native_database")

    def test_close_failure_retains_python_transport_for_released_style_retry(self):
        """A transient Python-driver close failure remains retryable."""
        db = Database()
        events: list[str] = []
        python_error = RuntimeError("python driver close failed")
        rust_error = ValueError("rust backend close failed")
        python_driver = MagicMock()
        rust_database = MagicMock()
        snapshot = MagicMock()

        python_attempts = 0

        def close_python_driver() -> None:
            nonlocal python_attempts
            python_attempts += 1
            events.append("python")
            if python_attempts == 1:
                raise python_error

        def close_rust_database() -> None:
            events.append("rust")
            raise rust_error

        python_driver.close.side_effect = close_python_driver
        rust_database.close.side_effect = close_rust_database
        snapshot.cleanup.side_effect = lambda: events.append("snapshot")
        db._driver = python_driver
        db._owns_driver = True
        setattr(db, "_native_database", rust_database)
        db._tls_root_ca_snapshot = snapshot
        db._tls_root_ca_snapshot_path = "/retained/root.pem"
        prepared = MagicMock()
        db._prepared_connection = prepared
        db._transport_committed = True

        with pytest.raises(RuntimeError, match="python driver close failed") as raised:
            db.close()

        assert raised.value is python_error
        assert events == ["python"]
        assert db._driver is python_driver
        assert getattr(db, "_native_database") is rust_database
        assert db._tls_root_ca_snapshot is snapshot
        assert db._tls_root_ca_snapshot_path == "/retained/root.pem"
        assert db._prepared_connection is prepared
        assert db._transport_committed is True

        db.close()
        assert events == ["python", "python", "rust", "snapshot"]
        assert db._driver is None
        assert not hasattr(db, "_native_database")
        assert db._tls_root_ca_snapshot is None
        assert db._tls_root_ca_snapshot_path is None
        assert db._prepared_connection is None
        assert db._transport_committed is False

    def test_close_masks_rust_failure_after_successful_python_cleanup(self, caplog):
        """Native cleanup remains explicit without changing released close errors."""
        db = Database()
        events: list[str] = []
        rust_error = RuntimeError("rust backend close failed")
        python_driver = MagicMock()
        rust_database = MagicMock()
        snapshot = MagicMock()
        python_driver.close.side_effect = lambda: events.append("python")

        def close_rust_database() -> None:
            events.append("rust")
            raise rust_error

        rust_database.close.side_effect = close_rust_database
        snapshot.cleanup.side_effect = lambda: events.append("snapshot")
        db._driver = python_driver
        db._owns_driver = True
        setattr(db, "_native_database", rust_database)
        db._tls_root_ca_snapshot = snapshot

        with caplog.at_level("WARNING", logger="type_bridge.session"):
            db.close()

        assert events == ["python", "rust", "snapshot"]
        assert db._driver is None
        assert not hasattr(db, "_native_database")
        assert db._tls_root_ca_snapshot is None
        assert "Native database cleanup failed" in caplog.text
        assert "rust backend close failed" not in caplog.text

        db.close()
        assert events == ["python", "rust", "snapshot"]

    def test_close_failure_does_not_overwrite_reentrant_driver_replacement(self):
        """Retry restoration must not clobber state installed during close()."""
        db = Database()
        old_driver = MagicMock()
        replacement = MagicMock()
        old_snapshot = MagicMock()
        replacement_snapshot = MagicMock()
        close_error = RuntimeError("old driver close failed")

        def replace_then_fail() -> None:
            db._driver = replacement
            db._owns_driver = False
            db._tls_root_ca_snapshot = replacement_snapshot
            db._tls_root_ca_snapshot_path = "/replacement/root.pem"
            db._prepared_connection = MagicMock(name="replacement_prepared")
            db._transport_committed = True
            raise close_error

        old_driver.close.side_effect = replace_then_fail
        db._driver = old_driver
        db._owns_driver = True
        db._tls_root_ca_snapshot = old_snapshot
        db._tls_root_ca_snapshot_path = "/old/root.pem"
        db._prepared_connection = MagicMock(name="old_prepared")
        db._transport_committed = True

        with pytest.raises(RuntimeError, match="old driver close failed") as raised:
            db.close()

        assert raised.value is close_error
        assert db._driver is replacement
        assert db._owns_driver is False
        assert db._tls_root_ca_snapshot is replacement_snapshot
        assert db._tls_root_ca_snapshot_path == "/replacement/root.pem"
        old_snapshot.cleanup.assert_called_once_with()
        replacement_snapshot.cleanup.assert_not_called()

    def test_context_manager_close_failure_keeps_released_exception_precedence(self):
        """An owned-driver close error still supersedes a body error on exit."""
        close_error = RuntimeError("python driver close failed")
        body_error = ValueError("context body failed")
        driver = MagicMock()
        driver.close.side_effect = close_error
        db = Database(driver=driver)
        db._owns_driver = True

        with pytest.raises(RuntimeError, match="python driver close failed") as raised:
            with db:
                raise body_error

        assert raised.value is close_error
        assert db._driver is driver
        driver.close.side_effect = None
        db.close()

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

    def test_database_administration_outcomes_are_normalized(self):
        """Bound create/delete report exact idempotent outcomes."""
        mock_driver = MagicMock()
        mock_driver.databases.contains.side_effect = [False, True, True, False]
        db = Database(database="test_db", driver=mock_driver)

        assert db.create_database_outcome() == "created"
        assert db.create_database_outcome() == "already_exists"
        assert db.delete_database_outcome() == "deleted"
        assert db.delete_database_outcome() == "already_absent"
        mock_driver.databases.create.assert_called_once_with("test_db")
        mock_driver.databases.get.assert_called_once_with("test_db")

    def test_database_create_race_rechecks_actual_state(self):
        """A losing concurrent create is normalized only after an exact recheck."""
        mock_driver = MagicMock()
        mock_driver.databases.contains.side_effect = [False, True]
        mock_driver.databases.create.side_effect = RuntimeError("already exists")
        db = Database(database="test_db", driver=mock_driver)

        assert db.create_database_outcome() == "already_exists"

    def test_database_delete_race_rechecks_actual_state(self):
        """A concurrent delete is normalized only after observing absence."""
        mock_driver = MagicMock()
        mock_driver.databases.contains.side_effect = [True, False]
        mock_driver.databases.get.return_value.delete.side_effect = RuntimeError("absent")
        db = Database(database="test_db", driver=mock_driver)

        assert db.delete_database_outcome() == "deleted"


def test_rust_context_rollback_remains_open_for_work_and_commit() -> None:
    context = TransactionContext(Database(), TransactionType.WRITE)
    native = MagicMock()
    context._native_tx = native
    context.rollback()
    assert context.transaction.is_open
    context.execute("insert $x isa marker;")
    context.commit()
    native.rollback.assert_called_once()
    native.execute.assert_called_once_with("insert $x isa marker;")
    native.commit.assert_called_once()
    assert not context.transaction.is_open
