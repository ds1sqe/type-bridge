#!/usr/bin/env python3
"""Run SDK query, model, administration, serialization and artifact acceptance."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import secrets
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
import c_artifact_journey as artifact_journey
import compare_sdk_conformance_v5 as codec_contract
import compare_sdk_conformance_v6 as conformance
import standalone_cli_artifact as cli_artifact
from persist_binding_reports import PublishError, publish_files

ROOT = Path(__file__).resolve().parents[2]
CORE = ROOT / "type-bridge-core"
NODE = CORE / "crates/node"
V1_BINDINGS = ("python", "node", "rust")
BINDINGS = (*V1_BINDINGS, "c")
SUITES = ("queries", "models", "administration", "serialization", "artifacts")
PYTHON_PROVIDER = ROOT / "tests/integration/schema/test_generated_sdk_v5_codec.py"
NODE_PROVIDER = CORE / "crates/schema-codegen/tests/typescript_acceptance/codec_check.py"
C_PROVIDER = ROOT / "tests/integration/schema/test_generated_c_sdk_v5_codec.py"
SURFACE_CONSUMER_FORMAT = "typebridge.sdk-v6-surface-consumer/v1"


class RunnerError(RuntimeError):
    """An SDK producer, comparator, or report publication failed."""


def run(
    command: list[str],
    *,
    env: dict[str, str] | None = None,
    cwd: Path = ROOT,
    capture: bool = False,
) -> str:
    result = subprocess.run(
        command,
        cwd=cwd,
        env=env,
        check=False,
        text=True,
        stdout=subprocess.PIPE if capture else None,
    )
    if result.returncode != 0:
        raise RunnerError(f"command failed with exit {result.returncode}: {' '.join(command)}")
    return result.stdout.strip() if capture else ""


def python_command(script: str, *arguments: str) -> list[str]:
    return [sys.executable, f"scripts/ci/{script}", *arguments]


def cargo_test(crate: str, test: str, *, suite: str | None = None) -> list[str]:
    return [
        "cargo",
        "test",
        "--quiet",
        "--locked",
        "--manifest-path",
        str(CORE / "Cargo.toml"),
        "-p",
        crate,
        *(["--lib"] if suite is None else ["--test", suite]),
        test,
        "--",
        "--exact",
    ]


def rust_live(test: str, suite: str) -> list[str]:
    return [
        "bash",
        "scripts/ci/run_exact_ignored_rust_test.sh",
        test,
        "--locked",
        "--manifest-path",
        "type-bridge-core/Cargo.toml",
        "-p",
        "type-bridge-schema-codegen",
        "--test",
        suite,
    ]


def fragment_environment(
    version: int, environment: dict[str, str], destination: Path, nonce: str
) -> dict[str, str]:
    return environment | {
        f"TYPE_BRIDGE_SDK_V{version}_PROOF_FRAGMENT": str(destination),
        f"TYPE_BRIDGE_SDK_V{version}_PROOF_RUN_NONCE": nonce,
    }


def node_live(pattern: str, environment: dict[str, str]) -> None:
    compiled = (
        ROOT
        / "tmp/node-projection-integration/tests/projection-integration/generated-package-live.test.js"
    )
    if not compiled.is_file():
        raise RunnerError("compiled Node projection integration test is missing")
    shutil.copy2(
        NODE / "tests/projection-integration/package.json", compiled.parent / "package.json"
    )
    run(
        ["node", "--test", "--test-concurrency=1", f"--test-name-pattern={pattern}", str(compiled)],
        cwd=NODE,
        env=environment,
    )


def produce_proofs(
    version: int,
    directory: Path,
    environment: dict[str, str],
    nonce: str,
    commands: dict[str, tuple[list[str], ...]],
    suffixes: tuple[str, ...] = (),
) -> None:
    fragments = directory / "fragments"
    fragments.mkdir()
    for binding in BINDINGS:
        paths = []
        for index, command in enumerate(commands[binding]):
            suffix = suffixes[index] if suffixes else str(index)
            destination = fragments / f"{binding}-{suffix}.json"
            run(command, env=fragment_environment(version, environment, destination, nonce))
            paths.append(destination)
        run(
            python_command(
                "proof_fragments.py",
                "--sdk",
                str(version),
                "--binding",
                binding,
                "--run-nonce",
                nonce,
                *(str(path) for path in paths),
            ),
            env=environment,
        )


def query_proofs(directory: Path, environment: dict[str, str], nonce: str) -> None:
    commands: dict[str, tuple[list[str], ...]] = {
        "python": (
            cargo_test(
                "type-bridge-core",
                "match_runtime::tests::python_direct_cancellation_fragment_is_measured_from_owned_execution",
            ),
            [
                "uv",
                "run",
                "python",
                "type-bridge-core/crates/schema-codegen/tests/acceptance/check.py",
            ],
        ),
        "node": (
            cargo_test(
                "type-bridge-node",
                "match_runtime::tests::node_direct_cancellation_fragment_is_measured_from_owned_execution",
            ),
            [
                "node",
                "type-bridge-core/crates/schema-codegen/tests/typescript_acceptance/check.mjs",
            ],
        ),
        "rust": (
            cargo_test("type-bridge", "remote::tests::sdk_v2_rust_deterministic_proof_fragment"),
        ),
        "c": (cargo_test("type-bridge-c", "query::tests::sdk_v2_c_deterministic_proof_fragment"),),
    }
    produce_proofs(2, directory, environment, nonce, commands)


def query_environment(
    environment: dict[str, str], directory: Path, binding: str, nonce: str
) -> dict[str, str]:
    fragments = sorted((directory / "fragments").glob(f"{binding}-*.json"))
    if not fragments:
        raise RunnerError(f"{binding} V2 proof fragments are missing")
    result = environment | {
        "TYPE_BRIDGE_SDK_REPORT_V2": str(directory / f"v2/{binding}.json"),
        "TYPE_BRIDGE_SDK_V2_PROOF_FRAGMENTS": os.pathsep.join(str(path) for path in fragments),
        "TYPE_BRIDGE_SDK_V2_PROOF_RUN_NONCE": nonce,
    }
    if binding in V1_BINDINGS:
        result["TYPE_BRIDGE_SDK_REPORT"] = str(directory / f"v1/{binding}.json")
    return result


def query_live(directory: Path, environment: dict[str, str], nonce: str) -> None:
    (directory / "v1").mkdir()
    (directory / "v2").mkdir()
    python_env = query_environment(environment, directory, "python", nonce) | {
        "USE_DOCKER": "false"
    }
    run(
        [
            "uv",
            "run",
            "pytest",
            "-q",
            "-m",
            "integration",
            "tests/integration/schema/test_generated_projection_live.py::test_generated_projection_round_trips_live_models",
        ],
        env=python_env,
    )
    run(["npm", "run", "build"], cwd=NODE, env=environment)
    run(["npm", "run", "typecheck:projection-integration"], cwd=NODE, env=environment)
    node_env = query_environment(environment, directory, "node", nonce) | {
        "TYPE_BRIDGE_SDK_V2_VALIDATOR_PYTHON": str(Path(sys.executable).resolve()),
        "TYPE_BRIDGE_NODE_INTG_DATABASE": f"type_bridge_v2_node_{os.getpid()}",
    }
    node_live("generated package round-trips exact models on TypeDB", node_env)
    rust_env = query_environment(environment, directory, "rust", nonce) | {
        "TYPE_BRIDGE_RUST_PROJECTION_INTG_DATABASE": f"type_bridge_v2_rust_{os.getpid()}"
    }
    run(
        rust_live(
            "generated_rust_projection_round_trips_exact_live_models", "rust_projection_live"
        ),
        env=rust_env,
    )
    c_target = directory / "c-target"
    c_env = query_environment(environment, directory, "c", nonce) | {
        "TYPE_BRIDGE_C_PROJECTION_INTG_DATABASE": f"type_bridge_v2_c_{os.getpid()}",
        "TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER": "1",
        "ACCEPTANCE_TARGET_DIR": str(c_target),
        "CARGO_TARGET_DIR": str(c_target),
    }
    run(
        [
            "cargo",
            "build",
            "--quiet",
            "--locked",
            "--manifest-path",
            str(CORE / "Cargo.toml"),
            "-p",
            "type-bridge-c",
            "--lib",
        ],
        env=c_env,
    )
    run(
        rust_live(
            "live_c17_generated_person_and_membership_crud_round_trips_exact_3_12_3",
            "c_projection_live",
        ),
        env=c_env,
    )


def compare_queries(directory: Path, environment: dict[str, str]) -> None:
    run(
        python_command(
            "compare_sdk_conformance.py",
            *(str(directory / f"v1/{binding}.json") for binding in V1_BINDINGS),
        ),
        env=environment,
    )
    run(
        python_command(
            "compare_sdk_conformance_v2.py",
            *(str(directory / f"v2/{binding}.json") for binding in BINDINGS),
        ),
        env=environment,
    )


def model_projections(directory: Path, environment: dict[str, str]) -> None:
    run(
        python_command("run_projected_parity.py", "--output", str(directory / "projected-parity")),
        env=environment,
    )
    projected_environment = environment | {
        "TYPE_BRIDGE_PROJECTED_LIVE_ADDRESS": environment["TYPEDB_ADDRESS"],
        "TYPE_BRIDGE_PROJECTED_LIVE_HTTP_PORT": environment["TYPEDB_HTTP_PORT"],
    }
    run(
        python_command(
            "run_generated_live.py", "projected", "--output", str(directory / "projected-live")
        ),
        env=projected_environment,
    )
    manager_environment = environment | {
        "TYPE_BRIDGE_MANAGER_LIVE_ADDRESS": environment["TYPEDB_ADDRESS"],
        "TYPE_BRIDGE_MANAGER_LIVE_HTTP_PORT": environment["TYPEDB_HTTP_PORT"],
    }
    run(
        python_command("run_generated_live.py", "manager", "--output", str(directory / "manager")),
        env=manager_environment,
    )


def model_artifacts(directory: Path, environment: dict[str, str]) -> None:
    atomic = directory / "atomic-generation.json"
    run(
        cargo_test(
            "type-bridge-cli",
            "schema_generation_atomicity_tests::injected_c_emitter_failure_preserves_all_four_ordered_packages",
        ),
        env=environment | {"TYPE_BRIDGE_SDK_V3_ATOMIC_GENERATION_OUTPUT": str(atomic)},
    )
    field_identity = directory / "field-identity.json"
    run(
        cargo_test(
            "type-bridge-schema-codegen",
            "exact_sdk_v3_field_name_identity_is_source_bound",
            suite="sdk_v3_fingerprints",
        ),
        env=environment | {"TYPE_BRIDGE_SDK_V3_FIELD_IDENTITY_OUTPUT": str(field_identity)},
    )
    run(python_command("validate_generation_artifact.py", "atomic", str(atomic)), env=environment)
    run(
        python_command("validate_generation_artifact.py", "field-identity", str(field_identity)),
        env=environment,
    )


def model_proofs(directory: Path, environment: dict[str, str], nonce: str) -> None:
    package_commands = {
        "python": [
            sys.executable,
            "type-bridge-core/crates/schema-codegen/tests/acceptance/check.py",
        ],
        "node": [
            "node",
            "type-bridge-core/crates/schema-codegen/tests/typescript_acceptance/check.mjs",
        ],
        "rust": cargo_test(
            "type-bridge-schema-codegen",
            "sdk_v3_generated_package_integrity",
            suite="rust_acceptance",
        ),
        "c": cargo_test(
            "type-bridge-schema-codegen", "sdk_v3_generated_package_integrity", suite="c_emitter"
        ),
    }
    data_tests = {
        "python": (
            "type-bridge-core",
            "runtime_projection::tests::sdk_v3_python_data_plane_fragment",
        ),
        "node": ("type-bridge-node", "runtime_projection::tests::sdk_v3_node_data_plane_fragment"),
        "rust": ("type-bridge", "transaction::tests::sdk_v3_rust_data_plane_fragment"),
        "c": ("type-bridge-c", "runtime::tests::sdk_v3_c_data_plane_fragment"),
    }
    commands = {
        binding: (package_commands[binding], cargo_test(*data_tests[binding]))
        for binding in BINDINGS
    }
    produce_proofs(3, directory, environment, nonce, commands, ("package", "data"))


def model_live(directory: Path, environment: dict[str, str]) -> None:
    supplements = directory / "supplements"
    supplements.mkdir()
    python_env = environment | {
        "USE_DOCKER": "false",
        "TYPE_BRIDGE_SDK_V3_PYTHON_SUPPLEMENT": str(supplements / "python.json"),
    }
    run(
        [
            "uv",
            "run",
            "pytest",
            "-q",
            "-m",
            "integration",
            "tests/integration/schema/test_generated_projection_live.py::test_generated_data_model_runtime_v3_live",
        ],
        env=python_env,
    )
    run(["npm", "run", "build"], cwd=NODE, env=environment)
    run(["npm", "run", "typecheck:projection-integration"], cwd=NODE, env=environment)
    node_env = environment | {
        "TYPE_BRIDGE_SDK_V3_NODE_SUPPLEMENT": str(supplements / "node.json"),
        "TYPE_BRIDGE_NODE_INTG_DATABASE": f"type_bridge_v3_node_{os.getpid()}",
    }
    node_live("node.generated_data_model_runtime_v3_live", node_env)
    rust_env = environment | {
        "TYPE_BRIDGE_SDK_V3_RUST_SUPPLEMENT": str(supplements / "rust.json"),
        "TYPE_BRIDGE_RUST_PROJECTION_INTG_DATABASE": f"type_bridge_v3_rust_{os.getpid()}",
    }
    run(
        rust_live(
            "generated_rust_projection_round_trips_exact_live_models", "rust_projection_live"
        ),
        env=rust_env,
    )
    c_env = environment | {
        "TYPE_BRIDGE_SDK_V3_C_SUPPLEMENT": str(supplements / "c.json"),
        "TYPE_BRIDGE_C_QUERY_INTG_DATABASE": f"type_bridge_v3_c_{os.getpid()}",
        "TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER": "1",
    }
    run(rust_live("generated_data_model_runtime_v3_live", "c_projection_live"), env=c_env)


def assemble_models(directory: Path, environment: dict[str, str], nonce: str) -> tuple[Path, ...]:
    reports = directory / "reports"
    reports.mkdir()
    for binding in BINDINGS:
        run(
            python_command(
                "assemble_sdk_conformance_v3.py",
                "--projected-parity",
                str(directory / f"projected-parity/{binding}.json"),
                "--projected-live",
                str(directory / f"projected-live/{binding}.json"),
                "--manager",
                str(directory / f"manager/{binding}.json"),
                "--atomic-generation",
                str(directory / "atomic-generation.json"),
                "--field-identity",
                str(directory / "field-identity.json"),
                "--supplement",
                str(directory / f"supplements/{binding}.json"),
                "--binding",
                binding,
                "--run-nonce",
                nonce,
                "--output",
                str(reports / f"{binding}.json"),
                str(directory / f"fragments/{binding}-package.json"),
                str(directory / f"fragments/{binding}-data.json"),
            ),
            env=environment,
        )
    paths = tuple(reports / f"{binding}.json" for binding in BINDINGS)
    run(
        python_command("compare_sdk_conformance_v3.py", *(str(path) for path in paths)),
        env=environment,
    )
    return paths


def published_port(project: str, container_port: str) -> str:
    value = run(
        [
            "docker",
            "compose",
            "-p",
            project,
            "-f",
            str(ROOT / "docker-compose.yml"),
            "port",
            "typedb",
            container_port,
        ],
        capture=True,
    )
    if not value.startswith("0.0.0.0:") and (not value.startswith("[::]:")):
        raise RunnerError(f"unexpected published TypeDB port: {value!r}")
    return value.rsplit(":", 1)[1]


def write_cleanup(path: Path, binding: str) -> None:
    path.write_bytes(
        codec_contract.canonical_json_bytes(
            {
                "binding": binding,
                "format": "typebridge.sdk-v5-cleanup-evidence/v1",
                "managed_database_absent": True,
                "partial_output_absent": True,
                "temporary_evidence_absent": True,
            }
        )
    )


def codec_offline(directory: Path, environment: dict[str, str]) -> None:
    python_env = environment | {
        "TYPE_BRIDGE_SDK_V5_PYTHON_CORPUS": str(directory / "python-corpus.json"),
        "TYPE_BRIDGE_SDK_V5_PYTHON_OPERATIONAL_EVIDENCE": str(
            directory / "python-operational.json"
        ),
    }
    run(["uv", "run", "pytest", "-q", "-m", "integration", str(PYTHON_PROVIDER)], env=python_env)
    run(["npm", "run", "build"], cwd=NODE, env=environment)
    node_env = environment | {
        "TYPE_BRIDGE_SDK_V5_NODE_CORPUS": str(directory / "node-corpus.json"),
        "TYPE_BRIDGE_SDK_V5_NODE_OPERATIONAL_EVIDENCE": str(directory / "node-operational.json"),
    }
    run([sys.executable, str(NODE_PROVIDER)], env=node_env)
    rust_env = environment | {
        "TYPE_BRIDGE_SDK_V5_RUST_CORPUS": str(directory / "rust-corpus.json"),
        "TYPE_BRIDGE_SDK_V5_RUST_OPERATIONAL_EVIDENCE": str(directory / "rust-operational.json"),
    }
    run(
        cargo_test(
            "type-bridge-schema-codegen",
            "generated_rust_sdk_v5_canonical_codec",
            suite="rust_acceptance",
        ),
        env=rust_env,
    )
    c_env = environment | {
        "TYPE_BRIDGE_SDK_V5_C_CORPUS": str(directory / "c-corpus.json"),
        "TYPE_BRIDGE_SDK_V5_C_OPERATIONAL_EVIDENCE": str(directory / "c-operational.json"),
    }
    run(["uv", "run", "pytest", "-q", "-m", "integration", str(C_PROVIDER)], env=c_env)
    run(
        [
            sys.executable,
            str(ROOT / "scripts/ci/compare_sdk_v5_corpora.py"),
            *(
                item
                for binding in BINDINGS
                for item in (f"--{binding}", str(directory / f"{binding}-corpus.json"))
            ),
        ],
        env=environment,
    )
    run(
        [
            sys.executable,
            str(ROOT / "scripts/ci/compare_sdk_v5_operational.py"),
            *(
                item
                for binding in BINDINGS
                for item in (f"--{binding}", str(directory / f"{binding}-operational.json"))
            ),
        ],
        env=environment,
    )


def codec_live(directory: Path, environment: dict[str, str]) -> None:
    python_env = environment | {
        "USE_DOCKER": "false",
        "TYPE_BRIDGE_SDK_V5_PYTHON_EVIDENCE": str(directory / "python-live.json"),
    }
    run(
        [
            "uv",
            "run",
            "pytest",
            "-q",
            "-m",
            "integration",
            "tests/integration/schema/test_generated_projection_live.py::test_generated_canonical_serialization_v5_live",
        ],
        env=python_env,
    )
    write_cleanup(directory / "python-cleanup.json", "python")
    node_env = environment | {
        "TYPE_BRIDGE_SDK_V5_NODE_EVIDENCE": str(directory / "node-live.json"),
        "TYPE_BRIDGE_NODE_INTG_DATABASE": f"type_bridge_v5_node_{os.getpid()}",
        "TYPEDB_VERSION": "3.12.3",
    }
    run(["npm", "run", "typecheck:projection-integration"], cwd=NODE, env=node_env)
    node_live("node.generated_canonical_serialization_v5_live", node_env)
    write_cleanup(directory / "node-cleanup.json", "node")
    rust_env = environment | {
        "TYPE_BRIDGE_SDK_V5_RUST_EVIDENCE": str(directory / "rust-live.json"),
        "TYPE_BRIDGE_RUST_PROJECTION_INTG_DATABASE": f"type_bridge_v5_rust_{os.getpid()}",
    }
    run(
        rust_live(
            "generated_rust_projection_round_trips_exact_live_models", "rust_projection_live"
        ),
        env=rust_env,
    )
    write_cleanup(directory / "rust-cleanup.json", "rust")
    c_env = environment | {
        "TYPE_BRIDGE_SDK_V5_C_EVIDENCE": str(directory / "c-live.json"),
        "TYPE_BRIDGE_C_PROJECTION_INTG_DATABASE": f"type_bridge_v5_c_{os.getpid()}",
        "TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER": "1",
    }
    run(
        rust_live(
            "live_c17_generated_person_and_membership_crud_round_trips_exact_3_12_3",
            "c_projection_live",
        ),
        env=c_env,
    )
    write_cleanup(directory / "c-cleanup.json", "c")


def assemble_codec(directory: Path, output: Path, environment: dict[str, str]) -> None:
    output.mkdir(parents=True)
    for binding in BINDINGS:
        nonce = os.urandom(32).hex()
        run(
            python_command(
                "assemble_sdk_conformance_v5.py",
                "--corpus",
                str(directory / f"{binding}-corpus.json"),
                "--live-evidence",
                str(directory / f"{binding}-live.json"),
                "--operational-evidence",
                str(directory / f"{binding}-operational.json"),
                "--cleanup-evidence",
                str(directory / f"{binding}-cleanup.json"),
                "--binding",
                binding,
                "--run-nonce",
                nonce,
                "--output",
                str(output / f"{binding}.json"),
            ),
            env=environment,
        )
    run(
        python_command(
            "compare_sdk_conformance_v5.py",
            *(str(output / f"{binding}.json") for binding in BINDINGS),
        ),
        env=environment,
    )


def regular(path: Path, label: str) -> Path:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise RunnerError(f"{label} cannot be inspected") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise RunnerError(f"{label} must be a regular non-symlink file")
    return path.resolve()


def predecessor_paths(root: Path, binding: str) -> tuple[tuple[int, Path], ...]:
    return tuple(
        (version, regular(root / f"v{version}" / f"{binding}.json", f"V{version} {binding} report"))
        for version in conformance.predecessor_versions(binding)
    )


def predecessor_arguments(paths: tuple[tuple[int, Path], ...]) -> list[str]:
    return [item for version, path in paths for item in ("--predecessor", f"{version}={path}")]


def c_surface_consumer(acceptance: dict[str, Any], *, source_commit: str) -> dict[str, Any]:
    artifacts = acceptance["artifacts"]
    return {
        "format": SURFACE_CONSUMER_FORMAT,
        "binding": "c",
        "source-commit": source_commit,
        "surface-sha256": artifacts["generated-package"]["sha256"],
        "cli-artifact-id": artifacts["cli"]["artifact-id"],
        "runtime-provenance": "artifact-c-runtime",
        "checks": ["query-c17-cpp17", "sanitizers", "loader-unload"],
        "cleanup": {"temporary-consumer-absent": True},
        "publication-authority": False,
    }


def assemble_artifacts(
    directory: Path,
    *,
    predecessors: Path,
    acceptance_path: Path,
    provider: Path,
    live: Path,
    cli: Path,
    runtime: Path,
    generated: Path,
    source_commit: str,
    acceptance: dict[str, Any],
    environment: dict[str, str],
) -> tuple[Path, ...]:
    nonce = secrets.token_hex(32)
    surfaces = directory / "surfaces"
    consumers = directory / "consumers"
    reports = directory / "reports"
    for path in (surfaces, consumers, reports):
        path.mkdir()
    run(
        python_command("sdk_v6_surfaces.py", "build", str(cli), "--output", str(surfaces)),
        env=environment,
    )
    for binding in BINDINGS[:-1]:
        surface = surfaces / f"type-bridge-{binding}-generated-sdk-v6.tar.gz"
        interpreter = ["uv", "run", "python"] if binding == "python" else [sys.executable]
        run(
            [
                *interpreter,
                "scripts/ci/validate_sdk_v6_surface_consumer.py",
                str(surface),
                "--binding",
                binding,
                "--output",
                str(consumers / f"{binding}.json"),
            ],
            env=environment,
        )
    (consumers / "c.json").write_bytes(
        conformance.canonical_json_bytes(
            c_surface_consumer(acceptance, source_commit=source_commit)
        )
    )
    output: list[Path] = []
    for binding in BINDINGS:
        historical = predecessor_paths(predecessors, binding)
        historical_args = predecessor_arguments(historical)
        report = reports / f"{binding}.json"
        surface = (
            generated
            if binding == "c"
            else surfaces / f"type-bridge-{binding}-generated-sdk-v6.tar.gz"
        )
        run(
            python_command(
                "assemble_sdk_conformance_v6.py",
                "--source-commit",
                source_commit,
                "--surface-consumer",
                str(consumers / f"{binding}.json"),
                "--binding",
                binding,
                "--run-nonce",
                nonce,
                "--acceptance",
                str(acceptance_path),
                "--provider",
                str(provider),
                "--live",
                str(live),
                "--cli",
                str(cli),
                "--runtime",
                str(runtime),
                "--generated",
                str(generated),
                "--generated-surface",
                str(surface),
                *historical_args,
                "--output",
                str(report),
            ),
            env=environment,
        )
        output.append(report)
    run(
        python_command("compare_sdk_conformance_v6.py", *(str(path) for path in output)),
        env=environment,
    )
    return tuple(output)


def administration(directory: Path, environment: dict[str, str]) -> tuple[Path, ...]:
    project = f"tb-sdk-v4-live-{os.getpid()}"
    temporary = directory
    compose = ["docker", "compose", "-p", project, "-f", str(ROOT / "docker-compose.yml")]
    interrupted = False

    def mark_interrupted(_signum: int, _frame: object) -> None:
        nonlocal interrupted
        interrupted = True
        raise KeyboardInterrupt

    previous = signal.signal(signal.SIGTERM, mark_interrupted)
    try:
        env = environment.copy()
        env.update(
            {"TYPEDB_IMAGE": "typedb/typedb:3.12.3", "TYPEDB_PORT": "0", "TYPEDB_HTTP_PORT": "0"}
        )
        run([*compose, "up", "-d", "--wait", "typedb"], env=env)
        driver_port = published_port(project, "1729")
        http_port = published_port(project, "8000")
        producer_env = env | {
            "TYPEDB_ADDRESS": f"127.0.0.1:{driver_port}",
            "TYPEDB_HTTP_PORT": http_port,
            "TYPEDB_USERNAME": "admin",
            "TYPEDB_PASSWORD": "password",
            "TYPE_BRIDGE_SDK_V4_REPORT_DIR": str(temporary),
        }
        for test in (f"sdk_v4_{binding}_live" for binding in BINDINGS):
            run(
                [
                    "cargo",
                    "test",
                    "--manifest-path",
                    str(CORE / "Cargo.toml"),
                    "-p",
                    "type-bridge-cli",
                    "--test",
                    test,
                    "--",
                    "--ignored",
                    "--nocapture",
                ],
                env=producer_env,
            )
        paths = [temporary / f"{binding}-sdk-v4-report.json" for binding in BINDINGS]
        missing = [path.name for path in paths if not path.is_file()]
        if missing:
            raise RunnerError(f"validated report publication is incomplete: {missing}")
        comparison = json.loads(
            run(
                [
                    sys.executable,
                    str(ROOT / "scripts/ci/compare_sdk_conformance_v4.py"),
                    *(str(path) for path in paths),
                ],
                capture=True,
            )
        )
        comparison["report_sha256"] = {
            binding: hashlib.sha256(path.read_bytes()).hexdigest()
            for binding, path in zip(("python", "node", "rust", "c"), paths, strict=True)
        }
        print(json.dumps(comparison, indent=2, sort_keys=True))
        return tuple(paths)
    except (KeyboardInterrupt, RunnerError) as error:
        if interrupted:
            print("Sdk V4 live fan-in interrupted", file=sys.stderr)
        else:
            print(f"Sdk V4 live fan-in failed: {error}", file=sys.stderr)
        raise RunnerError("administration acceptance failed") from error
    finally:
        signal.signal(signal.SIGTERM, previous)
        subprocess.run(
            [*compose, "down", "-v", "--remove-orphans"],
            cwd=ROOT,
            env=os.environ.copy(),
            check=False,
        )


def artifact_reports(
    directory: Path, arguments: argparse.Namespace, environment: dict[str, str]
) -> tuple[Path, ...]:
    inputs = {
        name: regular(getattr(arguments, name), name)
        for name in ("acceptance", "provider", "live", "cli", "runtime", "generated")
    }
    manifest = cli_artifact.validate(inputs["cli"])
    acceptance = artifact_journey.load_canonical_report(inputs["acceptance"])
    artifact_journey.validate_acceptance_report(
        acceptance,
        inputs["provider"],
        inputs["live"],
        inputs["cli"],
        inputs["runtime"],
        inputs["generated"],
    )
    return assemble_artifacts(
        directory,
        predecessors=arguments.predecessors.resolve(),
        acceptance_path=inputs["acceptance"],
        provider=inputs["provider"],
        live=inputs["live"],
        cli=inputs["cli"],
        runtime=inputs["runtime"],
        generated=inputs["generated"],
        source_commit=manifest["source-commit"],
        acceptance=acceptance,
        environment=environment,
    )


def execute(
    suite: str, directory: Path, environment: dict[str, str], arguments: argparse.Namespace
) -> dict[str, Path]:
    """Produce and independently validate a complete report set before publication."""
    if suite == "queries":
        nonce = secrets.token_hex(32)
        query_proofs(directory, environment, nonce)
        query_live(directory, environment, nonce)
        compare_queries(directory, environment)
        return {
            f"v{version}/{binding}.json": directory / f"v{version}/{binding}.json"
            for version in (1, 2)
            for binding in (V1_BINDINGS if version == 1 else BINDINGS)
        }
    if suite == "models":
        nonce = secrets.token_hex(32)
        model_projections(directory, environment)
        model_artifacts(directory, environment)
        model_proofs(directory, environment, nonce)
        model_live(directory, environment)
        reports = assemble_models(directory, environment, nonce)
    elif suite == "administration":
        reports = administration(directory, environment)
    elif suite == "serialization":
        codec_offline(directory, environment)
        codec_live(directory, environment)
        report_directory = directory / "reports"
        assemble_codec(directory, report_directory, environment)
        reports = tuple(report_directory / f"{binding}.json" for binding in BINDINGS)
    elif suite == "artifacts":
        reports = artifact_reports(directory, arguments, environment)
    else:
        raise RunnerError(f"unknown SDK suite: {suite}")
    return {f"{binding}.json": path for binding, path in zip(BINDINGS, reports, strict=True)}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("suite", choices=SUITES)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--address", default="127.0.0.1:1729")
    parser.add_argument("--http-port", default="8000")
    artifact_inputs = (
        "predecessors",
        "acceptance",
        "provider",
        "live",
        "cli",
        "runtime",
        "generated",
    )
    for name in artifact_inputs:
        parser.add_argument(f"--{name}", type=Path)
    arguments = parser.parse_args(argv)
    if arguments.suite == "artifacts":
        missing = [f"--{name}" for name in artifact_inputs if getattr(arguments, name) is None]
        if missing:
            parser.error("artifacts requires " + ", ".join(missing))
    elif any(getattr(arguments, name) is not None for name in artifact_inputs):
        parser.error("artifact inputs are only valid for the artifacts suite")
    try:
        output = arguments.output.resolve()
        if output.exists() or output == ROOT or ROOT in output.parents:
            raise RunnerError("output must be a new directory outside the checkout")
        environment = os.environ.copy()
        if arguments.suite in {"queries", "models", "serialization"}:
            environment.update(
                {
                    "TYPEDB_ADDRESS": arguments.address,
                    "TYPEDB_HTTP_PORT": str(arguments.http_port),
                    "TYPEDB_USERNAME": "admin",
                    "TYPEDB_PASSWORD": "password",
                }
            )
        if arguments.suite in {"queries", "models"}:
            environment["TYPEDB_VERSION"] = "3.12.3"
        if arguments.suite == "queries":
            environment["TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE"] = "typedb-3.12.1/v1"
        with tempfile.TemporaryDirectory(prefix=f"typebridge-sdk-{arguments.suite}-") as value:
            reports = execute(arguments.suite, Path(value), environment, arguments)
            publish_files(reports, output, checkout=ROOT)
        return 0
    except (
        RunnerError,
        PublishError,
        artifact_journey.JourneyError,
        cli_artifact.ArtifactError,
        codec_contract.ContractError,
        KeyError,
        OSError,
    ) as error:
        print(f"SDK {arguments.suite} acceptance failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
