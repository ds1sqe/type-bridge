# pyright: reportMissingImports=false
"""Unit tests for the _ir_dump subprocess entrypoint (Phase 2, sub-plan 08).

Covers:
- Subprocess smoke: import a tiny temp models module, capture stdout JSON,
  assert exit code 0 and that the JSON is a SchemaInfo dict with the
  expected entity/attribute names.
- Error path: exit code 1 on a non-existent module with a stderr diagnostic.

These tests run without a live TypeDB connection.  The success test needs the
Rust extension (type_bridge_core) and is guarded by ``pytest.importorskip``.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# The dotted package name used to expose the temp module to the subprocess.
_TEMP_PKG_NAME = "_ir_dump_test_models"

# Minimal models module content: one Entity with one String attribute.
# Uses the same authoring API as the rest of the unit test suite
# (Entity / String / Flag / Key / TypeFlags / AttributeFlags).
_MODELS_MODULE_CONTENT = """\
from type_bridge import Entity, Flag, Key, String, TypeFlags
from type_bridge.attribute import AttributeFlags


class IrDumpName(String):
    flags = AttributeFlags(name="ir-dump-name")


class IrDumpPerson(Entity):
    flags = TypeFlags(name="ir-dump-person")

    name: IrDumpName = Flag(Key)
"""


def _write_temp_models(tmp_path: Path) -> tuple[Path, str]:
    """Write the temp models module and return (tmp_path, dotted_module_name).

    The module is placed at ``<tmp_path>/_ir_dump_test_models.py`` so that
    adding *tmp_path* to PYTHONPATH makes it importable as
    ``_ir_dump_test_models``.
    """
    module_file = tmp_path / f"{_TEMP_PKG_NAME}.py"
    module_file.write_text(_MODELS_MODULE_CONTENT)
    return tmp_path, _TEMP_PKG_NAME


# ---------------------------------------------------------------------------
# Success case — needs the Rust extension for model_schema_info()
# ---------------------------------------------------------------------------


def test_ir_dump_success_returns_schema_info_json(tmp_path: Path) -> None:
    """_ir_dump emits a valid SchemaInfo JSON dict on exit 0 for a real models module.

    SchemaInfo shape (from test_schema_ir.py / descriptor_registry.schema_info()):
        {
            "entities":   { <type_name>: { "is_abstract": bool, "owned_attributes": [...], ... }, ... },
            "relations":  { ... },
            "attributes": { <attr_name>: { ... }, ... },
        }
    """
    pytest.importorskip("type_bridge_core")

    models_dir, module_name = _write_temp_models(tmp_path)

    env = os.environ.copy()
    # Prepend tmp_path to PYTHONPATH so the temp module is importable.
    existing_pypath = env.get("PYTHONPATH", "")
    env["PYTHONPATH"] = str(models_dir) + (os.pathsep + existing_pypath if existing_pypath else "")

    result = subprocess.run(
        [sys.executable, "-m", "type_bridge.migration._ir_dump", module_name],
        capture_output=True,
        text=True,
        env=env,
        # Run from the project root so type_bridge itself is importable via the
        # installed editable package (already in sys.path via the venv).
    )

    # Expect clean exit.
    assert result.returncode == 0, f"_ir_dump exited {result.returncode}; stderr:\n{result.stderr}"

    # stdout must be a single JSON document (no extra noise).
    stdout = result.stdout.strip()
    assert stdout, "_ir_dump produced no output on stdout"

    schema_info: dict[str, Any] = json.loads(stdout)
    assert isinstance(schema_info, dict), "stdout JSON must be a dict (SchemaInfo)"

    # Verify the top-level SchemaInfo keys are present.
    assert "entities" in schema_info, "SchemaInfo must contain 'entities'"
    assert "attributes" in schema_info, "SchemaInfo must contain 'attributes'"

    # Verify the entity and attribute registered by our tiny models module
    # appear in the output.
    entities: dict[str, Any] = schema_info["entities"]
    assert "ir-dump-person" in entities, (
        f"'ir-dump-person' entity not found; entities present: {list(entities)}"
    )

    attributes: dict[str, Any] = schema_info["attributes"]
    assert "ir-dump-name" in attributes, (
        f"'ir-dump-name' attribute not found; attributes present: {list(attributes)}"
    )


# ---------------------------------------------------------------------------
# Error path — no Rust extension required; the module is missing entirely.
# ---------------------------------------------------------------------------


def test_ir_dump_exit_1_on_nonexistent_module() -> None:
    """_ir_dump must exit 1 with a stderr diagnostic for a non-existent module."""
    result = subprocess.run(
        [sys.executable, "-m", "type_bridge.migration._ir_dump", "no.such.module.xyz"],
        capture_output=True,
        text=True,
    )

    assert result.returncode == 1, (
        f"expected exit code 1 for missing module; got {result.returncode}"
    )

    # A diagnostic must appear on stderr (not stdout).
    assert result.stderr.strip(), "_ir_dump must write a diagnostic to stderr on import error"
    # stdout must be empty (no partial JSON).
    assert result.stdout.strip() == "", (
        f"_ir_dump must not write to stdout on import error; got: {result.stdout!r}"
    )
