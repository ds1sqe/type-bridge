"""Tests for transaction and session edge cases.

Tests rollback behavior, context manager cleanup, and error handling
in transaction scenarios.
"""

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


@pytest.mark.integration
class TestTransactionRollback:
    """Test transaction rollback behavior."""

    @pytest.fixture
    def schema_for_transactions(self, clean_db: Database):
        """Set up schema for transaction tests."""

        class Name(String):
            pass

        class Count(Integer):
            pass

        class Counter(Entity):
            flags = TypeFlags(name="counter_tx_test")
            name: Name = Flag(Key)
            count: Count

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Counter)
        schema_manager.sync_schema(force=True)

        return clean_db, Counter, Name, Count

    def test_explicit_commit_persists_data(self, schema_for_transactions):
        """Data persists when transaction is explicitly committed."""
        db, Counter, Name, Count = schema_for_transactions

        # Use transaction context
        with db.transaction("write") as tx:
            manager = Counter.manager(tx)
            counter = Counter(name=Name("test"), count=Count(1))
            manager.insert(counter)
            # Context commits on exit

        # Verify data persisted
        manager = Counter.manager(db)
        results = manager.all()
        assert len(results) == 1
        assert str(results[0].name) == "test"

    def test_exception_rolls_back_transaction(self, schema_for_transactions):
        """Exception inside transaction context rolls back all changes."""
        db, Counter, Name, Count = schema_for_transactions

        # First insert something that should be rolled back
        try:
            with db.transaction("write") as tx:
                manager = Counter.manager(tx)
                counter = Counter(name=Name("rollback_test"), count=Count(1))
                manager.insert(counter)

                # Raise exception to trigger rollback
                raise ValueError("Intentional error")
        except ValueError:
            pass  # Expected

        # Verify nothing persisted
        manager = Counter.manager(db)
        results = manager.all()
        assert len(results) == 0

    def test_multiple_operations_in_single_transaction(self, schema_for_transactions):
        """Multiple operations in one transaction commit together."""
        db, Counter, Name, Count = schema_for_transactions

        with db.transaction("write") as tx:
            manager = Counter.manager(tx)

            # Insert multiple entities
            manager.insert(Counter(name=Name("counter1"), count=Count(10)))
            manager.insert(Counter(name=Name("counter2"), count=Count(20)))
            manager.insert(Counter(name=Name("counter3"), count=Count(30)))

        # All should be committed
        manager = Counter.manager(db)
        results = manager.all()
        assert len(results) == 3

    def test_partial_operations_rollback_on_exception(self, schema_for_transactions):
        """All operations roll back even if some succeeded before exception."""
        db, Counter, Name, Count = schema_for_transactions

        try:
            with db.transaction("write") as tx:
                manager = Counter.manager(tx)

                # These inserts should be rolled back
                manager.insert(Counter(name=Name("partial1"), count=Count(1)))
                manager.insert(Counter(name=Name("partial2"), count=Count(2)))

                # Raise exception
                raise RuntimeError("Partial failure")
        except RuntimeError:
            pass

        # Nothing should have persisted
        manager = Counter.manager(db)
        results = manager.all()
        assert len(results) == 0


@pytest.mark.integration
class TestTransactionContextCleanup:
    """Test transaction context resource cleanup."""

    @pytest.fixture
    def schema_for_cleanup(self, clean_db: Database):
        """Set up schema for cleanup tests."""

        class Name(String):
            pass

        class Value(Integer):
            pass

        class Record(Entity):
            flags = TypeFlags(name="record_cleanup_test")
            name: Name = Flag(Key)
            value: Value

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Record)
        schema_manager.sync_schema(force=True)

        return clean_db, Record, Name, Value

    def test_context_manager_releases_resources(self, schema_for_cleanup):
        """Transaction context releases resources on normal exit."""
        db, Record, Name, Value = schema_for_cleanup

        # Use context manager
        with db.transaction("write") as tx:
            manager = Record.manager(tx)
            manager.insert(Record(name=Name("test"), value=Value(42)))

        # After context exits, should be able to start new transaction
        with db.transaction("read") as tx:
            manager = Record.manager(tx)
            results = manager.all()
            assert len(results) == 1

    def test_context_manager_releases_on_exception(self, schema_for_cleanup):
        """Transaction context releases resources even on exception."""
        db, Record, Name, Value = schema_for_cleanup

        # First transaction with exception
        try:
            with db.transaction("read"):
                raise ValueError("Test error")
        except ValueError:
            pass

        # Should still be able to use database
        with db.transaction("write") as tx:
            manager = Record.manager(tx)
            manager.insert(Record(name=Name("after_error"), value=Value(1)))

        # Verify
        manager = Record.manager(db)
        assert len(manager.all()) == 1


@pytest.mark.integration
class TestSequentialTransactions:
    """Test sequential transaction operations."""

    @pytest.fixture
    def schema_for_sequential(self, clean_db: Database):
        """Set up schema for sequential transaction tests."""

        class Name(String):
            pass

        class Seq(Integer):
            pass

        class Item(Entity):
            flags = TypeFlags(name="item_seq_test")
            name: Name = Flag(Key)
            seq: Seq

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Item)
        schema_manager.sync_schema(force=True)

        return clean_db, Item, Name, Seq

    def test_sequential_transactions_see_each_others_changes(self, schema_for_sequential):
        """Later transactions see changes from earlier committed transactions."""
        db, Item, Name, Seq = schema_for_sequential

        # First transaction (write)
        with db.transaction("write") as tx:
            manager = Item.manager(tx)
            manager.insert(Item(name=Name("first"), seq=Seq(1)))

        # Second transaction should see first's changes
        with db.transaction("write") as tx:
            manager = Item.manager(tx)
            results = manager.all()
            assert len(results) == 1

            # Add another
            manager.insert(Item(name=Name("second"), seq=Seq(2)))

        # Final check
        manager = Item.manager(db)
        results = manager.all()
        assert len(results) == 2

    def test_update_in_separate_transaction(self, schema_for_sequential):
        """Update in separate transaction persists correctly."""
        db, Item, Name, Seq = schema_for_sequential

        # Insert
        with db.transaction("write") as tx:
            manager = Item.manager(tx)
            manager.insert(Item(name=Name("update_target"), seq=Seq(0)))

        # Update in new transaction
        with db.transaction("write") as tx:
            manager = Item.manager(tx)
            item = manager.get(name="update_target")[0]
            item.seq = Seq(100)
            manager.update(item)

        # Verify
        manager = Item.manager(db)
        item = manager.get(name="update_target")[0]
        assert int(item.seq) == 100
