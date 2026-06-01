"""Descriptor snapshot helpers for Phase 5 cross-language parity."""

from __future__ import annotations

import difflib
import json
import os
import shutil
import subprocess
from pathlib import Path
from typing import Any

import pytest

from tests.integration.parity.canonical import (
    canonical_json,
    normalize_descriptor_snapshot,
)
from tests.integration.parity.models import PARITY_ENTITIES, PARITY_RELATIONS
from type_bridge._rust_runtime import descriptor_for_model

REPO_ROOT = Path(__file__).resolve().parents[3]
NODE_SNAPSHOT_SCRIPT = Path(__file__).with_name("node_descriptor_snapshot.cjs")
DEFAULT_NODE_NATIVE = REPO_ROOT / "tmp" / "type_bridge_node.node"


def python_descriptor_snapshot() -> dict[str, Any]:
    return normalize_descriptor_snapshot(
        {
            "version": 1,
            "entities": [descriptor_for_model(model) for model in PARITY_ENTITIES],
            "relations": [descriptor_for_model(model) for model in PARITY_RELATIONS],
        }
    )


def node_descriptor_snapshot() -> dict[str, Any]:
    if shutil.which("node") is None:
        pytest.skip("node is required for the Node descriptor snapshot")

    env = os.environ.copy()
    if "TYPE_BRIDGE_NODE_NATIVE_PATH" not in env and DEFAULT_NODE_NATIVE.exists():
        env["TYPE_BRIDGE_NODE_NATIVE_PATH"] = str(DEFAULT_NODE_NATIVE)

    completed = subprocess.run(
        ["node", str(NODE_SNAPSHOT_SCRIPT)],
        check=False,
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
    )
    if completed.returncode != 0:
        if "Unable to load the type-bridge native Node module" in completed.stderr:
            pytest.skip(completed.stderr.strip())
        raise AssertionError(completed.stderr.strip() or completed.stdout.strip())

    return normalize_descriptor_snapshot(json.loads(completed.stdout))


def assert_descriptor_snapshots_equal(
    actual: dict[str, Any],
    expected: dict[str, Any],
    *,
    actual_name: str,
    expected_name: str,
) -> None:
    actual_json = canonical_json(actual)
    expected_json = canonical_json(expected)
    if actual_json == expected_json:
        return

    diff = "".join(
        difflib.unified_diff(
            expected_json.splitlines(keepends=True),
            actual_json.splitlines(keepends=True),
            fromfile=expected_name,
            tofile=actual_name,
        )
    )
    raise AssertionError(f"descriptor snapshot drift:\n{diff}")
