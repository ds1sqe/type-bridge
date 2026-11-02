"""Shared SDK deadline fixture's Python consumer (real TypeDB, direct/proxy)."""

from __future__ import annotations

import json
import os
import time
from pathlib import Path

import pytest
from type_bridge_core import (
    ProxyConnection,
    PyDatabase,
    QueryExecutionResourceLimits,
    query_v2_authority,
)

from .test_staged_plan_builder import _purchase

FIXTURE_DIR = os.environ.get("TB_DEADLINE_FIXTURE_DIR")
pytestmark = [
    pytest.mark.integration,
    pytest.mark.proxy,
    pytest.mark.skipif(not FIXTURE_DIR, reason="requires the SDK deadline live fixture"),
]


@pytest.mark.parametrize("transport", ["direct", "proxy"])
def test_deadline_none_and_typed_timeout(transport: str) -> None:
    assert FIXTURE_DIR is not None
    live = json.loads((Path(FIXTURE_DIR) / "fixture.json").read_text())
    entry = live["entries"][f"python-{transport}"]
    authority = query_v2_authority(
        bytes.fromhex(live["declared_hex"]), entry["scope"], live["profile"]
    )
    plan = _purchase(authority, "O1")
    arm = Path(entry["arm"])
    for timeout, delay in [(1_000, 2_000), (0, 61_000)]:
        db = None
        if transport == "direct":
            db = PyDatabase.connect(entry["address"], entry["database"], http_port=43131)
            tx = db.staged_transaction(timeout_milliseconds=timeout, collect_phase_timestamps=True)
        else:
            connection = ProxyConnection.connect(
                authority,
                entry["endpoint"],
                answer_limits=QueryExecutionResourceLimits(
                    timeout_milliseconds=timeout, collect_phase_timestamps=True
                ),
            )
            tx = connection.transaction()
        try:
            pending = tx.stage(plan)
            arm.write_text(str(delay))
            started = time.monotonic()
            if timeout:
                with pytest.raises(Exception) as raised:
                    tx.commit()
                error = raised.value
                assert getattr(error, "code", None) == "transaction_deadline_exceeded"
                timeline = getattr(error, "details", {})["execution_timeline"]["value"]
                stamps = timeline["stamps"]
                assert stamps[0]["phase"] == "fired"
                assert stamps[-1]["phase"] == "failed"
                assert stamps[-1]["failing_phase"] == (
                    "fired" if transport == "direct" else "accepted"
                ), getattr(error, "details", {})
                assert not any(stamp["phase"] == "db_write" for stamp in stamps)
                with pytest.raises(Exception):
                    pending.result()
                # Let the deliberately held pre-open send settle before the next call.
                time.sleep(2.1)
            else:
                tx.commit()
                assert time.monotonic() - started > 60
                # The timed-out purchase changed nothing: this purchase still matches.
                assert pending.result().count == 1
            assert not arm.exists(), "the test-only relay must actually delay the call"
        finally:
            arm.unlink(missing_ok=True)
            tx.close()
            if db is not None:
                db.close()
