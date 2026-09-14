"""SDK producer orchestration, cleanup, and publication contracts."""

from __future__ import annotations

import importlib.util
import json
import signal
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
CI = ROOT / "scripts/ci"
sys.path.insert(0, str(CI))
SPEC = importlib.util.spec_from_file_location(
    "sdk_conformance_runner", CI / "run_sdk_conformance.py"
)
assert SPEC is not None and SPEC.loader is not None
RUNNER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = RUNNER
SPEC.loader.exec_module(RUNNER)


def capture(monkeypatch: pytest.MonkeyPatch) -> list[list[str]]:
    commands: list[list[str]] = []
    monkeypatch.setattr(RUNNER, "run", lambda command, **_kwargs: commands.append(command))
    return commands


def test_fragment_ledger_covers_each_binding_and_exact_validator(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    commands: list[list[str]] = []
    monkeypatch.setattr(RUNNER, "run", lambda command, **_kwargs: commands.append(command))
    RUNNER.query_proofs(tmp_path, {}, "0" * 64)
    validators = [command for command in commands if "scripts/ci/proof_fragments.py" in command]
    assert len(validators) == 4
    assert all(command[command.index("--sdk") + 1] == "2" for command in validators)
    assert [
        "uv",
        "run",
        "python",
        "type-bridge-core/crates/schema-codegen/tests/acceptance/check.py",
    ] in commands
    for marker in (
        "python_direct_cancellation_fragment_is_measured_from_owned_execution",
        "node_direct_cancellation_fragment_is_measured_from_owned_execution",
        "sdk_v2_rust_deterministic_proof_fragment",
        "sdk_v2_c_deterministic_proof_fragment",
    ):
        command = next(command for command in commands if any(marker in item for item in command))
        assert "--locked" in command and command[-2:] == ["--", "--exact"]


def test_comparator_ledger_preserves_v1_three_and_v2_four(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    commands: list[list[str]] = []
    monkeypatch.setattr(RUNNER, "run", lambda command, **_kwargs: commands.append(command))
    RUNNER.compare_queries(tmp_path, {})
    assert len(commands) == 2
    assert len(commands[0]) == 2 + len(RUNNER.V1_BINDINGS)
    assert len(commands[1]) == 2 + len(RUNNER.BINDINGS)


def test_phase_report_ledger_uses_all_three_validated_fan_ins(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    commands = capture(monkeypatch)
    RUNNER.model_projections(
        tmp_path,
        {"TYPEDB_ADDRESS": "127.0.0.1:1729", "TYPEDB_HTTP_PORT": "8000"},
    )
    assert len(commands) == 3
    for marker in (
        "run_projected_parity.py",
        "projected",
        "manager",
    ):
        assert (
            sum(
                any(item == marker or item.endswith(marker) for item in command)
                for command in commands
            )
            == 1
        )


def test_assembly_runs_four_compositors_four_assemblers_and_one_fan_in(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    commands = capture(monkeypatch)
    (tmp_path / "fragments").mkdir()
    RUNNER.assemble_models(tmp_path, {}, "0" * 64)
    assert len(commands) == 5
    assert (
        sum(any("assemble_sdk_conformance_v3" in item for item in command) for command in commands)
        == 4
    )
    assert any("compare_sdk_conformance_v3" in item for item in commands[-1])


def test_cleanup_evidence_is_exact_and_non_inferential(tmp_path: Path) -> None:
    path = tmp_path / "cleanup.json"
    RUNNER.write_cleanup(path, "node")
    assert json.loads(path.read_bytes()) == {
        "binding": "node",
        "format": "typebridge.sdk-v5-cleanup-evidence/v1",
        "managed_database_absent": True,
        "partial_output_absent": True,
        "temporary_evidence_absent": True,
    }
    assert path.read_bytes() == RUNNER.codec_contract.canonical_json_bytes(
        json.loads(path.read_bytes())
    )


def test_assembly_runs_each_real_composer_and_one_fan_in(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    commands: list[list[str]] = []
    monkeypatch.setattr(
        RUNNER,
        "run",
        lambda command, **_kwargs: commands.append(command),
    )
    RUNNER.assemble_codec(tmp_path / "evidence", tmp_path / "reports", {})
    assert len(commands) == 5
    assert (
        sum(
            any(item.endswith("assemble_sdk_conformance_v5.py") for item in command)
            for command in commands
        )
        == 4
    )
    assert (
        sum(
            any(item.endswith("assemble_sdk_conformance_v5.py") for item in command)
            for command in commands
        )
        == 4
    )
    assert any(item.endswith("compare_sdk_conformance_v5.py") for item in commands[-1])


def test_provider_free_fan_in_names_each_binding(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    commands: list[list[str]] = []
    monkeypatch.setattr(
        RUNNER,
        "run",
        lambda command, **_kwargs: commands.append(command),
    )
    RUNNER.codec_offline(tmp_path, {})
    fan_in = [command for command in commands if any("compare_sdk_v5_" in item for item in command)]
    assert len(fan_in) == 2
    for command in fan_in:
        for binding in RUNNER.BINDINGS:
            assert command.count(f"--{binding}") == 1


def _reports(root: Path) -> None:
    for binding in RUNNER.BINDINGS:
        for version in RUNNER.conformance.predecessor_versions(binding):
            path = root / f"v{version}" / f"{binding}.json"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("{}", encoding="utf-8")


def test_historical_inventory_does_not_invent_c_v1(tmp_path: Path) -> None:
    _reports(tmp_path)
    assert [version for version, _ in RUNNER.predecessor_paths(tmp_path, "rust")] == [1, 2, 3, 4, 5]
    assert [version for version, _ in RUNNER.predecessor_paths(tmp_path, "c")] == [2, 3, 4, 5]
    assert not (tmp_path / "v1/c.json").exists()


def test_missing_real_predecessor_fails_closed(tmp_path: Path) -> None:
    _reports(tmp_path)
    (tmp_path / "v4/c.json").unlink()
    with pytest.raises(RUNNER.RunnerError, match="V4 c report"):
        RUNNER.predecessor_paths(tmp_path, "c")


def test_c_consumer_is_bound_only_to_query_artifacts() -> None:
    query = {
        "artifacts": {
            "cli": {"artifact-id": f"sha256:{'1' * 64}"},
            "generated-package": {"sha256": "2" * 64},
        }
    }
    report = RUNNER.c_surface_consumer(query, source_commit="3" * 40)
    assert report["surface-sha256"] == "2" * 64
    assert report["cli-artifact-id"] == f"sha256:{'1' * 64}"
    assert report["runtime-provenance"] == "artifact-c-runtime"
    assert report["publication-authority"] is False


@pytest.mark.parametrize(
    ("suite", "stages"),
    [
        ("queries", ("query_proofs", "query_live", "compare_queries")),
        (
            "models",
            (
                "model_projections",
                "model_artifacts",
                "model_proofs",
                "model_live",
                "assemble_models",
            ),
        ),
        ("administration", ("administration",)),
        ("serialization", ("codec_offline", "codec_live", "assemble_codec")),
        ("artifacts", ("artifact_reports",)),
    ],
)
@pytest.mark.parametrize("fail", [False, True])
def test_suite_runs_in_order_and_publishes_only_validated_reports(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, suite: str, stages: tuple[str, ...], fail: bool
) -> None:
    calls: list[str] = []
    scratch: list[Path] = []
    names = (
        [
            f"v{version}/{binding}.json"
            for version in (1, 2)
            for binding in (RUNNER.V1_BINDINGS if version == 1 else RUNNER.BINDINGS)
        ]
        if suite == "queries"
        else [f"{binding}.json" for binding in RUNNER.BINDINGS]
    )

    def stage(name: str) -> Any:
        def produce(directory: Path, *_args: Any) -> tuple[Path, ...]:
            calls.append(name)
            scratch.append(directory)
            if name != stages[-1]:
                return ()
            paths = []
            for filename in names:
                path = (
                    directory / "reports" / filename
                    if suite == "serialization"
                    else directory / filename
                )
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(filename.encode())
                paths.append(path)
            if fail:
                raise RUNNER.RunnerError("validation failed")
            return tuple(paths)

        return produce

    for name in stages:
        monkeypatch.setattr(RUNNER, name, stage(name))
    output = tmp_path / "published"
    argv = [suite, "--output", str(output)]
    if suite == "artifacts":
        argv += [
            item
            for name in (
                "predecessors",
                "acceptance",
                "provider",
                "live",
                "cli",
                "runtime",
                "generated",
            )
            for item in (f"--{name}", str(tmp_path / name))
        ]
    assert RUNNER.main(argv) == int(fail)
    assert calls == list(stages)
    assert all(not path.exists() for path in scratch)
    assert output.exists() is not fail
    if not fail:
        assert sorted(
            path.relative_to(output).as_posix() for path in output.rglob("*.json")
        ) == sorted(names)
        for name in names:
            assert (output / name).read_bytes() == name.encode()


@pytest.mark.parametrize("argv", [["artifacts"], ["queries", "--cli", "unused"]])
def test_invalid_artifact_arguments_fail_before_execution(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, argv: list[str]
) -> None:
    monkeypatch.setattr(RUNNER, "execute", lambda *_args: pytest.fail("must not execute"))
    with pytest.raises(SystemExit) as error:
        RUNNER.main([*argv, "--output", str(tmp_path / "reports")])
    assert error.value.code == 2


@pytest.mark.parametrize("failure", [None, "producer", "termination"])
def test_administration_owns_provider_and_always_cleans_up(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, failure: str | None
) -> None:
    commands: list[list[str]] = []
    previous = signal.getsignal(signal.SIGTERM)

    def run(command: list[str], **kwargs: Any) -> str:
        commands.append(command)
        if "up" in command:
            assert kwargs["env"]["TYPEDB_IMAGE"] == "typedb/typedb:3.12.3"
            assert kwargs["env"]["TYPEDB_PORT"] == "0"
            assert kwargs["env"]["TYPEDB_HTTP_PORT"] == "0"
        if command[0] == "cargo":
            assert kwargs["env"]["TYPEDB_ADDRESS"] == "127.0.0.1:5010"
            assert kwargs["env"]["TYPEDB_HTTP_PORT"] == "5020"
            if failure == "producer":
                raise RUNNER.RunnerError("producer failed")
            if failure == "termination":
                handler = signal.getsignal(signal.SIGTERM)
                assert callable(handler)
                handler(signal.SIGTERM, None)
            binding = command[command.index("--test") + 1].split("_")[2]
            (tmp_path / f"{binding}-sdk-v4-report.json").write_text("{}")
        return "{}"

    monkeypatch.setattr(RUNNER, "run", run)
    monkeypatch.setattr(
        RUNNER, "published_port", lambda _project, port: "5010" if port == "1729" else "5020"
    )
    monkeypatch.setattr(
        RUNNER.subprocess, "run", lambda command, **_kwargs: commands.append(command)
    )
    if failure:
        with pytest.raises(RUNNER.RunnerError, match="administration acceptance failed"):
            RUNNER.administration(tmp_path, {})
    else:
        reports = RUNNER.administration(tmp_path, {})
        assert len(reports) == 4 and all(path.is_file() for path in reports)
        assert [
            command[command.index("--test") + 1] for command in commands if command[0] == "cargo"
        ] == [f"sdk_v4_{binding}_live" for binding in RUNNER.BINDINGS]
        assert any(item.endswith("compare_sdk_conformance_v4.py") for item in commands[-2])
    assert commands[-1][-3:] == ["down", "-v", "--remove-orphans"]
    assert commands[0][:6] == commands[-1][:6]
    assert signal.getsignal(signal.SIGTERM) == previous


def test_subprocess_error_is_not_accepted(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        RUNNER.subprocess,
        "run",
        lambda *_args, **_kwargs: subprocess.CompletedProcess(["producer"], 23),
    )
    with pytest.raises(RUNNER.RunnerError, match="exit 23"):
        RUNNER.run(["producer"], env={})


def test_full_c_preserves_versioned_layout_with_named_suites(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    full_c = importlib.import_module("run_full_c_artifact")

    calls: list[tuple[str, ...]] = []
    output = tmp_path / "full-c"

    def run(*arguments: str) -> None:
        calls.append(arguments)
        if arguments[1] == "queries":
            for version in (1, 2):
                (output / f"v12/v{version}").mkdir(parents=True)

    monkeypatch.setattr(full_c, "run", run)
    monkeypatch.setattr(
        sys, "argv", ["full-c", "--inputs", str(tmp_path / "inputs"), "--output", str(output)]
    )
    full_c.main()
    assert [call[:2] for call in calls] == [
        ("run_sdk_conformance.py", suite) for suite in RUNNER.SUITES
    ]
    assert [Path(call[call.index("--output") + 1]).name for call in calls] == [
        "v12",
        "v3",
        "v4",
        "v5",
        "v6",
    ]
    assert (output / "v1").is_dir() and (output / "v2").is_dir()
    assert not (output / "v12").exists()
    assert calls[-1][calls[-1].index("--predecessors") + 1] == str(output)


@pytest.mark.parametrize(
    ("function", "python_test", "node_pattern", "c_test"),
    [
        (
            "query_live",
            "test_generated_projection_round_trips_live_models",
            "generated package round-trips exact models on TypeDB",
            "live_c17_generated_person_and_membership_crud_round_trips_exact_3_12_3",
        ),
        (
            "model_live",
            "test_generated_data_model_runtime_v3_live",
            "node.generated_data_model_runtime_v3_live",
            "generated_data_model_runtime_v3_live",
        ),
        (
            "codec_live",
            "test_generated_canonical_serialization_v5_live",
            "node.generated_canonical_serialization_v5_live",
            "live_c17_generated_person_and_membership_crud_round_trips_exact_3_12_3",
        ),
    ],
)
def test_live_suites_keep_all_four_producers(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    function: str,
    python_test: str,
    node_pattern: str,
    c_test: str,
) -> None:
    compiled = (
        tmp_path
        / "tmp/node-projection-integration/tests/projection-integration/generated-package-live.test.js"
    )
    compiled.parent.mkdir(parents=True)
    compiled.touch()
    monkeypatch.setattr(RUNNER, "ROOT", tmp_path)
    fragments = tmp_path / "fragments"
    fragments.mkdir()
    for binding in RUNNER.BINDINGS:
        (fragments / f"{binding}-0.json").write_text("{}")
    commands = capture(monkeypatch)
    args: list[Any] = [tmp_path, {}]
    if function == "query_live":
        args.append("0" * 64)
    getattr(RUNNER, function)(*args)
    assert (
        sum(any(item.endswith(f"::{python_test}") for item in command) for command in commands) == 1
    )
    assert [
        "node",
        "--test",
        "--test-concurrency=1",
        f"--test-name-pattern={node_pattern}",
        str(compiled),
    ] in commands
    rust = [
        command for command in commands if "scripts/ci/run_exact_ignored_rust_test.sh" in command
    ]
    assert [command[2] for command in rust] == [
        "generated_rust_projection_round_trips_exact_live_models",
        c_test,
    ]
    assert all("--locked" in command for command in rust)


def test_artifacts_build_once_and_compare_all_bindings(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _reports(tmp_path / "predecessors")
    directory = tmp_path / "evidence"
    directory.mkdir()
    commands = capture(monkeypatch)
    RUNNER.assemble_artifacts(
        directory,
        predecessors=tmp_path / "predecessors",
        acceptance_path=tmp_path / "acceptance.json",
        provider=tmp_path / "provider.json",
        live=tmp_path / "live.json",
        cli=tmp_path / "cli.tar.gz",
        runtime=tmp_path / "runtime.tar.gz",
        generated=tmp_path / "generated.tar.gz",
        source_commit="3" * 40,
        acceptance={
            "artifacts": {
                "cli": {"artifact-id": f"sha256:{'1' * 64}"},
                "generated-package": {"sha256": "2" * 64},
            }
        },
        environment={},
    )
    assert len(commands) == 9
    assert commands[0][1:3] == ["scripts/ci/sdk_v6_surfaces.py", "build"]
    assert commands[1][:3] == ["uv", "run", "python"]
    for script in ("assemble_sdk_conformance_v6.py", "assemble_sdk_conformance_v6.py"):
        assert sum(f"scripts/ci/{script}" in command for command in commands) == 4
    assert commands[-1][1] == "scripts/ci/compare_sdk_conformance_v6.py"
    assert len(commands[-1][2:]) == 4
