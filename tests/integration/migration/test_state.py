"""Integration tests for the Rust-backed migration state manager.

These tests exercise the TypeDB-backed ``MigrationStateManager`` end-to-end
against a live TypeDB (docker fixtures). They are the live validation of the
fetch-document-shape parsing in ``crates/migration/src/state/typedb.rs``: the
Rust isolated unit tests assert against a hand-built document shape, while these
tests prove that shape matches what TypeDB actually returns.

Lifecycle covered:
- empty-state load returns no records;
- ensure_schema -> record_applied -> load_state returns the record;
- idempotent re-record does not duplicate;
- record_unapplied removes the record;
- state is durable in TypeDB across a fresh ``MigrationStateManager`` (reloaded
  from the database, not a process-local cache).
"""

# pyright: reportMissingImports=false
import pytest

from type_bridge.migration import MigrationStateManager


@pytest.mark.integration
@pytest.mark.order(320)
def test_load_state_empty(clean_db):
    """A fresh database has no applied migrations."""
    manager = MigrationStateManager(clean_db)
    manager.ensure_schema()

    state = manager.load_state()

    assert state.applied == []
    assert not state.is_applied("myapp", "0001_initial")


@pytest.mark.integration
@pytest.mark.order(321)
def test_record_then_load(clean_db):
    """Recording a migration makes it visible on the next load."""
    manager = MigrationStateManager(clean_db)
    manager.ensure_schema()

    manager.record_applied("myapp", "0001_initial", "checksum-abc")

    state = manager.load_state()

    assert state.is_applied("myapp", "0001_initial")
    records = state.get_all_for_app("myapp")
    assert len(records) == 1
    assert records[0].app_label == "myapp"
    assert records[0].name == "0001_initial"
    assert records[0].checksum == "checksum-abc"
    # applied_at is populated from the stored datetime.
    assert records[0].applied_at != ""


@pytest.mark.integration
@pytest.mark.order(322)
def test_record_applied_is_idempotent(clean_db):
    """Re-recording the same migration must not duplicate the row."""
    manager = MigrationStateManager(clean_db)
    manager.ensure_schema()

    manager.record_applied("myapp", "0001_initial", "checksum-abc")
    manager.record_applied("myapp", "0001_initial", "checksum-abc")

    # Reload from TypeDB with a fresh manager to bypass the local cache.
    state = MigrationStateManager(clean_db).load_state()

    records = state.get_all_for_app("myapp")
    assert len(records) == 1, "re-recording the same migration must not duplicate"


@pytest.mark.integration
@pytest.mark.order(323)
def test_record_unapplied_removes_record(clean_db):
    """Unrecording a migration removes it from the loaded state."""
    manager = MigrationStateManager(clean_db)
    manager.ensure_schema()

    manager.record_applied("myapp", "0001_initial", "checksum-abc")
    assert manager.load_state().is_applied("myapp", "0001_initial")

    manager.record_unapplied("myapp", "0001_initial")

    state = MigrationStateManager(clean_db).load_state()
    assert not state.is_applied("myapp", "0001_initial")
    assert state.applied == []


@pytest.mark.integration
@pytest.mark.order(324)
def test_state_durable_across_fresh_manager(clean_db):
    """Recorded state persists in TypeDB, not just a process-local cache.

    A second, independently constructed manager must observe the records the
    first one wrote — proving the read goes to TypeDB rather than an in-memory
    cache on the writing manager.
    """
    writer = MigrationStateManager(clean_db)
    writer.ensure_schema()
    writer.record_applied("orders", "0001_initial", "sum-1")
    writer.record_applied("orders", "0002_add_index", "sum-2")

    # Fresh manager, no shared local cache.
    reader = MigrationStateManager(clean_db)
    state = reader.load_state()

    assert state.is_applied("orders", "0001_initial")
    assert state.is_applied("orders", "0002_add_index")
    records = state.get_all_for_app("orders")
    assert {r.name for r in records} == {"0001_initial", "0002_add_index"}
    assert {r.checksum for r in records} == {"sum-1", "sum-2"}
