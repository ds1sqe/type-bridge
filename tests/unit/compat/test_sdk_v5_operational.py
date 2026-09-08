"""Fail-closed tests for Sdk V5 operational evidence comparison."""

from __future__ import annotations

import importlib.util
import json
from collections.abc import Callable
from copy import deepcopy
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "scripts/ci/compare_sdk_v5_operational.py"
SPEC = importlib.util.spec_from_file_location("sdk_v5_operational", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
operational = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(operational)


def evidence(binding: str) -> dict[str, Any]:
    return {
        "binding": binding,
        **deepcopy(operational.EXPECTED),
        "format": operational.FORMAT,
        "test_id": operational.TEST_IDS[binding],
    }


def write(path: Path, value: dict[str, Any]) -> None:
    path.write_bytes(operational.canonical_json_bytes(value))


def paths(tmp_path: Path) -> dict[str, Path]:
    result = {binding: tmp_path / f"{binding}.json" for binding in operational.BINDINGS}
    for binding, path in result.items():
        write(path, evidence(binding))
    return result


def test_accepts_exact_equal_four_binding_operational_evidence(tmp_path: Path) -> None:
    result = operational.compare(paths(tmp_path))
    assert result == {
        "bindings": list(operational.BINDINGS),
        **operational.EXPECTED,
        "format": operational.COMPARISON_FORMAT,
    }


@pytest.mark.parametrize(
    ("mutation", "code"),
    [
        (lambda value: value.update({"binding": "node"}), "operational_identity_mismatch"),
        (lambda value: value.update({"extra": True}), "invalid_operational_shape"),
        (
            lambda value: value["cancellation"].update({"partial_output": True}),
            "operational_observation_drift",
        ),
        (
            lambda value: value["diagnostic"].update({"payload": "secret"}),
            "operational_observation_drift",
        ),
    ],
)
def test_rejects_identity_shape_and_observation_drift(
    tmp_path: Path,
    mutation: Callable[[dict[str, Any]], None],
    code: str,
) -> None:
    value = evidence("python")
    mutation(value)
    path = tmp_path / "python.json"
    write(path, value)
    with pytest.raises(operational.OperationalError) as rejected:
        operational.load_evidence(path, "python")
    assert rejected.value.code == code


def test_rejects_duplicate_noncanonical_symlink_and_binding_set(tmp_path: Path) -> None:
    duplicate = tmp_path / "duplicate.json"
    duplicate.write_text('{"binding":"python","binding":"python"}', encoding="utf-8")
    with pytest.raises(operational.OperationalError) as rejected:
        operational.load_evidence(duplicate, "python")
    assert rejected.value.code == "duplicate_operational_key"

    noncanonical = tmp_path / "noncanonical.json"
    noncanonical.write_text(json.dumps(evidence("python")), encoding="utf-8")
    with pytest.raises(operational.OperationalError) as rejected:
        operational.load_evidence(noncanonical, "python")
    assert rejected.value.code == "noncanonical_operational_json"

    target = tmp_path / "target.json"
    write(target, evidence("python"))
    symlink = tmp_path / "evidence.json"
    symlink.symlink_to(target)
    with pytest.raises(operational.OperationalError) as rejected:
        operational.load_evidence(symlink, "python")
    assert rejected.value.code == "invalid_operational_file"

    incomplete = paths(tmp_path)
    incomplete.pop("c")
    with pytest.raises(operational.OperationalError) as rejected:
        operational.compare(incomplete)
    assert rejected.value.code == "operational_binding_set_mismatch"
