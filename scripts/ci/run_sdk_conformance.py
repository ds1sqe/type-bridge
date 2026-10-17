#!/usr/bin/env python3
"""Run SDK query, model, administration, serialization, artifact and writes acceptance."""

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
import time
from collections.abc import Iterator, Mapping
from contextlib import contextmanager
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
import c_artifact_journey as artifact_journey
import sdk_artifacts_compare as conformance
import sdk_profile_matrix as profile_matrix
import sdk_serialization_compare as codec_contract
import sdk_writes_compare as writes_contract
import standalone_cli_artifact as cli_artifact
from persist_binding_reports import PublishError, publish_files

ROOT = Path(__file__).resolve().parents[2]
CORE = ROOT / "type-bridge-core"
NODE = CORE / "crates/node"
BINDINGS = ("python", "node", "rust", "c")
SUITES = tuple(profile_matrix.SUITES)
# Suites outside the FULL-C report fan-in: `writes` runs one staged program in
# every binding and needs the staged surface of TypeDB 3.13.6.
STAGED_SUITES = ("writes",)
WRITES = ROOT / "tests/contracts/sdk_conformance/writes"
# Each suite directory: its own schema and generated packages, one program per SDK.
WRITES_SUITES = ("purchase", "constraints")
WRITES_PROGRAMS = ROOT / "examples/purchase"
WRITES_FIXTURE_TIMEOUT = 600.0
PYTHON_PROVIDER = ROOT / "tests/integration/schema/test_sdk_serialization_codec.py"
NODE_PROVIDER = CORE / "crates/schema-codegen/tests/typescript_acceptance/codec_check.py"
C_PROVIDER = ROOT / "tests/integration/schema/test_sdk_serialization_c_codec.py"
SURFACE_CONSUMER_FORMAT = "typebridge.sdk-artifacts-surface-consumer/v1"


class RunnerError(RuntimeError):
    """An SDK producer, comparator, or report publication failed."""


def _runner_error(_code: str, message: str) -> RunnerError:
    return RunnerError(message)


def lane(profile: str | None, environment: Mapping[str, str]) -> profile_matrix.ProfileIdentity:
    """The matrix entry a run targets.

    ``profile`` (``--semantic-profile``) wins, then
    ``TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE``, then the catalogs' own profile.
    Every conformance set must list it.
    """

    if profile is None:
        profile = environment.get(profile_matrix.SEMANTIC_PROFILE_ENV) or (
            profile_matrix.catalog_profile("artifacts")
        )
    return profile_matrix.select_lane(profile, error=_runner_error)


def typedb_version(environment: Mapping[str, str]) -> str:
    """The exact TypeDB server version the run's profile names."""

    return environment.get("TYPEDB_VERSION") or lane(None, environment).live_server_version


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
    suite: str, environment: dict[str, str], destination: Path, nonce: str
) -> dict[str, str]:
    return environment | {
        f"TYPE_BRIDGE_SDK_{suite.upper()}_PROOF_FRAGMENT": str(destination),
        f"TYPE_BRIDGE_SDK_{suite.upper()}_PROOF_RUN_NONCE": nonce,
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
    suite: str,
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
            run(command, env=fragment_environment(suite, environment, destination, nonce))
            paths.append(destination)
        run(
            python_command(
                "proof_fragments.py",
                "--suite",
                str(suite),
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
            cargo_test(
                "type-bridge", "remote::tests::sdk_queries_rust_deterministic_proof_fragment"
            ),
        ),
        "c": (
            cargo_test("type-bridge-c", "query::tests::sdk_queries_c_deterministic_proof_fragment"),
        ),
    }
    produce_proofs("queries", directory, environment, nonce, commands)


def query_environment(
    environment: dict[str, str], directory: Path, binding: str, nonce: str
) -> dict[str, str]:
    fragments = sorted((directory / "fragments").glob(f"{binding}-*.json"))
    if not fragments:
        raise RunnerError(f"{binding} queries proof fragments are missing")
    result = environment | {
        "TYPE_BRIDGE_SDK_QUERIES_REPORT": str(directory / f"reports/{binding}.json"),
        "TYPE_BRIDGE_SDK_QUERIES_PROOF_FRAGMENTS": os.pathsep.join(str(path) for path in fragments),
        "TYPE_BRIDGE_SDK_QUERIES_PROOF_RUN_NONCE": nonce,
    }
    return result


def query_live(directory: Path, environment: dict[str, str], nonce: str) -> None:
    (directory / "reports").mkdir()
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
        "TYPE_BRIDGE_SDK_QUERIES_VALIDATOR_PYTHON": str(Path(sys.executable).resolve()),
        "TYPE_BRIDGE_NODE_INTG_DATABASE": f"type_bridge_queries_node_{os.getpid()}",
    }
    node_live("generated package round-trips exact models on TypeDB", node_env)
    rust_env = query_environment(environment, directory, "rust", nonce) | {
        "TYPE_BRIDGE_RUST_PROJECTION_INTG_DATABASE": f"type_bridge_queries_rust_{os.getpid()}"
    }
    run(
        rust_live(
            "generated_rust_projection_round_trips_exact_live_models", "rust_projection_live"
        ),
        env=rust_env,
    )
    c_target = Path(environment.get("CARGO_TARGET_DIR", directory / "c-target"))
    c_env = query_environment(environment, directory, "c", nonce) | {
        "TYPE_BRIDGE_C_PROJECTION_INTG_DATABASE": f"type_bridge_queries_c_{os.getpid()}",
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
            "live_c17_generated_person_and_membership_crud_round_trips_on_profile_server",
            "c_projection_live",
        ),
        env=c_env,
    )


