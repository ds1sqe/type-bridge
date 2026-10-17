"""The `writes` comparator holds four binding reports to one staged program."""

from __future__ import annotations

import copy
import importlib.util
import json
import sys
from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
CI = ROOT / "scripts/ci"
sys.path.insert(0, str(CI))
SPEC = importlib.util.spec_from_file_location("sdk_writes_compare", CI / "sdk_writes_compare.py")
assert SPEC is not None and SPEC.loader is not None
COMPARE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = COMPARE
SPEC.loader.exec_module(COMPARE)

PROGRAM, EXPECTED = COMPARE.load_program()


def report(binding: str, *, live: bool) -> dict[str, Any]:
    """The report a correct program run gives for `binding`."""
    return {
        "format": COMPARE.REPORT_FORMAT,
        "binding": binding,
        "program": "purchase",
        "semantic_profile": EXPECTED["semantic_profile"],
        "plans": dict(EXPECTED["plans"]),
        "results": (
            {transport: copy.deepcopy(EXPECTED["results"]) for transport in COMPARE.TRANSPORTS}
            if live
            else {}
        ),
        "observed": (
            {
                "proxy_commits": list(EXPECTED["proxy_commits"]),
                "direct_state": dict(EXPECTED["state"]),
                "proxy_state": dict(EXPECTED["state"]),
            }
            if live
            else None
        ),
    }


def write(tmp_path: Path, reports: list[dict[str, Any]]) -> list[Path]:
    paths = []
    for value in reports:
        path = tmp_path / f"{value['binding']}.json"
        path.write_bytes(COMPARE.canonical_json_bytes(value))
        paths.append(path)
    return paths


def test_the_program_contract_names_every_step_plan_and_answer() -> None:
    steps = {step["name"]: step["plans"] for step in PROGRAM["steps"]}
    assert steps == {
        "seed": ["seed"],
        "program": ["purchase", "touch", "batch"],
        "read": ["sold_out", "charged", "orders", "on_call"],
        "journeys": ["put_doctor", "returns", "refer", "remove"],
        "after": ["doctors_on_call", "referred", "removed"],
        "rollback": ["kept"],
    }
    # The rollback step stages ghost, rolls it back, then stages kept.
    rolled_back = {step["name"]: step.get("rolled_back", []) for step in PROGRAM["steps"]}
    assert {step: plans for step, plans in rolled_back.items() if plans} == {"rollback": ["ghost"]}
    assert {"ghost", "kept"} <= set(EXPECTED["plans"])
    assert EXPECTED["results"]["rollback"] == {"kept": ["matched", 1]}
    assert EXPECTED["results"]["program"] == {
        "purchase": ["matched", 1],
        "touch": ["matched", 1],
        "batch": ["matched", 50],
    }
    # Put, update answering rows, insert with inserted players, delete.
    assert EXPECTED["results"]["journeys"] == {
        "put_doctor": ["matched", 1],
        "returns": ["rows", 1],
        "refer": ["matched", 1],
        "remove": ["matched", 1],
    }
    assert EXPECTED["results"]["after"] == {
        "doctors_on_call": ["count", 2],
        "referred": ["count", 1],
        "removed": ["exists", 0],
    }
    assert len(PROGRAM["plans"]["batch"]["rows"]) == 50
    # 1 purchase + 50 batch orders + R1 - O1.
    assert EXPECTED["state"]["orders"] == 51
    # The rolled-back ghost (D3, on call) left no state.
    assert EXPECTED["state"]["on_call"] == 2
    # Seed, program, read, journeys, after and rollback: one signed commit
    # each; the rollback itself sends nothing.
    assert EXPECTED["proxy_commits"] == ["true:ok"] * 6


@pytest.mark.parametrize("live", [False, True])
def test_four_matching_reports_compare_in_any_order(tmp_path: Path, live: bool) -> None:
    paths = write(tmp_path, [report(binding, live=live) for binding in COMPARE.BINDINGS])
    summary = COMPARE.compare_reports(list(reversed(paths)), live=live)
    assert summary["bindings"] == list(COMPARE.BINDINGS)
    assert summary["transports"] == (["direct", "proxy"] if live else [])
    assert set(summary["plan_sha256"]) == set(EXPECTED["plans"])
    assert set(summary["report_sha256"]) == set(COMPARE.BINDINGS)


