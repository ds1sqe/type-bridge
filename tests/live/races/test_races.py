"""Staged plans raced from three processes on a live TypeDB 3.13.6 (#253).

Each scenario runs TB_RACE_ROUNDS rounds (default 5) on a fresh database
per round and reproduces the outcomes in the conditional-write race
results. The paused-server case needs TB_RACE_CONTAINER, the name of a
TypeDB container this run alone uses: it is `docker pause`d mid-commit.

Run: TYPEDB_ADDRESS=... TYPEDB_HTTP_PORT=... pytest tests/live/races -m slow
"""

from __future__ import annotations

import os
import shutil
import subprocess
from collections import Counter
from collections.abc import Callable
from pathlib import Path

import pytest
from type_bridge_core import PyRustDatabase

from type_bridge.failure import CommitConflict, CommitOutcomeUnknown, CommitRejected

from . import driver
from .driver import Outcome, Target

ADDRESS = os.environ.get("TYPEDB_ADDRESS")
HTTP_PORT = os.environ.get("TYPEDB_HTTP_PORT")
PROFILE = os.environ.get("TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE", "typedb-3.13.6/v1")
CONTAINER = os.environ.get("TB_RACE_CONTAINER")
ROOT = Path(__file__).resolve().parents[3]

pytestmark = [
    pytest.mark.integration,
    pytest.mark.slow,
    pytest.mark.skipif(
        not (ADDRESS and HTTP_PORT), reason="requires TYPEDB_ADDRESS and TYPEDB_HTTP_PORT"
    ),
]


def _export_declared(directory: Path) -> bytes:
    """The workspace's canonical declared schema, from the CLI."""
    workspace = directory / "workspace"
    shutil.copytree(driver.WORKSPACE, workspace)
    cli = os.environ.get("TB_RACE_CLI")
    command = (
        [cli]
        if cli
        else [
            "cargo",
            "run",
            "--quiet",
            "--locked",
            "--manifest-path",
            str(ROOT / "type-bridge-core/Cargo.toml"),
            "-p",
            "type-bridge-cli",
            "--",
        ]
    )
    subprocess.run([*command, "schema", "export-declared"], cwd=workspace, check=True)
    return (workspace / "declared-schema.json").read_bytes()


@pytest.fixture(scope="module")
def base(tmp_path_factory: pytest.TempPathFactory) -> Target:
    assert ADDRESS is not None and HTTP_PORT is not None
    probe = PyRustDatabase.connect(ADDRESS, "tb_race_probe", http_port=int(HTTP_PORT))
    try:
        version = probe.server_version() or ""
    finally:
        probe.close()
    if PROFILE != "typedb-3.13.6/v1" or not version.startswith("3.13.6"):
        pytest.skip(f"the race outcomes are pinned to TypeDB 3.13.6, not {version!r}")
    declared = _export_declared(tmp_path_factory.mktemp("races"))
    return Target(ADDRESS, int(HTTP_PORT), "", declared, PROFILE)


Check = Callable[[list[Outcome], dict[str, int]], None]


def _rounds(
    base: Target,
    scenario: str,
    state: dict[str, Callable[[driver.QueryV2Authority], driver.Plan]],
    check: Check,
) -> list[Counter[str]]:
    """Race `scenario` for every round; check each round; return the tallies."""
    tallies = []
    for number in range(driver.rounds(os.environ)):
        target = driver.fresh_database(base, scenario, driver.SCENARIOS[scenario].seed)
        try:
            outcomes = driver.race(target, scenario)
            counts = driver.read_counts(target, state)
            tally = driver.tally(outcomes)
            print(f"{scenario} round {number}: {dict(tally)} {counts}")
            assert not [o for o in outcomes if o.kind in ("error", "unknown")], outcomes
            check(outcomes, counts)
            tallies.append(tally)
        finally:
            driver.drop_database(target)
    return tallies


def _contended(tallies: list[Counter[str]], label: str) -> None:
    # Overlap is likely, not guaranteed: some round must show the conflict.
    assert any(tally[label] for tally in tallies), f"no round saw {label}: {tallies}"


def _assert_conflict(outcome: Outcome) -> None:
    assert outcome.error_class == CommitConflict.__name__, outcome
    assert outcome.code == "STC2" and outcome.phase == "commit", outcome


def test_purchase_sells_each_unit_once(base: Target) -> None:
    def check(outcomes: list[Outcome], counts: dict[str, int]) -> None:
        assert driver.winners(outcomes) == 1, outcomes
        for loser in (o for o in outcomes if driver.winners([o]) == 0):
            if loser.kind == "committed":
                # Serialised behind the winner: the plan found no stock.
                assert loser.matched == (0,), loser
            else:
                _assert_conflict(loser)
        assert counts == {"sold_out": 1, "purchases": 1, "charged": 1, "untouched": 2}

    tallies = _rounds(
        base,
        "purchase",
        {
            "sold_out": lambda a: driver.count_of(a, driver.Item, item_id="I1", stock=0),
            "purchases": lambda a: driver.count_of(a, driver.Purchase),
            "charged": lambda a: driver.count_of(a, driver.Account, balance=60),
            "untouched": lambda a: driver.count_of(a, driver.Account, balance=100),
        },
        check,
    )
    _contended(tallies, "conflict:STC2")


