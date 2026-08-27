#!/usr/bin/env python3
"""Run and atomically persist the real Workforce V1 and V2 report sets."""

from __future__ import annotations

import argparse
import os
import secrets
import shutil
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CORE = ROOT / "type-bridge-core"
NODE = CORE / "crates/node"
V1_BINDINGS = ("python", "node", "rust")
V2_BINDINGS = (*V1_BINDINGS, "c")


class RunnerError(RuntimeError):
    """One exact predecessor producer, comparator, or publication step failed."""


def run(command: list[str], *, env: dict[str, str], cwd: Path = ROOT) -> None:
    result = subprocess.run(command, cwd=cwd, env=env, check=False)
    if result.returncode != 0:
        raise RunnerError(f"command failed with exit {result.returncode}: {' '.join(command)}")


def fragment_environment(
    environment: dict[str, str], destination: Path, nonce: str
) -> dict[str, str]:
    return environment | {
        "TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENT": str(destination),
        "TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE": nonce,
    }


def emit_fragments(directory: Path, environment: dict[str, str], nonce: str) -> None:
    fragments = directory / "fragments"
    fragments.mkdir()
    commands: dict[str, tuple[list[str], ...]] = {
        "python": (
            [
                "cargo",
                "test",
                "--quiet",
                "--locked",
                "--manifest-path",
                str(CORE / "Cargo.toml"),
                "-p",
                "type-bridge-core",
                "--lib",
                "match_runtime::tests::python_direct_cancellation_fragment_is_measured_from_owned_execution",
                "--",
                "--exact",
            ],
            [
                "uv",
                "run",
                "python",
                "type-bridge-core/crates/schema-codegen/tests/acceptance/check.py",
            ],
        ),
        "node": (
            [
                "cargo",
                "test",
                "--quiet",
                "--locked",
                "--manifest-path",
                str(CORE / "Cargo.toml"),
                "-p",
                "type-bridge-node",
                "--lib",
                "match_runtime::tests::node_direct_cancellation_fragment_is_measured_from_owned_execution",
                "--",
                "--exact",
            ],
            [
                "node",
                "type-bridge-core/crates/schema-codegen/tests/typescript_acceptance/check.mjs",
            ],
        ),
        "rust": (
            [
                "cargo",
                "test",
                "--quiet",
                "--locked",
                "--manifest-path",
                str(CORE / "Cargo.toml"),
                "-p",
                "type-bridge",
                "--lib",
                "remote::tests::workforce_v2_rust_deterministic_proof_fragment",
                "--",
                "--exact",
            ],
        ),
        "c": (
            [
                "cargo",
                "test",
                "--quiet",
                "--locked",
                "--manifest-path",
                str(CORE / "Cargo.toml"),
                "-p",
                "type-bridge-c",
                "--lib",
                "query::tests::workforce_v2_c_deterministic_proof_fragment",
                "--",
                "--exact",
            ],
        ),
    }
    for binding in V2_BINDINGS:
        paths: list[Path] = []
        for index, command in enumerate(commands[binding]):
            destination = fragments / f"{binding}-{index}.json"
            run(command, env=fragment_environment(environment, destination, nonce))
            paths.append(destination)
        run(
            [
                sys.executable,
                "scripts/ci/workforce_v2_proof_fragments.py",
                "--binding",
                binding,
                "--run-nonce",
                nonce,
                *(str(path) for path in paths),
            ],
            env=environment,
        )


def report_environment(
    environment: dict[str, str],
    directory: Path,
    binding: str,
    nonce: str,
) -> dict[str, str]:
    fragments = sorted((directory / "fragments").glob(f"{binding}-*.json"))
    if not fragments:
        raise RunnerError(f"{binding} V2 proof fragments are missing")
    result = environment | {
        "TYPE_BRIDGE_WORKFORCE_REPORT_V2": str(directory / f"v2/{binding}.json"),
        "TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENTS": os.pathsep.join(
            str(path) for path in fragments
        ),
        "TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE": nonce,
    }
    if binding in V1_BINDINGS:
        result["TYPE_BRIDGE_WORKFORCE_REPORT"] = str(directory / f"v1/{binding}.json")
    return result


