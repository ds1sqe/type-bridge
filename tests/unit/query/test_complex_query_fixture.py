"""Independent fixture witnesses must keep every complex-query predicate observable."""

from __future__ import annotations

import importlib.util
from pathlib import Path

SOURCE = Path(__file__).parents[3] / "tests/live/query_complex/fixture.py"
spec = importlib.util.spec_from_file_location("complex_fixture", SOURCE)
assert spec is not None and spec.loader is not None
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


def test_every_predicate_has_an_independent_witness_at_every_join_width() -> None:
    for size in (1, 2, 16):
        rows = fixture.records(size)
        assert len(rows) == size + len(fixture.PREDICATE_WIDTH)
        for width in (2, 4, 6, 8, 10, 12):
            baseline = sum(fixture.matches(row, width) for row in rows)
            for predicate in fixture.raw_predicates(width):
                assert sum(fixture.matches(row, width, predicate) for row in rows) > baseline
                assert fixture.width_query(width, predicate) != fixture.width_query(width)


def test_original_oracle_keeps_its_distinct_known_answer() -> None:
    assert fixture.expected_rows(16) == [["P0002", "job-0002"], ["P0008", "job-0008"]]
    assert len(fixture.original_expected_rows(16)) == 5
    assert fixture.original_expected_rows(16) != fixture.expected_rows(16)
