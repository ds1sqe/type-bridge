#!/usr/bin/env python3
"""Compare the four `writes` reports of one staged program.

Every binding runs the same program (`tests/contracts/sdk_conformance/writes`):
its plans must have the expected canonical bytes, and, on a live run, the
same answers directly and over the proxy, with the same proxy commits and
provider state the fixture observed.
"""

from __future__ import annotations

import argparse
import hashlib
import sys
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
from sdk_conformance_common import (  # noqa: E402
    ContractError,
    LoadedJson,
    _exact_object,
    _load_json,
    canonical_json_bytes,
)

ROOT = Path(__file__).resolve().parents[2]
SUITE = "tests/contracts/sdk_conformance/writes"
BINDINGS = ("python", "node", "rust", "c")
TRANSPORTS = ("direct", "proxy")
REPORT_FORMAT = "typebridge.sdk-writes-report/v1"
SUMMARY_FORMAT = "typebridge.sdk-writes-comparison/v1"
REPORT_KEYS = {"format", "binding", "program", "semantic_profile", "plans", "results", "observed"}
OBSERVED_KEYS = {"proxy_commits", "direct_state", "proxy_state"}


def load_program(name: str = "purchase", *, root: Path = ROOT) -> tuple[Any, Any]:
    """The program's step layout and its expected bytes, answers and state."""

    directory = root / SUITE / name
    program = _load_json(directory / "program.json", f"{name} program").value
    expected = _load_json(directory / "expected.json", f"{name} expected results").value
    if program.get("program") != name or expected.get("program") != name:
        raise ContractError("invalid_program", f"{name} contract names another program")
    steps = {step["name"]: step["plans"] for step in program["steps"]}
    # A step's `rolled_back` plans are staged and rolled back before its
    # `plans`: they have bytes but no answers.
    built = {
        plan for step in program["steps"] for plan in (*step.get("rolled_back", ()), *step["plans"])
    }
    if set(expected["plans"]) != built:
        raise ContractError("invalid_program", f"{name} expected plans differ from its steps")
    if {step: set(answers) for step, answers in expected["results"].items()} != {
        step: set(plans) for step, plans in steps.items()
    }:
        raise ContractError("invalid_program", f"{name} expected answers differ from its steps")
    return program, expected


def _compare(label: str, actual: Any, expected: Any, code: str) -> None:
    if actual != expected:
        raise ContractError(code, f"{label} is {actual!r}, expected {expected!r}")


def _validate(report: LoadedJson, expected: Any, *, live: bool) -> str:
    value = _exact_object(report.value, REPORT_KEYS, f"{report.path.name}")
    binding = value["binding"]
    if binding not in BINDINGS:
        raise ContractError("unknown_binding", f"{report.path.name} names binding {binding!r}")
    _compare(f"{binding} report format", value["format"], REPORT_FORMAT, "invalid_report_format")
    _compare(f"{binding} program", value["program"], expected["program"], "program_mismatch")
    _compare(
        f"{binding} semantic profile",
        value["semantic_profile"],
        expected["semantic_profile"],
        "semantic_profile_mismatch",
    )
    plans = _exact_object(value["plans"], set(expected["plans"]), f"{binding} plans")
    for name, encoded in expected["plans"].items():
        _compare(f"{binding} plan {name} bytes", plans[name], encoded, "plan_bytes_mismatch")
    results = _exact_object(
        value["results"], set(TRANSPORTS) if live else set(), f"{binding} results"
    )
    for transport in sorted(results):
        steps = _exact_object(
            results[transport], set(expected["results"]), f"{binding} {transport} results"
        )
        for step, answers in expected["results"].items():
            _compare(f"{binding} {transport} {step}", steps[step], answers, "answer_mismatch")
    if not live:
        _compare(f"{binding} observed", value["observed"], None, "unexpected_observation")
        return binding
    observed = _exact_object(value["observed"], OBSERVED_KEYS, f"{binding} observed")
    _compare(
        f"{binding} proxy commits",
        observed["proxy_commits"],
        expected["proxy_commits"],
        "proxy_commits_mismatch",
    )
    for transport in TRANSPORTS:
        _compare(
            f"{binding} {transport} state",
            observed[f"{transport}_state"],
            expected["state"],
            "state_mismatch",
        )
    return binding


def compare_reports(paths: list[Path], *, live: bool, root: Path = ROOT) -> dict[str, Any]:
    """Validate one report per binding against the program's expectations."""

    reports = [_load_json(path, path.name, require_canonical=True) for path in paths]
    # The reports of one run name one suite; its contract is the expectation.
    names = {
        report.value.get("program") if type(report.value) is dict else None for report in reports
    }
    if len(names) != 1 or not isinstance(name := names.pop(), str) or "/" in name:
        raise ContractError("program_mismatch", "the reports name no single program")
    if not (root / SUITE / name).is_dir():
        raise ContractError("program_mismatch", f"no writes suite named {name!r}")
    _, expected = load_program(name, root=root)
    bindings = [_validate(report, expected, live=live) for report in reports]
    if sorted(bindings) != sorted(BINDINGS):
        raise ContractError("incomplete_binding_set", f"reports cover {sorted(bindings)}")
    return {
        "format": SUMMARY_FORMAT,
        "program": expected["program"],
        "semantic_profile": expected["semantic_profile"],
        "bindings": list(BINDINGS),
        "transports": list(TRANSPORTS) if live else [],
        "plan_sha256": {
            name: hashlib.sha256(bytes.fromhex(encoded)).hexdigest()
            for name, encoded in sorted(expected["plans"].items())
        },
        "report_sha256": {
            binding: report.sha256 for binding, report in zip(bindings, reports, strict=True)
        },
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reports", nargs=len(BINDINGS), type=Path, metavar="REPORT")
    parser.add_argument(
        "--live", action="store_true", help="require direct and proxy results and observations"
    )
    arguments = parser.parse_args(argv)
    try:
        summary = compare_reports(arguments.reports, live=arguments.live)
    except (ContractError, OSError, KeyError, TypeError) as error:
        print(f"writes conformance rejected: {error}", file=sys.stderr)
        return 1
    sys.stdout.buffer.write(canonical_json_bytes(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
