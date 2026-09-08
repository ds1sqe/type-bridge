"""Source-level gates for the generated-only Python artifact consumer."""

from __future__ import annotations

import importlib.util
import stat
import subprocess
import sys
import zipfile
from pathlib import Path
from types import ModuleType

import pytest

ROOT = Path(__file__).resolve().parents[3]


def load_runner() -> ModuleType:
    path = ROOT / "scripts/ci/run_generated_python_artifact.py"
    spec = importlib.util.spec_from_file_location("run_generated_python_artifact", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


runner = load_runner()


def write_wheel(path: Path, members: dict[str, bytes]) -> Path:
    with zipfile.ZipFile(path, "w") as archive:
        for name, content in members.items():
            archive.writestr(name, content)
    return path


def root_members() -> dict[str, bytes]:
    return {name: b"# fixture\n" for name in runner.ROOT_REQUIRED_MEMBERS}


def core_members() -> dict[str, bytes]:
    members = {name: b"# fixture\n" for name in runner.CORE_REQUIRED_MEMBERS}
    members["type_bridge_core/type_bridge_core.abi3.so"] = b"native"
    return members


def test_environment_and_executable_validation(tmp_path: Path) -> None:
    environment = runner.clean_environment(
        {
            "PATH": "/bin",
            "PYTHONHOME": "/runtime",
            "PYTHONPATH": "/checkout",
            "VIRTUAL_ENV": "/venv",
            "UV_PROJECT_ENVIRONMENT": "/uv",
            "__PYVENV_LAUNCHER__": "/launcher",
        }
    )
    assert environment["PATH"] == "/bin"
    assert environment["PYTHONNOUSERSITE"] == "1"
    assert environment["PYTHONDONTWRITEBYTECODE"] == "1"
    assert all(key not in environment for key in runner.IMPORT_ENVIRONMENT_KEYS)

    executable = tmp_path / "python"
    executable.touch(mode=0o755)
    assert runner.absolute_executable(executable, label="Python") == executable.absolute()
    with pytest.raises(runner.RunnerError, match="does not exist"):
        runner.absolute_executable(tmp_path / "missing", label="Pyright")


def test_wheel_inventory_requires_generated_runtime_and_native_extension(
    tmp_path: Path,
) -> None:
    root = write_wheel(tmp_path / "root.whl", root_members())
    core = write_wheel(tmp_path / "core.whl", core_members())
    report = runner.inspect_wheels(root, core)
    assert report["native_extension"] == "type_bridge_core/type_bridge_core.abi3.so"

    incomplete = root_members()
    del incomplete["type_bridge/_runtime_projection.py"]
    with pytest.raises(runner.RunnerError, match="generated-runtime files"):
        runner.inspect_wheels(write_wheel(tmp_path / "incomplete.whl", incomplete), core)


def test_safe_wheel_extraction_rejects_traversal_links_and_collisions(tmp_path: Path) -> None:
    first = write_wheel(tmp_path / "first.whl", {"package/one.py": b"one"})
    second = write_wheel(tmp_path / "second.whl", {"package/two.py": b"two"})
    destination = tmp_path / "artifact"
    runner.extract_wheels((first, second), destination)
    assert (destination / "package/one.py").read_bytes() == b"one"
    assert (destination / "package/two.py").read_bytes() == b"two"

    collision = write_wheel(tmp_path / "collision.whl", {"package/one.py": b"other"})
    with pytest.raises(runner.RunnerError, match="collision"):
        runner.extract_wheels((first, collision), tmp_path / "collision-output")

    traversal = write_wheel(tmp_path / "traversal.whl", {"../escape.py": b"escape"})
    with pytest.raises(runner.RunnerError, match="unsafe member path"):
        runner.wheel_members(traversal)

    link = tmp_path / "link.whl"
    with zipfile.ZipFile(link, "w") as archive:
        info = zipfile.ZipInfo("package/link.py")
        info.create_system = 3
        info.external_attr = (stat.S_IFLNK | 0o777) << 16
        archive.writestr(info, "target.py")
    with pytest.raises(runner.RunnerError, match="symbolic link"):
        runner.wheel_members(link)


def test_generated_tree_copy_rejects_links(tmp_path: Path) -> None:
    source = tmp_path / "source"
    source.mkdir()
    (source / "generated_v2").mkdir()
    (source / "generated_v2/__init__.py").write_text("# fixture\n", encoding="utf-8")
    runner.copy_tree_without_links(source, tmp_path / "copied")
    assert (tmp_path / "copied/generated_v2/__init__.py").is_file()

    linked = tmp_path / "linked"
    linked.mkdir()
    (linked / "escape").symlink_to(source / "generated_v2")
    with pytest.raises(runner.RunnerError, match="symbolic links"):
        runner.copy_tree_without_links(linked, tmp_path / "unused")


def test_negative_markers_and_json_reports_are_exact(tmp_path: Path) -> None:
    negative = tmp_path / "negative.py"
    negative.write_text(
        "bad()  # E: first:reportCallIssue\nbad()  # E: second:reportArgumentType\n",
        encoding="utf-8",
    )
    assert runner.expected_diagnostics(negative) == {
        0: "reportCallIssue",
        1: "reportArgumentType",
    }
    assert runner.parse_json_output(
        'diagnostic\n{"status": "ok", "value": 3}\n', description="fixture"
    ) == {"status": "ok", "value": 3}
    with pytest.raises(runner.RunnerError, match="produced no report"):
        runner.parse_json_output("\n", description="fixture")


def test_interpreter_version_uses_supplied_python(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    calls: list[list[str]] = []

    def fake_run(
        command: list[str | Path],
        *,
        cwd: Path,
        environment: dict[str, str],
    ) -> subprocess.CompletedProcess[str]:
        del cwd, environment
        rendered = [str(part) for part in command]
        calls.append(rendered)
        return subprocess.CompletedProcess(rendered, 0, "3.14\n", "")

    monkeypatch.setattr(runner, "run_command", fake_run)
    assert (
        runner.interpreter_language_version(Path("/candidate/python"), cwd=tmp_path, environment={})
        == "3.14"
    )
    assert calls[0][:3] == ["/candidate/python", "-I", "-c"]


def test_parser_requires_wheels_generated_stage_and_prepared_tools() -> None:
    destinations = {action.dest for action in runner.build_parser()._actions}
    assert {"root_wheel", "core_wheel", "generated_stage", "python", "pyright"} <= destinations
    assert "artifact" not in destinations
    assert "bootstrap_python" not in destinations
    assert "live_address" not in destinations


def test_generated_stage_requires_compiled_authority_artifact() -> None:
    source = (ROOT / "scripts/ci/run_generated_python_artifact.py").read_text(encoding="utf-8")
    assert 'generated_root / "schema-authority.json"' in source
    assert "declared-schema.json" not in source


def test_generated_runtime_consumer_has_no_handwritten_imports() -> None:
    source = (ROOT / "tests/compat/generated_python/runtime.py").read_text(encoding="utf-8")
    for forbidden in (
        "from type_bridge import Entity",
        "from type_bridge import Relation",
        "from type_bridge import Role",
        "type_bridge.models",
        "type_bridge.attribute",
    ):
        assert forbidden not in source
    assert '"TypeDBType"' in source
    assert '"QueryBuilder"' in source
