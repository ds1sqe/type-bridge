"""The released-version harness: shims, installs, report conversion and the CLI.

No test installs a package or talks to TypeDB: ``measure`` is replaced by a
report written here.
"""

from __future__ import annotations

import ast
import json
import sys
from pathlib import Path

import pytest
import yaml

from perf.harness import micro, released
from perf.harness.validate import check_file
from perf.released.python import workload

SHA = "c7ee52d30dcea6358a8a6a8f1b023f9df7fb5e90"
GITHUB = {
    "GITHUB_SHA": SHA,
    "GITHUB_REF_NAME": "master",
    "GITHUB_EVENT_NAME": "schedule",
    "ImageOS": "ubuntu24",
    "ImageVersion": "20261012.1",
}


@pytest.fixture(autouse=True)
def _rustc(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(micro, "_rustc_version", lambda: "1.94.1")


def report(version: str = "2.0.2", ops: dict[str, list[int]] | None = None) -> dict[str, object]:
    samples = ops or {name: [3_000_000, 1_000_000, 2_000_000] for name in workload.OPS}
    return {
        "format": workload.FORMAT,
        "version": version,
        "python": "3.13.11",
        "ops": [
            {"name": name, "operations": workload.OPS[name], "samples": values}
            for name, values in samples.items()
        ],
    }


def write(tmp_path: Path, document: dict[str, object]) -> Path:
    path = tmp_path / "report.json"
    path.write_text(json.dumps(document), encoding="utf-8")
    return path


def test_every_version_line_has_a_shim() -> None:
    assert workload.shim("2.0.2") == workload.Shim(models="handwritten", schema="schema-manager")
    # 2.1.0's ``migration apply`` needs a 3.12.1 server; the run uses 3.12.3.
    assert workload.shim("2.1.0") == workload.Shim(models="generated", schema="migration-typeql")
    # Every later line, including ones not yet released, takes the current shim.
    for version in ("2.2.6", "2.3.0", "2.4.1", "3.0.0", "head"):
        assert workload.shim(version) == workload.CURRENT
    with pytest.raises(ValueError, match="starts at 2.0"):
        workload.shim("1.5.11")
    with pytest.raises(ValueError, match="not a stable version"):
        workload.shim("2.3.0rc1")


def test_the_harness_takes_any_stable_2x_release_or_head() -> None:
    for version in ("2.0.2", "2.3.0", "2.10.4", "head"):
        released.check_version(version)
    for version in ("1.5.11", "2.3.0rc1", "latest", "2.3"):
        with pytest.raises(ValueError, match="unknown version"):
            released.check_version(version)


def test_workload_imports_only_the_standard_library_at_import_time() -> None:
    tree = ast.parse(Path(workload.__file__).read_text(encoding="utf-8"))
    modules = {
        alias.name.split(".")[0]
        for node in tree.body
        if isinstance(node, ast.Import)
        for alias in node.names
    } | {
        node.module.split(".")[0]
        for node in tree.body
        if isinstance(node, ast.ImportFrom) and node.module
    }
    assert modules <= sys.stdlib_module_names


def test_split_queries_keeps_each_define_block_in_order() -> None:
    typeql = "define\nentity item;\n\ndefine\nattribute stock;\n\n"
    assert workload.split_queries(typeql) == ["define\nentity item;", "define\nattribute stock;"]
    with pytest.raises(ValueError, match="expected a define query"):
        workload.split_queries("match $x isa item;")


def test_the_release_schema_keys_items_by_item_id() -> None:
    fragment = (released.SCHEMA / "fragments" / "item.yaml").read_text(encoding="utf-8")
    assert "item-id: { key: true }" in fragment


def test_install_command_takes_releases_from_pypi_and_head_from_the_tree() -> None:
    python = Path("/w/venv/bin/python")
    assert released.install_command("2.1.0", python, None) == [
        "uv", "pip", "install", "-q", "-p", str(python), "type-bridge==2.1.0",
    ]  # fmt: skip
    wheel = Path("/w/type_bridge_core.whl")
    head = released.install_command("head", python, wheel, root=Path("/src"))
    assert head[-2:] == [str(wheel), "/src"]
    assert "--no-sources" in head
    with pytest.raises(ValueError, match="head needs --core-wheel"):
        released.install_command("head", python, None)
    with pytest.raises(ValueError, match="unknown version '1.4.0'"):
        released.install_command("1.4.0", python, None)


def test_manifest_targets_the_shared_profile_and_package() -> None:
    text = released.manifest("release_python_2_1_0", "localhost:1729", 8000)
    assert "semantic-profile: typedb-3.12.1/v1" in text
    assert f"output: generated/{workload.MODELS_PACKAGE}" in text
    assert "database: release_python_2_1_0" in text
    assert released.database_name("2.1.0") == "release_python_2_1_0"
    assert released.database_name("head") == "release_python_head"


def test_convert_names_one_ns_metric_per_op_with_percentiles(tmp_path: Path) -> None:
    metrics = released.convert(released.read_report(write(tmp_path, report())))
    assert [metric.id for metric in metrics] == [f"release.python.{name}" for name in workload.OPS]
    first = metrics[0]
    assert (first.family, first.unit, first.value, first.p50, first.n) == (
        "release", "ns", 2_000_000, 2_000_000, 3,
    )  # fmt: skip
    assert first.p90 == pytest.approx(2_800_000)
    assert first.samples == [3_000_000, 1_000_000, 2_000_000]


@pytest.mark.parametrize(
    ("document", "message"),
    [
        ({"format": "other"}, "expected format"),
        (report() | {"version": ""}, "version must be"),
        (report(ops={"insert_one": [1]}), "ops must be"),
        (report(ops={name: [] for name in workload.OPS}), "samples must be"),
        (report(ops={name: [-1] for name in workload.OPS}), "samples must be"),
    ],
)
def test_read_report_rejects_malformed_reports(
    tmp_path: Path, document: dict[str, object], message: str
) -> None:
    with pytest.raises(ValueError, match=message):
        released.read_report(write(tmp_path, document))


def test_assemble_records_the_measured_version_and_release_matrix() -> None:
    run = micro.collect_run(GITHUB, started_at="2026-10-17T00:00:00+00:00")
    assembled = released.assemble(run, "2.1.0", report("2.1.0"), "3.12.3")
    assert (assembled.version, assembled.trigger, assembled.typedb) == (
        "2.1.0", "release-matrix", "3.12.3",
    )  # fmt: skip
    assert (assembled.toolchains.rust, assembled.toolchains.python) == (None, "3.13.11")
    assert assembled.source == "registry"
    head = released.assemble(run, "head", report(run.version), "3.12.3")
    assert (head.version, head.source) == (run.version, "tree")
    with pytest.raises(ValueError, match="installed type-bridge 2.2.6, expected 2.1.0"):
        released.assemble(run, "2.1.0", report("2.2.6"), "3.12.3")


def test_main_writes_a_valid_result(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    def measure(version: str, work: Path, **_: object) -> Path:
        return write(work, report(version))

    monkeypatch.setattr(released, "measure", measure)
    out = tmp_path / "release.json"
    work = tmp_path / "work"
    code = released.main(
        ["--version", "2.0.2", "--typedb", "3.12.3", "--out", str(out), "--work-dir", str(work)],
        env=GITHUB,
    )
    assert code == 0
    assert check_file(out).errors == []
    document = json.loads(out.read_text(encoding="utf-8"))
    assert document["run"]["version"] == "2.0.2"
    assert document["run"]["trigger"] == "release-matrix"
    assert {metric["family"] for metric in document["metrics"]} == {"release"}
    assert "release.python.batch_insert_1k [2.0.2]: p50 2.000 ms" in capsys.readouterr().out
    # A kept work directory must not exist yet.
    with pytest.raises(SystemExit):
        released.main(
            [
                "--version",
                "2.0.2",
                "--typedb",
                "3.12.3",
                "--out",
                str(out),
                "--work-dir",
                str(work),
            ],
            env=GITHUB,
        )


def test_main_fails_without_a_result_on_a_version_mismatch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        released, "measure", lambda version, work, **_: write(work, report("9.9.9"))
    )
    out = tmp_path / "release.json"
    code = released.main(
        ["--version", "2.0.2", "--typedb", "3.12.3", "--out", str(out)], env=GITHUB
    )
    assert code == 1
    assert not out.exists()


def test_the_released_job_runs_every_sdk_and_version_into_the_store() -> None:
    workflow = yaml.safe_load((micro.REPO_ROOT / ".github/workflows/perf.yml").read_text())
    job = workflow["jobs"]["released"]

    # The version list comes from the registries, so a release joins on its own.
    assert job["strategy"]["matrix"] == {
        "sdk": ["python", "node"],
        "version": "${{ fromJSON(needs.versions.outputs.versions) }}",
    }
    assert job["needs"] == ["metadata", "versions"]
    versions = workflow["jobs"]["versions"]
    for event in ("schedule", "workflow_dispatch", "release"):
        assert event in versions["if"]
    pick = "\n".join(step.get("run", "") for step in versions["steps"])
    assert "python -m perf.harness.versions" in pick
    assert "--require" in pick
    assert workflow[True]["release"] == {"types": ["published"]}
    # Master commits are measured after their CI run, never on a push or tag.
    assert "push" not in workflow[True]
    assert workflow[True]["workflow_run"] == {
        "workflows": ["CI"],
        "types": ["completed"],
        "branches": ["master"],
    }
    service = job["services"]["typedb"]
    assert service["image"] == "typedb/typedb:3.12.3"
    assert service["options"] == "--tmpfs /var/lib/typedb/data:rw"
    commands = "\n".join(step.get("run", "") for step in job["steps"])
    assert "maturin build --release" in commands
    assert "-m crates/core" not in commands
    assert "npm ci --ignore-scripts" in commands
    assert "npm pack" in commands
    assert "perf.harness.released_node" in commands
    store = workflow["jobs"]["store"]
    assert "released" in store["needs"] and "versions" in store["needs"]
    assert "github.event_name == 'release'" in store["if"]
    # A skipped released job (master pushes) must not skip the store.
    assert store["if"].startswith("!cancelled()")
