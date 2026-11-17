"""The concurrency guide's TypeBridge examples are tested files.

The guide includes them with `--8<-- "path:section"`. The four purchase
builders are the `writes` conformance programs, which the conformance runner
builds and runs; here the Python one must equal the golden purchase, and the
recovery recipes run against scripted transactions.
"""

from __future__ import annotations

import importlib.util
import re
import sys
from pathlib import Path
from types import ModuleType, SimpleNamespace
from typing import Any, cast

import pytest
from type_bridge_core import Exists, Matched

import type_bridge.plan as plan_module
from tests.unit.typed_query import test_plan_builder as golden
from type_bridge import CommitConflict, CommitOutcomeUnknown, CommitRejected, Failure

ROOT = Path(__file__).resolve().parents[3]
GUIDE = ROOT / "docs/guide/concurrency.md"
PURCHASE = ROOT / "examples/purchase"
INCLUDE = re.compile(r'^ *--8<-- "([^":]+):([a-z][-_0-9a-z]*)"$', re.MULTILINE)


def _load(path: Path, name: str) -> ModuleType:
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_every_guide_include_names_a_section_of_an_existing_file() -> None:
    # pymdownx.snippets skips a missing file or section silently.
    includes = INCLUDE.findall(GUIDE.read_text(encoding="utf-8"))
    assert {path for path, _ in includes} == {
        "examples/purchase/python/program.py",
        "examples/purchase/python/recovery.py",
        "examples/purchase/ts/program.ts",
        "examples/purchase/rust/main.rs",
        "examples/purchase/c/program.c",
    }
    for path, section in includes:
        text = (ROOT / path).read_text(encoding="utf-8")
        for marker in ("start", "end"):
            assert text.count(f"--8<-- [{marker}:{section}]") == 1, (path, section, marker)


def _models() -> ModuleType:
    """The generated package's tokens, shaped as the plan builder reads them."""

    authority = golden._authority()
    projection = None

    def model(kind: str, label: str, **fields: str) -> type[Any]:
        owner = type(label.title(), (), {"__type_id__": f'{{"kind":"{kind}","label":"{label}"}}'})
        setattr(owner, "__runtime_projection__", projection)
        setattr(owner, "__plan_roles__", [])
        handle = type(f"{label.title()}Handle", (plan_module.Handle,), {})
        token_class = type("Token", (plan_module.FieldToken,), {})
        for name, attribute in fields.items():
            token = token_class()
            object.__setattr__(token, "owner", owner)
            object.__setattr__(token, "fact", golden.field(owner, attribute).fact)
            setattr(owner, name, token)
            setattr(handle, name, property(lambda self, token=token: self._value(token)))
        setattr(handle, "set", lambda self, **values: self._set(values))

        def match(cls: type[Any], **values: Any) -> Any:
            return plan_module._match(cls, handle, authority, None, plan_module._MISSING, values)

        def insert(cls: type[Any], **values: Any) -> Any:
            players = {key: value for key, value in values.items() if key in {"buyer", "item"}}
            scalars = {key: value for key, value in values.items() if key not in players}
            return plan_module._insert(cls, authority, scalars, players)

        setattr(owner, "match", classmethod(match))
        setattr(owner, "insert", classmethod(insert))
        return owner

    purchase = model("relation", "purchase", order_id="order-id")
    for label in ("buyer", "item"):
        setattr(purchase, label, golden.role(purchase, label))
    models = ModuleType("docs_purchase_models")
    models.__dict__.update(
        Item=model("entity", "item", item_id="item-id", stock="stock", price="price"),
        Account=model("entity", "account", account_id="account-id", balance="balance"),
        Doctor=model("entity", "doctor", name="name", on_call="on-call"),
        Purchase=purchase,
    )
    return models


@pytest.fixture
def program(monkeypatch: pytest.MonkeyPatch) -> ModuleType:
    monkeypatch.setitem(sys.modules, "docs_purchase_models", _models())
    monkeypatch.setenv("TYPE_BRIDGE_SDK_WRITES_PYTHON_PACKAGE", "docs_purchase_models")
    return _load(PURCHASE / "python/program.py", "docs_purchase_program")


@pytest.fixture
def recovery() -> ModuleType:
    return _load(PURCHASE / "python/recovery.py", "docs_purchase_recovery")


def test_the_guide_purchase_matches_the_explicit_core_schedule(program: ModuleType) -> None:
    args = golden.FIXTURE["purchase"]
    assert (args["order"], args["item_id"], args["account_id"]) == ("O1", "I1", "A1")
    built = program.purchase(golden._authority())
    explicit = _load(PURCHASE / "python/program_explicit.py", "docs_purchase_explicit")
    oracle = explicit.purchase(program.models, golden._authority())
    assert built.canonical_bytes() == oracle.canonical_bytes()


