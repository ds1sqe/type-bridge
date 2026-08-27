"""Historical-inventory and orchestration tests for the Workforce V6 runner."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
CI = ROOT / "scripts/ci"
sys.path.insert(0, str(CI))
SPEC = importlib.util.spec_from_file_location(
    "workforce_v6_candidate_runner", CI / "run_workforce_v6_candidate.py"
)
assert SPEC is not None and SPEC.loader is not None
RUNNER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = RUNNER
SPEC.loader.exec_module(RUNNER)


def _reports(root: Path) -> None:
    for binding in RUNNER.BINDINGS:
        for version in RUNNER.conformance.predecessor_versions(binding):
            path = root / f"v{version}" / f"{binding}.json"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("{}", encoding="utf-8")


def test_historical_inventory_does_not_invent_c_v1(tmp_path: Path) -> None:
    _reports(tmp_path)
    assert [version for version, _ in RUNNER.predecessor_paths(tmp_path, "rust")] == [1, 2, 3, 4, 5]
    assert [version for version, _ in RUNNER.predecessor_paths(tmp_path, "c")] == [2, 3, 4, 5]
    assert not (tmp_path / "v1/c.json").exists()


def test_missing_real_predecessor_fails_closed(tmp_path: Path) -> None:
    _reports(tmp_path)
    (tmp_path / "v4/c.json").unlink()
    with pytest.raises(RUNNER.RunnerError, match="V4 c report"):
        RUNNER.predecessor_paths(tmp_path, "c")


def test_c_consumer_is_bound_only_to_phase4_artifacts() -> None:
    phase4 = {
        "artifacts": {
            "cli": {"candidate-id": f"sha256:{'1' * 64}"},
            "generated-package": {"sha256": "2" * 64},
        }
    }
    report = RUNNER.c_surface_consumer(phase4, source_commit="3" * 40)
    assert report["surface-sha256"] == "2" * 64
    assert report["cli-candidate-id"] == f"sha256:{'1' * 64}"
    assert report["runtime-provenance"] == "candidate-c-runtime"
    assert report["publication-authority"] is False


def test_runner_uses_one_surface_build_four_compositions_and_one_comparator() -> None:
    source = (CI / "run_workforce_v6_candidate.py").read_text(encoding="utf-8")
    assert '["uv", "run", "python"] if binding == "python"' in source
    assert source.count('"scripts/ci/workforce_v6_surfaces.py"') == 1
    assert source.count('"scripts/ci/compose_workforce_v6_evidence.py"') == 1
    assert source.count('"scripts/ci/assemble_workforce_conformance_v6.py"') == 1
    assert source.count('"scripts/ci/compare_workforce_conformance_v6.py"') == 1
