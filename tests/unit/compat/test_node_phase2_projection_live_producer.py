"""Fail-closed tests for Node's exact-3.12.3 Phase-2 live producer."""

from __future__ import annotations

import importlib.util
import json
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
PRODUCER_PATH = (
    ROOT
    / "type-bridge-core/crates/schema-codegen/tests/typescript_acceptance"
    / "phase2_live_check.mjs"
)
COMPARATOR_PATH = ROOT / "scripts/ci/compare_phase2_projection_live.py"
CATALOG_PATH = ROOT / "tests/contracts/sdk_conformance/workforce-v3/catalog-v3.json"


def _node(source: str, *arguments: str, expected: int = 0) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        [
            "node",
            "--input-type=module",
            "--eval",
            source,
            PRODUCER_PATH.as_uri(),
            *arguments,
        ],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert completed.returncode == expected, (
        f"Node exited {completed.returncode}, expected {expected}\n"
        f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
    )
    return completed


def _comparator_module():
    spec = importlib.util.spec_from_file_location("phase2_live_contract", COMPARATOR_PATH)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_projection_fingerprint_matches_finalized_catalog() -> None:
    completed = _node(
        """
const producer = await import(process.argv[1]);
process.stdout.write(JSON.stringify(producer.PROJECTION_FINGERPRINT));
"""
    )
    catalog = json.loads(CATALOG_PATH.read_bytes())
    assert json.loads(completed.stdout) == catalog["expected_fingerprints"]["projections"]["node"]


def test_report_is_canonical_bounded_atomic_and_create_new(tmp_path: Path) -> None:
    destination = tmp_path / "node.json"
    publish = """
const producer = await import(process.argv[1]);
producer.publishReport(process.argv[2], {
  binding: "node",
  observations: { value: 38 },
});
"""
    _node(publish, str(destination))
    assert destination.read_bytes() == b'{"binding":"node","observations":{"value":38}}\n'
    assert list(tmp_path.glob("*.tmp")) == []

    reject_existing = """
const producer = await import(process.argv[1]);
try {
  producer.publishReport(process.argv[2], {});
} catch (error) {
  process.stdout.write(error.code);
  process.exit(23);
}
"""
    completed = _node(reject_existing, str(destination), expected=23)
    assert completed.stdout == "output_exists"

    oversized = """
const producer = await import(process.argv[1]);
try {
  producer.publishReport(process.argv[2], {
    value: "x".repeat(producer.MAX_REPORT_BYTES),
  });
} catch (error) {
  process.stdout.write(error.code);
  process.exit(23);
}
"""
    completed = _node(oversized, str(tmp_path / "oversized.json"), expected=23)
    assert completed.stdout == "report_size_limit"
    assert not (tmp_path / "oversized.json").exists()


def test_paths_http_port_and_authority_files_fail_closed(tmp_path: Path) -> None:
    probe = """
const producer = await import(process.argv[1]);
const results = {};
for (const value of ["1", "32943", "65535"]) {
  results[value] = producer.httpPort({ [producer.HTTP_PORT_ENV]: value });
}
for (const value of ["0", "65536", "-1", "+1", " 32943", "32943 ", "３２９４３"]) {
  try {
    producer.httpPort({ [producer.HTTP_PORT_ENV]: value });
    throw new Error(`accepted ${value}`);
  } catch (error) {
    if (error.code !== "invalid_http_port") throw error;
  }
}
try {
  producer.outputPath({ [producer.OUTPUT_ENV]: "relative.json" });
  throw new Error("accepted relative output");
} catch (error) {
  if (error.code !== "invalid_output_path") throw error;
}
process.stdout.write(JSON.stringify(results));
"""
    completed = _node(probe)
    assert json.loads(completed.stdout) == {"1": 1, "32943": 32943, "65535": 65535}

    real_parent = tmp_path / "real-parent"
    real_parent.mkdir()
    linked_parent = tmp_path / "linked-parent"
    linked_parent.symlink_to(real_parent, target_is_directory=True)
    reject_output_parent = """
const producer = await import(process.argv[1]);
try {
  producer.outputPath({ [producer.OUTPUT_ENV]: process.argv[2] });
} catch (error) {
  process.stdout.write(error.code);
  process.exit(23);
}
"""
    completed = _node(
        reject_output_parent,
        str(linked_parent / "report.json"),
        expected=23,
    )
    assert completed.stdout == "invalid_output_parent"

    real_root = tmp_path / "real-root"
    for relative_text in (
        "tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml",
        "tests/contracts/sdk_conformance/workforce-v3/journey-v3.json",
        "tests/contracts/sdk_conformance/workforce-v3/provider-3.12.1-v3.tql",
    ):
        relative = Path(relative_text)
        destination = real_root / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / relative, destination)
    journey = real_root / "tests/contracts/sdk_conformance/workforce-v3/journey-v3.json"
    journey_copy = tmp_path / "journey-copy.json"
    shutil.copyfile(journey, journey_copy)
    journey.unlink()
    journey.symlink_to(journey_copy)
    reject_authority = """
const producer = await import(process.argv[1]);
try {
  producer.sourceAuthority(process.argv[2]);
} catch (error) {
  process.stdout.write(error.code);
  process.exit(23);
}
"""
    completed = _node(reject_authority, str(real_root), expected=23)
    assert completed.stdout == "invalid_authority"


