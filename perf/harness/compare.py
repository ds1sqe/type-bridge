"""Advisory base/head comparison; regressions never fail the performance gate."""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Sequence
from html import escape
from pathlib import Path
from typing import Any

from perf.harness.validate import check

THRESHOLDS = {"instructions": 1, "ns": 10, "bytes": 1, "s": 15, "ops_per_s": 10}
LIMIT = 200


def validate(document: dict[str, Any]) -> None:
    # An empty comparison baseline is useful even though producers must emit
    # observations. Keep all other result validation, including run metadata.
    candidate = document
    if isinstance(document, dict) and document.get("metrics") == []:
        candidate = document | {
            "metrics": [{"id": "compare.empty", "family": "micro", "unit": "ns", "value": 0}]
        }
    if errors := check(candidate).errors:
        raise ValueError("; ".join(errors))


def render(base: dict[str, Any], head: dict[str, Any]) -> str:
    validate(base)
    validate(head)
    old = {metric["id"]: metric for metric in base["metrics"]}
    new = {metric["id"]: metric for metric in head["metrics"]}
    rows = []
    rank = {"regression": 0, "improvement": 1, "unchanged": 2, "unit changed": 3}
    for name in old.keys() & new.keys():
        a, b = old[name], new[name]
        unit = a["unit"]
        marker, delta = "unit changed", "n/a"
        if unit == b["unit"]:
            change = (
                (b["value"] - a["value"]) / a["value"] * 100
                if a["value"]
                else (float("inf") if b["value"] else 0)
            )
            marker = "unchanged"
            if abs(change) > THRESHOLDS[unit]:
                worse = change < 0 if unit == "ops_per_s" else change > 0
                marker = "regression" if worse else "improvement"
            delta = f"{change:+.2f}%" if change != float("inf") else "+∞"
        displayed_unit = unit if unit == b["unit"] else f"{unit} → {b['unit']}"
        rows.append(
            (
                rank[marker],
                name,
                f"| {name} | {a['value']} | {b['value']} | {displayed_unit} | {delta} | {marker} |",
            )
        )
    lines = [
        "# Performance comparison",
        "",
        "Advisory only; instruction counts are the primary signal.",
        "",
        f"Base `{base['run']['commit']}` → head `{head['run']['commit']}`.",
        "",
        "| Metric | Base | Head | Unit | Delta | Marker |",
        "| --- | ---: | ---: | --- | ---: | --- |",
    ]
    shared = sorted(rows)
    lines += [row[2] for row in shared[:LIMIT]]
    shown = min(len(shared), LIMIT)
    total = len(shared)
    for title, names, source in [
        ("Only in head", new.keys() - old.keys(), new),
        ("Only in base", old.keys() - new.keys(), old),
    ]:
        total += len(names)
        if names:
            lines += ["", f"## {title}", ""]
        for name in sorted(names):
            if shown >= LIMIT:
                break
            metric = source[name]
            lines.append(f"- {name}: {metric['value']} {metric['unit']}")
            shown += 1
    if total > shown:
        lines += ["", f"+{total - shown} more (200-row limit)."]
    return "\n".join(lines) + "\n"


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("base", type=Path)
    parser.add_argument("head", type=Path)
    parser.add_argument("--out", type=Path, required=True)
    try:
        args = parser.parse_args(argv)
    except SystemExit:
        return 0
    try:
        report = render(json.loads(args.base.read_text()), json.loads(args.head.read_text()))
    except (OSError, ValueError, TypeError, KeyError) as error:
        report = f"# Performance comparison\n\nComparison unavailable: {escape(str(error))}\n"
    try:
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(report, encoding="utf-8")
    except OSError as error:
        print(f"comparison unavailable: {error}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
