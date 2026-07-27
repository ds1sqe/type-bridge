"""Tests for the isolated legacy Python compatibility runner."""

from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
from pathlib import Path
from types import ModuleType

import pytest

ROOT = Path(__file__).resolve().parents[3]


def load_module(name: str, path: Path) -> ModuleType:
    """Load a standalone harness module without import-path assumptions."""
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


runner = load_module("run_legacy_python_compat", ROOT / "scripts/ci/run_legacy_python_compat.py")
probe = load_module("legacy_python_probe", ROOT / "tests/compat/legacy_python/probe.py")


def test_clean_environment_removes_import_overrides() -> None:
    environment = runner.clean_environment(
        {
            "PATH": "/bin",
            "PYTHONPATH": "/checkout",
            "PYTHONHOME": "/python-home",
            "VIRTUAL_ENV": "/venv",
            "UV_PROJECT_ENVIRONMENT": "/uv-venv",
            "__PYVENV_LAUNCHER__": "/launcher",
        }
    )

    assert environment["PATH"] == "/bin"
    assert environment["PYTHONNOUSERSITE"] == "1"
    assert environment["PYTHONDONTWRITEBYTECODE"] == "1"
    for key in runner.IMPORT_ENVIRONMENT_KEYS:
        assert key not in environment


def test_validate_artifacts_resolves_files_and_rejects_missing(tmp_path: Path) -> None:
    artifact = tmp_path / "type_bridge.whl"
    artifact.touch()

    assert runner.validate_artifacts([artifact]) == (artifact.resolve(),)
    with pytest.raises(runner.RunnerError, match="does not exist"):
        runner.validate_artifacts([tmp_path / "missing.whl"])
    with pytest.raises(runner.RunnerError, match="At least one"):
        runner.validate_artifacts([])


def test_absolute_executable_preserves_virtualenv_symlink(tmp_path: Path) -> None:
    base = tmp_path / "base-python"
    base.touch()
    venv_python = tmp_path / "venv/bin/python"
    venv_python.parent.mkdir(parents=True)
    venv_python.symlink_to(base)

    assert runner.absolute_executable(venv_python) == venv_python.absolute()
    assert runner.absolute_executable(venv_python) != base.resolve()


def test_parse_probe_report_uses_final_nonempty_json_line() -> None:
    report = runner.parse_probe_report('diagnostic\n{"status": "ok", "version": "1.2"}\n')

    assert report == {"status": "ok", "version": "1.2"}
    with pytest.raises(runner.RunnerError, match="produced no report"):
        runner.parse_probe_report("\n")
    with pytest.raises(runner.RunnerError, match="did not end with a JSON"):
        runner.parse_probe_report("not-json")
    with pytest.raises(runner.RunnerError, match="reported failure"):
        runner.parse_probe_report('{"status": "failed"}')


def test_probe_distribution_versions_bind_released_root_to_candidate_core() -> None:
    report = {
        "package_version": "1.5.11",
        "locations": {
            "distributions": {
                "type-bridge": {"version": "1.5.11"},
                "type-bridge-core": {"version": "2.0.0"},
            }
        },
        "locations_after_probe": {
            "distributions": {
                "type-bridge": {"version": "1.5.11"},
                "type-bridge-core": {"version": "2.0.0"},
            }
        },
    }

    runner.validate_distribution_versions(
        report,
        expected_root_version="1.5.11",
        expected_core_version="2.0.0",
    )
    report["locations"]["distributions"]["type-bridge-core"]["version"] = "1.5.11"
    with pytest.raises(runner.RunnerError, match="wrong released-root/candidate-core pair"):
        runner.validate_distribution_versions(
            report,
            expected_root_version="1.5.11",
            expected_core_version="2.0.0",
        )


def test_expected_distribution_versions_must_be_provided_together() -> None:
    with pytest.raises(runner.RunnerError, match="must be provided together"):
        runner.main(
            [
                "--python",
                sys.executable,
                "--expected-root-version",
                "1.5.11",
            ]
        )


