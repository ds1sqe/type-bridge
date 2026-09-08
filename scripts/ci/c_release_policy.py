#!/usr/bin/env python3
"""Freeze the selected C release inventory and the explicit CI step contract."""

from __future__ import annotations

import hashlib
import itertools
import json
import re
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
POLICY = ROOT / ".github/release/c-2.2.0.json"
CI = ROOT / ".github/workflows/ci.yml"
VERSION = "2.2.0"
TARGET = "x86_64-unknown-linux-gnu"
CLI = f"type-bridge-cli-artifact-{TARGET}.tar.gz"
RUNTIME = f"type-bridge-c-runtime-abi-1.6-{TARGET}.tar.gz"
GENERATED = "tb_sdkv3-c-de6ec4acca4cfc7ba35cfbbd969dce49c7bf3b709532fca43ac41ed482606264.tar.gz"
SECURITY_FILES = [
    "artifact-manifest.json",
    "cli.spdx.json",
    "runtime.spdx.json",
    "generated.spdx.json",
    "rustsec-report.json",
    "security-scan.json",
    "provenance.json",
    "signature-policy.json",
]
ARTIFACTS = {
    "standalone-cli-artifact-linux-x86_64-gnu": [CLI],
    "c-package-artifacts-linux-x86_64-gnu": [RUNTIME, GENERATED],
    "c-distribution-security-linux-x86_64-gnu": SECURITY_FILES,
    "c-artifact-clean-consumer-linux-x86_64-gnu": ["c-artifact-clean-consumer.json"],
    "c-artifact-live-consumer-linux-x86_64-gnu": [
        "c-artifact-live-journey.json",
        "c-artifact-acceptance-report.json",
    ],
}
PUBLIC_FILES = {
    CLI: f"type-bridge-cli-{VERSION}-{TARGET}.tar.gz",
    RUNTIME: f"type-bridge-c-runtime-{VERSION}-abi-1.6-{TARGET}.tar.gz",
    GENERATED: f"type-bridge-c-sdk-example-{VERSION}.tar.gz",
}
EVIDENCE = f"type-bridge-c-evidence-{VERSION}.tar.gz"
RECEIPT = f"type-bridge-c-verification-{VERSION}.json"
PROMOTION = f"type-bridge-c-promotion-{VERSION}.json"
VERIFY_ARTIFACT = "c-verified-release-2.2.0"
VERIFY_JOB = "Verify FULL C release artifacts"
PUBLISH_JOB = "Promote verified C release artifacts"
WORKFLOW = ".github/workflows/c-release.yml"
IDENTITY = f"https://github.com/ds1sqe/type-bridge/{WORKFLOW}@refs/tags/v{VERSION}"


def expand(value: str, matrix: dict[str, str]) -> str:
    return re.sub(r"\$\{\{ matrix\.([\w-]+) \}\}", lambda m: matrix[m[1]], value)


def step_enabled(expression: str, matrix: dict[str, str], runner: str) -> bool:
    """Evaluate only the closed comparison grammar used by this CI workflow."""
    if expression in {"always()", "success()"}:
        return True
    if expression == "failure()":
        return False
    results = []
    for term in expression.split("&&"):
        match = re.fullmatch(r"(matrix\.[\w-]+|runner\.os)\s*(==|!=)\s*'([^']*)'", term.strip())
        if match is None:
            raise ValueError(f"Unreviewed CI condition: {expression}")
        actual = runner if match[1] == "runner.os" else matrix[match[1].removeprefix("matrix.")]
        results.append((actual == match[3]) == (match[2] == "=="))
    return all(results)


def workflow_jobs() -> dict[str, dict[str, str]]:
    # Only policy maintenance uses YAML. Promotion consumes committed JSON and
    # therefore needs no dependency installer or build toolchain.
    import yaml

    workflow = yaml.load(CI.read_text(), Loader=yaml.BaseLoader)
    result = {}
    for job in workflow["jobs"].values():
        matrix = job.get("strategy", {}).get("matrix", {})
        dimensions = {k: v for k, v in matrix.items() if k not in {"include", "exclude"}}
        if "exclude" in matrix:
            raise ValueError("CI exclusions need an explicit policy implementation")
        cells = [
            dict(zip(dimensions, row, strict=True))
            for row in itertools.product(*dimensions.values())
        ]
        if not dimensions:
            cells = [] if matrix.get("include") else [{}]
        for addition in matrix.get("include", []):
            matches = [
                cell
                for cell in cells
                if all(cell[k] == v for k, v in addition.items() if k in dimensions)
            ]
            if dimensions and matches:
                for cell in matches:
                    cell.update(addition)
            else:
                cells.append(addition.copy())
        for cell in cells:
            runner_name = expand(job["runs-on"], cell)
            runner = (
                "Windows"
                if runner_name.startswith("windows")
                else "macOS"
                if runner_name.startswith("macos")
                else "Linux"
            )
            name = expand(job["name"], cell)
            if name in result:
                raise ValueError(f"Duplicate CI job: {name}")
            steps = {}
            for step in job["steps"]:
                step_name = expand(step["name"], cell)
                if step_name in steps:
                    raise ValueError(f"Duplicate CI step: {name}/{step_name}")
                enabled = step_enabled(step.get("if", "success()"), cell, runner)
                steps[step_name] = "success" if enabled else "skipped"
            result[name] = steps
    return result


def selected_policy() -> dict[str, Any]:
    import yaml

    release_workflow = yaml.load((ROOT / WORKFLOW).read_text(), Loader=yaml.BaseLoader)
    return {
        "format": "typebridge.c-release-policy/v1",
        "version": VERSION,
        "repository": "ds1sqe/type-bridge",
        "repository_id": 1085407082,
        "tag": f"v{VERSION}",
        "abi": "1.6.0",
        "target": TARGET,
        "runner": "ubuntu-24.04",
        "linkage": "shared",
        "typedb": "3.12.3",
        "semantic_profile": "typedb-3.12.1/v1",
        "workflow": WORKFLOW,
        "certificate_identity": IDENTITY,
        "certificate_issuer": "https://token.actions.githubusercontent.com",
        "cosign_version": "3.0.6",
        "artifact_disposition": "unchanged; promotion is a separate signed record",
        "ci_workflow_sha256": hashlib.sha256(CI.read_bytes()).hexdigest(),
        "ci_jobs": workflow_jobs(),
        "ci_artifacts": ARTIFACTS,
        "public_files": PUBLIC_FILES,
        "evidence": EVIDENCE,
        "verification_receipt": RECEIPT,
        "promotion_record": PROMOTION,
        "verification_artifact": VERIFY_ARTIFACT,
        "verification_steps": {
            step["name"]: "success" for step in release_workflow["jobs"]["verify"]["steps"]
        },
        "recovery": "draft-only; absent-or-identical payloads; verify and retain existing signatures",
    }


if __name__ == "__main__":
    POLICY.write_text(json.dumps(selected_policy(), indent=2, sort_keys=True) + "\n")
