"""Run one released ``@type-bridge/node`` through the release workload.

Usage::

    python -m perf.harness.released_node --version VERSION --typedb VERSION --out FILE
        [--core-wheel FILE] [--node-package FILE] [--work-dir DIR]
        [--iterations N] [--warmup N]

``VERSION`` is a stable 2.x release or ``head``. A release installs ``@type-bridge/node``
of that version from npm into its own npm prefix under the work directory
(never a global install), with ``typescript`` to compile the generated
package. ``head`` installs ``--node-package``, an ``npm pack`` tarball of
this tree's ``type-bridge-core/crates/node`` built with ``npm run build``.

Node has no hand-written models, so the workload
(``perf/released/node/workload.mjs``) runs the generated ``Item`` manager.
The package is generated from ``perf/released/workspace`` by the matching
PyPI ``type-bridge`` CLI in a venv, as ``perf.harness.released`` installs it
(``head`` from the source tree and ``--core-wheel``), at semantic profile
``typedb-3.12.1/v1``. ``MISSING`` names the ops a version line's generated
manager does not have; those ops are skipped and have no metric, so the
page shows a gap, never a value.

Each op present becomes ``release.node.<op>`` in ns, like
``release.python.<op>``, with trigger ``release-matrix`` and the measured
package version as ``run.version`` and ``source`` as in
``perf.harness.released``. TypeDB comes from ``TYPEDB_ADDRESS``,
``TYPEDB_HTTP_PORT``, ``TYPEDB_USERNAME`` and ``TYPEDB_PASSWORD``.
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

from perf.harness import released, versions
from perf.harness.micro import REPO_ROOT, collect_run
from perf.harness.pipeline import percentile
from perf.harness.producer import write_result
from perf.harness.schema import Metric, Run, metric_id
from perf.released.python import workload

SDK = "node"
HEAD = released.HEAD
OPS = tuple(workload.OPS)
# Ops a version line's generated ``ProjectedModelManager`` has no call for.
# 2.0 has only ``insert``, ``getByIid`` and ``all``; later lines have all six.
MISSING: dict[str, tuple[str, ...]] = {
    "2.0": ("fetch_by_key", "fetch_filtered", "update", "delete", "batch_insert_1k"),
}


def missing(version: str) -> tuple[str, ...]:
    """The ops ``version`` has no generated call for."""
    released.check_version(version)
    return () if version == HEAD else MISSING.get(versions.line(version), ())


WORKLOAD = REPO_ROOT / "perf" / "released" / "node" / "workload.mjs"
TYPESCRIPT = "typescript@5"
MANIFEST = released.MANIFEST.replace(
    "  python:\n    output: generated/{package}", "  typescript:\n    output: generated/{package}"
)


def database_name(version: str) -> str:
    """``release_node_2_0_2`` for ``2.0.2``."""
    return metric_id("release", SDK, *version.split(".")).replace(".", "_")


def present(version: str) -> tuple[str, ...]:
    """The ops ``version`` runs, in run order."""
    gaps = missing(version)
    return tuple(op for op in OPS if op not in gaps)


def npm_install_command(version: str, prefix: Path, node_package: Path | None) -> list[str]:
    """The isolated ``npm install`` of one version and ``typescript``."""
    present(version)
    if version == HEAD:
        if node_package is None:
            raise ValueError("head needs --node-package, an npm pack tarball of crates/node")
        spec = str(node_package)
    else:
        spec = f"@type-bridge/node@{version}"
    return ["npm", "install", "--silent", "--no-save", "--prefix", str(prefix), spec, TYPESCRIPT]


def manifest(database: str, address: str, http_port: int) -> str:
    return MANIFEST.format(
        profile=released.PROFILE,
        package="typescript",
        database=database,
        address=address,
        port=http_port,
    )


def read_report(path: Path, version: str) -> dict[str, object]:
    """A workload report holding exactly the ops ``version`` has."""
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError(f"{path}: cannot read the workload report: {error}") from error
    if not isinstance(data, dict) or data.get("format") != workload.FORMAT:
        raise ValueError(f"{path}: expected format {workload.FORMAT!r}")
    for key in ("version", "node"):
        if not isinstance(data.get(key), str) or not data[key]:
            raise ValueError(f"{path}: {key} must be a non-empty string")
    ops = data.get("ops")
    expected = list(present(version))
    if (
        not isinstance(ops, list)
        or [op.get("name") for op in ops if isinstance(op, dict)] != expected
    ):
        raise ValueError(f"{path}: ops must be {', '.join(expected)}, in order")
    for op in ops:
        samples = op.get("samples")
        if not isinstance(samples, list) or not samples or not all(map(released._count, samples)):
            raise ValueError(f"{path}: {op['name']}.samples must be non-negative integer ns")
    return data


def convert(report: Mapping[str, object]) -> list[Metric]:
    """``release.node.<op>`` per op of a report that passed ``read_report``."""
    ops = report["ops"]
    assert isinstance(ops, list)
    metrics: list[Metric] = []
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
    """The run block; the installed package must be the requested version."""
    expected = run.version if requested == HEAD else requested
    if report["version"] != expected:
        raise ValueError(f"installed @type-bridge/node {report['version']}, expected {expected}")
    return replace(
        run,
        version=str(report["version"]),
        trigger=released.TRIGGER,
        typedb=typedb,
        source="tree" if requested == HEAD else "registry",
        toolchains=replace(run.toolchains, rust=None, node=str(report["node"])),
    )


def measure(
    version: str,
    work: Path,
    *,
    core_wheel: Path | None,
    node_package: Path | None,
    env: Mapping[str, str],
    iterations: int,
    warmup: int,
) -> Path:
    """Install ``version`` under ``work``, run the workload, return its report."""
    address = env.get("TYPEDB_ADDRESS") or "localhost:1729"
    http_port = int(env.get("TYPEDB_HTTP_PORT") or "8000")
    database = database_name(version)
    prefix = work / "npm"
    prefix.mkdir(parents=True)
    subprocess.run(npm_install_command(version, prefix, node_package), check=True)

    venv = work / "venv"
    python = venv / "bin" / "python"
    minor = f"{sys.version_info.major}.{sys.version_info.minor}"
    subprocess.run(["uv", "venv", "-q", "-p", minor, str(venv)], check=True)
    subprocess.run(released.install_command(version, python, core_wheel), check=True)
    root = work / "workspace"
    shutil.copytree(released.SCHEMA, root / "schema")
    (root / released.MIGRATION.parent).mkdir(parents=True)
    (root / "typebridge.yaml").write_text(manifest(database, address, http_port), encoding="utf-8")
    cli = [str(venv / "bin" / "type-bridge"), "--manifest", str(root / "typebridge.yaml")]
    subprocess.run([*cli, "schema", "generate"], check=True, stdout=subprocess.DEVNULL)

    # Beneath the prefix, so the package resolves @type-bridge/node from it.
    models = prefix / "models"
    shutil.copytree(root / "generated" / "typescript", models)
    tsc = prefix / "node_modules" / ".bin" / "tsc"
    subprocess.run([str(tsc), "-p", str(models)], check=True)

    report = work / "report.json"
    command = ["node", str(WORKLOAD), "--database", database, "--models", str(models)]
    for op in missing(version):
        command += ["--skip", op]
    command += ["--out", str(report), "--iterations", str(iterations), "--warmup", str(warmup)]
    subprocess.run(command, check=True, env=dict(env))
    return report


def main(argv: Sequence[str] | None = None, env: Mapping[str, str] = os.environ) -> int:
    parser = argparse.ArgumentParser(
        prog="python -m perf.harness.released_node",
        description="Run one released @type-bridge/node through the release workload.",
    )
    parser.add_argument(
        "--version", required=True, type=released._version_argument, help="stable release or head"
    )
    parser.add_argument("--typedb", required=True, help="TypeDB server version measured")
    parser.add_argument("--out", type=Path, required=True, help="result file to write")
    parser.add_argument("--core-wheel", type=Path, help="type-bridge-core wheel for head")
    parser.add_argument("--node-package", type=Path, help="crates/node npm pack tarball for head")
    parser.add_argument("--work-dir", type=Path, help="kept work directory; must not exist")
    parser.add_argument("--iterations", type=int, default=30)
    parser.add_argument("--warmup", type=int, default=5)
    parser.add_argument("--started-at", help="RFC 3339 run start; defaults to now")
    arguments = parser.parse_args(argv)
    if arguments.work_dir is not None and arguments.work_dir.exists():
        parser.error(f"work directory {arguments.work_dir} already exists")
    work = arguments.work_dir or Path(tempfile.mkdtemp(prefix="perf-released-node-"))
    work.mkdir(parents=True, exist_ok=True)
    try:
        path = measure(
            arguments.version,
            work,
            core_wheel=arguments.core_wheel,
            node_package=arguments.node_package,
            env=env,
            iterations=arguments.iterations,
            warmup=arguments.warmup,
        )
        report = read_report(path, arguments.version)
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
    for op in missing(arguments.version):
        print(f"{metric_id('release', SDK, op)} [{run.version}]: gap (no generated call)")
    print(f"{arguments.out}: {len(metrics)} metric(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
