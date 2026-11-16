"""A failed walkthrough child must drain its database before deletion."""

from __future__ import annotations

import importlib.util
import subprocess
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import cast

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

    @contextmanager
    def raw_transaction(self, kind: str, *, commit_on_exit: bool) -> Iterator[None]:
        assert kind == "read" and not commit_on_exit
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


def test_failed_child_requires_delayed_read_close_before_delete(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    db = DatabaseProbe(failures=1)
    monkeypatch.setattr(runner.time, "sleep", lambda _: db.events.append("wait"))
    failure = subprocess.CalledProcessError(-6, ["c-walkthrough"])
    runner.cleanup_database(cast(Database, db), "tb_failed_child", failure)
    assert db.events == ["wait", "read-open", "wait", "read-open", "read-close", "delete", "exists"]
    assert "tb_failed_child" in capsys.readouterr().err


def test_failed_child_leaves_database_when_read_never_closes(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    db = DatabaseProbe(failures=3)
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
    monkeypatch.setattr(runner.time, "sleep", lambda _: db.events.append("wait"))
    with pytest.raises(RuntimeError, match="Leaving walkthrough database tb_failed_close"):
        runner.cleanup_database(
            cast(Database, db), "tb_failed_close", subprocess.CalledProcessError(1, ["walkthrough"])
        )
    assert db.events == ["wait", "read-open", "read-close-failed"] * 3