def compare_queries(directory: Path, environment: dict[str, str]) -> None:
    run(
        python_command(
            "sdk_queries_compare.py",
            *(str(directory / f"reports/{binding}.json") for binding in BINDINGS),
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
        env=environment | {"TYPE_BRIDGE_SDK_MODELS_ATOMIC_GENERATION_OUTPUT": str(atomic)},
    )
    field_identity = directory / "field-identity.json"
    run(
        cargo_test(
            "type-bridge-schema-codegen",
            "exact_sdk_models_field_name_identity_is_source_bound",
            suite="sdk_models_fingerprints",
        ),
        env=environment | {"TYPE_BRIDGE_SDK_MODELS_FIELD_IDENTITY_OUTPUT": str(field_identity)},
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
            "sdk_models_generated_package_integrity",
            suite="rust_acceptance",
        ),
        "c": cargo_test(
            "type-bridge-schema-codegen",
            "sdk_models_generated_package_integrity",
            suite="c_emitter",
        ),
    }
    data_tests = {
        "python": (
            "type-bridge-core",
            "runtime_projection::tests::sdk_models_python_data_plane_fragment",
        ),
        "node": (
            "type-bridge-node",
            "runtime_projection::tests::sdk_models_node_data_plane_fragment",
        ),
        "rust": ("type-bridge", "transaction::tests::sdk_models_rust_data_plane_fragment"),
        "c": ("type-bridge-c", "runtime::tests::sdk_models_c_data_plane_fragment"),
    }
    commands = {
        binding: (package_commands[binding], cargo_test(*data_tests[binding]))
        for binding in BINDINGS
    }
    produce_proofs("models", directory, environment, nonce, commands, ("package", "data"))


def model_live(directory: Path, environment: dict[str, str]) -> None:
    supplements = directory / "supplements"
    supplements.mkdir()
    python_env = environment | {
        "USE_DOCKER": "false",
        "TYPE_BRIDGE_SDK_MODELS_PYTHON_SUPPLEMENT": str(supplements / "python.json"),
    }
    run(
        [
            "uv",
            "run",
            "pytest",
            "-q",
            "-m",
            "integration",
            "tests/integration/schema/test_generated_projection_live.py::test_sdk_models_data_model_runtime_live",
        ],
        env=python_env,
    )
    run(["npm", "run", "build"], cwd=NODE, env=environment)
    run(["npm", "run", "typecheck:projection-integration"], cwd=NODE, env=environment)
    node_env = environment | {
        "TYPE_BRIDGE_SDK_MODELS_NODE_SUPPLEMENT": str(supplements / "node.json"),
        "TYPE_BRIDGE_NODE_INTG_DATABASE": f"type_bridge_models_node_{os.getpid()}",
    }
    node_live("node.sdk_models_data_model_runtime_live", node_env)
    rust_env = environment | {
        "TYPE_BRIDGE_SDK_MODELS_RUST_SUPPLEMENT": str(supplements / "rust.json"),
        "TYPE_BRIDGE_RUST_PROJECTION_INTG_DATABASE": f"type_bridge_models_rust_{os.getpid()}",
    }
    run(
        rust_live(
            "generated_rust_projection_round_trips_exact_live_models", "rust_projection_live"
        ),
        env=rust_env,
    )
    c_env = environment | {
        "TYPE_BRIDGE_SDK_MODELS_C_SUPPLEMENT": str(supplements / "c.json"),
        "TYPE_BRIDGE_C_QUERY_INTG_DATABASE": f"type_bridge_models_c_{os.getpid()}",
        "TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER": "1",
    }
    run(rust_live("sdk_models_data_model_runtime_live", "c_projection_live"), env=c_env)