def test_generated_package_requires_exact_workforce_v3_semantic_fingerprint() -> None:
    validate = """
const producer = await import(process.argv[1]);
const semantic = { ...producer.SEMANTIC_SCHEMA_FINGERPRINT };
const token = (typeKey) => ({ typeKey, create() {}, manager() {} });
const package_ = {
  SEMANTIC_SCHEMA_FINGERPRINT_JSON: JSON.stringify(semantic),
  PROJECTION_FINGERPRINT_JSON: JSON.stringify(producer.PROJECTION_FINGERPRINT),
  RUNTIME_PROJECTION_JSON: JSON.stringify({
    projection_fingerprint: producer.PROJECTION_FINGERPRINT,
    semantic_fingerprint: semantic,
    target: "typescript",
  }),
  Container: token('{"kind":"relation","label":"container"}'),
  Event: token('{"kind":"relation","label":"event"}'),
  Interaction: token('{"kind":"relation","label":"interaction"}'),
  Person: token('{"kind":"entity","label":"person"}'),
  PlainActivity: token('{"kind":"relation","label":"plain-activity"}'),
  Robot: token('{"kind":"entity","label":"robot"}'),
};
producer.validateGeneratedPackage(package_);
semantic.digest = "a98b9c10a2942f263eb723a4e532cced5221d0e0ecaf3296ee4017412e8fa5b0";
package_.SEMANTIC_SCHEMA_FINGERPRINT_JSON = JSON.stringify(semantic);
package_.RUNTIME_PROJECTION_JSON = JSON.stringify({
  projection_fingerprint: producer.PROJECTION_FINGERPRINT,
  semantic_fingerprint: semantic,
  target: "typescript",
});
try {
  producer.validateGeneratedPackage(package_);
} catch (error) {
  process.stdout.write(error.code);
  process.exit(23);
}
"""
    completed = _node(validate, expected=23)
    assert completed.stdout == "generated_package_identity"


def test_generated_package_rejects_foreign_projection_fingerprint() -> None:
    validate = """
const producer = await import(process.argv[1]);
const foreign = { ...producer.PROJECTION_FINGERPRINT, digest: "foreign-compatible" };
const token = (typeKey) => ({ typeKey, create() {}, manager() {} });
const package_ = {
  SEMANTIC_SCHEMA_FINGERPRINT_JSON: JSON.stringify(producer.SEMANTIC_SCHEMA_FINGERPRINT),
  PROJECTION_FINGERPRINT_JSON: JSON.stringify(foreign),
  RUNTIME_PROJECTION_JSON: JSON.stringify({
    projection_fingerprint: foreign,
    semantic_fingerprint: producer.SEMANTIC_SCHEMA_FINGERPRINT,
    target: "typescript",
  }),
  Container: token('{"kind":"relation","label":"container"}'),
  Event: token('{"kind":"relation","label":"event"}'),
  Interaction: token('{"kind":"relation","label":"interaction"}'),
  Person: token('{"kind":"entity","label":"person"}'),
  PlainActivity: token('{"kind":"relation","label":"plain-activity"}'),
  Robot: token('{"kind":"entity","label":"robot"}'),
};
try {
  producer.validateGeneratedPackage(package_);
} catch (error) {
  process.stdout.write(error.code);
  process.exit(23);
}
"""
    completed = _node(validate, expected=23)
    assert completed.stdout == "generated_package_identity"


