"""Source-level tests for the artifact-only typed Python consumer."""

from __future__ import annotations

import importlib.util
import json
import stat
import subprocess
import sys
import zipfile
from pathlib import Path
from types import ModuleType

import pytest

ROOT = Path(__file__).resolve().parents[3]


def load_module(name: str, path: Path) -> ModuleType:
    """Load the standalone CI runner without package-path assumptions."""
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


runner = load_module(
    "run_typed_python_artifact",
    ROOT / "scripts/ci/run_typed_python_artifact.py",
)


def write_wheel(path: Path, members: dict[str, bytes]) -> Path:
    """Write the smallest ZIP-compatible wheel fixture needed by the runner."""
    with zipfile.ZipFile(path, "w") as archive:
        for name, content in members.items():
            archive.writestr(name, content)
    return path


def root_members() -> dict[str, bytes]:
    """Return all root members required by artifact inspection."""
    return {name: b"# fixture\n" for name in runner.ROOT_REQUIRED_MEMBERS}


def core_members() -> dict[str, bytes]:
    """Return all native members required by artifact inspection."""
    members = {name: b"# fixture\n" for name in runner.CORE_REQUIRED_MEMBERS}
    members["type_bridge_core/type_bridge_core.abi3.so"] = b"native"
    return members


def test_environment_and_explicit_executable_validation(tmp_path: Path) -> None:
    environment = runner.clean_environment(
        {
            "PATH": "/bin",
            "PYTHONPATH": "/checkout",
            "PYTHONHOME": "/runtime",
            "VIRTUAL_ENV": "/venv",
            "UV_PROJECT_ENVIRONMENT": "/uv-venv",
            "__PYVENV_LAUNCHER__": "/launcher",
        }
    )
    assert environment["PATH"] == "/bin"
    assert environment["PYTHONNOUSERSITE"] == "1"
    assert environment["PYTHONDONTWRITEBYTECODE"] == "1"
    assert all(key not in environment for key in runner.IMPORT_ENVIRONMENT_KEYS)

    executable = tmp_path / "prepared-python"
    executable.touch()
    executable.chmod(0o755)
    assert runner.absolute_executable(executable, label="Python") == executable.absolute()
    with pytest.raises(runner.RunnerError, match="does not exist"):
        runner.absolute_executable(tmp_path / "missing", label="Pyright")


def test_wheel_inspection_requires_typed_files_stub_marker_and_native(
    tmp_path: Path,
) -> None:
    root = write_wheel(tmp_path / "type_bridge.whl", root_members())
    core = write_wheel(tmp_path / "type_bridge_core.whl", core_members())

    assert runner.wheel_path(root, label="root") == root.resolve()
    report = runner.inspect_wheels(root, core)
    assert report["native_extension"] == "type_bridge_core/type_bridge_core.abi3.so"
    assert report["root_member_count"] == len(root_members())
    assert report["core_member_count"] == len(core_members())

    incomplete = root_members()
    del incomplete["type_bridge/typed/session.py"]
    missing = write_wheel(tmp_path / "missing_typed.whl", incomplete)
    with pytest.raises(runner.RunnerError, match="missing typed facade files"):
        runner.inspect_wheels(missing, core)


def test_safe_extraction_rejects_traversal_links_and_collisions(tmp_path: Path) -> None:
    first = write_wheel(tmp_path / "first.whl", {"package/one.py": b"one"})
    second = write_wheel(tmp_path / "second.whl", {"package/two.py": b"two"})
    destination = tmp_path / "site-packages"
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


def test_report_marker_and_pyright_config_helpers(tmp_path: Path) -> None:
    assert runner.parse_json_output(
        'diagnostic\n{"status": "ok", "value": 3}\n',
        description="fixture",
    ) == {"status": "ok", "value": 3}
    with pytest.raises(runner.RunnerError, match="produced no report"):
        runner.parse_json_output("\n", description="fixture")
    with pytest.raises(runner.RunnerError, match="did not end with JSON"):
        runner.parse_json_output("not-json", description="fixture")

    negative = tmp_path / "negative.py"
    negative.write_text(
        "bad()  # artifact-type-error: first\nok()\nbad()  # artifact-type-error: second\n",
        encoding="utf-8",
    )
    assert runner.marker_lines(negative, runner.NEGATIVE_MARKER) == {1, 3}

    artifact_root = tmp_path / "artifact"
    config = runner.pyright_config(
        fixture=negative,
        artifact_root=artifact_root,
        consumer_root=tmp_path,
        python_version="3.14",
    )
    assert config["include"] == ["negative.py"]
    assert config["pythonVersion"] == "3.14"
    assert config["typeCheckingMode"] == "strict"
    assert config["executionEnvironments"][0]["extraPaths"] == [str(artifact_root)]