def assemble_models(directory: Path, environment: dict[str, str], nonce: str) -> tuple[Path, ...]:
    reports = directory / "reports"
    reports.mkdir()
    for binding in BINDINGS:
        run(
            python_command(
                "sdk_models_assemble.py",
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
        python_command("sdk_models_compare.py", *(str(path) for path in paths)),
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
                "format": "typebridge.sdk-serialization-cleanup-evidence/v1",
                "managed_database_absent": True,
                "partial_output_absent": True,
                "temporary_evidence_absent": True,
            }
        )
    )


def codec_offline(directory: Path, environment: dict[str, str]) -> None:
    python_env = environment | {
        "TYPE_BRIDGE_SDK_SERIALIZATION_PYTHON_CORPUS": str(directory / "python-corpus.json"),
        "TYPE_BRIDGE_SDK_SERIALIZATION_PYTHON_OPERATIONAL_EVIDENCE": str(
            directory / "python-operational.json"
        ),
    }
    run(["uv", "run", "pytest", "-q", "-m", "integration", str(PYTHON_PROVIDER)], env=python_env)
    run(["npm", "run", "build"], cwd=NODE, env=environment)
    node_env = environment | {
        "TYPE_BRIDGE_SDK_SERIALIZATION_NODE_CORPUS": str(directory / "node-corpus.json"),
        "TYPE_BRIDGE_SDK_SERIALIZATION_NODE_OPERATIONAL_EVIDENCE": str(
            directory / "node-operational.json"
        ),
    }
    run([sys.executable, str(NODE_PROVIDER)], env=node_env)
    rust_env = environment | {
        "TYPE_BRIDGE_SDK_SERIALIZATION_RUST_CORPUS": str(directory / "rust-corpus.json"),
        "TYPE_BRIDGE_SDK_SERIALIZATION_RUST_OPERATIONAL_EVIDENCE": str(
            directory / "rust-operational.json"
        ),
    }
    run(
        cargo_test(
            "type-bridge-schema-codegen",
            "sdk_serialization_canonical_codec",
            suite="rust_acceptance",
        ),
        env=rust_env,
    )
    c_env = environment | {
        "TYPE_BRIDGE_SDK_SERIALIZATION_C_CORPUS": str(directory / "c-corpus.json"),
        "TYPE_BRIDGE_SDK_SERIALIZATION_C_OPERATIONAL_EVIDENCE": str(
            directory / "c-operational.json"
        ),
    }
    run(["uv", "run", "pytest", "-q", "-m", "integration", str(C_PROVIDER)], env=c_env)
    run(
        [
            sys.executable,
            str(ROOT / "scripts/ci/sdk_serialization_corpora.py"),
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
            str(ROOT / "scripts/ci/sdk_serialization_operational.py"),
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
        "TYPE_BRIDGE_SDK_SERIALIZATION_PYTHON_EVIDENCE": str(directory / "python-live.json"),
    }
    run(
        [
            "uv",
            "run",
            "pytest",
            "-q",
            "-m",
            "integration",
            "tests/integration/schema/test_generated_projection_live.py::test_sdk_serialization_live",
        ],
        env=python_env,
    )
    write_cleanup(directory / "python-cleanup.json", "python")
    node_env = environment | {
        "TYPE_BRIDGE_SDK_SERIALIZATION_NODE_EVIDENCE": str(directory / "node-live.json"),
        "TYPE_BRIDGE_NODE_INTG_DATABASE": f"type_bridge_serialization_node_{os.getpid()}",
        "TYPEDB_VERSION": typedb_version(environment),
    }
    run(["npm", "run", "typecheck:projection-integration"], cwd=NODE, env=node_env)
    node_live("node.sdk_serialization_live", node_env)
    write_cleanup(directory / "node-cleanup.json", "node")
    rust_env = environment | {
        "TYPE_BRIDGE_SDK_SERIALIZATION_RUST_EVIDENCE": str(directory / "rust-live.json"),
        "TYPE_BRIDGE_RUST_PROJECTION_INTG_DATABASE": f"type_bridge_serialization_rust_{os.getpid()}",
    }
    run(
        rust_live(
            "generated_rust_projection_round_trips_exact_live_models", "rust_projection_live"
        ),
        env=rust_env,
    )
    write_cleanup(directory / "rust-cleanup.json", "rust")
    c_env = environment | {
        "TYPE_BRIDGE_SDK_SERIALIZATION_C_EVIDENCE": str(directory / "c-live.json"),
        "TYPE_BRIDGE_C_PROJECTION_INTG_DATABASE": f"type_bridge_serialization_c_{os.getpid()}",
        "TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER": "1",
    }
    run(
        rust_live(
            "live_c17_generated_person_and_membership_crud_round_trips_on_profile_server",
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
                "sdk_serialization_assemble.py",
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
            "sdk_serialization_compare.py",
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


def predecessor_paths(root: Path, binding: str) -> tuple[tuple[str, Path], ...]:
    return tuple(
        (suite, regular(root / suite / f"{binding}.json", f"{suite} {binding} report"))
        for suite in conformance.predecessor_suites(binding)
    )


def predecessor_arguments(paths: tuple[tuple[str, Path], ...]) -> list[str]:
    return [item for suite, path in paths for item in ("--predecessor", f"{suite}={path}")]


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
        python_command("sdk_artifacts_surfaces.py", "build", str(cli), "--output", str(surfaces)),
        env=environment,
    )
    for binding in BINDINGS[:-1]:
        surface = surfaces / f"type-bridge-{binding}-generated-sdk-artifacts.tar.gz"
        interpreter = ["uv", "run", "python"] if binding == "python" else [sys.executable]
        run(
            [
                *interpreter,
                "scripts/ci/sdk_artifacts_surface_consumer.py",
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
            else surfaces / f"type-bridge-{binding}-generated-sdk-artifacts.tar.gz"
        )
        run(
            python_command(
                "sdk_artifacts_assemble.py",
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
        python_command("sdk_artifacts_compare.py", *(str(path) for path in output)),
        env=environment,
    )
    return tuple(output)


def administration(directory: Path, environment: dict[str, str]) -> tuple[Path, ...]:
    project = f"tb-administration-live-{os.getpid()}"
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
            {
                "TYPEDB_IMAGE": f"typedb/typedb:{typedb_version(environment)}",
                "TYPEDB_PORT": "0",
                "TYPEDB_HTTP_PORT": "0",
            }
        )
        run([*compose, "up", "-d", "--wait", "typedb"], env=env)
        driver_port = published_port(project, "1729")
        http_port = published_port(project, "8000")
        producer_env = env | {
            "TYPEDB_ADDRESS": f"127.0.0.1:{driver_port}",
            "TYPEDB_HTTP_PORT": http_port,
            "TYPEDB_USERNAME": "admin",
            "TYPEDB_PASSWORD": "password",
            "TYPE_BRIDGE_SDK_ADMINISTRATION_REPORT_DIR": str(temporary),
        }
        for test in (f"sdk_administration_{binding}_live" for binding in BINDINGS):
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
        paths = [temporary / f"{binding}-administration-report.json" for binding in BINDINGS]
        missing = [path.name for path in paths if not path.is_file()]
        if missing:
            raise RunnerError(f"validated report publication is incomplete: {missing}")
        comparison = json.loads(
            run(
                [
                    sys.executable,
                    str(ROOT / "scripts/ci/sdk_administration_compare.py"),
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
            print("SDK administration live fan-in interrupted", file=sys.stderr)
        else:
            print(f"SDK administration live fan-in failed: {error}", file=sys.stderr)
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


def cargo_target(environment: Mapping[str, str]) -> Path:
    return Path(environment.get("CARGO_TARGET_DIR", CORE / "target"))


def writes_profile(suite: str) -> str:
    return json.loads((WRITES / suite / "program.json").read_text(encoding="utf-8"))[
        "semantic_profile"
    ]


def writes_packages(directory: Path, suite: str, environment: dict[str, str]) -> Path:
    """Generate the four packages from a private copy of the suite workspace."""

    workspace = directory / "workspace"
    workspace.mkdir()
    shutil.copy2(WRITES / suite / "typebridge.yaml", workspace / "typebridge.yaml")
    shutil.copytree(WRITES / suite / "schema", workspace / "schema")
    run(
        [
            "cargo",
            "build",
            "--quiet",
            "--locked",
            "--manifest-path",
            str(CORE / "Cargo.toml"),
            "-p",
            "type-bridge-cli",
        ],
        env=environment,
    )
    cli = cargo_target(environment) / "debug/type-bridge"
    run([str(cli), "schema", "generate"], cwd=workspace, env=environment)
    return workspace / "generated"


def writes_rust_manifest(generated: Path) -> str:
    rust = CORE / "crates/rust"
    return (
        '[package]\nname = "sdk-writes-purchase"\nversion = "0.0.0"\nedition = "2024"\n'
        "publish = false\n\n[dependencies]\n"
        f'models = {{ package = "type-bridge-generated-schema", path = "{generated}" }}\n'
        f'type-bridge = {{ path = "{rust}", features = ["typedb"] }}\n'
        'serde_json = "1"\n'
        'tokio = { version = "1", features = ["macros", "rt-multi-thread"] }\n\n'
        f'[patch.crates-io]\ntype-bridge = {{ path = "{rust}" }}\n\n[workspace]\n'
    )


def writes_programs(
    directory: Path, generated: Path, environment: dict[str, str]
) -> dict[str, tuple[list[str], Path, dict[str, str]]]:
    """Build each binding's program; return its command, directory and environment."""

    python_path = directory / "python"
    python_path.mkdir()
    shutil.copytree(generated / "python", python_path / "purchase_models")
    python = (
        ["uv", "run", "python", str(WRITES_PROGRAMS / "python/program.py")],
        ROOT,
        {"PYTHONPATH": str(python_path)},
    )

    run(["npm", "run", "build"], cwd=NODE, env=environment)
    typescript = generated / "typescript"
    run(
        [
            "npm",
            "install",
            "--ignore-scripts",
            "--no-package-lock",
            "--no-audit",
            "--no-fund",
            str(NODE),
        ],
        cwd=typescript,
        env=environment,
    )
    shutil.copy2(WRITES_PROGRAMS / "ts/program.ts", typescript / "src/program.ts")
    run(
        ["node", str(NODE / "node_modules/typescript/bin/tsc"), "-p", "tsconfig.json"],
        cwd=typescript,
        env=environment,
    )
    node = (["node", "dist/program.js"], typescript, {})

    # The consumer resolves offline from the workspace lockfile's versions.
    consumer = directory / "rust"
    (consumer / "src").mkdir(parents=True)
    shutil.copy2(WRITES_PROGRAMS / "rust/main.rs", consumer / "src/main.rs")
    shutil.copy2(CORE / "Cargo.lock", consumer / "Cargo.lock")
    (consumer / "Cargo.toml").write_text(writes_rust_manifest(generated / "rust"))
    manifest = ["--quiet", "--offline", "--manifest-path", str(consumer / "Cargo.toml")]
    run(["cargo", "build", *manifest], env=environment)
    rust = (["cargo", "run", *manifest], consumer, {})

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
        env=environment,
    )
    library = cargo_target(environment) / "debug"
    executable = directory / "c-program"
    run(
        [
            "cc",
            "-std=c17",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-I",
            str(generated / "c/include"),
            "-I",
            str(CORE / "crates/c/include"),
            str(WRITES_PROGRAMS / "c/program.c"),
            str(generated / "c/src/models.c"),
            "-L",
            str(library),
            "-ltype_bridge_c",
            f"-Wl,-rpath,{library}",
            "-o",
            str(executable),
        ],
        env=environment,
    )
    c = ([str(executable)], directory, {})
    return {"python": python, "node": node, "rust": rust, "c": c}


@contextmanager
def writes_fixture(
    directory: Path, suite: str, environment: dict[str, str]
) -> Iterator[dict[str, Any]]:
    """Serve the server crate's writes fixture until the block ends.

    Yields ``fixture.json``; on exit the fixture's ``observed.json`` is in the
    yielded mapping under ``"observed"``.
    """

    fixture_dir = directory / "fixture"
    fixture_dir.mkdir()
    command = [
        "cargo",
        "test",
        "--quiet",
        "--locked",
        "--manifest-path",
        str(CORE / "Cargo.toml"),
        "-p",
        "type-bridge-server",
        "--features",
        "v2-query",
        "--test",
        "v3_staged_integration_tests",
    ]
    run([*command, "--no-run"], env=environment)
    fixture_env = environment | {
        "TB_SDK_WRITES_FIXTURE_DIR": str(fixture_dir),
        "TB_SDK_WRITES_SUITE": suite,
    }
    process = subprocess.Popen(
        [*command, "sdk_writes_fixture_serves_until_stopped", "--", "--ignored", "--exact"],
        cwd=ROOT,
        env=fixture_env,
    )
    try:
        deadline = time.monotonic() + WRITES_FIXTURE_TIMEOUT
        while not (fixture_dir / "fixture.json").is_file():
            if process.poll() is not None:
                raise RunnerError("the writes fixture exited before serving")
            if time.monotonic() > deadline:
                raise RunnerError("the writes fixture did not start in time")
            time.sleep(0.2)
        fixture = json.loads((fixture_dir / "fixture.json").read_text(encoding="utf-8"))
        yield fixture
        (fixture_dir / "stop").touch()
        if process.wait(timeout=WRITES_FIXTURE_TIMEOUT) != 0:
            raise RunnerError("the writes fixture failed while stopping")
        fixture["observed"] = json.loads(
            (fixture_dir / "observed.json").read_text(encoding="utf-8")
        )
    finally:
        (fixture_dir / "stop").touch()
        if process.poll() is None:
            try:
                process.wait(timeout=WRITES_FIXTURE_TIMEOUT)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


def writes_run(
    programs: dict[str, tuple[list[str], Path, dict[str, str]]],
    suite: str,
    environment: dict[str, str],
    targets: Mapping[str, Mapping[str, str]],
) -> dict[str, Any]:
    """Run every program once, against its binding's live target when given."""

    outputs = {}
    for binding in BINDINGS:
        command, cwd, extra = programs[binding]
        target = targets.get(binding, {})
        program_env = environment | extra | {"TYPE_BRIDGE_SDK_WRITES_SUITE": suite}
        for name in (
            "TYPE_BRIDGE_SDK_WRITES_DIRECT_DATABASE",
            "TYPE_BRIDGE_SDK_WRITES_PROXY_ENDPOINT",
        ):
            program_env.pop(name, None)
        if target:
            program_env |= {
                "TYPE_BRIDGE_SDK_WRITES_DIRECT_DATABASE": target["direct_database"],
                "TYPE_BRIDGE_SDK_WRITES_PROXY_ENDPOINT": target["proxy_endpoint"],
            }
        output = json.loads(run(command, cwd=cwd, env=program_env, capture=True))
        if type(output) is not dict or output.get("binding") != binding:
            raise RunnerError(f"{binding} program reported another binding")
        outputs[binding] = output
    return outputs


def writes_reports(
    directory: Path,
    suite: str,
    outputs: dict[str, Any],
    profile: str,
    observed: Mapping[str, Any] | None,
) -> tuple[Path, ...]:
    reports = directory / "reports"
    reports.mkdir()
    paths = []
    for binding in BINDINGS:
        output = outputs[binding]
        report = {
            "format": writes_contract.REPORT_FORMAT,
            "binding": binding,
            "program": suite,
            "semantic_profile": profile,
            "plans": output["plans"],
            "results": output["results"],
            "observed": None if observed is None else observed[binding],
        }
        path = reports / f"{binding}.json"
        path.write_bytes(writes_contract.canonical_json_bytes(report))
        paths.append(path)
    return tuple(paths)


def writes_suite(
    directory: Path, suite: str, environment: dict[str, str], *, live: bool
) -> dict[str, Path]:
    """One suite: plan bytes offline; with ``live``, results directly and over the proxy."""

    expected_profile = writes_profile(suite)
    profile = environment[profile_matrix.SEMANTIC_PROFILE_ENV]
    if profile != expected_profile:
        raise RunnerError(f"the writes suite runs on {expected_profile}, not {profile}")
    generated = writes_packages(directory, suite, environment)
    programs = writes_programs(directory, generated, environment)
    observed = None
    if live:
        with writes_fixture(directory, suite, environment) as fixture:
            if fixture["profile"] != profile:
                raise RunnerError(f"the writes fixture serves {fixture['profile']}")
            outputs = writes_run(programs, suite, environment, fixture["bindings"])
        observed = fixture["observed"]
    else:
        outputs = writes_run(programs, suite, environment, {})
    paths = writes_reports(directory, suite, outputs, profile, observed)
    summary = directory / "comparison.json"
    summary.write_text(
        run(
            python_command(
                "sdk_writes_compare.py", *(["--live"] if live else []), *map(str, paths)
            ),
            env=environment,
            capture=True,
        )
        + "\n"
    )
    return {
        **{f"{binding}.json": path for binding, path in zip(BINDINGS, paths, strict=True)},
        "comparison.json": summary,
    }


def writes(directory: Path, environment: dict[str, str], *, live: bool) -> dict[str, Path]:
    """Every writes suite in its own directory, each checked by the one comparator."""

    files = {}
    for suite in WRITES_SUITES:
        suite_directory = directory / suite
        suite_directory.mkdir()
        produced = writes_suite(suite_directory, suite, environment, live=live)
        files |= {f"{suite}/{name}": path for name, path in produced.items()}
    return files


def execute(
    suite: str, directory: Path, environment: dict[str, str], arguments: argparse.Namespace
) -> dict[str, Path]:
    """Produce and independently validate a complete report set before publication."""
    if suite == "writes":
        return writes(directory, environment, live=not arguments.offline)
    if suite == "queries":
        nonce = secrets.token_hex(32)
        query_proofs(directory, environment, nonce)
        query_live(directory, environment, nonce)
        compare_queries(directory, environment)
        return {f"{binding}.json": directory / f"reports/{binding}.json" for binding in BINDINGS}
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
    parser.add_argument("suite", choices=(*SUITES, *STAGED_SUITES))
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--address", default="127.0.0.1:1729")
    parser.add_argument("--http-port", default="8000")
    parser.add_argument(
        "--semantic-profile",
        help=(
            "semantic profile to run (default: TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE, "
            "else the catalogs' own profile); selects the exact TypeDB server"
        ),
    )
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
    parser.add_argument(
        "--offline",
        action="store_true",
        help="writes only: compare plan bytes without a TypeDB server",
    )
    arguments = parser.parse_args(argv)
    if arguments.offline and arguments.suite != "writes":
        parser.error("--offline is only valid for the writes suite")
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
        if arguments.suite in {"queries", "models", "serialization", "writes"}:
            environment.update(
                {
                    "TYPEDB_ADDRESS": arguments.address,
                    "TYPEDB_HTTP_PORT": str(arguments.http_port),
                    "TYPEDB_USERNAME": "admin",
                    "TYPEDB_PASSWORD": "password",
                }
            )
        # Producers, fixtures, assemblers and comparers all read one profile and
        # its exact server so a run cannot mix TypeDB lines.
        profile = arguments.semantic_profile
        if arguments.suite == "writes" and profile is None:
            # The program has one profile; an unset lane selects it.
            profile = environment.get(profile_matrix.SEMANTIC_PROFILE_ENV) or writes_profile(
                WRITES_SUITES[0]
            )
        identity = lane(profile, environment)
        environment[profile_matrix.SEMANTIC_PROFILE_ENV] = identity.semantic_profile
        environment["TYPEDB_VERSION"] = identity.live_server_version
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