def test_owned_database_success_tears_down_before_return_and_forwards_http_port() -> None:
    lifecycle = """
const producer = await import(process.argv[1]);
const events = [];
let exists = false;
const database = {
  databaseExists() { events.push("exists"); return exists; },
  createDatabase() { events.push("create"); exists = true; },
  deleteDatabase() { events.push("delete"); exists = false; },
  close() { events.push("close"); },
  transaction(kind) {
    events.push(`transaction:${kind}`);
    return {
      query(value) { events.push(`schema:${value}`); },
      commit() { events.push("commit"); },
      close() { events.push("transaction-close"); },
    };
  },
};
let connected;
const runtime = { RustDatabase: { connect(address, name, options) {
  connected = { address, name, options };
  events.push("connect");
  return database;
} } };
const result = await producer.runOwnedDatabase({
  package_: {}, runtime,
  address: "127.0.0.1:32942", databaseName: "fresh_phase2", port: 32943,
  providerSchema: "define entity person;", records: new Map(),
  createOrder: [], cleanupOrder: [],
  versionDetector: async (address, port) => {
    events.push(`version:${address}:${port}`);
    return "3.12.3";
  },
  journeyRunner: () => { events.push("journey"); return { observed: true }; },
});
process.stdout.write(JSON.stringify({ connected, events, result }));
"""
    completed = _node(lifecycle)
    result = json.loads(completed.stdout)
    assert result["connected"] == {
        "address": "127.0.0.1:32942",
        "name": "fresh_phase2",
        "options": {"httpPort": 32943},
    }
    assert result["result"] == {"observed": True}
    assert result["events"] == [
        "connect",
        "version:127.0.0.1:32942:32943",
        "exists",
        "create",
        "exists",
        "transaction:schema",
        "schema:define entity person;",
        "commit",
        "journey",
        "delete",
        "exists",
        "close",
    ]


def test_preexisting_database_and_wrong_version_are_never_deleted() -> None:
    reject = """
const producer = await import(process.argv[1]);
const mode = process.argv[2];
const events = [];
const database = {
  databaseExists() { events.push("exists"); return true; },
  createDatabase() { events.push("create"); },
  deleteDatabase() { events.push("delete"); },
  close() { events.push("close"); },
};
const runtime = { RustDatabase: { connect() { events.push("connect"); return database; } } };
try {
  await producer.runOwnedDatabase({
    package_: {}, runtime, address: "127.0.0.1:32942",
    databaseName: "must_be_absent", port: 32943, providerSchema: "",
    records: new Map(), createOrder: [], cleanupOrder: [],
    versionDetector: async () => {
      events.push("version");
      return mode === "version" ? "3.12.0" : "3.12.3";
    },
    journeyRunner: () => ({}),
  });
} catch (error) {
  process.stdout.write(JSON.stringify({ code: error.code, events }));
  process.exit(23);
}
"""
    preexisting = json.loads(_node(reject, "preexisting", expected=23).stdout)
    assert preexisting == {
        "code": "database_preexisting",
        "events": ["connect", "version", "exists", "close"],
    }
    wrong_version = json.loads(_node(reject, "version", expected=23).stdout)
    assert wrong_version == {
        "code": "server_version_mismatch",
        "events": ["connect", "version", "close"],
    }


def test_schema_and_journey_failures_delete_owned_database_and_close() -> None:
    reject = """
const producer = await import(process.argv[1]);
const mode = process.argv[2];
const events = [];
let exists = false;
const database = {
  databaseExists() { events.push("exists"); return exists; },
  createDatabase() { events.push("create"); exists = true; },
  deleteDatabase() { events.push("delete"); exists = false; },
  close() { events.push("close"); },
  transaction() {
    events.push("transaction");
    return {
      query() { events.push("schema"); if (mode === "schema") throw new Error("schema failed"); },
      commit() { events.push("commit"); },
      close() { events.push("transaction-close"); },
    };
  },
};
const runtime = { RustDatabase: { connect() { events.push("connect"); return database; } } };
try {
  await producer.runOwnedDatabase({
    package_: {}, runtime, address: "127.0.0.1:32942",
    databaseName: "fresh", port: 32943, providerSchema: "schema",
    records: new Map(), createOrder: [], cleanupOrder: [],
    versionDetector: async () => { events.push("version"); return "3.12.3"; },
    journeyRunner: () => {
      events.push("journey");
      if (mode === "journey") throw new Error("journey failed");
      return {};
    },
  });
} catch (error) {
  process.stdout.write(JSON.stringify({ message: error.message, events }));
  process.exit(23);
}
"""
    schema = json.loads(_node(reject, "schema", expected=23).stdout)
    assert schema["message"] == "schema failed"
    assert schema["events"][-4:] == ["transaction-close", "delete", "exists", "close"]
    assert "journey" not in schema["events"]

    journey = json.loads(_node(reject, "journey", expected=23).stdout)
    assert journey["message"] == "journey failed"
    assert journey["events"][-4:] == ["journey", "delete", "exists", "close"]


