"""Retained TypeDB driver-band and live-matrix coherence."""

from __future__ import annotations

import re
import tomllib
from pathlib import Path
from typing import Any

import pytest
import type_bridge_core
import yaml

REPO_ROOT = Path(__file__).resolve().parents[3]
RUNTIME_MANIFEST = REPO_ROOT / "type-bridge-core/crates/typedb-runtime/Cargo.toml"
WORKSPACE_MANIFEST = REPO_ROOT / "type-bridge-core/Cargo.toml"
CI_WORKFLOW = REPO_ROOT / ".github/workflows/ci.yml"

EXPECTED_DRIVERS = {8: "3.11.5", 9: "3.12.1"}
EXPECTED_SERVERS = {"typedb/typedb:3.11.5", "typedb/typedb:3.12.1"}


def _ci_jobs() -> dict[str, Any]:
    payload = yaml.safe_load(CI_WORKFLOW.read_text(encoding="utf-8"))
    return payload["jobs"]


def test_native_runtime_exposes_exactly_the_retained_driver_pins() -> None:
    assert type_bridge_core.embedded_driver_versions() == EXPECTED_DRIVERS
    assert type_bridge_core.embedded_driver_version() == EXPECTED_DRIVERS[8]


@pytest.mark.parametrize("server", ["3.8.3", "3.10.4"])
def test_retired_server_lines_fail_the_native_gate(server: str) -> None:
    with pytest.raises(type_bridge_core.VersionError):
        type_bridge_core.check_server_supported(server)


@pytest.mark.parametrize("server", ["3.11.5", "3.12.1"])
def test_retained_server_lines_pass_the_native_gate(server: str) -> None:
    type_bridge_core.check_server_supported(server)


def test_runtime_manifest_has_only_band8_and_band9_features() -> None:
    runtime = tomllib.loads(RUNTIME_MANIFEST.read_text(encoding="utf-8"))
    features = runtime["features"]
    dependencies = runtime["dependencies"]

    assert features["default"] == ["band8", "band9"]
    assert set(features) == {"default", "band8", "band9"}
    assert dependencies["type-bridge-typedb-driver-b8"]["version"] == "=3.11.5"
    assert dependencies["typedb-driver"]["version"] == "=3.12.1"
    assert all("b7" not in name and "band7" not in name for name in dependencies)


def test_workspace_contains_only_the_active_namespaced_vendor_band() -> None:
    workspace = tomllib.loads(WORKSPACE_MANIFEST.read_text(encoding="utf-8"))["workspace"]
    members = set(workspace["members"])

    assert "vendor/typedb-driver-b8" in members
    assert "vendor/typedb-protocol-b8" in members
    assert not any("b7" in member for member in members)
    assert not (REPO_ROOT / "type-bridge-core/vendor/typedb-driver-b7").exists()
    assert not (REPO_ROOT / "type-bridge-core/vendor/typedb-protocol-b7").exists()


def test_single_feature_ci_compiles_each_retained_band() -> None:
    include = _ci_jobs()["band-feature-check"]["strategy"]["matrix"]["include"]
    assert include == [
        {
            "band": "band8",
            "features": "band8,v2-query",
            "required_driver": "type-bridge-typedb-driver-b8 v3.11.5",
            "forbidden_driver": "typedb-driver v3.12.1",
        },
        {
            "band": "band9",
            "features": "band9,v2-query",
            "required_driver": "typedb-driver v3.12.1",
            "forbidden_driver": "type-bridge-typedb-driver-b8 v3.11.5",
        },
    ]


@pytest.mark.parametrize(
    "job_name",
    ["test-integration", "rust-integration", "node-integration"],
)
def test_positive_live_matrices_contain_only_retained_servers(job_name: str) -> None:
    matrix = _ci_jobs()[job_name]["strategy"]["matrix"]
    assert set(matrix["typedb-server"]) == EXPECTED_SERVERS


def test_python_live_matrix_pairs_each_server_with_its_driver() -> None:
    job = _ci_jobs()["test-integration"]
    include = job["strategy"]["matrix"]["include"]
    assert include == [
        {
            "typedb-server": "typedb/typedb:3.11.5",
            "python-driver": "3.11.5",
        },
        {
            "typedb-server": "typedb/typedb:3.12.1",
            "python-driver": "3.12.1",
        },
    ]
    workflow_text = CI_WORKFLOW.read_text(encoding="utf-8")
    assert "TYPE_BRIDGE_EXPECT_LEGACY_WARNING" not in workflow_text
    assert "legacy-warning:" not in workflow_text


def test_tls_matrix_covers_exactly_the_retained_topologies() -> None:
    include = _ci_jobs()["tls-transport-matrix"]["strategy"]["matrix"]["include"]
    assert include == [
        {
            "lane": "band8-packaging",
            "typedb-server": "typedb/typedb:3.11.5",
            "server-version": "3.11.5",
            "driver-band": "8",
            "driver-version": "3.11.5",
        },
        {
            "lane": "band9-upstream",
            "typedb-server": "typedb/typedb:3.12.1",
            "server-version": "3.12.1",
            "driver-band": "9",
            "driver-version": "3.12.1",
        },
    ]


def test_gate_matrix_keeps_retired_lines_negative_only() -> None:
    include = _ci_jobs()["version-gate-cells"]["strategy"]["matrix"]["include"]
    retired_cells = [
        cell
        for cell in include
        if "3.10." in cell["typedb-server"] or str(cell["python-driver"]).startswith("3.10.")
    ]
    assert retired_cells
    assert all(str(cell["cell"]).startswith("NEG-") for cell in retired_cells)
    assert any(
        cell["typedb-server"] == "typedb/typedb:3.10.4"
        and cell["probe"] == "connect"
        and cell["expect"] == "window"
        for cell in retired_cells
    )


def test_compose_and_local_harness_default_to_the_band9_server() -> None:
    expected = "typedb/typedb:3.12.1"
    for name in ("docker-compose.yml", "docker-compose.proxy.yml"):
        text = (REPO_ROOT / name).read_text(encoding="utf-8")
        match = re.search(r"\$\{TYPEDB_IMAGE:-(typedb/typedb:[\w.\-]+)\}", text)
        assert match is not None
        assert match.group(1) == expected

    assert (REPO_ROOT / "test.sh").read_text(encoding="utf-8").count(
        "${TYPEDB_IMAGE:-typedb/typedb:3.12.1}"
    ) == 2


def test_optional_python_driver_groups_exclude_retired_lines() -> None:
    groups = tomllib.loads((REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8"))["project"][
        "optional-dependencies"
    ]
    expected = {
        "dev": {
            "typedb-driver~=3.11.5; python_version < '3.14'",
            "typedb-driver==3.12.1; python_version >= '3.14'",
        },
        "typedb-driver": {
            "typedb-driver>=3.11,<3.13; python_version < '3.14'",
            "typedb-driver==3.12.1; python_version >= '3.14'",
        },
    }
    for group, requirements in expected.items():
        actual = {item for item in groups[group] if item.startswith("typedb-driver")}
        assert actual == requirements
