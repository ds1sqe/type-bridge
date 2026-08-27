#!/usr/bin/env python3
"""Run and assemble all four real Workforce V3 candidate reports."""

from __future__ import annotations

import argparse
import os
import secrets
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from persist_binding_reports import PublishError, publish  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
CORE = ROOT / "type-bridge-core"
NODE = CORE / "crates/node"
BINDINGS = ("python", "node", "rust", "c")


class RunnerError(RuntimeError):
    """One exact V3 producer or assembly step failed."""


def run(command: list[str], *, env: dict[str, str], cwd: Path = ROOT) -> None:
    result = subprocess.run(command, cwd=cwd, env=env, check=False)
    if result.returncode != 0:
        raise RunnerError(f"command failed with exit {result.returncode}: {' '.join(command)}")


def run_phase_reports(directory: Path, environment: dict[str, str]) -> None:
    run(
        [
            sys.executable,
            "scripts/ci/run_phase2_projection_parity.py",
            "--output",
            str(directory / "phase2-parity"),
        ],
        env=environment,
    )
    phase2_environment = environment | {
        "TYPE_BRIDGE_PHASE2_LIVE_ADDRESS": environment["TYPEDB_ADDRESS"],
        "TYPE_BRIDGE_PHASE2_LIVE_HTTP_PORT": environment["TYPEDB_HTTP_PORT"],
    }
    run(
        [
            sys.executable,
            "scripts/ci/run_phase2_projection_live.py",
            "--output",
            str(directory / "phase2-live"),
        ],
        env=phase2_environment,
    )
    phase5_environment = environment | {
        "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_ADDRESS": environment["TYPEDB_ADDRESS"],
        "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_HTTP_PORT": environment["TYPEDB_HTTP_PORT"],
    }
    run(
        [
            sys.executable,
            "scripts/ci/run_phase5_manager_filter_live.py",
            "--output",
            str(directory / "phase5-manager"),
        ],
        env=phase5_environment,
    )


def run_artifacts(directory: Path, environment: dict[str, str]) -> None:
    atomic = directory / "atomic-generation.json"
    run(
        [
            "cargo",
            "test",
            "--quiet",
            "--locked",
            "--manifest-path",
            str(CORE / "Cargo.toml"),
            "-p",
            "type-bridge-cli",
            "--lib",
            "schema_generation_atomicity_tests::injected_c_emitter_failure_preserves_all_four_ordered_packages",
            "--",
            "--exact",
        ],
        env=environment | {"TYPE_BRIDGE_WORKFORCE_V3_ATOMIC_GENERATION_OUTPUT": str(atomic)},
    )
    field_identity = directory / "field-identity.json"
    run(
        [
            "cargo",
            "test",
            "--quiet",
            "--locked",
            "--manifest-path",
            str(CORE / "Cargo.toml"),
            "-p",
            "type-bridge-schema-codegen",
            "--test",
            "workforce_v3_fingerprints",
            "exact_workforce_v3_field_name_identity_is_source_bound",
            "--",
            "--exact",
        ],
        env=environment | {"TYPE_BRIDGE_WORKFORCE_V3_FIELD_IDENTITY_OUTPUT": str(field_identity)},
    )
    run(
        [sys.executable, "scripts/ci/validate_workforce_v3_atomic_generation.py", str(atomic)],
        env=environment,
    )
    run(
        [sys.executable, "scripts/ci/validate_workforce_v3_field_identity.py", str(field_identity)],
        env=environment,
    )


def fragment_environment(
    environment: dict[str, str], destination: Path, nonce: str
) -> dict[str, str]:
    return environment | {
        "TYPE_BRIDGE_WORKFORCE_V3_PROOF_FRAGMENT": str(destination),
        "TYPE_BRIDGE_WORKFORCE_V3_PROOF_RUN_NONCE": nonce,
    }


def run_proof_fragments(directory: Path, environment: dict[str, str], nonce: str) -> None:
    fragments = directory / "fragments"
    fragments.mkdir()
    package_commands = {
        "python": [
            sys.executable,
            "type-bridge-core/crates/schema-codegen/tests/acceptance/check.py",
        ],
        "node": [
            "node",
            "type-bridge-core/crates/schema-codegen/tests/typescript_acceptance/check.mjs",
        ],
        "rust": [
            "cargo",
            "test",
            "--quiet",
            "--locked",
            "--manifest-path",
            str(CORE / "Cargo.toml"),
            "-p",
            "type-bridge-schema-codegen",
            "--test",
            "rust_acceptance",
            "workforce_v3_generated_package_integrity",
            "--",
            "--exact",
        ],
        "c": [
            "cargo",
            "test",
            "--quiet",
            "--locked",
            "--manifest-path",
            str(CORE / "Cargo.toml"),
            "-p",
            "type-bridge-schema-codegen",
            "--test",
            "c_emitter",
            "workforce_v3_generated_package_integrity",
            "--",
            "--exact",
        ],
    }
    data_tests = {
        "python": (
            "type-bridge-core",
            "runtime_projection::tests::workforce_v3_python_data_plane_fragment",
        ),
        "node": (
            "type-bridge-node",
            "runtime_projection::tests::workforce_v3_node_data_plane_fragment",
        ),
        "rust": ("type-bridge", "transaction::tests::workforce_v3_rust_data_plane_fragment"),
        "c": ("type-bridge-c", "runtime::tests::workforce_v3_c_data_plane_fragment"),
    }
    for binding in BINDINGS:
        package = fragments / f"{binding}-package.json"
        run(
            package_commands[binding],
            env=fragment_environment(environment, package, nonce),
        )
        crate, test = data_tests[binding]
        data = fragments / f"{binding}-data.json"
        run(
            [
                "cargo",
                "test",
                "--quiet",
                "--locked",
                "--manifest-path",
                str(CORE / "Cargo.toml"),
                "-p",
                crate,
                "--lib",
                test,
                "--",
                "--exact",
            ],
            env=fragment_environment(environment, data, nonce),
        )
        run(
            [
                sys.executable,
                "scripts/ci/workforce_v3_proof_fragments.py",
                "--binding",
                binding,
                "--run-nonce",
                nonce,
                str(package),
                str(data),
            ],
            env=environment,
        )


