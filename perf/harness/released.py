"""Run one released ``type-bridge`` through the release workload.

Usage::

    python -m perf.harness.released --version VERSION --typedb VERSION --out FILE
        [--core-wheel FILE] [--work-dir DIR] [--iterations N] [--warmup N]

``VERSION`` is a stable release from 2.0 on, installed from PyPI with
``uv pip install type-bridge==VERSION``, or ``head``, installed from this
source tree with the ``type-bridge-core`` wheel ``--core-wheel`` names (a
plain ``maturin build --release`` of ``type-bridge-core``, without
``perf-stages``). Each version gets its own venv under the work directory,
so nothing is installed anywhere else.

The workload is ``perf/released/python/workload.py``, run with that venv's
interpreter; its ``shim`` says where each version's ``Item`` model and
schema come from. For a generated-model version this harness first makes
the migration and generates ``release_models`` from
``perf/released/workspace`` with the version's own ``type-bridge`` CLI, at
semantic profile ``typedb-3.12.1/v1``, which every compared version
accepts.

Each op becomes ``release.python.<op>`` in ns: ``value`` and ``p50`` are the
median call, with ``p90``, ``p99``, ``n`` and the raw ``samples``. The run
block names the measured version (``head`` records the source tree's
project version), trigger ``release-matrix`` and ``source``: ``registry``
for a release, ``tree`` for ``head``, which is how the store and the page
tell the head apart without a fake version. TypeDB comes from
``TYPEDB_ADDRESS``, ``TYPEDB_HTTP_PORT``, ``TYPEDB_USERNAME`` and
``TYPEDB_PASSWORD``. A work directory the harness creates is removed at the
end; one passed with ``--work-dir`` must not exist yet and is kept.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from collections.abc import Mapping, Sequence
from dataclasses import replace
from pathlib import Path
from typing import TypeGuard

from perf.harness import versions
from perf.harness.micro import REPO_ROOT, collect_run
from perf.harness.pipeline import percentile
from perf.harness.producer import write_result
from perf.harness.schema import Metric, Run, metric_id
from perf.released.python import workload

SDK = "python"
TRIGGER = "release-matrix"
HEAD = versions.HEAD
PROFILE = "typedb-3.12.1/v1"
WORKLOAD = Path(workload.__file__).resolve()
SCHEMA = REPO_ROOT / "perf" / "released" / "workspace" / "schema"
MIGRATION = Path("migrations", "v2", "0001_initial.typeql")

MANIFEST = """format: typebridge.workspace/v1
schema:
  root: schema/schema.yaml
  ownership: exclusive
  managed-scope: released-perf
compatibility:
  semantic-profile: {profile}
migrations:
  directory: migrations/v2
  app-label: perf
bindings:
  python:
    output: generated/{package}
environments:
  perf:
    database: {database}
    uri: {address}
    http-port: '{port}'
    tls: 'false'
    migrate: 'true'
    credential:
      username: env:TYPEDB_USERNAME
      password: env:TYPEDB_PASSWORD
