#!/usr/bin/env python3
"""Fail closed when a filtered Trivy image report contains findings."""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any


class ValidationError(RuntimeError):
    """The security scan report is missing, malformed, or non-empty."""


def validate(payload: Any) -> None:
    if not isinstance(payload, dict):
        raise ValidationError("Trivy report must be a JSON object")
    results = payload.get("Results")
    if not isinstance(results, list):
        raise ValidationError("Trivy report has no Results array")
    findings: list[str] = []
    for result_index, result in enumerate(results):
        if not isinstance(result, dict):
            raise ValidationError(f"Trivy result {result_index} is not an object")
        target = result.get("Target")
        if not isinstance(target, str) or not target:
            target = f"result-{result_index}"
        for field, identifier in (
            ("Vulnerabilities", "VulnerabilityID"),
            ("Secrets", "RuleID"),
            ("Misconfigurations", "ID"),
        ):
            entries = result.get(field)
            if entries is None:
                continue
            if not isinstance(entries, list):
                raise ValidationError(f"Trivy {field} for {target!r} is not an array")
            for entry in entries:
                if not isinstance(entry, dict):
                    raise ValidationError(f"Trivy {field} entry for {target!r} is not an object")
                finding = entry.get(identifier)
                findings.append(f"{target}:{finding if isinstance(finding, str) else 'unknown'}")
    if findings:
        raise ValidationError(
            f"accepted server image has {len(findings)} security findings: {findings!r}"
        )


def main() -> int:
    if len(sys.argv) != 2:
        raise ValidationError("usage: validate_trivy_report.py REPORT.json")
    path = Path(sys.argv[1])
    if not path.is_file() or path.is_symlink():
        raise ValidationError(f"Trivy report is missing or non-regular: {path}")
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValidationError(f"cannot read Trivy report {path}: {error}") from error
    validate(payload)
    print("Trivy report contains no accepted-severity vulnerabilities or secrets.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValidationError as error:
        raise SystemExit(f"Trivy report validation failed: {error}") from error