def run_live_supplements(directory: Path, environment: dict[str, str]) -> None:
    supplements = directory / "supplements"
    supplements.mkdir()
    python_env = environment | {
        "USE_DOCKER": "false",
        "TYPE_BRIDGE_WORKFORCE_V3_PYTHON_SUPPLEMENT": str(supplements / "python.json"),
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
    compiled = (
        ROOT
        / "tmp/node-projection-integration/tests/projection-integration/generated-package-live.test.js"
    )
    if not compiled.is_file():
        raise RunnerError("compiled Node projection integration test is missing")
    shutil.copy2(
        NODE / "tests/projection-integration/package.json", compiled.parent / "package.json"
    )
    node_env = environment | {
        "TYPE_BRIDGE_WORKFORCE_V3_NODE_SUPPLEMENT": str(supplements / "node.json"),
        "TYPE_BRIDGE_NODE_INTG_DATABASE": f"type_bridge_v3_node_{os.getpid()}",
    }
    run(
        [
            "node",
            "--test",
            "--test-concurrency=1",
            "--test-name-pattern=node.generated_data_model_runtime_v3_live",
            str(compiled),
        ],
        cwd=NODE,
        env=node_env,
    )

    rust_env = environment | {
        "TYPE_BRIDGE_WORKFORCE_V3_RUST_SUPPLEMENT": str(supplements / "rust.json"),
        "TYPE_BRIDGE_RUST_PROJECTION_INTG_DATABASE": f"type_bridge_v3_rust_{os.getpid()}",
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

    c_env = environment | {
        "TYPE_BRIDGE_WORKFORCE_V3_C_SUPPLEMENT": str(supplements / "c.json"),
        "TYPE_BRIDGE_C_PHASE4_INTG_DATABASE": f"type_bridge_v3_c_{os.getpid()}",
        "TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER": "1",
    }
    run(
        [
            "bash",
            "scripts/ci/run_exact_ignored_rust_test.sh",
            "generated_data_model_runtime_v3_live",
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


def assemble(directory: Path, environment: dict[str, str], nonce: str) -> tuple[Path, ...]:
    live = directory / "live"
    reports = directory / "reports"
    live.mkdir()
    reports.mkdir()
    for binding in BINDINGS:
        run(
            [
                sys.executable,
                "scripts/ci/compose_workforce_v3_live_observations.py",
                "--binding",
                binding,
                "--phase2-parity",
                str(directory / f"phase2-parity/{binding}.json"),
                "--phase2-live",
                str(directory / f"phase2-live/{binding}.json"),
                "--phase5-manager",
                str(directory / f"phase5-manager/{binding}.json"),
                "--atomic-generation",
                str(directory / "atomic-generation.json"),
                "--field-identity",
                str(directory / "field-identity.json"),
                "--supplement",
                str(directory / f"supplements/{binding}.json"),
                "--output",
                str(live / f"{binding}.json"),
            ],
            env=environment,
        )
        run(
            [
                sys.executable,
                "scripts/ci/assemble_workforce_conformance_v3.py",
                "--binding",
                binding,
                "--live-observations",
                str(live / f"{binding}.json"),
                "--run-nonce",
                nonce,
                "--output",
                str(reports / f"{binding}.json"),
                str(directory / f"fragments/{binding}-package.json"),
                str(directory / f"fragments/{binding}-data.json"),
            ],
            env=environment,
        )
    paths = tuple(reports / f"{binding}.json" for binding in BINDINGS)
    run(
        [
            sys.executable,
            "scripts/ci/compare_workforce_conformance_v3.py",
            *(str(path) for path in paths),
        ],
        env=environment,
    )
    return paths


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--address", default="127.0.0.1:1729")
    parser.add_argument("--http-port", default="8000")
    arguments = parser.parse_args()
    output = arguments.output.resolve()
    try:
        if output.exists() or output == ROOT or ROOT in output.parents:
            raise RunnerError("V3 output must be a new directory outside the checkout")
        environment = os.environ.copy()
        environment.update(
            {
                "TYPEDB_ADDRESS": arguments.address,
                "TYPEDB_HTTP_PORT": str(arguments.http_port),
                "TYPEDB_USERNAME": "admin",
                "TYPEDB_PASSWORD": "password",
                "TYPEDB_VERSION": "3.12.3",
            }
        )
        nonce = secrets.token_hex(32)
        with tempfile.TemporaryDirectory(prefix="typebridge-workforce-v3-") as value:
            directory = Path(value)
            run_phase_reports(directory, environment)
            run_artifacts(directory, environment)
            run_proof_fragments(directory, environment, nonce)
            run_live_supplements(directory, environment)
            reports = assemble(directory, environment, nonce)
            publish(reports, output, checkout=ROOT)
        return 0
    except (RunnerError, PublishError, OSError) as error:
        print(f"Workforce V3 candidate run failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