def _set(path: tuple[str, ...], value: Any) -> Callable[[dict[str, Any]], None]:
    def mutate(item: dict[str, Any]) -> None:
        target = item
        for key in path[:-1]:
            target = target[key]
        target[path[-1]] = value

    return mutate


def _flip_plan(item: dict[str, Any]) -> None:
    encoded = item["plans"]["batch"]
    item["plans"]["batch"] = encoded[:-2] + ("00" if encoded[-2:] != "00" else "01")


@pytest.mark.parametrize(
    ("mutate", "code"),
    [
        (_flip_plan, "plan_bytes_mismatch"),
        (_set(("results", "proxy", "program", "batch"), ["matched", 49]), "answer_mismatch"),
        (_set(("results", "direct", "read", "on_call"), ["exists", 0]), "answer_mismatch"),
        (lambda item: item["results"].pop("proxy"), "invalid_object_fields"),
        (_set(("observed", "proxy_state", "orders"), 50), "state_mismatch"),
        (_set(("observed", "proxy_commits"), ["true:ok"] * 2), "proxy_commits_mismatch"),
        (_set(("semantic_profile",), "typedb-3.12.1/v1"), "semantic_profile_mismatch"),
        (lambda item: item["plans"].pop("touch"), "invalid_object_fields"),
        (_set(("format",), "typebridge.sdk-writes-report/v0"), "invalid_report_format"),
    ],
)
def test_one_divergent_binding_rejects_the_live_set(
    tmp_path: Path, mutate: Callable[[dict[str, Any]], None], code: str
) -> None:
    reports = [report(binding, live=True) for binding in COMPARE.BINDINGS]
    mutate(reports[3])
    with pytest.raises(COMPARE.ContractError) as error:
        COMPARE.compare_reports(write(tmp_path, reports), live=True)
    assert error.value.code == code


def test_an_offline_report_set_cannot_pass_as_live(tmp_path: Path) -> None:
    paths = write(tmp_path, [report(binding, live=False) for binding in COMPARE.BINDINGS])
    with pytest.raises(COMPARE.ContractError) as error:
        COMPARE.compare_reports(paths, live=True)
    assert error.value.code == "invalid_object_fields"


def test_a_duplicate_binding_and_noncanonical_json_are_refused(tmp_path: Path) -> None:
    reports = [report(binding, live=False) for binding in ("python", "node", "rust", "rust")]
    paths = [tmp_path / f"{index}.json" for index in range(4)]
    for path, value in zip(paths, reports, strict=True):
        path.write_bytes(COMPARE.canonical_json_bytes(value))
    with pytest.raises(COMPARE.ContractError) as error:
        COMPARE.compare_reports(paths, live=False)
    assert error.value.code == "incomplete_binding_set"
    paths[0].write_text(json.dumps(reports[0], indent=2))
    with pytest.raises(COMPARE.ContractError) as error:
        COMPARE.compare_reports(paths, live=False)
    assert error.value.code == "noncanonical_report_json"


def test_the_cli_requires_live_results_only_when_asked(tmp_path: Path) -> None:
    paths = [
        str(path) for path in write(tmp_path, [report(b, live=False) for b in COMPARE.BINDINGS])
    ]
    assert COMPARE.main(paths) == 0
    assert COMPARE.main(["--live", *paths]) == 1


def test_every_writes_suite_contract_is_consistent() -> None:
    program, expected = COMPARE.load_program("constraints")
    assert [step["name"] for step in program["steps"]] == ["seed", "reject"]
    # The duplicate key is rejected while the commit runs the staged insert.
    assert expected["results"]["reject"]["duplicate_doctor"] == [
        "failure",
        {"code": "CNT9", "phase": "query", "reason": "unique", "retry_safe": True},
    ]


def test_reports_naming_no_single_program_are_refused(tmp_path: Path) -> None:
    paths = []
    for binding, program in zip(
        COMPARE.BINDINGS, ("purchase", "constraints", "purchase", "purchase")
    ):
        value = report(binding, live=False) | {"program": program}
        path = tmp_path / f"{binding}.json"
        path.write_bytes(COMPARE.canonical_json_bytes(value))
        paths.append(path)
    with pytest.raises(COMPARE.ContractError, match="no single program"):
        COMPARE.compare_reports(paths, live=False)
