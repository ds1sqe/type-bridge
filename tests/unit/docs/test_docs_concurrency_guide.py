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
from type_bridge_core import Matched

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

    def model(kind: str, label: str, **fields: str) -> type[Any]:
        owner = type(label.title(), (), {"__type_id__": f'{{"kind":"{kind}","label":"{label}"}}'})
        for name, attribute in fields.items():
            setattr(owner, name, golden.field(owner, attribute))
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


def test_the_guide_purchase_is_the_golden_purchase(program: ModuleType) -> None:
    args = golden.FIXTURE["purchase"]
    assert (args["order"], args["item_id"], args["account_id"]) == ("O1", "I1", "A1")
    built = program.purchase(golden._authority())
    assert built.canonical_bytes().hex() == golden.FIXTURE["purchase_hex"]


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

    def __init__(self, failures: list[BaseException], answer: int, unknown: bool) -> None:
        self.failures, self.answer, self.unknown = failures, answer, unknown
        self.staged: list[object] = []
        self.commits = self.retries = 0
        self.closed = False

    def stage(self, plan: object) -> SimpleNamespace:
        self.staged.append(plan)
        return SimpleNamespace(result=lambda: Matched(self.answer))

    def commit(self) -> None:
        self.commits += 1
        if self.failures:
            raise self.failures.pop(0)

    def has_unknown_outcome(self) -> bool:
        return self.unknown

    def commit_retry(self) -> None:
        assert self.unknown
        self.retries += 1

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
        self.opened.append(Transaction(failures, self.answer, unknown=False))
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


def _proxy(tx: Transaction) -> Any:
    return SimpleNamespace(transaction=lambda kind: tx)


def test_buy_over_proxy_resends_an_unknown_outcome_once(recovery: ModuleType) -> None:
    lost = RuntimeError("commit_outcome_unknown")
    tx = Transaction([lost], answer=1, unknown=True)
    assert recovery.buy_over_proxy(_proxy(tx), object()) is True
    assert (tx.commits, tx.retries, tx.closed) == (1, 1, True)


def test_buy_over_proxy_raises_a_definite_failure(recovery: ModuleType) -> None:
    tx = Transaction([_conflict()], answer=1, unknown=False)
    with pytest.raises(CommitConflict):
        recovery.buy_over_proxy(_proxy(tx), object())
    assert (tx.retries, tx.closed) == (0, True)
