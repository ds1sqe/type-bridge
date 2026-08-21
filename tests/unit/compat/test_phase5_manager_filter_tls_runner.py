"""Provider-free checks for ordered Python/Node custom-root TLS parity."""

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
CI = ROOT / "scripts/ci"
RUNNER = CI / "run_phase5_manager_filter_tls.py"


def load_runner():
    sys.path.insert(0, str(CI))
    spec = importlib.util.spec_from_file_location("phase5_manager_filter_tls_runner", RUNNER)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_tls_runner_selects_only_ordered_python_and_node_commands() -> None:
    runner = load_runner()

    assert runner.SELECTED_COMMANDS == {
        "install the Python native facade",
        "generate the local Python package",
        "generate the foreign Python package",
        "produce the Python report",
        "build the Node package",
        "generate the local TypeScript package",
        "generate the foreign TypeScript package",
        "compile the local TypeScript package",
        "compile the foreign TypeScript package",
        "produce the Node report",
    }


def test_tls_report_requires_cross_binding_semantic_identity(tmp_path: Path) -> None:
    runner = load_runner()
    common = {
        "authority": {"schema": {"sha256": "a"}},
        "format": "typebridge.phase5-manager-filter-live-report/v1",
        "observation": {"terminals": {"count": 2}},
        "semantic_profile": "typedb-3.12.1/v1",
    }
    python_path = tmp_path / "python.json"
    node_path = tmp_path / "node.json"
    python_path.write_text(json.dumps({**common, "binding": "python"}), encoding="utf-8")
    node_path.write_text(json.dumps({**common, "binding": "node"}), encoding="utf-8")

    summary = json.loads(runner._parity_report(python_path, node_path))
    assert summary == {
        "bindings": ["node", "python"],
        "format": "typebridge.phase5-manager-filter-tls-summary/v1",
        "semantic_profile": "typedb-3.12.1/v1",
        "status": "passed",
        "transport": "custom_root_tls",
        "wrong_trust": "rejected",
    }

    node_path.write_text(
        json.dumps({**common, "binding": "node", "observation": {"terminals": {"count": 3}}}),
        encoding="utf-8",
    )
    with pytest.raises(runner.live.RunnerError, match="not semantically identical"):
        runner._parity_report(python_path, node_path)


def test_tls_fixture_rejects_missing_or_non_regular_root(tmp_path: Path) -> None:
    runner = load_runner()
    environment = {
        runner.TLS_ADDRESS_ENV: "127.0.0.1:1729",
        runner.TLS_HTTP_PORT_ENV: "8000",
    }
    with pytest.raises(runner.live.RunnerError, match=runner.TLS_ROOT_CA_ENV):
        runner._root_ca(environment)
    with pytest.raises(runner.live.RunnerError, match="regular non-symlink"):
        runner._root_ca({**environment, runner.TLS_ROOT_CA_ENV: str(tmp_path)})