def test_pyright_language_version_comes_from_supplied_interpreter(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    commands: list[list[str]] = []

    def fake_run_command(
        command: list[str | Path],
        *,
        cwd: Path,
        environment: dict[str, str],
    ) -> subprocess.CompletedProcess[str]:
        del cwd, environment
        rendered = [str(part) for part in command]
        commands.append(rendered)
        return subprocess.CompletedProcess(rendered, 0, "3.14\n", "")

    monkeypatch.setattr(runner, "run_command", fake_run_command)

    assert (
        runner.interpreter_language_version(
            Path("/matrix/python"),
            cwd=tmp_path,
            environment={},
        )
        == "3.14"
    )
    assert commands == [
        [
            "/matrix/python",
            "-I",
            "-c",
            "import sys; print(f'{sys.version_info.major}.{sys.version_info.minor}')",
        ]
    ]


def test_pyright_harness_accepts_clean_positive_and_exact_negative(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    positive = tmp_path / "positive.py"
    positive.write_text("value: int = 1\n", encoding="utf-8")
    negative = tmp_path / "negative.py"
    negative.write_text(
        'value: int = "bad"  # artifact-type-error: assignment\n',
        encoding="utf-8",
    )
    calls: list[list[str]] = []

    def fake_run_command(
        command: list[str | Path],
        *,
        cwd: Path,
        environment: dict[str, str],
    ) -> subprocess.CompletedProcess[str]:
        del cwd, environment
        rendered = [str(part) for part in command]
        calls.append(rendered)
        project = Path(rendered[rendered.index("--project") + 1])
        if project.name.endswith("positive.json"):
            payload = {"generalDiagnostics": []}
            return subprocess.CompletedProcess(rendered, 0, json.dumps(payload), "")
        payload = {
            "generalDiagnostics": [
                {
                    "file": str(negative),
                    "severity": "error",
                    "rule": "reportAssignmentType",
                    "range": {"start": {"line": 0, "character": 0}},
                }
            ]
        }
        return subprocess.CompletedProcess(rendered, 1, json.dumps(payload), "")

    monkeypatch.setattr(runner, "run_command", fake_run_command)
    report = runner.execute_pyright(
        python=Path("/prepared/python"),
        pyright=Path("/prepared/pyright"),
        positive=positive,
        negative=negative,
        artifact_root=tmp_path / "artifact",
        consumer_root=tmp_path,
        environment={},
        python_version="3.14",
    )

    assert report == {
        "positive": {"errors": 0},
        "negative": {
            "errors": 1,
            "lines": [1],
            "rules": ["reportAssignmentType"],
        },
    }
    assert len(calls) == 2
    assert all("--pythonpath" in command for command in calls)


def test_main_routes_interpreter_version_only_to_pyright(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    root = write_wheel(tmp_path / "root.whl", root_members())
    core = write_wheel(tmp_path / "core.whl", core_members())
    python = tmp_path / "python"
    pyright = tmp_path / "pyright"
    for executable in (python, pyright):
        executable.touch()
        executable.chmod(0o755)
    routed: dict[str, str] = {}

    def fake_runtime(
        *,
        python: Path,
        runtime_fixture: Path,
        artifact_root: Path,
        source_root: Path,
        consumer_root: Path,
        environment: dict[str, str],
    ) -> dict[str, str]:
        del python, runtime_fixture, artifact_root, source_root, consumer_root, environment
        routed["runtime"] = "called"
        return {"status": "ok"}

    def fake_pyright(
        *,
        python: Path,
        pyright: Path,
        positive: Path,
        negative: Path,
        artifact_root: Path,
        consumer_root: Path,
        environment: dict[str, str],
        python_version: str,
    ) -> dict[str, str]:
        del (
            python,
            pyright,
            positive,
            negative,
            artifact_root,
            consumer_root,
            environment,
        )
        routed["pyright"] = python_version
        return {"status": "ok"}

    monkeypatch.setattr(runner, "execute_runtime", fake_runtime)
    monkeypatch.setattr(runner, "execute_pyright", fake_pyright)
    monkeypatch.setattr(
        runner,
        "interpreter_language_version",
        lambda python, *, cwd, environment: "3.14",
    )

    assert (
        runner.main(
            [
                "--root-wheel",
                str(root),
                "--core-wheel",
                str(core),
                "--python",
                str(python),
                "--pyright",
                str(pyright),
                "--work-directory",
                str(tmp_path / "work"),
            ]
        )
        == 0
    )
    assert routed == {"runtime": "called", "pyright": "3.14"}
    assert json.loads(capsys.readouterr().out)["python_version"] == "3.14"


def test_live_harness_passes_connection_and_parses_report(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    calls: list[list[str]] = []

    def fake_run_command(
        command: list[str | Path],
        *,
        cwd: Path,
        environment: dict[str, str],
    ) -> subprocess.CompletedProcess[str]:
        del cwd, environment
        rendered = [str(part) for part in command]
        calls.append(rendered)
        report = {"status": "ok", "artifact": "wheel", "summary": {"count": 2}}
        return subprocess.CompletedProcess(rendered, 0, json.dumps(report), "")

    monkeypatch.setattr(runner, "run_command", fake_run_command)
    report = runner.execute_live(
        python=Path("/prepared/python"),
        live_fixture=tmp_path / "live.py",
        artifact_root=tmp_path / "artifact",
        source_root=tmp_path / "source",
        consumer_root=tmp_path,
        contract_fixture=tmp_path / "contract.json",
        address="localhost:1729",
        database="typed-query-parity",
        http_port=8000,
        username="admin",
        password="password",
        environment={},
    )

    assert report == {"status": "ok", "artifact": "wheel", "summary": {"count": 2}}
    command = calls[0]
    assert command[:3] == ["/prepared/python", "-I", str(tmp_path / "live.py")]
    assert command[command.index("--address") + 1] == "localhost:1729"
    assert command[command.index("--database") + 1] == "typed-query-parity"
    assert command[command.index("--http-port") + 1] == "8000"


def test_parser_requires_prebuilt_wheels_and_prepared_tools() -> None:
    parser = runner.build_parser()
    destinations = {action.dest for action in parser._actions}
    assert {"root_wheel", "core_wheel", "python", "pyright"} <= destinations
    assert {
        "live_address",
        "live_database",
        "live_http_port",
        "live_fixture",
    } <= destinations
    assert "artifact" not in destinations
    assert "bootstrap_python" not in destinations
    assert "wheelhouse" not in destinations


def test_source_package_declares_inline_typing() -> None:
    assert (ROOT / "type_bridge/py.typed").is_file()
