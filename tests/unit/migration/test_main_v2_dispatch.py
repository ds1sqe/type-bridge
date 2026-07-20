# pyright: reportMissingImports=false
"""The console entry point serves V2 workspace verbs from the wheel itself.

Covers:
- ``schema`` / ``migration`` / ``--manifest`` invocations run the in-process
  V2 CLI shipped inside the native extension (no external binary involved)
- a real split-YAML workspace passes ``schema check`` through the entry point
- legacy verbs still take the released subprocess-forwarding path
- exit codes propagate unchanged
"""

from __future__ import annotations

from pathlib import Path

import pytest

from type_bridge.migration.__main__ import _is_v2_invocation, main

pytest.importorskip("type_bridge_core")


def test_v2_invocations_are_detected() -> None:
    assert _is_v2_invocation(["schema", "check"])
    assert _is_v2_invocation(["migration", "make", "--name", "init"])
    assert _is_v2_invocation(["--manifest", "x.yaml", "schema", "check"])
    assert _is_v2_invocation(["--manifest=x.yaml", "schema", "check"])
    assert _is_v2_invocation(["--help"])
    assert _is_v2_invocation(["--version"])


def test_legacy_invocations_still_forward() -> None:
    for argv in (
        ["migrate", "--database", "mydb"],
        ["showmigrations"],
        ["makemigrations", "--name", "add_phone"],
        ["plan"],
        ["sqlmigrate", "0001_initial"],
        [],
    ):
        assert not _is_v2_invocation(argv)


def _write_workspace(root: Path) -> Path:
    (root / "schema/fragments").mkdir(parents=True)
    (root / "migrations/v2").mkdir(parents=True)
    manifest = root / "typebridge.yaml"
    manifest.write_text(
        "format: typebridge.workspace/v1\n"
        "schema:\n  root: schema/schema.yaml\n  ownership: exclusive\n"
        "  managed-scope: shim-smoke\n"
        "compatibility:\n  semantic-profile: typedb-3.12.1/v1\n"
        "migrations:\n  directory: migrations/v2\n  app-label: shimsmoke\n"
    )
    (root / "schema/schema.yaml").write_text(
        "format: typebridge.schema-set/v1\nsources: [fragments/*.yaml]\n"
    )
    (root / "schema/fragments/model.yaml").write_text(
        "format: typebridge.schema/v2\nattributes:\n  nickname: { value: string }\n"
        "entities:\n  person: { owns: [nickname] }\n"
    )
    return manifest


def test_schema_check_runs_in_process(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manifest = _write_workspace(tmp_path)
    monkeypatch.setattr("sys.argv", ["type-bridge", "--manifest", str(manifest), "schema", "check"])
    with pytest.raises(SystemExit) as excinfo:
        main()
    assert excinfo.value.code == 0


def test_v2_failure_exit_code_propagates(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    missing = tmp_path / "absent.yaml"
    monkeypatch.setattr("sys.argv", ["type-bridge", "--manifest", str(missing), "schema", "check"])
    with pytest.raises(SystemExit) as excinfo:
        main()
    assert excinfo.value.code == 1