def _failure(phase: str, reason: str | None, *, retry_safe: bool, code: str) -> Failure:
    return Failure(phase, reason, None, None, 0, retry_safe, code)


def _conflict() -> CommitConflict:
    error = CommitConflict("conflict")
    error.failure = _failure("commit", "conflict", retry_safe=True, code="STC2")
    return error


def _rejected() -> CommitRejected:
    error = CommitRejected("rejected")
    error.failure = _failure("commit", "cardinality", retry_safe=True, code="CNT5")
    return error


def _unknown() -> CommitOutcomeUnknown:
    error = CommitOutcomeUnknown("unknown")
    error.failure = _failure("commit", None, retry_safe=False, code="STC1")
    return error


class Transaction:
    """A staged transaction whose commits fail as scripted, then succeed."""

    def __init__(self, failures: list[BaseException], result: object) -> None:
        self.failures, self.result = failures, result
        self.staged: list[object] = []
        self.commits = 0
        self.closed = False

    def stage(self, plan: object) -> SimpleNamespace:
        self.staged.append(plan)
        return SimpleNamespace(result=lambda: self.result)

    def commit(self) -> None:
        self.commits += 1
        if self.failures:
            raise self.failures.pop(0)

    def close(self) -> None:
        self.closed = True

    def __enter__(self) -> Transaction:
        return self

    def __exit__(self, exc_type: object, *_: object) -> None:
        try:
            if exc_type is None:
                self.commit()
        finally:
            self.close()


class Database:
    """Opens one fresh transaction per attempt; commits fail as scripted."""

    def __init__(self, *failures: BaseException, answer: int = 1) -> None:
        self.failures, self.answer = list(failures), answer
        self.opened: list[Transaction] = []

    def staged_transaction(self, kind: str) -> Transaction:
        assert kind == "write"
        failures = [self.failures.pop(0)] if self.failures else []
        self.opened.append(Transaction(failures, Matched(self.answer)))
        return self.opened[-1]


def test_buy_retries_a_conflict_on_a_fresh_transaction(
    program: ModuleType, recovery: ModuleType, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(recovery.time, "sleep", lambda _: None)
    plan = program.purchase(golden._authority())
    db = Database(_conflict(), _conflict())
    assert recovery.buy(cast(Any, db), plan) is True
    assert [tx.commits for tx in db.opened] == [1, 1, 1]
    assert all(tx.closed and tx.staged == [plan] for tx in db.opened)


def test_buy_reports_a_failed_condition_without_retrying(recovery: ModuleType) -> None:
    db = Database(answer=0)
    assert recovery.buy(cast(Any, db), object()) is False
    assert len(db.opened) == 1


@pytest.mark.parametrize("failure", [_rejected, _unknown])
def test_buy_raises_every_other_commit_failure_at_once(recovery: ModuleType, failure: Any) -> None:
    db = Database(failure())
    with pytest.raises((CommitRejected, CommitOutcomeUnknown)):
        recovery.buy(cast(Any, db), object())
    assert len(db.opened) == 1


def test_buy_raises_the_last_conflict(
    recovery: ModuleType, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(recovery.time, "sleep", lambda _: None)
    db = Database(*[_conflict() for _ in range(3)])
    with pytest.raises(CommitConflict):
        recovery.buy(cast(Any, db), object(), attempts=3)
    assert len(db.opened) == 3


def _proxy(tx: Transaction, check: Transaction | None = None) -> Any:
    return SimpleNamespace(transaction=lambda kind: tx if kind == "write" else check)


def _lost_reply() -> RuntimeError:
    error = RuntimeError("the proxy reply was not received")
    error.code = "commit_outcome_unknown"  # type: ignore[attr-defined]
    return error


@pytest.mark.parametrize("applied", [True, False])
def test_buy_over_proxy_reads_the_database_after_a_lost_reply(
    recovery: ModuleType, applied: bool
) -> None:
    tx = Transaction([_lost_reply()], Matched(1))
    check = Transaction([], Exists(applied))
    placed = object()
    assert recovery.buy_over_proxy(_proxy(tx, check), object(), placed) is applied
    assert (tx.commits, tx.closed) == (1, True)
    assert (check.staged, check.commits, check.closed) == ([placed], 1, True)


def test_buy_over_proxy_raises_a_definite_failure(recovery: ModuleType) -> None:
    tx = Transaction([_conflict()], Matched(1))
    with pytest.raises(CommitConflict):
        recovery.buy_over_proxy(_proxy(tx), object(), object())
    assert tx.closed