"""


def database_name(version: str) -> str:
    """``release_python_2_0_2`` for ``2.0.2``."""
    return metric_id("release", SDK, *version.split(".")).replace(".", "_")


def check_version(version: str) -> None:
    """Refuse anything but ``head`` or a stable release from the 2.0 line on."""
    parsed = versions.parse(version)
    if version != HEAD and (parsed is None or parsed[:2] < versions.FIRST_LINE):
        raise ValueError(f"unknown version {version!r}; compare head or a stable 2.x release")


def _version_argument(version: str) -> str:
    try:
        check_version(version)
    except ValueError as error:
        raise argparse.ArgumentTypeError(str(error)) from None
    return version


def install_command(
    version: str, python: Path, core_wheel: Path | None, root: Path = REPO_ROOT
) -> list[str]:
    """The ``uv pip install`` of one version into the venv of ``python``."""
    check_version(version)
    if version != HEAD:
        return ["uv", "pip", "install", "-q", "-p", str(python), f"type-bridge=={version}"]
    if core_wheel is None:
        raise ValueError("head needs --core-wheel, a type-bridge-core wheel of this tree")
    # Without --no-sources, uv resolves type-bridge-core to the workspace path.
    return [
        "uv",
        "pip",
        "install",
        "-q",
        "--no-sources",
        "-p",
        str(python),
        str(core_wheel),
        str(root),
    ]


def manifest(database: str, address: str, http_port: int) -> str:
    return MANIFEST.format(
        profile=PROFILE,
        package=workload.MODELS_PACKAGE,
        database=database,
        address=address,
        port=http_port,
    )


def read_report(path: Path) -> dict[str, object]:
    """A workload report; raises ``ValueError`` when malformed."""
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError(f"{path}: cannot read the workload report: {error}") from error
    if not isinstance(data, dict) or data.get("format") != workload.FORMAT:
        raise ValueError(f"{path}: expected format {workload.FORMAT!r}")
    for key in ("version", "python"):
        if not isinstance(data.get(key), str) or not data[key]:
            raise ValueError(f"{path}: {key} must be a non-empty string")
    ops = data.get("ops")
    if not isinstance(ops, list) or [op.get("name") for op in ops if isinstance(op, dict)] != list(
        workload.OPS
    ):
        raise ValueError(f"{path}: ops must be {', '.join(workload.OPS)}, in order")
    for op in ops:
        samples = op.get("samples")
        if not isinstance(samples, list) or not samples or not all(map(_count, samples)):
            raise ValueError(f"{path}: {op['name']}.samples must be non-negative integer ns")
    return data


def _count(value: object) -> TypeGuard[int]:
    return isinstance(value, int) and not isinstance(value, bool) and value >= 0


def convert(report: Mapping[str, object]) -> list[Metric]:
    """``release.python.<op>`` per op of a report that passed ``read_report``."""
    metrics: list[Metric] = []
    ops = report["ops"]
    assert isinstance(ops, list)
    for op in ops:
        samples = [float(value) for value in op["samples"]]
        p50 = percentile(samples, 0.50)
        metrics.append(
            Metric(
                id=metric_id("release", SDK, op["name"]),
                family="release",
                unit="ns",
                value=p50,
                n=len(samples),
                samples=samples,
                p50=p50,
                p90=percentile(samples, 0.90),
                p99=percentile(samples, 0.99),
            )
        )
    return metrics


def assemble(run: Run, requested: str, report: Mapping[str, object], typedb: str) -> Run:
    """The run block of one version's result.

    The report's ``type_bridge.__version__`` must be the requested release,
    or for ``head`` the source tree's project version (``run.version``), so a wrong install
    never records under another version.
    """
    expected = run.version if requested == HEAD else requested
    if report["version"] != expected:
        raise ValueError(f"installed type-bridge {report['version']}, expected {expected}")
    return replace(
        run,
        version=str(report["version"]),
        trigger=TRIGGER,
        typedb=typedb,
        source="tree" if requested == HEAD else "registry",
        # The measured package was not built in this run.
        toolchains=replace(run.toolchains, rust=None, python=str(report["python"])),
    )


def measure(
    version: str,
    work: Path,
    *,
    core_wheel: Path | None,
    env: Mapping[str, str],
    iterations: int,
    warmup: int,
) -> Path:
    """Install ``version`` under ``work``, run the workload, return its report."""
    selected = workload.shim(version)
    address = env.get("TYPEDB_ADDRESS") or "localhost:1729"
    http_port = int(env.get("TYPEDB_HTTP_PORT") or "8000")
    database = database_name(version)
    venv = work / "venv"
    python = venv / "bin" / "python"
    # The venv runs the harness interpreter's minor version.
    minor = f"{sys.version_info.major}.{sys.version_info.minor}"
    subprocess.run(["uv", "venv", "-q", "-p", minor, str(venv)], check=True)
    subprocess.run(install_command(version, python, core_wheel), check=True)

    command = [str(python), "-I", str(WORKLOAD), "--version", version, "--database", database]
    if selected.models == "generated":
        root = work / "workspace"
        shutil.copytree(SCHEMA, root / "schema")
        (root / MIGRATION.parent).mkdir(parents=True)
        (root / "typebridge.yaml").write_text(
            manifest(database, address, http_port), encoding="utf-8"
        )
        cli = [str(venv / "bin" / "type-bridge"), "--manifest", str(root / "typebridge.yaml")]
        steps = [["migration", "make", "--name", "initial"], ["schema", "generate"]]
        if selected.schema == "migration-apply":
            steps.append(["migration", "apply", "--environment", "perf"])
        for step in steps:
            subprocess.run([*cli, *step], check=True, stdout=subprocess.DEVNULL, env=dict(env))
        command += ["--models", str(root / "generated")]
        if selected.schema == "migration-typeql":
            command += ["--schema", str(root / MIGRATION)]
    report = work / "report.json"
    command += ["--out", str(report), "--iterations", str(iterations), "--warmup", str(warmup)]
    subprocess.run(command, check=True, env=dict(env))
    return report


def main(argv: Sequence[str] | None = None, env: Mapping[str, str] = os.environ) -> int:
    parser = argparse.ArgumentParser(
        prog="python -m perf.harness.released",
        description="Run one released type-bridge through the release workload.",
    )
    parser.add_argument(
        "--version", required=True, type=_version_argument, help="stable release or head"
    )
    parser.add_argument("--typedb", required=True, help="TypeDB server version measured")
    parser.add_argument("--out", type=Path, required=True, help="result file to write")
    parser.add_argument("--core-wheel", type=Path, help="type-bridge-core wheel for head")
    parser.add_argument("--work-dir", type=Path, help="kept work directory; must not exist")
    parser.add_argument("--iterations", type=int, default=30)
    parser.add_argument("--warmup", type=int, default=5)
    parser.add_argument("--started-at", help="RFC 3339 run start; defaults to now")
    arguments = parser.parse_args(argv)
    if arguments.work_dir is not None and arguments.work_dir.exists():
        parser.error(f"work directory {arguments.work_dir} already exists")
    work = arguments.work_dir or Path(tempfile.mkdtemp(prefix="perf-released-"))
    work.mkdir(parents=True, exist_ok=True)
    try:
        path = measure(
            arguments.version,
            work,
            core_wheel=arguments.core_wheel,
            env=env,
            iterations=arguments.iterations,
            warmup=arguments.warmup,
        )
        report = read_report(path)
        run = assemble(
            collect_run(env, started_at=arguments.started_at),
            arguments.version,
            report,
            arguments.typedb,
        )
        metrics = convert(report)
        write_result(run, metrics, arguments.out)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    finally:
        if arguments.work_dir is None:
            shutil.rmtree(work, ignore_errors=True)
    for metric in metrics:
        print(f"{metric.id} [{run.version}]: p50 {metric.value / 1e6:.3f} ms")
    print(f"{arguments.out}: {len(metrics)} metric(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
