"""Pure helpers of the live race driver in tests/live/races."""

from __future__ import annotations

import pytest
from live.races import driver
from live.races.driver import Outcome

from type_bridge.failure import CommitConflict, CommitOutcomeUnknown, CommitRejected, Failure


def _failure(code: str, *, phase: str = "commit", retry_safe: bool = True) -> Failure:
    return Failure(phase, None, None, None, None, retry_safe, code)


def _raised(error: BaseException, failure: Failure | None) -> BaseException:
    if failure is not None:
        error.failure = failure  # type: ignore[attr-defined]
    return error


def test_rounds_default_and_override() -> None:
    assert driver.rounds({}) == 5
    assert driver.rounds({"TB_RACE_ROUNDS": " 12 "}) == 12
    with pytest.raises(ValueError, match="at least 1"):
        driver.rounds({"TB_RACE_ROUNDS": "0"})


def test_classify_reads_the_class_and_the_typed_failure() -> None:
    conflict = driver.classify(_raised(CommitConflict("lost"), _failure("STC2")))
    assert (conflict.kind, conflict.code, conflict.phase) == ("conflict", "STC2", "commit")
    assert conflict.error_class == "CommitConflict" and conflict.retry_safe is True
    rejected = driver.classify(_raised(CommitRejected("card"), _failure("CNT5")))
    assert rejected.label == "rejected:CNT5"
    unknown = driver.classify(
        _raised(CommitOutcomeUnknown("gone"), _failure("CXN06", retry_safe=False))
    )
    assert unknown.label == "unknown:CXN06" and unknown.retry_safe is False
    # A typed query-phase failure outside the commit classes is still a rejection.
    query = driver.classify(_raised(RuntimeError("key"), _failure("CNT9", phase="query")))
    assert (query.kind, query.phase, query.error_class) == ("rejected", "query", "RuntimeError")
    untyped = driver.classify(RuntimeError("closed"))
    assert untyped.label == "error:None" and untyped.detail == "RuntimeError: closed"


def test_tally_and_winners() -> None:
    outcomes = [
        Outcome("committed", matched=(1,)),
        Outcome("committed", matched=(0,)),
        Outcome("conflict", code="STC2"),
    ]
    assert driver.tally(outcomes) == {"committed:1": 1, "committed:0": 1, "conflict:STC2": 1}
    assert driver.winners(outcomes) == 1


def test_next_pause_moves_towards_the_commit() -> None:
    # Paused before the commit was sent: halfway towards the ceiling.
    assert driver.next_pause(2.0, 4.0, Outcome("error"), 25.0) == (3.0, 4.0)
    # The commit finished first: its time is the new ceiling.
    delay, ceiling = driver.next_pause(2.0, 4.0, Outcome("committed"), 1.0)
    assert (delay, ceiling) == (pytest.approx(0.85), 1.0)