def run_live_reports(directory: Path, environment: dict[str, str], nonce: str) -> None:
    (directory / "v1").mkdir()
    (directory / "v2").mkdir()

    python_env = report_environment(environment, directory, "python", nonce) | {
        "USE_DOCKER": "false",
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
    compiled = (
        ROOT
        / "tmp/node-projection-integration/tests/projection-integration/generated-package-live.test.js"
    )
    if not compiled.is_file():
        raise RunnerError("compiled Node projection integration test is missing")
    shutil.copy2(
        NODE / "tests/projection-integration/package.json", compiled.parent / "package.json"
    )
    node_env = report_environment(environment, directory, "node", nonce) | {
        "TYPE_BRIDGE_WORKFORCE_V2_VALIDATOR_PYTHON": str(Path(sys.executable).resolve()),
        "TYPE_BRIDGE_NODE_INTG_DATABASE": f"type_bridge_v2_node_{os.getpid()}",
    }
    run(
        [
            "node",
            "--test",
            "--test-concurrency=1",
            "--test-name-pattern=generated package round-trips exact models on TypeDB",
            str(compiled),
        ],
        cwd=NODE,
        env=node_env,
    )

    rust_env = report_environment(environment, directory, "rust", nonce) | {
        "TYPE_BRIDGE_RUST_PROJECTION_INTG_DATABASE": f"type_bridge_v2_rust_{os.getpid()}",
    }
    run(
        [
            "bash",
            "scripts/ci/run_exact_ignored_rust_test.sh",
            "generated_rust_projection_round_trips_exact_live_models",
            "--locked",
            "--manifest-path",
            "type-bridge-core/Cargo.toml",
            "-p",
            "type-bridge-schema-codegen",
            "--test",
            "rust_projection_live",
        ],
        env=rust_env,
    )

    c_target = directory / "c-target"
    c_env = report_environment(environment, directory, "c", nonce) | {
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
        [
            "bash",
            "scripts/ci/run_exact_ignored_rust_test.sh",
            "live_c17_generated_person_and_membership_crud_round_trips_exact_3_12_3",
            "--locked",
            "--manifest-path",
            "type-bridge-core/Cargo.toml",
            "-p",
            "type-bridge-schema-codegen",
            "--test",
            "c_projection_live",
        ],
        env=c_env,
    )


def compare(directory: Path, environment: dict[str, str]) -> None:
    run(
        [
            sys.executable,
            "scripts/ci/compare_workforce_conformance.py",
            *(str(directory / f"v1/{binding}.json") for binding in V1_BINDINGS),
        ],
        env=environment,
    )
    run(
        [
            sys.executable,
            "scripts/ci/compare_workforce_conformance_v2.py",
            *(str(directory / f"v2/{binding}.json") for binding in V2_BINDINGS),
        ],
        env=environment,
    )


def publish(directory: Path, output: Path) -> None:
    output = output.resolve()
    if output.exists() or output == ROOT or ROOT in output.parents:
        raise RunnerError("output must be a new directory outside the checkout")
    try:
        parent = output.parent.lstat()
    except OSError as error:
        raise RunnerError("output parent cannot be inspected") from error
    if stat.S_ISLNK(parent.st_mode) or not stat.S_ISDIR(parent.st_mode):
        raise RunnerError("output parent must be a real directory")
    reports = {
        "v1": tuple(directory / f"v1/{binding}.json" for binding in V1_BINDINGS),
        "v2": tuple(directory / f"v2/{binding}.json" for binding in V2_BINDINGS),
    }
    for version, paths in reports.items():
        for binding, path in zip(
            V1_BINDINGS if version == "v1" else V2_BINDINGS, paths, strict=True
        ):
            try:
                metadata = path.lstat()
            except OSError as error:
                raise RunnerError(f"{version} {binding} report cannot be inspected") from error
            if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
                raise RunnerError(f"{version} {binding} report must be a regular file")
    stage = Path(tempfile.mkdtemp(prefix=f".{output.name}.", dir=output.parent))
    try:
        for version, paths in reports.items():
            target = stage / version
            target.mkdir()
            bindings = V1_BINDINGS if version == "v1" else V2_BINDINGS
            for binding, path in zip(bindings, paths, strict=True):
                shutil.copyfile(path, target / f"{binding}.json")
        stage.rename(output)
    except OSError as error:
        raise RunnerError("validated predecessor reports could not be persisted") from error
    finally:
        if stage.exists():
            shutil.rmtree(stage)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--address", default="127.0.0.1:1729")
    parser.add_argument("--http-port", default="8000")
    arguments = parser.parse_args()
    try:
        output = arguments.output.resolve()
        if output.exists() or output == ROOT or ROOT in output.parents:
            raise RunnerError("output must be a new directory outside the checkout")
        environment = os.environ.copy()
        environment.update(
            {
                "TYPEDB_ADDRESS": arguments.address,
                "TYPEDB_HTTP_PORT": str(arguments.http_port),
                "TYPEDB_USERNAME": "admin",
                "TYPEDB_PASSWORD": "password",
                "TYPEDB_VERSION": "3.12.3",
                "TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE": "typedb-3.12.1/v1",
            }
        )
        nonce = secrets.token_hex(32)
        with tempfile.TemporaryDirectory(prefix="typebridge-workforce-v1-v2-") as value:
            directory = Path(value)
            emit_fragments(directory, environment, nonce)
            run_live_reports(directory, environment, nonce)
            compare(directory, environment)
            publish(directory, output)
        return 0
    except (RunnerError, OSError) as error:
        print(f"Workforce V1/V2 candidate run failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
