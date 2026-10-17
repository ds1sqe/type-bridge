"""The ``perf-result/v1`` model.

Every producer writes one JSON document per run: a ``run`` block describing
where and on what the measurements were taken, and a list of ``metrics``.
``perf/schema/perf-result-v1.json`` is the normative JSON Schema; the
constants here mirror it and a unit test keeps the two in step.
"""

from __future__ import annotations

import re
from dataclasses import asdict, dataclass
from typing import Any, Literal, get_args

FORMAT = "perf-result/v1"

type Family = Literal["micro", "binding", "pipeline", "release", "build", "size", "suite"]
type Unit = Literal["ns", "instructions", "bytes", "s", "ops_per_s"]
# GitHub event names, ``local`` for hand-run measurements, and
# ``release-matrix`` for released versions run through one workload.
type Trigger = Literal[
    "push", "pull_request", "schedule", "workflow_dispatch", "release", "local", "release-matrix"
]

FAMILIES: tuple[str, ...] = get_args(Family.__value__)
UNITS: tuple[str, ...] = get_args(Unit.__value__)
TRIGGERS: tuple[str, ...] = get_args(Trigger.__value__)

# Stable dotted name, e.g. ``pipeline.python.insert.stage.compile``.
METRIC_ID = re.compile(r"^[a-z0-9_]+(\.[a-z0-9_]+)+$")
# Span names share the dotted shape, e.g. ``tb.query.compile``.
STAGE_NAME = METRIC_ID
COMMIT = re.compile(r"^[0-9a-f]{40}$")
_NOT_ID_CHAR = re.compile(r"[^a-z0-9_]")


def metric_id(*segments: str) -> str:
    """Join name parts into a metric id, lowercasing them and mapping any
    character outside ``[a-z0-9_]`` to ``_``.

    ``metric_id("micro", "core", "compile", "simple-match")`` is
    ``micro.core.compile.simple_match``. Raises ``ValueError`` for fewer than
    two parts or an empty part.
    """
    parts = [_NOT_ID_CHAR.sub("_", segment.lower()) for segment in segments]
    joined = ".".join(parts)
    if not METRIC_ID.fullmatch(joined):
        raise ValueError(f"cannot form a metric id from {segments!r}")
    return joined


@dataclass(frozen=True)
class Runner:
    image: str
    cpu: str
    cores: int


@dataclass(frozen=True)
class Toolchains:
    """Toolchain versions; ``None`` means the run did not use that toolchain."""

    rust: str | None
    python: str | None
    node: str | None
    cc: str | None


@dataclass(frozen=True)
class Run:
    commit: str
    branch: str
    version: str
    trigger: Trigger
    runner: Runner
    toolchains: Toolchains
    # ``None`` for runs that never talk to TypeDB (micro, build, size).
    typedb: str | None
    # RFC 3339 with an explicit offset.
    started_at: str


@dataclass(frozen=True)
class Metric:
    """One measurement.

    ``samples`` holds raw measurements only. ``ci95`` is the 95% confidence
    interval of ``value`` as ``[lower, upper]``. ``stages`` maps a span name to
    ns and needs unit ``ns``.
    """

    id: str
    family: Family
    unit: Unit
    value: float
    n: int | None = None
    samples: list[float] | None = None
    ci95: list[float] | None = None
    p50: float | None = None
    p90: float | None = None
    p99: float | None = None
    stages: dict[str, float] | None = None


@dataclass(frozen=True)
class PerfResult:
    run: Run
    metrics: list[Metric]

    def to_dict(self) -> dict[str, Any]:
        """JSON-ready form; absent optional metric fields are omitted."""
        return {
            "format": FORMAT,
            "run": asdict(self.run),
            "metrics": [
                {key: value for key, value in asdict(metric).items() if value is not None}
                for metric in self.metrics
            ],
        }

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> PerfResult:
        """Build from a document that already passed ``perf.harness.validate``."""
        run = data["run"]
        return cls(
            run=Run(
                commit=run["commit"],
                branch=run["branch"],
                version=run["version"],
                trigger=run["trigger"],
                runner=Runner(**run["runner"]),
                toolchains=Toolchains(**run["toolchains"]),
                typedb=run["typedb"],
                started_at=run["started_at"],
            ),
            metrics=[Metric(**metric) for metric in data["metrics"]],
        )
