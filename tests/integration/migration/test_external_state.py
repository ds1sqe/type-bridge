"""Live coverage for externally owned migration state (#165)."""

from __future__ import annotations

# pyright: reportMissingImports=false
from pathlib import Path

import pytest

from type_bridge.migration import (
    MIGRATION_STATE_SCHEMA,
    MigrationExecutor,
    MigrationRecord,
    MigrationRunRecord,
    MigrationState,
    SchemaIntrospector,
)


class _ExternalStateStore:
    """Minimal process-local ledger used to prove target-schema isolation."""

    def __init__(self) -> None:
        self.state = MigrationState()
        self.run_events: list[tuple[str, str]] = []

    def load_state(self) -> MigrationState:
        return self.state

    def record_applied(self, app_label: str, name: str, checksum: str) -> None:
        self.state.add(
            MigrationRecord(
                app_label=app_label,
                name=name,
                applied_at="2026-07-10T00:00:00.000000",
                checksum=checksum,
            )
        )

    def record_unapplied(self, app_label: str, name: str) -> None:
        self.state.remove(app_label, name)

    def record_run_started(
        self,
        app_label: str,
        name: str,
        checksum: str,
        direction: str,
    ) -> MigrationRunRecord:
        self.run_events.append(("started", name))
        return MigrationRunRecord(
            run_id=f"run-{name}",
            app_label=app_label,
            name=name,
            checksum=checksum,
            direction=direction,
            status="started",
            started_at="2026-07-10T00:00:00.000000",
        )

    def record_run_finished(
        self,
        record: MigrationRunRecord,
        status: str,
        error: str | None = None,
    ) -> MigrationRunRecord:
        self.run_events.append((status, record.name))
        return MigrationRunRecord(
            run_id=record.run_id,
            app_label=record.app_label,
            name=record.name,
            checksum=record.checksum,
            direction=record.direction,
            status=status,
            started_at=record.started_at,
            finished_at="2026-07-10T00:00:01.000000",
            error=error,
        )


@pytest.mark.integration
@pytest.mark.order(326)
def test_external_state_store_keeps_target_schema_free_of_typebridge_ledger(
    clean_db,
    tmp_path: Path,
) -> None:
    """A real Rust migration uses the external ledger and no target state schema."""
    migrations_dir = tmp_path / "external_migrations"
    migrations_dir.mkdir()
    (migrations_dir / "0001_external_state.py").write_text(
        """from type_bridge.migration import Migration, operations as ops


class ExternalStateMigration(Migration):
    dependencies = []
    operations = [
        ops.RunTypeQL(
            forward="define attribute issue-165-app-value, value string;",
            reverse="undefine attribute issue-165-app-value;",
        )
    ]
"""
    )
    state_store = _ExternalStateStore()
    executor = MigrationExecutor(
        clean_db,
        migrations_dir,
        state_manager=state_store,
    )

    results = executor.migrate()

    assert [(result.name, result.action, result.success) for result in results] == [
        ("0001_external_state", "applied", True)
    ]
    assert state_store.state.is_applied("external_migrations", "0001_external_state")
    assert state_store.run_events == []

    raw_schema = SchemaIntrospector(clean_db).introspect()
    assert "issue-165-app-value" in raw_schema.get_attribute_names()
    assert MIGRATION_STATE_SCHEMA.entities.isdisjoint(raw_schema.get_entity_names())
    assert MIGRATION_STATE_SCHEMA.relations.isdisjoint(raw_schema.get_relation_names())
    assert MIGRATION_STATE_SCHEMA.attributes.isdisjoint(raw_schema.get_attribute_names())


@pytest.mark.integration
@pytest.mark.order(327)
def test_mixed_external_state_plan_keeps_all_run_logging_outside_target(
    clean_db,
    tmp_path: Path,
) -> None:
    """Rust/Python/Rust execution uses one external ledger without DB infrastructure."""
    migrations_dir = tmp_path / "external_migrations"
    migrations_dir.mkdir()
    (migrations_dir / "0001_before_python.py").write_text(
        """from type_bridge.migration import Migration, operations as ops


class BeforePythonMigration(Migration):
    dependencies = []
    operations = [
        ops.RunTypeQL(
            forward="define attribute issue-165-before-python, value string;",
            reverse="undefine attribute issue-165-before-python;",
        )
    ]
"""
    )
    (migrations_dir / "0002_python_step.py").write_text(
        """from type_bridge.migration import Migration, operations as ops


def forwards(db):
    db.execute_query(
        "define attribute issue-165-python-value, value string;",
        transaction_type="schema",
    )


class PythonStepMigration(Migration):
    dependencies = [("external_migrations", "0001_before_python")]
    operations = [ops.RunPython(forwards)]
"""
    )
    (migrations_dir / "0003_after_python.py").write_text(
        """from type_bridge.migration import Migration, operations as ops


class AfterPythonMigration(Migration):
    dependencies = [("external_migrations", "0002_python_step")]
    operations = [
        ops.RunTypeQL(
            forward="define attribute issue-165-after-python, value string;",
            reverse="undefine attribute issue-165-after-python;",
        )
    ]
"""
    )
    state_store = _ExternalStateStore()

    results = MigrationExecutor(
        clean_db,
        migrations_dir,
        state_manager=state_store,
    ).migrate()

    assert [(result.name, result.action, result.success) for result in results] == [
        ("0001_before_python", "applied", True),
        ("0002_python_step", "applied", True),
        ("0003_after_python", "applied", True),
    ]
    assert {record.name for record in state_store.state.applied} == {
        "0001_before_python",
        "0002_python_step",
        "0003_after_python",
    }
    assert state_store.run_events == [
        ("started", "0002_python_step"),
        ("succeeded", "0002_python_step"),
    ]

    raw_schema = SchemaIntrospector(clean_db).introspect()
    assert {
        "issue-165-before-python",
        "issue-165-python-value",
        "issue-165-after-python",
    } <= raw_schema.get_attribute_names()
    assert MIGRATION_STATE_SCHEMA.entities.isdisjoint(raw_schema.get_entity_names())
    assert MIGRATION_STATE_SCHEMA.relations.isdisjoint(raw_schema.get_relation_names())
    assert MIGRATION_STATE_SCHEMA.attributes.isdisjoint(raw_schema.get_attribute_names())