def test_create_failure_is_never_claimed_or_deleted() -> None:
    reject = """
const producer = await import(process.argv[1]);
const events = [];
const database = {
  databaseExists() { events.push("exists"); return false; },
  createDatabase() { events.push("create"); throw new Error("create failed"); },
  deleteDatabase() { events.push("DELETE_FOREIGN_DATABASE"); },
  close() { events.push("close"); },
};
const runtime = { RustDatabase: { connect() { events.push("connect"); return database; } } };
try {
  await producer.runOwnedDatabase({
    package_: {}, runtime, address: "127.0.0.1:32942",
    databaseName: "fresh", port: 32943, providerSchema: "schema",
    records: new Map(), createOrder: [], cleanupOrder: [],
    versionDetector: async () => "3.12.3",
    journeyRunner: () => ({}),
  });
} catch (error) {
  process.stdout.write(JSON.stringify({ message: error.message, events }));
  process.exit(23);
}
"""
    result = json.loads(_node(reject, expected=23).stdout)
    assert result == {
        "message": "create failed",
        "events": ["connect", "exists", "create", "close"],
    }


def test_delete_failure_takes_precedence_and_still_closes() -> None:
    reject = """
const producer = await import(process.argv[1]);
const events = [];
let exists = false;
const database = {
  databaseExists() { events.push("exists"); return exists; },
  createDatabase() { events.push("create"); exists = true; },
  deleteDatabase() { events.push("delete"); throw new Error("delete failed"); },
  close() { events.push("close"); },
  transaction() { return { query() {}, commit() {}, close() {} }; },
};
const runtime = { RustDatabase: { connect() { events.push("connect"); return database; } } };
try {
  await producer.runOwnedDatabase({
    package_: {}, runtime, address: "127.0.0.1:32942",
    databaseName: "fresh", port: 32943, providerSchema: "schema",
    records: new Map(), createOrder: [], cleanupOrder: [],
    versionDetector: async () => "3.12.3",
    journeyRunner: () => { throw new Error("journey failed"); },
  });
} catch (error) {
  process.stdout.write(JSON.stringify({
    code: error.code,
    cause: error.cause?.message,
    prior: error.cause?.cause?.message,
    events,
  }));
  process.exit(23);
}
"""
    result = json.loads(_node(reject, expected=23).stdout)
    assert result["code"] == "database_teardown_failed"
    assert result["cause"] == "delete failed"
    assert result["prior"] == "journey failed"
    assert result["events"][-2:] == ["delete", "close"]


def test_report_envelope_source_independence_and_public_crud_boundary() -> None:
    source = PRODUCER_PATH.read_text()
    comparator = _comparator_module()
    comparator.validate_producer_source(source)

    for forbidden in (
        "compare_phase2_projection_live",
        "compare_phase2_projection_parity",
        "expected_observations",
        "expected_report",
        "load_contract",
    ):
        assert forbidden not in source
    assert source.count("transaction.query(") == 1
    assert "Aliases.create" not in source
    assert "NetworkLink" not in source
    assert "@type-bridge/node" in source
    assert "RustDatabase.connect" in source
    assert "3c8d072b60c575b4c0381c1ea44088a9c18e01ed52e90730311aadccb72cd0c8" in source
    assert source.index("await runOwnedDatabase({") < source.index(
        "publishReport(output, buildReport(authority, observations))"
    )

    envelope = """
const producer = await import(process.argv[1]);
const observations = Object.fromEntries(producer.OBSERVATION_REFS.map((name) => [name, {}]));
process.stdout.write(JSON.stringify(producer.buildReport({ frozen: true }, observations)));
"""
    report = json.loads(_node(envelope).stdout)
    assert report == {
        "authority": {"frozen": True},
        "binding": "node",
        "format": "typebridge.phase2-projected-live-report/v1",
        "observations": {name: {} for name in comparator.OBSERVATION_REFS},
        "semantic_profile": "typedb-3.12.1/v1",
    }
