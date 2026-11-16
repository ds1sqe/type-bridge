"""A failed walkthrough child must drain its database before deletion."""

from __future__ import annotations

import importlib.util
import subprocess
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any, cast

import pytest

from type_bridge import Database

PATH = Path(__file__).parents[3] / "examples/walkthrough/run.py"
SPEC = importlib.util.spec_from_file_location("walkthrough_runner", PATH)
assert SPEC is not None and SPEC.loader is not None
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class DatabaseProbe:
    def __init__(self, failures: int = 0, close_failures: int = 0) -> None:
        self.failures = failures
        self.close_failures = close_failures
        self.events: list[str] = []
        self.address = "127.0.0.1:1729"
        self.username = "admin"
        self.password = "password"
        self.tls = False
        self.tls_root_ca = None
        self.driver_closes = 0

    @contextmanager
    def transaction(self, name: str, kind: Any, options: Any) -> Iterator[None]:
        assert name.startswith("tb_")
        assert kind == runner.TransactionType.READ
        assert options.transaction_timeout_millis == 1_000
        assert options.schema_lock_acquire_timeout_millis == 1_000
        self.events.append("read-open")
        if self.failures:
            self.failures -= 1
            raise RuntimeError("connection still draining")
        yield
        if self.close_failures:
            self.close_failures -= 1
            self.events.append("read-close-failed")
            raise RuntimeError("read close failed")
        self.events.append("read-close")

    def delete_database(self) -> None:
        self.events.append("delete")

    def database_exists(self) -> bool:
        self.events.append("exists")
        return False

    def _current_connection_identity(self) -> Any:
        return Database(
            address=self.address, username=self.username, password=self.password
        )._current_connection_identity()


def install_driver(monkeypatch: pytest.MonkeyPatch, db: DatabaseProbe) -> None:
    @contextmanager
    def driver(address: str, credentials: Any, options: Any) -> Iterator[DatabaseProbe]:
        assert address == db.address
        assert options.request_timeout_millis == 1_000
        assert options.primary_failover_retries == 0
        try:
            yield db
        finally:
            db.driver_closes += 1

    monkeypatch.setattr(runner.TypeDB, "driver", driver)


def test_failed_child_requires_delayed_read_close_before_delete(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    db = DatabaseProbe(failures=1)
    install_driver(monkeypatch, db)
    monkeypatch.setattr(runner.time, "sleep", lambda _: db.events.append("wait"))
    failure = subprocess.CalledProcessError(-6, ["c-walkthrough"])
    runner.cleanup_database(cast(Database, db), "tb_failed_child", failure)
    assert db.events == ["wait", "read-open", "wait", "read-open", "read-close", "delete", "exists"]
    assert "tb_failed_child" in capsys.readouterr().err
    assert db.driver_closes == 2


def test_failed_child_leaves_database_when_read_never_closes(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = DatabaseProbe(failures=3)
    install_driver(monkeypatch, db)
    monkeypatch.setattr(runner.time, "sleep", lambda _: db.events.append("wait"))
    with pytest.raises(RuntimeError, match="Leaving walkthrough database tb_failed_child"):
        runner.cleanup_database(
            cast(Database, db), "tb_failed_child", subprocess.CalledProcessError(1, ["walkthrough"])
        )
    assert db.events == ["wait", "read-open"] * 3


def test_successful_child_keeps_immediate_cleanup() -> None:
    db = DatabaseProbe()
    runner.cleanup_database(cast(Database, db), "tb_success", None)
    assert db.events == ["delete", "exists"]


def test_failed_read_close_does_not_allow_deletion(monkeypatch: pytest.MonkeyPatch) -> None:
    db = DatabaseProbe(close_failures=3)
    install_driver(monkeypatch, db)
    monkeypatch.setattr(runner.time, "sleep", lambda _: db.events.append("wait"))
    with pytest.raises(RuntimeError, match="Leaving walkthrough database tb_failed_close"):
        runner.cleanup_database(
            cast(Database, db), "tb_failed_close", subprocess.CalledProcessError(1, ["walkthrough"])
        )
    assert db.events == ["wait", "read-open", "read-close-failed"] * 3


def test_keyboard_interrupt_from_child_requires_probe_before_delete(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    class MainDatabase(DatabaseProbe):
        def create_database_outcome(self) -> str:
            return "created"

        @contextmanager
        def raw_transaction(self, kind: str) -> Iterator[None]:
            assert kind == "schema"
            yield

        def close(self) -> None:
            self.events.append("database-close")

    db = MainDatabase()
    install_driver(monkeypatch, db)
    monkeypatch.setattr(runner.time, "sleep", lambda _: db.events.append("wait"))
    monkeypatch.setattr(runner, "Database", lambda **kwargs: db)
    monkeypatch.setenv("TYPEDB_ADDRESS", db.address)
    monkeypatch.setenv("TYPEDB_HTTP_PORT", "8000")
    monkeypatch.setenv("TYPE_BRIDGE_WALKTHROUGH_DATABASE_PREFIX", "tb_")
    monkeypatch.setattr(
        "sys.argv", ["walkthrough", "--sdk", "python", "--output", str(tmp_path / "out")]
    )

    def workspace(path: Path) -> None:
        (path / "migrations").mkdir(parents=True)
        (path / "migrations/0001_initial.typeql").write_text("")
        (path / "generated/python").mkdir(parents=True)

    def run(command: list[str], **kwargs: Any) -> None:
        if kwargs.get("output") is not None:
            db.events.append("child-interrupt")
            raise KeyboardInterrupt

    monkeypatch.setattr(runner, "stage_workspace", workspace)
    monkeypatch.setattr(runner, "run", run)
    with pytest.raises(KeyboardInterrupt):
        runner.main()
    assert db.events == [
        "child-interrupt",
        "wait",
        "read-open",
        "read-close",
        "delete",
        "exists",
        "database-close",
    ]