ON_CALL: dict[str, Callable[[driver.QueryV2Authority], driver.Plan]] = {
    "on_call": lambda a: driver.count_of(a, driver.Doctor, on_call=True),
}


def test_a_touch_serialises_the_write_skew(base: Target) -> None:
    def check(outcomes: list[Outcome], counts: dict[str, int]) -> None:
        for outcome in outcomes:
            if outcome.kind != "committed":
                _assert_conflict(outcome)
        # Whatever interleaving, the touch keeps someone on call.
        assert counts["on_call"] >= 1, (outcomes, counts)
        if driver.tally(outcomes)["conflict:STC2"] == driver.PROCESSES - 1:
            assert driver.winners(outcomes) == 1 and counts["on_call"] == 2

    _contended(_rounds(base, "touch", ON_CALL, check), "conflict:STC2")


def test_write_skew_without_a_touch_commits_every_racer(base: Target) -> None:
    def check(outcomes: list[Outcome], counts: dict[str, int]) -> None:
        assert all(o.kind == "committed" for o in outcomes), outcomes

    tallies = _rounds(base, "skew", ON_CALL, check)
    # The anomaly: every racer read a colleague on call and left.
    assert any(tally["committed:1"] == driver.PROCESSES for tally in tallies), tallies


def test_plays_card_serialises_then_rejects_a_late_assignment(base: Target) -> None:
    assignments = {"assignments": lambda a: driver.count_of(a, driver.Assignment)}

    def check(outcomes: list[Outcome], counts: dict[str, int]) -> None:
        assert driver.winners(outcomes) == 1, outcomes
        for loser in (o for o in outcomes if o.kind != "committed"):
            if loser.code == "CNT5":
                # Serialised behind the winner: the cardinality check at commit.
                assert loser.error_class == CommitRejected.__name__, loser
            else:
                _assert_conflict(loser)
        assert counts == {"assignments": 1}

    _contended(_rounds(base, "plays-card", assignments, check), "conflict:STC2")

    # Later, with the first assignment committed, the card rejects outright.
    target = driver.fresh_database(base, "plays-card", driver.SCENARIOS["plays-card"].seed)
    try:
        first = driver.commit_alone(target, lambda a: [driver.assign(a, "alice", "S0")])
        assert first == Outcome("committed", matched=(1,)), first
        late = driver.commit_alone(target, lambda a: [driver.assign(a, "alice", "S3")])
        print(f"plays-card late: {late.label}")
        assert late.error_class == CommitRejected.__name__, late
        assert (late.kind, late.code, late.phase) == ("rejected", "CNT5", "commit"), late
        assert driver.read_counts(target, assignments) == {"assignments": 1}
    finally:
        driver.drop_database(target)


def test_a_key_admits_one_racer(base: Target) -> None:
    keys = {"k1": lambda a: driver.count_of(a, driver.Item, item_id="K1")}

    def check(outcomes: list[Outcome], counts: dict[str, int]) -> None:
        assert driver.winners(outcomes) == 1, outcomes
        for loser in (o for o in outcomes if o.kind != "committed"):
            if loser.code == "CNT9":
                # Serialised behind the winner: the key is already taken.
                assert loser.kind == "rejected", loser
            else:
                _assert_conflict(loser)
        assert counts == {"k1": 1}

    _contended(_rounds(base, "key", keys, check), "conflict:STC2")


@pytest.mark.skipif(not CONTAINER, reason="requires TB_RACE_CONTAINER (paused mid-commit)")
def test_a_server_paused_mid_commit_leaves_the_outcome_unknown(base: Target) -> None:
    assert CONTAINER is not None
    items = {"items": lambda a: driver.count_of(a, driver.Item)}
    target = driver.fresh_database(base, "pause-baseline", ())
    try:
        outcome, baseline = driver.timed_commit(target)
        assert outcome.kind == "committed", outcome
    finally:
        driver.drop_database(target)
    delay, ceiling = baseline * 0.85, baseline
    attempts = []
    for _ in range(driver.PAUSE_ATTEMPTS):
        target = driver.fresh_database(base, "pause", ())
        try:
            outcome, elapsed = driver.commit_while_paused(target, CONTAINER, delay)
            # The commit is atomic whatever the client learned.
            count = driver.read_counts(target, items)["items"]
        finally:
            driver.drop_database(target)
        attempts.append((round(delay, 2), round(elapsed, 2), outcome.kind, outcome.code, count))
        print(
            f"pause baseline {baseline:.2f}s attempt (delay, took, kind, code, items): {attempts[-1]}"
        )
        assert count in (0, driver.BULK_ITEMS), attempts
        if outcome.kind == "unknown":
            assert outcome.error_class == CommitOutcomeUnknown.__name__, outcome
            assert outcome.phase == "commit" and outcome.retry_safe is False, outcome
            return
        if outcome.kind == "committed":
            assert count == driver.BULK_ITEMS, attempts
        else:
            # Paused before the commit: the transaction never committed.
            assert outcome.kind == "error" and count == 0, (outcome, attempts)
        delay, ceiling = driver.next_pause(delay, ceiling, outcome, elapsed)
    pytest.fail(f"no pause landed inside the commit: {attempts}")
