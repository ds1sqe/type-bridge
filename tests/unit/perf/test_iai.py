"""gungraun summaries to ``perf-result/v1`` instruction-count metrics.

The ``fixtures/iai`` summaries are hand-built from the gungraun-runner 0.20.0
summary model (``summary/model.rs``, format version 7), not captured from a
run: valgrind was not available where they were written. CI runs the real
benches, and the converter rejects any other summary version.
"""

from __future__ import annotations

import json
import shutil
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

from perf.harness import criterion
from perf.harness.iai import IaiError, convert

FIXTURES = Path(__file__).parent / "fixtures"
SUMMARIES = FIXTURES / "iai"
CORE_SUMMARY = Path(
    "core/type-bridge-core-lib/validation_iai/validate_type_name/type_name.simple/summary.json"
)

type Summary = dict[str, Any]


def _copy_core(tmp_path: Path, mutate: Callable[[Summary], None] | None = None) -> Path:
    root = tmp_path / "iai"
    shutil.copytree(SUMMARIES / "core", root / "core")
    if mutate is not None:
        path = root / CORE_SUMMARY
        summary = json.loads(path.read_text(encoding="utf-8"))
        mutate(summary)
        path.write_text(json.dumps(summary), encoding="utf-8")
    return root


def _ir(summary: Summary) -> Summary:
    return summary["profiles"][0]["data"]["total"]["metrics"]["Ir"]


def test_summaries_convert() -> None:
    metrics = convert(SUMMARIES)

    assert [(m.id, m.unit, m.value) for m in metrics] == [
        ("micro.core.validate_type_name.simple.instructions", "instructions", 1284),
        ("micro.orm.serde_attribute_value.string_roundtrip.instructions", "instructions", 2617),
    ]
    assert {m.family for m in metrics} == {"micro"}


def test_ids_pair_with_the_criterion_ids() -> None:
    [criterion_metric] = criterion.convert(FIXTURES / "criterion")
    core = next(m for m in convert(SUMMARIES) if m.id.startswith("micro.core."))

    assert core.id == f"{criterion_metric.id}.instructions"


def test_function_name_names_a_bench_without_id(tmp_path: Path) -> None:
    def drop_id(summary: Summary) -> None:
        del summary["id"]

    [metric] = convert(_copy_core(tmp_path, drop_id))

    assert metric.id == "micro.core.validate_type_name.type_name.instructions"


def test_other_tools_are_ignored(tmp_path: Path) -> None:
    def prepend_dhat(summary: Summary) -> None:
        summary["profiles"].insert(0, {"tool": "DHAT", "data": {}})

    [metric] = convert(_copy_core(tmp_path, prepend_dhat))

    assert metric.value == 1284


def _set_version(summary: Summary) -> None:
    summary["version"] = "6"


def _drop_callgrind(summary: Summary) -> None:
    summary["profiles"] = []


def _drop_ir(summary: Summary) -> None:
    del summary["profiles"][0]["data"]["total"]["metrics"]["Ir"]


def _ir_float(summary: Summary) -> None:
    _ir(summary)["values"]["new"] = 12.5


def _ir_bool(summary: Summary) -> None:
    _ir(summary)["values"]["new"] = True


def _old_only(summary: Summary) -> None:
    _ir(summary)["values"] = {"old": 1200}


def _no_group(summary: Summary) -> None:
    del summary["group"]


@pytest.mark.parametrize(
    ("mutate", "message"),
    [
        (_set_version, "summary version '6', expected '7'"),
        (_drop_callgrind, "no Callgrind profile"),
        (_drop_ir, "no new Ir total"),
        (_old_only, "no new Ir total"),
        (_ir_float, "Ir is not a count"),
        (_ir_bool, "Ir is not a count"),
        (_no_group, "no group or benchmark name"),
    ],
    ids=["version", "no callgrind", "no ir", "old only", "float ir", "bool ir", "no group"],
)
def test_malformed_summary_is_rejected(
    tmp_path: Path, mutate: Callable[[Summary], None], message: str
) -> None:
    with pytest.raises(IaiError, match=message):
        convert(_copy_core(tmp_path, mutate))


def test_empty_or_missing_root_is_rejected(tmp_path: Path) -> None:
    with pytest.raises(IaiError, match="not a directory"):
        convert(tmp_path / "absent")
    (tmp_path / "core").mkdir()
    with pytest.raises(IaiError, match="no gungraun summaries"):
        convert(tmp_path)
