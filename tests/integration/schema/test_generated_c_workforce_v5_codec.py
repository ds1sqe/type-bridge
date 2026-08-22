"""Provider-free acceptance for private C's generated Workforce V5 codec."""

from __future__ import annotations

import base64
import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest

pytestmark = pytest.mark.integration

ROOT = Path(__file__).resolve().parents[3]
CORE = ROOT / "type-bridge-core"
PRODUCER = CORE / "crates/schema-codegen/tests/c_acceptance/workforce_v5_codec.c"
SCHEMA = ROOT / "tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml"


def generate_package(stage: Path, schema: Path, *, app_label: str, scope: str) -> Path:
    workspace = stage / "workspace"
    fragments = workspace / "schema/fragments"
    (workspace / "migrations/v2").mkdir(parents=True)
    fragments.mkdir(parents=True)
    shutil.copy2(schema, fragments / "models.yaml")
    (workspace / "schema/schema.yaml").write_text(
        "format: typebridge.schema-set/v1\nsources: [fragments/*.yaml]\n"
    )
    (workspace / "typebridge.yaml").write_text(
        f"""format: typebridge.workspace/v1
schema:
  root: schema/schema.yaml
  ownership: exclusive
  managed-scope: {scope}
compatibility:
  semantic-profile: typedb-3.12.1/v1
migrations:
  directory: migrations/v2
  app-label: {app_label}
bindings:
  c:
    output: generated/workforce
"""
    )
    subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "--locked",
            "--manifest-path",
            str(CORE / "Cargo.toml"),
            "-p",
            "type-bridge-cli",
            "--bin",
            "type-bridge",
            "--",
            "--manifest",
            str(workspace / "typebridge.yaml"),
            "schema",
            "generate",
        ],
        cwd=ROOT,
        check=True,
    )
    return workspace / "generated/workforce"


def rust_corpus(path: Path) -> dict[str, object]:
    environment = os.environ.copy()
    environment["TYPE_BRIDGE_WORKFORCE_V5_RUST_CORPUS"] = str(path)
    subprocess.run(
        [
            "cargo",
            "test",
            "--quiet",
            "--locked",
            "-p",
            "type-bridge-schema-codegen",
            "--test",
            "rust_acceptance",
            "generated_rust_workforce_v5_canonical_codec",
            "--",
            "--exact",
        ],
        cwd=CORE,
        env=environment,
        check=True,
    )
    return json.loads(path.read_bytes())


def compile_producer(package: Path, foreign_package: Path, executable: Path) -> None:
    compiler = shutil.which("gcc") or shutil.which("clang")
    if compiler is None:
        pytest.fail("a supported C compiler is required")
    subprocess.run(
        [
            "cargo",
            "build",
            "--quiet",
            "--locked",
            "-p",
            "type-bridge-c",
        ],
        cwd=CORE,
        check=True,
    )
    subprocess.run(
        [
            compiler,
            "-std=c17",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic-errors",
            "-I",
            str(CORE / "crates/c/include"),
            "-I",
            str(package / "include"),
            "-I",
            str(foreign_package / "include"),
            str(package / "src/models.c"),
            str(foreign_package / "src/models.c"),
            str(PRODUCER),
            "-L",
            str(CORE / "target/debug"),
            "-ltype_bridge_c",
            f"-Wl,-rpath,{CORE / 'target/debug'}",
            "-o",
            str(executable),
        ],
        cwd=ROOT,
        check=True,
    )


def run_c_producer(executable: Path, seed: dict[str, object], stage: Path) -> tuple[bytes, bytes]:
    inputs = stage / "input"
    outputs = stage / "output"
    inputs.mkdir(parents=True)
    outputs.mkdir()
    operational = stage / "operational.json"
    records = seed["record_b64"]
    assert isinstance(records, list)
    for index, encoded in enumerate(records):
        assert isinstance(encoded, str)
        (inputs / f"record-{index}.bin").write_bytes(base64.b64decode(encoded))
    environment = os.environ.copy()
    library_path = str(CORE / "target/debug")
    environment["LD_LIBRARY_PATH"] = os.pathsep.join(
        filter(None, [library_path, environment.get("LD_LIBRARY_PATH", "")])
    )
    subprocess.run(
        [str(executable), str(inputs), str(outputs), str(operational)],
        env=environment,
        check=True,
    )
    produced_records = [(outputs / f"record-{index}.bin").read_bytes() for index in range(9)]
    payload = {
        "archive_b64": base64.b64encode((outputs / "archive.bin").read_bytes()).decode(),
        "binding": "c",
        "format": "typebridge.workforce-v5-provider-free-corpus/v1",
        "record_b64": [base64.b64encode(record).decode() for record in produced_records],
    }
    return (
        json.dumps(payload, sort_keys=True, separators=(",", ":")).encode(),
        operational.read_bytes(),
    )


def test_generated_c_workforce_v5_canonical_codec(tmp_path: Path) -> None:
    seed_path = tmp_path / "rust-corpus.json"
    seed = rust_corpus(seed_path)
    package = generate_package(
        tmp_path / "generated", SCHEMA, app_label="generatedcv5", scope="generated-c-v5"
    )
    foreign_schema = tmp_path / "schema-foreign.yaml"
    source = SCHEMA.read_text(encoding="utf-8")
    foreign_source = source.replace("range: { min: 0, max: 80 }", "range: { min: 0, max: 79 }", 1)
    assert foreign_source != source
    foreign_schema.write_text(foreign_source, encoding="utf-8")
    foreign_package = generate_package(
        tmp_path / "generated-foreign",
        foreign_schema,
        app_label="generatedcv5foreign",
        scope="generated-c-v5-foreign",
    )
    executable = tmp_path / "workforce-v5-codec"
    compile_producer(package, foreign_package, executable)
    first, operational = run_c_producer(executable, seed, tmp_path / "first")
    repeated, repeated_operational = run_c_producer(executable, seed, tmp_path / "repeated")
    assert first == repeated
    assert operational == repeated_operational
    corpus = json.loads(first)
    assert corpus["record_b64"] == seed["record_b64"]
    assert corpus["archive_b64"] == seed["archive_b64"]
    evidence = json.loads(operational)
    assert evidence["format"] == "typebridge.workforce-v5-operational-evidence/v1"
    assert evidence["binding"] == "c"
    assert evidence["diagnostic"] == {
        "category": "invalid_input",
        "code": "projected_record_schema_mismatch",
        "path": ["declared_schema_identity"],
        "payload_absent": True,
    }
    assert evidence["lifecycle"] == {
        "archive_closed": True,
        "builder_closed": True,
        "bytes_closed": True,
        "decoded_closed": True,
        "repeat_close": True,
        "sibling_usable": True,
    }
    export_value = os.environ.get("TYPE_BRIDGE_WORKFORCE_V5_C_CORPUS")
    if export_value is not None:
        export = Path(export_value)
        if not export.is_absolute():
            raise AssertionError("C Workforce V5 corpus path must be absolute")
        with export.open("xb") as stream:
            stream.write(first)
            stream.flush()
            os.fsync(stream.fileno())
    operational_export_value = os.environ.get("TYPE_BRIDGE_WORKFORCE_V5_C_OPERATIONAL_EVIDENCE")
    if operational_export_value is not None:
        operational_export = Path(operational_export_value)
        if not operational_export.is_absolute():
            raise AssertionError("C Workforce V5 operational path must be absolute")
        with operational_export.open("xb") as stream:
            stream.write(operational)
            stream.flush()
            os.fsync(stream.fileno())
