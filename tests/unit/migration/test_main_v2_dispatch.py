# pyright: reportMissingImports=false
"""The console entry point serves V2 workspace verbs from the wheel itself.

Covers:
- ``schema`` / ``migration`` / ``--manifest`` invocations run the in-process
  V2 CLI shipped inside the native extension (no external binary involved)
- a real split-YAML workspace passes ``schema check`` through the entry point
- legacy verbs and global help/version forms use the released native parser
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


def test_legacy_invocations_still_forward() -> None:
    for argv in (
        ["migrate", "--database", "mydb"],
        ["showmigrations"],
        ["makemigrations", "--name", "add_phone"],
        ["plan"],
        ["sqlmigrate", "0001_initial"],
        ["--help"],
        ["-h"],
        ["--version"],
        ["-V"],
        [],
    ):
        assert not _is_v2_invocation(argv)


@pytest.mark.parametrize("flag", ["--help", "-h", "--version", "-V"])
def test_global_forms_run_verbatim_through_the_released_native_parser(
    flag: str, monkeypatch: pytest.MonkeyPatch
) -> None:
    observed: list[list[str]] = []

    class Native:
        @staticmethod
        def run_legacy_migration_cli(arguments: list[str]) -> int:
            observed.append(arguments)
            return 27

    monkeypatch.setattr("type_bridge._rust_runtime.rust_core", lambda: Native())
    monkeypatch.setattr("sys.argv", ["type-bridge", flag])

    with pytest.raises(SystemExit) as excinfo:
        main()

    assert excinfo.value.code == 27
    assert observed == [["type-bridge-migration", flag]]


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


@pytest.mark.parametrize("verb", ["schema", "migration"])
def test_v2_verb_help_remains_available_in_process(
    verb: str,
    monkeypatch: pytest.MonkeyPatch,
    capfd: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.setattr("sys.argv", ["type-bridge", verb, "--help"])
    with pytest.raises(SystemExit) as excinfo:
        main()
    stdout, stderr = capfd.readouterr()

    assert excinfo.value.code == 0
    assert f"Usage: type-bridge {verb}" in stdout
    assert stderr == ""


def test_v2_version_remains_available_after_an_opt_in_manifest_selector(
    monkeypatch: pytest.MonkeyPatch,
    capfd: pytest.CaptureFixture[str],
) -> None:
    observed: list[tuple[object, str, str]] = []
    for arguments in (
        ["--manifest=unused.yaml", "--version"],
        ["--manifest", "unused.yaml", "-V"],
    ):
        monkeypatch.setattr("sys.argv", ["type-bridge", *arguments])
        with pytest.raises(SystemExit) as excinfo:
            main()
        stdout, stderr = capfd.readouterr()
        observed.append((excinfo.value.code, stdout, stderr))

    assert observed[0] == observed[1]
    assert observed[0][0] == 0