def test_execute_probe_copies_outside_source_and_uses_isolated_mode(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    source_root = tmp_path / "source"
    source_root.mkdir()
    source_probe = source_root / "probe.py"
    source_probe.write_text("# fixture\n", encoding="utf-8")
    consumer = tmp_path / "consumer"
    consumer.mkdir()
    calls: list[tuple[list[str], Path]] = []

    def fake_run_checked(
        command: list[str | Path],
        *,
        cwd: Path,
        environment: dict[str, str],
    ) -> subprocess.CompletedProcess[str]:
        del environment
        calls.append(([str(part) for part in command], cwd))
        return subprocess.CompletedProcess(
            command,
            0,
            stdout=json.dumps({"status": "ok"}),
            stderr="",
        )

    monkeypatch.setattr(runner, "run_checked", fake_run_checked)
    report = runner.execute_probe(
        python=Path(sys.executable),
        probe=source_probe,
        source_root=source_root,
        consumer_root=consumer,
        environment=runner.clean_environment({}),
    )

    assert report["status"] == "ok"
    assert (consumer / "legacy_python_probe.py").read_text(encoding="utf-8") == "# fixture\n"
    assert calls == [
        (
            [
                sys.executable,
                "-I",
                str(consumer / "legacy_python_probe.py"),
                "--source-root",
                str(source_root),
            ],
            consumer,
        )
    ]


def test_probe_source_leak_detection_resolves_symlinks(tmp_path: Path) -> None:
    source_root = tmp_path / "checkout"
    package = source_root / "type_bridge"
    package.mkdir(parents=True)
    module = package / "__init__.py"
    module.touch()
    outside = tmp_path / "outside"
    outside.mkdir()
    symlink = outside / "type_bridge.py"
    symlink.symlink_to(module)

    assert probe.path_is_within(module, source_root)
    assert probe.source_leaks([symlink], source_root) == [str(module.resolve())]
    assert probe.source_leaks([outside / "installed.py"], source_root) == []


def test_probe_behavior_matches_the_current_legacy_surface() -> None:
    name, age, person, company, employment = probe.define_models()

    raw = probe.probe_raw_query(person)
    descriptors = probe.probe_descriptors(name, age, person, company, employment)
    rust_query = probe.probe_rust_query(person)

    assert 'has LegacyCompatName "Alice"' in raw["query_builder"]
    assert descriptors == {
        "field_ref": "StringFieldRef",
        "field_value": "LegacyCompatName",
        "legacy_generic_arity": [
            "FieldRef",
            "StringFieldRef",
            "NumericFieldRef",
            "FieldDescriptor",
            "RoleRef",
        ],
        "role_ref": "RoleRef",
        "role_value": "LegacyCompatPerson",
    }
    assert rust_query["initial_spec"]["limit"] == 2
    assert rust_query["initial_spec"]["offset"] == 1


def test_probe_requires_separate_typed_subpath_exports(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    import type_bridge

    typed_module = ModuleType("type_bridge.typed")
    typed_query = type("Query", (), {"__module__": "type_bridge.typed.query"})
    query_session = type("QuerySession", (), {"__module__": "type_bridge.typed.session"})
    setattr(typed_module, "Query", typed_query)
    setattr(typed_module, "QuerySession", query_session)
    setattr(typed_module, "__all__", ["Query", "QuerySession"])
    monkeypatch.setitem(sys.modules, "type_bridge.typed", typed_module)
    monkeypatch.setattr(type_bridge, "typed", typed_module, raising=False)

    typed = probe.probe_typed_facade()

    assert typed == {
        "module": "type_bridge.typed",
        "query": "type_bridge.typed.query.Query",
        "session": "type_bridge.typed.session.QuerySession",
        "root_query": "type_bridge.query.Query",
    }
