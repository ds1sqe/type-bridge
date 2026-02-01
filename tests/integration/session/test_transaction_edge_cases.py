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

        class TxName(String):
            pass

        class TxCount(Integer):
            pass

        class TxCounter(Entity):
            flags = TypeFlags(name="counter_tx_test")
            name: TxName = Flag(Key)
            count: TxCount

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(TxCounter)
        schema_manager.sync_schema(force=True)

        return clean_db, TxCounter, TxName, TxCount

    def test_explicit_commit_persists_data(self, schema_for_transactions):
        """Data persists when transaction is explicitly committed."""
        db, TxCounter, TxName, TxCount = schema_for_transactions

        # Use transaction context
        with db.transaction("write") as tx:
            manager = TxCounter.manager(tx)
            counter = TxCounter(name=TxName("test"), count=TxCount(1))
            manager.insert(counter)
            # Context commits on exit

        # Verify data persisted
        manager = TxCounter.manager(db)
        results = manager.all()
        assert len(results) == 1
        assert str(results[0].name) == "test"

    def test_exception_rolls_back_transaction(self, schema_for_transactions):
        """Exception inside transaction context rolls back all changes."""
        db, TxCounter, TxName, TxCount = schema_for_transactions

        # First insert something that should be rolled back
        try:
            with db.transaction("write") as tx:
                manager = TxCounter.manager(tx)
                counter = TxCounter(name=TxName("rollback_test"), count=TxCount(1))
                manager.insert(counter)

                # Raise exception to trigger rollback
                raise ValueError("Intentional error")
        except ValueError:
            pass  # Expected

        # Verify nothing persisted
        manager = TxCounter.manager(db)
        results = manager.all()
        assert len(results) == 0

    def test_multiple_operations_in_single_transaction(self, schema_for_transactions):
        """Multiple operations in one transaction commit together."""
        db, TxCounter, TxName, TxCount = schema_for_transactions

        with db.transaction("write") as tx:
            manager = TxCounter.manager(tx)

            # Insert multiple entities
            manager.insert(TxCounter(name=TxName("counter1"), count=TxCount(10)))
            manager.insert(TxCounter(name=TxName("counter2"), count=TxCount(20)))
            manager.insert(TxCounter(name=TxName("counter3"), count=TxCount(30)))

        # All should be committed
        manager = TxCounter.manager(db)
        results = manager.all()
        assert len(results) == 3

    def test_partial_operations_rollback_on_exception(self, schema_for_transactions):
        """All operations roll back even if some succeeded before exception."""
        db, TxCounter, TxName, TxCount = schema_for_transactions

        try:
            with db.transaction("write") as tx:
                manager = TxCounter.manager(tx)

                # These inserts should be rolled back
                manager.insert(TxCounter(name=TxName("partial1"), count=TxCount(1)))
                manager.insert(TxCounter(name=TxName("partial2"), count=TxCount(2)))

                # Raise exception
                raise RuntimeError("Partial failure")
        except RuntimeError:
            pass

        # Nothing should have persisted
        manager = TxCounter.manager(db)
        results = manager.all()
        assert len(results) == 0


@pytest.mark.integration
class TestTransactionContextCleanup:
    """Test transaction context resource cleanup."""

    @pytest.fixture
    def schema_for_cleanup(self, clean_db: Database):
        """Set up schema for cleanup tests."""

        class RecName(String):
            pass

        class RecValue(Integer):
            pass

        class Record(Entity):
            flags = TypeFlags(name="record_cleanup_test")
            name: RecName = Flag(Key)
            value: RecValue

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Record)
        schema_manager.sync_schema(force=True)

        return clean_db, Record, RecName, RecValue

    def test_context_manager_releases_resources(self, schema_for_cleanup):
        """Transaction context releases resources on normal exit."""
        db, Record, RecName, RecValue = schema_for_cleanup

        # Use context manager
        with db.transaction("write") as tx:
            manager = Record.manager(tx)
            manager.insert(Record(name=RecName("test"), value=RecValue(42)))

        # After context exits, should be able to start new transaction
        with db.transaction("read") as tx:
            manager = Record.manager(tx)
            results = manager.all()
            assert len(results) == 1

    def test_context_manager_releases_on_exception(self, schema_for_cleanup):
        """Transaction context releases resources even on exception."""
        db, Record, RecName, RecValue = schema_for_cleanup

        # First transaction with exception
        try:
            with db.transaction("read"):
                raise ValueError("Test error")
        except ValueError:
            pass

        # Should still be able to use database
        with db.transaction("write") as tx:
            manager = Record.manager(tx)
            manager.insert(Record(name=RecName("after_error"), value=RecValue(1)))


@pytest.mark.integration
class TestSequentialTransactions:
    """Test sequential transaction behavior."""

    @pytest.fixture
    def schema_for_sequential(self, clean_db: Database):
        """Set up schema for sequential transaction tests."""

        class SeqName(String):
            pass

        class SeqNum(Integer):
            pass

        class SeqItem(Entity):
            flags = TypeFlags(name="item_seq_test")
            name: SeqName = Flag(Key)
            seq: SeqNum

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(SeqItem)
        schema_manager.sync_schema(force=True)

        return clean_db, SeqItem, SeqName, SeqNum

    def test_sequential_transactions_see_each_others_changes(self, schema_for_sequential):
        """Later transactions see changes from earlier committed transactions."""
        db, SeqItem, SeqName, SeqNum = schema_for_sequential

        # First transaction (write)
        with db.transaction("write") as tx:
            manager = SeqItem.manager(tx)
            manager.insert(SeqItem(name=SeqName("first"), seq=SeqNum(1)))

        # Second transaction should see first's changes
        with db.transaction("write") as tx:
            manager = SeqItem.manager(tx)
            results = manager.all()
            assert len(results) == 1

            # Add another
            manager.insert(SeqItem(name=SeqName("second"), seq=SeqNum(2)))

        # Final check
        manager = SeqItem.manager(db)
        results = manager.all()
        assert len(results) == 2

    def test_update_in_separate_transaction(self, schema_for_sequential):
        """Update in separate transaction persists correctly."""
        db, SeqItem, SeqName, SeqNum = schema_for_sequential

        # Insert
        with db.transaction("write") as tx:
            manager = SeqItem.manager(tx)
            manager.insert(SeqItem(name=SeqName("update_target"), seq=SeqNum(0)))

        # Update in new transaction
        with db.transaction("write") as tx:
            manager = SeqItem.manager(tx)
            item = manager.get(name="update_target")[0]
            item.seq = SeqNum(100)
            manager.update(item)

        # Verify
        manager = SeqItem.manager(db)
        item = manager.get(name="update_target")[0]
        assert int(item.seq) == 100
