"""Fail-closed tests for Python's provider-free Phase-2 report producer."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from types import ModuleType

import pytest

ROOT = Path(__file__).resolve().parents[3]
PRODUCER_PATH = (
    ROOT / "type-bridge-core/crates/schema-codegen/tests/acceptance/phase2_parity_check.py"
)
ACCEPTANCE_PATH = ROOT / "type-bridge-core/crates/schema-codegen/tests/acceptance/check.py"


def _load_producer() -> ModuleType:
    spec = importlib.util.spec_from_file_location("python_phase2_parity_producer", PRODUCER_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


producer = _load_producer()


def test_publisher_is_canonical_create_new_and_bounded(tmp_path: Path) -> None:
    report = {"binding": "python", "observed": {"value": 38}}
    output = tmp_path / "python.json"

    producer._publish_report(output, report)

    assert output.read_bytes() == producer._canonical(report)
    with pytest.raises(producer.ProducerError) as exists:
        producer._publish_report(output, report)
    assert exists.value.code == "output_exists"

    oversized = {"value": "x" * producer.MAX_REPORT_BYTES}
    with pytest.raises(producer.ProducerError) as bounded:
        producer._publish_report(tmp_path / "oversized.json", oversized)
    assert bounded.value.code == "report_size_limit"


def test_paths_and_authority_files_reject_symlinks(tmp_path: Path) -> None:
    with pytest.raises(producer.ProducerError) as relative:
        producer._output_path({producer.OUTPUT_ENV: "relative.json"})
    assert relative.value.code == "invalid_output_path"

    real_parent = tmp_path / "real"
    real_parent.mkdir()
    linked_parent = tmp_path / "linked"
    linked_parent.symlink_to(real_parent, target_is_directory=True)
    with pytest.raises(producer.ProducerError) as linked_output:
        producer._publish_report(linked_parent / "report.json", {})
    assert linked_output.value.code == "invalid_output_parent"

    authority = tmp_path / "authority.json"
    authority.write_text("{}")
    linked_authority = tmp_path / "linked-authority.json"
    linked_authority.symlink_to(authority)
    with pytest.raises(producer.ProducerError) as linked_source:
        producer._source_identity(tmp_path, linked_authority.name, "linked authority")
    assert linked_source.value.code == "invalid_authority"


def test_acceptance_runs_real_producer_then_committed_comparator() -> None:
    acceptance = ACCEPTANCE_PATH.read_text()
    producer_source = PRODUCER_PATH.read_text()

    producer_call = 'str(STAGE / "phase2_parity_check.py")'
    comparator_call = '"scripts/ci/compare_phase2_projection_parity.py"'
    assert acceptance.index(producer_call) < acceptance.index(comparator_call)
    assert "module._load_report" in acceptance
    assert "expected_report" not in producer_source
    assert "compare_phase2_projection_parity" not in producer_source
    assert "journey-v3.json" in producer_source
    assert "json.loads(_bounded_regular_bytes" not in producer_source
