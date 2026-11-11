"""The released Node harness: gaps, installs, report conversion and the CLI.

No test installs a package or talks to TypeDB: ``measure`` is replaced by a
report written here.
"""

from __future__ import annotations

import json
import re
import subprocess
from pathlib import Path

import pytest

from perf.harness import micro, released_node
from perf.harness.validate import check_file
from perf.released.python import workload

GITHUB = {
    "GITHUB_SHA": "c7ee52d30dcea6358a8a6a8f1b023f9df7fb5e90",
    "GITHUB_REF_NAME": "master",
    "GITHUB_EVENT_NAME": "schedule",
    "ImageOS": "ubuntu24",
    "ImageVersion": "20261012.1",
}


@pytest.fixture(autouse=True)
def _rustc(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(micro, "_rustc_version", lambda: "1.94.1")


def report(version: str = "2.1.0") -> dict[str, object]:
    return {
        "format": workload.FORMAT,
        "version": version,
        "node": "22.12.0",
        "ops": [
            {"name": name, "operations": workload.OPS[name], "samples": [3, 1, 2]}
            for name in released_node.present(version)
        ],
    }


def write(directory: Path, document: dict[str, object]) -> Path:
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / "report.json"
    path.write_text(json.dumps(document), encoding="utf-8")
    return path


def test_the_node_workload_runs_the_python_ops_in_order() -> None:
    source = released_node.WORKLOAD.read_text(encoding="utf-8")
    block = source.split("const OPS = [", 1)[1].split("];", 1)[0]
    assert tuple(re.findall(r'\["(\w+)"', block)) == released_node.OPS == tuple(workload.OPS)


def _run_stub(tmp_path: Path, sdk: str) -> tuple[subprocess.CompletedProcess[str], Path]:
    """Run the workload on empty models and a stub ``@type-bridge/node`` of source ``sdk``."""
    models = tmp_path / "models"
    (models / "dist").mkdir(parents=True)
    (models / "package.json").write_text('{"type":"module"}')
    (models / "dist/index.js").write_text("export {};\n")
    package = models / "node_modules/@type-bridge/node"
    (package / "dist").mkdir(parents=True)
    (package / "package.json").write_text(
        json.dumps({"name": "@type-bridge/node", "main": "dist/index.cjs", "version": "3.0.0"})
    )
    (package / "dist/index.cjs").write_text(sdk)
    output = tmp_path / "report.json"
    result = subprocess.run(
        [
            "node",
            str(released_node.WORKLOAD),
            "--database",
            "constructor_stub",
            "--models",
            str(models),
            "--out",
            str(output),
            "--iterations",
            "1",
            "--warmup",
            "0",
        ],
        capture_output=True,
        text=True,
        check=False,
        timeout=10,
    )
    return result, output


@pytest.mark.parametrize(
    ("exports", "message"),
    [
        ({"Database": "modern"}, "constructor-selected:modern"),
        ({"RustDatabase": "legacy"}, "constructor-selected:legacy"),
        (
            {"Database": "modern", "RustDatabase": "legacy"},
            "constructor-selected:modern",
        ),
        ({}, "Node SDK exports no supported database constructor"),
    ],
)
def test_workload_resolves_released_and_current_database_exports(
    tmp_path: Path, exports: dict[str, str], message: str
) -> None:
    result, output = _run_stub(
        tmp_path,
        "\n".join(
            f"exports.{name} = {{ connect() {{ throw new Error("
            f"{json.dumps('constructor-selected:' + marker)}); }} }};"
            for name, marker in exports.items()
        ),
    )
    assert result.returncode == 1
    assert message in result.stderr
    assert not output.exists()


# A database that logs every call and whose delete fails, as TypeDB's does
# while a transaction is open. The empty models make the ops fail after the
# schema step.
_STUB_DATABASE = """
const log = (call) => process.stderr.write(`call:${call}\\n`);
const db = {
  databaseExists() { return true; },
  deleteDatabase() { log("delete"); throw new Error("delete refused"); },
  close() { log("close"); },
  %s
};
exports.Database = { connect: () => db };
"""


@pytest.mark.parametrize(
    ("schema_call", "calls", "error"),
    [
        (
            "async executeQuery(query, kind) { log(`executeQuery:${kind}`); },",
            ["executeQuery:schema", "delete", "close"],
            "reading 'manager'",
        ),
        (
            """transaction(kind) {
              log(`transaction:${kind}`);
              return {
                query() { log("query"); },
                commit() { log("commit"); },
                close() { log("tx-close"); },
              };
            },""",
            ["transaction:schema", "query", "commit", "delete", "close"],
            "reading 'manager'",
        ),
        (
            """transaction() {
              return {
                query() { throw new Error("schema refused"); },
                close() { log("tx-close"); },
              };
            },""",
            ["tx-close", "delete", "close"],
            "schema refused",
        ),
    ],
)
def test_workload_defines_the_schema_per_version_and_reports_its_own_error(
    tmp_path: Path, schema_call: str, calls: list[str], error: str
) -> None:
    result, output = _run_stub(tmp_path, _STUB_DATABASE % schema_call)

    assert result.returncode == 1
    assert re.findall(r"^call:(.+)$", result.stderr, re.M) == calls
    assert "cleanup: delete refused" in result.stderr
    assert error in result.stderr.split("error: ")[-1]
    assert not output.exists()


def test_gaps_name_only_known_ops_and_lines() -> None:
    for line, ops in released_node.MISSING.items():
        assert re.fullmatch(r"\d+\.\d+", line)
        assert set(ops) <= set(released_node.OPS)
    assert released_node.present("2.0.2") == ("insert_one",)
    # Later lines, including ones not yet released, have every op.
    for version in ("2.1.0", "2.2.6", "2.3.0", "head"):
        assert released_node.present(version) == released_node.OPS
    with pytest.raises(ValueError, match="unknown version"):
        released_node.present("1.9.0")


def test_npm_install_stays_in_the_prefix(tmp_path: Path) -> None:
    command = released_node.npm_install_command("2.1.0", tmp_path, None)
    assert command[:2] == ["npm", "install"]
    assert "-g" not in command and "--global" not in command
    assert command[command.index("--prefix") + 1] == str(tmp_path)
    assert "@type-bridge/node@2.1.0" in command
    tarball = tmp_path / "node.tgz"
    assert str(tarball) in released_node.npm_install_command("head", tmp_path, tarball)
    with pytest.raises(ValueError, match="--node-package"):
        released_node.npm_install_command("head", tmp_path, None)


def test_manifest_generates_typescript_at_the_shared_profile() -> None:
    text = released_node.manifest("release_node_head", "localhost:1729", 8000)
    assert "typescript:\n    output: generated/typescript" in text
    assert "python:" not in text
    assert "semantic-profile: typedb-3.12.1/v1" in text
    assert released_node.database_name("2.0.2") == "release_node_2_0_2"


def test_read_report_takes_only_the_ops_the_version_has(tmp_path: Path) -> None:
    path = write(tmp_path, report("2.0.2"))
    ops = released_node.read_report(path, "2.0.2")["ops"]
    assert isinstance(ops, list)
    assert [op["name"] for op in ops] == ["insert_one"]
    # A gap must stay a gap, and a present op must not go missing.
    with pytest.raises(ValueError, match="ops must be"):
        released_node.read_report(path, "2.1.0")
    with pytest.raises(ValueError, match="ops must be"):
        released_node.read_report(write(tmp_path, report("2.1.0")), "2.0.2")


def test_main_writes_a_valid_result_without_gap_metrics(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    def measure(version: str, work: Path, **_: object) -> Path:
        return write(work, report(version))

    monkeypatch.setattr(released_node, "measure", measure)
    out = tmp_path / "release.json"
    arguments = ["--version", "2.0.2", "--typedb", "3.12.3", "--out", str(out)]
    assert released_node.main([*arguments, "--work-dir", str(tmp_path / "w")], env=GITHUB) == 0
    assert check_file(out).errors == []
    document = json.loads(out.read_text(encoding="utf-8"))
    assert document["run"]["version"] == "2.0.2"
    assert document["run"]["trigger"] == "release-matrix"
    assert document["run"]["source"] == "registry"
    assert document["run"]["toolchains"]["node"] == "22.12.0"
    assert [metric["id"] for metric in document["metrics"]] == ["release.node.insert_one"]
    printed = capsys.readouterr().out
    assert "release.node.update [2.0.2]: gap (no generated call)" in printed


def test_main_fails_without_a_result_on_a_version_mismatch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        released_node, "measure", lambda version, work, **_: write(work, report("2.2.6"))
    )
    out = tmp_path / "release.json"
    code = released_node.main(
        ["--version", "2.1.0", "--typedb", "3.12.3", "--out", str(out)], env=GITHUB
    )
    assert code == 1
    assert not out.exists()
