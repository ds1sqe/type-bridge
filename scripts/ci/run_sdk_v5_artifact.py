#!/usr/bin/env python3
"""Run and assemble all four real Sdk V5 artifact reports."""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import compare_sdk_conformance_v5 as conformance

ROOT = Path(__file__).resolve().parents[2]
CORE = ROOT / "type-bridge-core"
NODE = CORE / "crates/node"
PYTHON_PROVIDER = ROOT / "tests/integration/schema/test_generated_sdk_v5_codec.py"
NODE_PROVIDER = (
    ROOT / "type-bridge-core/crates/schema-codegen/tests/typescript_acceptance/codec_check.py"
)
C_PROVIDER = ROOT / "tests/integration/schema/test_generated_c_sdk_v5_codec.py"
BINDINGS = ("python", "node", "rust", "c")


class RunnerError(RuntimeError):
    """One exact V5 producer or assembly step failed."""


def run(command: list[str], *, cwd: Path = ROOT, env: dict[str, str] | None = None) -> None:
    result = subprocess.run(command, cwd=cwd, env=env, check=False)
    if result.returncode != 0:
        raise RunnerError(f"command failed with exit {result.returncode}: {' '.join(command)}")


def write_cleanup(path: Path, binding: str) -> None:
    path.write_bytes(
        conformance.canonical_json_bytes(
            {
                "binding": binding,
                "format": "typebridge.sdk-v5-cleanup-evidence/v1",
                "managed_database_absent": True,
                "partial_output_absent": True,
                "temporary_evidence_absent": True,
            }
        )
    )


def provider_free(directory: Path, environment: dict[str, str]) -> None:
    python_env = environment | {
        "TYPE_BRIDGE_SDK_V5_PYTHON_CORPUS": str(directory / "python-corpus.json"),
        "TYPE_BRIDGE_SDK_V5_PYTHON_OPERATIONAL_EVIDENCE": str(
            directory / "python-operational.json"
        ),
    }
    run(
        ["uv", "run", "pytest", "-q", "-m", "integration", str(PYTHON_PROVIDER)],
        env=python_env,
    )

    run(["npm", "run", "build"], cwd=NODE, env=environment)
    node_env = environment | {
        "TYPE_BRIDGE_SDK_V5_NODE_CORPUS": str(directory / "node-corpus.json"),
        "TYPE_BRIDGE_SDK_V5_NODE_OPERATIONAL_EVIDENCE": str(directory / "node-operational.json"),
    }
    run(
        [sys.executable, str(NODE_PROVIDER)],
        env=node_env,
    )

    rust_env = environment | {
        "TYPE_BRIDGE_SDK_V5_RUST_CORPUS": str(directory / "rust-corpus.json"),
        "TYPE_BRIDGE_SDK_V5_RUST_OPERATIONAL_EVIDENCE": str(directory / "rust-operational.json"),
    }
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
            "rust_acceptance",
            "generated_rust_sdk_v5_canonical_codec",
            "--",
            "--exact",
        ],
        env=rust_env,
    )

    c_env = environment | {
        "TYPE_BRIDGE_SDK_V5_C_CORPUS": str(directory / "c-corpus.json"),
        "TYPE_BRIDGE_SDK_V5_C_OPERATIONAL_EVIDENCE": str(directory / "c-operational.json"),
    }
    run(
        ["uv", "run", "pytest", "-q", "-m", "integration", str(C_PROVIDER)],
        env=c_env,
    )
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
                for item in (
                    f"--{binding}",
                    str(directory / f"{binding}-operational.json"),
                )
            ),
        ],
        env=environment,
    )


def live(directory: Path, environment: dict[str, str]) -> None:
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
    compiled = (
        ROOT
        / "tmp/node-projection-integration/tests/projection-integration/generated-package-live.test.js"
    )
    if not compiled.is_file():
        raise RunnerError("compiled Node projection integration test is missing")
    shutil.copy2(
        NODE / "tests/projection-integration/package.json",
        compiled.parent / "package.json",
    )
    run(
        [
            "node",
            "--test",
            "--test-concurrency=1",
            "--test-name-pattern=node.generated_canonical_serialization_v5_live",
            str(compiled),
        ],
        cwd=NODE,
        env=node_env,
    )
    write_cleanup(directory / "node-cleanup.json", "node")

    rust_env = environment | {
        "TYPE_BRIDGE_SDK_V5_RUST_EVIDENCE": str(directory / "rust-live.json"),
        "TYPE_BRIDGE_RUST_PROJECTION_INTG_DATABASE": f"type_bridge_v5_rust_{os.getpid()}",
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
    write_cleanup(directory / "rust-cleanup.json", "rust")

    c_env = environment | {
        "TYPE_BRIDGE_SDK_V5_C_EVIDENCE": str(directory / "c-live.json"),
        "TYPE_BRIDGE_C_PROJECTION_INTG_DATABASE": f"type_bridge_v5_c_{os.getpid()}",
        "TYPE_BRIDGE_C_REQUIRE_SHARED_CONSUMER": "1",
    }
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
    write_cleanup(directory / "c-cleanup.json", "c")


def assemble(directory: Path, output: Path, environment: dict[str, str]) -> None:
    output.mkdir(parents=True)
    for binding in BINDINGS:
        nonce = os.urandom(32).hex()
        producer = directory / f"{binding}-producer.json"
        run(
            [
                sys.executable,
                "scripts/ci/compose_sdk_v5_evidence.py",
                "--binding",
                binding,
                "--run-nonce",
                nonce,
                "--corpus",
                str(directory / f"{binding}-corpus.json"),
                "--live-evidence",
                str(directory / f"{binding}-live.json"),
                "--operational-evidence",
                str(directory / f"{binding}-operational.json"),
                "--cleanup-evidence",
                str(directory / f"{binding}-cleanup.json"),
                "--output",
                str(producer),
            ],
            env=environment,
        )
        run(
            [
                sys.executable,
                "scripts/ci/assemble_sdk_conformance_v5.py",
                "--binding",
                binding,
                "--run-nonce",
                nonce,
                "--evidence",
                str(producer),
                "--output",
                str(output / f"{binding}.json"),
            ],
            env=environment,
        )
    run(
        [
            sys.executable,
            "scripts/ci/compare_sdk_conformance_v5.py",
            *(str(output / f"{binding}.json") for binding in BINDINGS),
        ],
        env=environment,
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--address", default="127.0.0.1:1729")
    parser.add_argument("--http-port", default="8000")
    arguments = parser.parse_args()
    try:
        output = arguments.output.resolve()
        if output.exists() or output == ROOT or ROOT in output.parents:
            raise RunnerError("V5 output must be a new directory outside the checkout")
        environment = os.environ.copy()
        environment.update(
            {
                "TYPEDB_ADDRESS": arguments.address,
                "TYPEDB_HTTP_PORT": str(arguments.http_port),
                "TYPEDB_USERNAME": "admin",
                "TYPEDB_PASSWORD": "password",
            }
        )
        with tempfile.TemporaryDirectory(prefix="typebridge-sdk-v5-") as value:
            directory = Path(value)
            provider_free(directory, environment)
            live(directory, environment)
            assemble(directory, output, environment)
        return 0
    except (RunnerError, conformance.ContractError, OSError) as error:
        print(f"Sdk V5 artifact run failed: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
