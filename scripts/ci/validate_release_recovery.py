#!/usr/bin/env python3
"""Validate the immutable GitHub evidence used to recover the 2.0.0 release.

The committed manifest is the recovery authority. Its schema is intentionally
small and closed:

.. code-block:: json

    {
      "schema_version": 1,
      "run": {
        "id": 123,
        "event": "push",
        "head_branch": "v2.0.0",
        "head_sha": "<40 lowercase hex characters>",
        "run_attempt": 1,
        "status": "completed",
        "conclusion": "failure",
        "workflow": {
          "id": 456,
          "name": "Release",
          "path": ".github/workflows/release.yml"
        }
      },
      "jobs": {"exact GitHub job name": "success"},
      "known_failure": {
        "job": "Preflight selected release channel",
        "step": "Authenticate npm publication credential"
      },
      "skipped_mutators": ["Create GitHub Release"],
      "artifacts": [
        {
          "id": 789,
          "name": "artifact-name",
          "size_in_bytes": 1234,
          "digest": "sha256:<64 lowercase hex characters>",
          "files": [
            {
              "path": "relative/payload.whl",
              "size_in_bytes": 1234,
              "sha256": "<64 lowercase hex characters>"
            }
          ]
        }
      ]
    }

``files`` remains optional for internal API-only validation. The command-line
recovery gate requires ``--artifact-root``; every artifact must define
``files``, and the root must contain exactly one unmerged directory per
artifact.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import sys
from collections import Counter
from collections.abc import Mapping, Sequence
from pathlib import Path, PurePosixPath
from typing import Any

SCHEMA_VERSION = 1
RELEASE_TAG = "v2.0.0"
RELEASE_RUN_ID = 30_612_912_483
RELEASE_RUN_ATTEMPT = 1
RELEASE_HEAD_SHA = "aacf4d16486a3a3bae47c3b10c1d526c587dd7a7"
RELEASE_WORKFLOW_ID = 229_807_619
RELEASE_WORKFLOW_NAME = "Release"
RELEASE_WORKFLOW_PATH = ".github/workflows/release.yml"
KNOWN_FAILURE_JOB = "Preflight selected release channel"
KNOWN_FAILURE_STEP = "Authenticate npm publication credential"
SKIPPED_MUTATORS = frozenset(
    {
        "Publish accepted TypeBridge server OCI",
        "Publish Node package to npm",
        "Publish type-bridge-core to PyPI",
        "Publish type-bridge to PyPI",
        "Create GitHub Release",
    }
)

EXPECTED_JOB_COUNTS = {"success": 34, "failure": 1, "skipped": 5}
EXPECTED_JOB_COUNT = sum(EXPECTED_JOB_COUNTS.values())
EXPECTED_ARTIFACT_COUNT = 19
EXPECTED_PAYLOAD_FILE_COUNT = 36

MAX_JSON_BYTES = {
    "manifest": 4 * 1024 * 1024,
    "run": 2 * 1024 * 1024,
    "jobs": 16 * 1024 * 1024,
    "artifacts": 16 * 1024 * 1024,
}
MAX_JSON_DEPTH = 64
MAX_JSON_NODES = 200_000
MAX_JSON_CONTAINER_ITEMS = 20_000
MAX_JSON_STRING_BYTES = 1024 * 1024
MAX_JOB_STEPS = 100
MAX_ARTIFACT_SIZE_BYTES = 10 * 1024**3
MAX_PAYLOAD_FILE_SIZE_BYTES = 10 * 1024**3
MAX_TOTAL_PAYLOAD_SIZE_BYTES = 32 * 1024**3
MAX_PAYLOAD_PATH_BYTES = 1024
MAX_PAYLOAD_COMPONENT_BYTES = 255
MAX_PAYLOAD_DEPTH = 32
MAX_FILESYSTEM_ENTRIES = 20_000

SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")
SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")
ARTIFACT_DIGEST_PATTERN = re.compile(r"^sha256:[0-9a-f]{64}$")
ARTIFACT_NAME_PATTERN = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,254}$")
KNOWN_JOB_CONCLUSIONS = frozenset(EXPECTED_JOB_COUNTS)
KNOWN_STEP_CONCLUSIONS = frozenset({"success", "failure", "skipped"})


class ValidationError(RuntimeError):
    """The recovery evidence is malformed, ambiguous, or not exact."""


def _unique_json_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    """Reject duplicate object keys instead of accepting the last value."""
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValidationError(f"JSON contains a duplicate key: {key!r}")
        result[key] = value
    return result


def _reject_json_float(value: str) -> None:
    """Reject numbers outside the integer-only GitHub evidence schema."""
    raise ValidationError(f"JSON contains a floating-point number: {value!r}")


def _reject_json_constant(value: str) -> None:
    """Reject non-standard NaN and infinity constants accepted by json.loads."""
    raise ValidationError(f"JSON contains a non-standard numeric constant: {value!r}")


def _enforce_json_structure(value: Any, *, label: str) -> None:
    """Bound the decoded tree even when untrusted fields are not otherwise used."""
    nodes = 0
    stack: list[tuple[Any, int]] = [(value, 0)]
    while stack:
        item, depth = stack.pop()
        nodes += 1
        if nodes > MAX_JSON_NODES:
            raise ValidationError(f"{label} JSON exceeds the decoded-node budget")
        if depth > MAX_JSON_DEPTH:
            raise ValidationError(f"{label} JSON exceeds the nesting-depth budget")

        if isinstance(item, dict):
            if len(item) > MAX_JSON_CONTAINER_ITEMS:
                raise ValidationError(f"{label} JSON object exceeds the item budget")
            for key, child in item.items():
                if len(key.encode("utf-8")) > MAX_JSON_STRING_BYTES:
                    raise ValidationError(f"{label} JSON contains an oversized object key")
                stack.append((child, depth + 1))
        elif isinstance(item, list):
            if len(item) > MAX_JSON_CONTAINER_ITEMS:
                raise ValidationError(f"{label} JSON array exceeds the item budget")
            stack.extend((child, depth + 1) for child in item)
        elif isinstance(item, str):
            if len(item.encode("utf-8")) > MAX_JSON_STRING_BYTES:
                raise ValidationError(f"{label} JSON contains an oversized string")
        elif item is None or isinstance(item, (bool, int)):
            if type(item) is int and item.bit_length() > 128:
                raise ValidationError(f"{label} JSON contains an oversized integer")
        else:
            raise ValidationError(
                f"{label} JSON contains an unsupported value type: {type(item).__name__}"
            )


def _read_regular_file(path: Path, *, label: str, maximum_bytes: int) -> bytes:
    """Read one bounded regular file without accepting a final symlink."""
    try:
        path_stat = path.lstat()
    except OSError as error:
        raise ValidationError(f"Could not inspect {label} {path}: {error}") from error
    if stat.S_ISLNK(path_stat.st_mode) or not stat.S_ISREG(path_stat.st_mode):
        raise ValidationError(f"{label} is linked or non-regular: {path}")
    if path_stat.st_size < 0 or path_stat.st_size > maximum_bytes:
        raise ValidationError(
            f"{label} exceeds the byte budget: size={path_stat.st_size}, maximum={maximum_bytes}"
        )

    flags = os.O_RDONLY | getattr(os, "O_BINARY", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        raise ValidationError(f"Could not open {label} {path}: {error}") from error
    try:
        with os.fdopen(descriptor, "rb") as stream:
            opened_stat = os.fstat(stream.fileno())
            if not stat.S_ISREG(opened_stat.st_mode):
                raise ValidationError(f"{label} is non-regular: {path}")
            if (
                opened_stat.st_dev,
                opened_stat.st_ino,
            ) != (
                path_stat.st_dev,
                path_stat.st_ino,
            ):
                raise ValidationError(f"{label} changed before it was opened: {path}")
            body = stream.read(maximum_bytes + 1)
            finished_stat = os.fstat(stream.fileno())
    except OSError as error:
        raise ValidationError(f"Could not read {label} {path}: {error}") from error

    if len(body) > maximum_bytes:
        raise ValidationError(f"{label} exceeds the byte budget while being read: {path}")
    if (
        opened_stat.st_size != finished_stat.st_size
        or opened_stat.st_mtime_ns != finished_stat.st_mtime_ns
        or len(body) != finished_stat.st_size
    ):
        raise ValidationError(f"{label} changed while it was read: {path}")
    return body


def read_json(path: Path, *, label: str) -> Any:
    """Read unique-key, bounded, integer-only UTF-8 JSON."""
    body = _read_regular_file(path, label=f"{label} JSON", maximum_bytes=MAX_JSON_BYTES[label])
    try:
        payload = json.loads(
            body.decode("utf-8"),
            object_pairs_hook=_unique_json_object,
            parse_float=_reject_json_float,
            parse_constant=_reject_json_constant,
        )
    except ValidationError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError, RecursionError, ValueError) as error:
        raise ValidationError(f"{label} JSON is malformed: {error}") from error
    _enforce_json_structure(payload, label=label)
    return payload


def canonical_json_sha256(value: Any) -> str:
    """Return the canonical SHA-256 used to bind the committed ledger."""
    canonical = json.dumps(
        value,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return hashlib.sha256(canonical).hexdigest()


def validate_manifest_digest(value: Any, *, expected_sha256: str) -> str:
    """Require the parsed manifest to match one explicit canonical digest."""
    if SHA256_PATTERN.fullmatch(expected_sha256) is None:
        raise ValidationError("expected manifest SHA-256 is not canonical lowercase hexadecimal")
    actual_sha256 = canonical_json_sha256(value)
    if actual_sha256 != expected_sha256:
        raise ValidationError(
            "recovery manifest SHA-256 mismatch: "
            f"actual={actual_sha256!r}, expected={expected_sha256!r}"
        )
    return actual_sha256


def _exact_object(
    value: Any,
    *,
    label: str,
    required: set[str] | frozenset[str],
    optional: set[str] | frozenset[str] = frozenset(),
) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ValidationError(f"{label} must be an object")
    actual = set(value)
    missing = sorted(required - actual)
    unexpected = sorted(actual - required - optional)
    if missing or unexpected:
        raise ValidationError(
            f"{label} has invalid fields: missing={missing!r}, unexpected={unexpected!r}"
        )
    return value


def _integer(
    value: Any,
    *,
    label: str,
    minimum: int = 0,
    maximum: int = 2**63 - 1,
) -> int:
    if type(value) is not int or not minimum <= value <= maximum:
        raise ValidationError(
            f"{label} must be an integer in the range {minimum}..{maximum}: {value!r}"
        )
    return value


def _text(value: Any, *, label: str, maximum_bytes: int = 512) -> str:
    if (
        not isinstance(value, str)
        or not value
        or len(value.encode("utf-8")) > maximum_bytes
        or not value.isprintable()
    ):
        raise ValidationError(f"{label} must be non-empty bounded printable text: {value!r}")
    return value


def _expected(value: Any, expected: Any, *, label: str) -> None:
    if type(value) is not type(expected) or value != expected:
        raise ValidationError(f"{label} mismatch: actual={value!r}, expected={expected!r}")


def _validate_workflow_manifest(value: Any) -> dict[str, Any]:
    workflow = _exact_object(
        value,
        label="manifest run workflow",
        required={"id", "name", "path"},
    )
    _expected(
        workflow["id"],
        RELEASE_WORKFLOW_ID,
        label="manifest workflow id",
    )
    _expected(
        workflow["name"],
        RELEASE_WORKFLOW_NAME,
        label="manifest workflow name",
    )
    _expected(
        workflow["path"],
        RELEASE_WORKFLOW_PATH,
        label="manifest workflow path",
    )
    return workflow


def _validate_run_manifest(value: Any) -> dict[str, Any]:
    run = _exact_object(
        value,
        label="manifest run",
        required={
            "id",
            "event",
            "head_branch",
            "head_sha",
            "run_attempt",
            "status",
            "conclusion",
            "workflow",
        },
    )
    _expected(run["id"], RELEASE_RUN_ID, label="manifest run id")
    _expected(run["event"], "push", label="manifest run event")
    _expected(run["head_branch"], RELEASE_TAG, label="manifest run head branch")
    if not isinstance(run["head_sha"], str) or SHA_PATTERN.fullmatch(run["head_sha"]) is None:
        raise ValidationError("manifest run head SHA must be 40 lowercase hexadecimal characters")
    _expected(run["head_sha"], RELEASE_HEAD_SHA, label="manifest run head SHA")
    _expected(run["run_attempt"], RELEASE_RUN_ATTEMPT, label="manifest run attempt")
    _expected(run["status"], "completed", label="manifest run status")
    _expected(run["conclusion"], "failure", label="manifest run conclusion")
    _validate_workflow_manifest(run["workflow"])
    return run


def _validate_jobs_manifest(
    value: Any,
    *,
    known_failure: Mapping[str, Any],
    skipped_mutators: Sequence[Any],
) -> dict[str, str]:
    if not isinstance(value, dict):
        raise ValidationError("manifest jobs must be a job-name-to-conclusion object")
    if len(value) != EXPECTED_JOB_COUNT:
        raise ValidationError(
            f"manifest jobs count mismatch: actual={len(value)}, expected={EXPECTED_JOB_COUNT}"
        )

    jobs: dict[str, str] = {}
    for raw_name, conclusion in value.items():
        name = _text(raw_name, label="manifest job name")
        if not isinstance(conclusion, str) or conclusion not in KNOWN_JOB_CONCLUSIONS:
            raise ValidationError(
                f"manifest job has an invalid conclusion: name={name!r}, conclusion={conclusion!r}"
            )
        jobs[name] = conclusion

    actual_counts = Counter(jobs.values())
    if dict(actual_counts) != EXPECTED_JOB_COUNTS:
        raise ValidationError(
            "manifest job conclusion counts mismatch: "
            f"actual={dict(sorted(actual_counts.items()))!r}, "
            f"expected={EXPECTED_JOB_COUNTS!r}"
        )

    failure_names = {name for name, conclusion in jobs.items() if conclusion == "failure"}
    if failure_names != {known_failure["job"]}:
        raise ValidationError(
            f"manifest failure-job set mismatch: actual={sorted(failure_names)!r}"
        )
    skipped_names = {name for name, conclusion in jobs.items() if conclusion == "skipped"}
    if skipped_names != set(skipped_mutators):
        raise ValidationError(
            f"manifest skipped-job set mismatch: actual={sorted(skipped_names)!r}"
        )
    return jobs


def _validate_known_failure(value: Any) -> dict[str, str]:
    failure = _exact_object(
        value,
        label="manifest known failure",
        required={"job", "step"},
    )
    _expected(failure["job"], KNOWN_FAILURE_JOB, label="known failure job")
    _expected(failure["step"], KNOWN_FAILURE_STEP, label="known failure step")
    return failure


def _validate_skipped_mutators(value: Any) -> tuple[str, ...]:
    if not isinstance(value, list):
        raise ValidationError("manifest skipped_mutators must be an array")
    mutators = tuple(_text(item, label="manifest skipped mutator") for item in value)
    if len(set(mutators)) != len(mutators):
        raise ValidationError("manifest skipped_mutators contains a duplicate name")
    if set(mutators) != SKIPPED_MUTATORS:
        raise ValidationError(
            "manifest skipped mutators mismatch: "
            f"actual={sorted(mutators)!r}, expected={sorted(SKIPPED_MUTATORS)!r}"
        )
    return mutators


def _payload_path(value: Any, *, label: str) -> str:
    if not isinstance(value, str) or not value or "\\" in value or not value.isprintable():
        raise ValidationError(f"{label} is not a safe relative POSIX path: {value!r}")
    if len(value.encode("utf-8")) > MAX_PAYLOAD_PATH_BYTES:
        raise ValidationError(f"{label} exceeds the path byte budget: {value!r}")
    path = PurePosixPath(value)
    if (
        path.is_absolute()
        or path.as_posix() != value
        or len(path.parts) > MAX_PAYLOAD_DEPTH
        or any(
            part in {"", ".", ".."} or len(part.encode("utf-8")) > MAX_PAYLOAD_COMPONENT_BYTES
            for part in path.parts
        )
    ):
        raise ValidationError(f"{label} is not a normalized bounded relative path: {value!r}")
    return value


def _validate_payload_files(value: Any, *, artifact_name: str) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        raise ValidationError(f"artifact {artifact_name!r} files must be an array")
    if len(value) > MAX_FILESYSTEM_ENTRIES:
        raise ValidationError(f"artifact {artifact_name!r} file inventory exceeds the entry budget")

    files: list[dict[str, Any]] = []
    paths: set[str] = set()
    for index, raw_file in enumerate(value):
        file = _exact_object(
            raw_file,
            label=f"artifact {artifact_name!r} file {index}",
            required={"path", "size_in_bytes", "sha256"},
        )
        path = _payload_path(
            file["path"],
            label=f"artifact {artifact_name!r} file {index} path",
        )
        if path in paths:
            raise ValidationError(
                f"artifact {artifact_name!r} contains a duplicate file path: {path!r}"
            )
        paths.add(path)
        _integer(
            file["size_in_bytes"],
            label=f"artifact {artifact_name!r} file {path!r} size",
            maximum=MAX_PAYLOAD_FILE_SIZE_BYTES,
        )
        if not isinstance(file["sha256"], str) or SHA256_PATTERN.fullmatch(file["sha256"]) is None:
            raise ValidationError(
                f"artifact {artifact_name!r} file {path!r} has an invalid SHA-256"
            )
        files.append(file)
    return files


def _validate_artifacts_manifest(value: Any) -> dict[str, dict[str, Any]]:
    if not isinstance(value, list):
        raise ValidationError("manifest artifacts must be an array")
    if len(value) != EXPECTED_ARTIFACT_COUNT:
        raise ValidationError(
            "manifest artifact count mismatch: "
            f"actual={len(value)}, expected={EXPECTED_ARTIFACT_COUNT}"
        )

    artifacts: dict[str, dict[str, Any]] = {}
    artifact_ids: set[int] = set()
    casefold_names: set[str] = set()
    total_payload_size = 0
    for index, raw_artifact in enumerate(value):
        artifact = _exact_object(
            raw_artifact,
            label=f"manifest artifact {index}",
            required={"id", "name", "size_in_bytes", "digest"},
            optional={"files"},
        )
        artifact_id = _integer(
            artifact["id"],
            label=f"manifest artifact {index} id",
            minimum=1,
        )
        name = artifact["name"]
        if (
            not isinstance(name, str)
            or ARTIFACT_NAME_PATTERN.fullmatch(name) is None
            or name.endswith(".")
        ):
            raise ValidationError(f"manifest artifact {index} has an unsafe name: {name!r}")
        if name in artifacts or name.casefold() in casefold_names:
            raise ValidationError(f"manifest contains a duplicate artifact name: {name!r}")
        if artifact_id in artifact_ids:
            raise ValidationError(f"manifest contains a duplicate artifact id: {artifact_id}")
        artifact_ids.add(artifact_id)
        casefold_names.add(name.casefold())
        _integer(
            artifact["size_in_bytes"],
            label=f"manifest artifact {name!r} size",
            minimum=1,
            maximum=MAX_ARTIFACT_SIZE_BYTES,
        )
        if (
            not isinstance(artifact["digest"], str)
            or ARTIFACT_DIGEST_PATTERN.fullmatch(artifact["digest"]) is None
        ):
            raise ValidationError(f"manifest artifact {name!r} has an invalid digest")
        if "files" in artifact:
            files = _validate_payload_files(artifact["files"], artifact_name=name)
            total_payload_size += sum(file["size_in_bytes"] for file in files)
        artifacts[name] = artifact

    if total_payload_size > MAX_TOTAL_PAYLOAD_SIZE_BYTES:
        raise ValidationError(
            "manifest payload inventory exceeds the total-size budget: "
            f"actual={total_payload_size}, maximum={MAX_TOTAL_PAYLOAD_SIZE_BYTES}"
        )
    return artifacts


def validate_manifest(value: Any) -> dict[str, Any]:
    """Validate and normalize the closed recovery-manifest schema."""
    manifest = _exact_object(
        value,
        label="recovery manifest",
        required={
            "schema_version",
            "run",
            "jobs",
            "known_failure",
            "skipped_mutators",
            "artifacts",
        },
    )
    _expected(manifest["schema_version"], SCHEMA_VERSION, label="manifest schema version")
    run = _validate_run_manifest(manifest["run"])
    known_failure = _validate_known_failure(manifest["known_failure"])
    skipped_mutators = _validate_skipped_mutators(manifest["skipped_mutators"])
    jobs = _validate_jobs_manifest(
        manifest["jobs"],
        known_failure=known_failure,
        skipped_mutators=skipped_mutators,
    )
    artifacts = _validate_artifacts_manifest(manifest["artifacts"])
    return {
        "schema_version": SCHEMA_VERSION,
        "run": run,
        "jobs": jobs,
        "known_failure": known_failure,
        "skipped_mutators": skipped_mutators,
        "artifacts": artifacts,
    }


def validate_run_snapshot(value: Any, *, expected_run: Mapping[str, Any]) -> None:
    """Require one API run snapshot to match every manifest identity field."""
    run = _exact_object(
        value,
        label="GitHub run snapshot",
        required={
            "id",
            "event",
            "head_branch",
            "head_sha",
            "run_attempt",
            "status",
            "conclusion",
            "workflow_id",
            "name",
            "path",
        },
        optional=set(value) if isinstance(value, dict) else frozenset(),
    )
    workflow = expected_run["workflow"]
    comparisons = {
        "id": expected_run["id"],
        "event": expected_run["event"],
        "head_branch": expected_run["head_branch"],
        "head_sha": expected_run["head_sha"],
        "run_attempt": expected_run["run_attempt"],
        "status": expected_run["status"],
        "conclusion": expected_run["conclusion"],
        "workflow_id": workflow["id"],
        "name": workflow["name"],
        "path": workflow["path"],
    }
    for field, expected_value in comparisons.items():
        _expected(run[field], expected_value, label=f"GitHub run {field}")


def _validate_job_provenance(job: Mapping[str, Any], *, expected_run: Mapping[str, Any]) -> None:
    comparisons = {
        "run_id": expected_run["id"],
        "run_attempt": expected_run["run_attempt"],
        "workflow_name": expected_run["workflow"]["name"],
        "head_branch": expected_run["head_branch"],
        "head_sha": expected_run["head_sha"],
    }
    for field, expected_value in comparisons.items():
        if field not in job:
            raise ValidationError(f"GitHub job is missing provenance field {field!r}")
        _expected(job[field], expected_value, label=f"GitHub job {job.get('name')!r} {field}")


def _validated_steps(job: Mapping[str, Any]) -> list[dict[str, Any]]:
    steps = job.get("steps")
    if not isinstance(steps, list) or len(steps) > MAX_JOB_STEPS:
        raise ValidationError(f"GitHub job {job.get('name')!r} has an invalid steps array")
    validated: list[dict[str, Any]] = []
    for index, raw_step in enumerate(steps):
        if not isinstance(raw_step, dict):
            raise ValidationError(f"GitHub job {job.get('name')!r} step {index} must be an object")
        for field in ("name", "status", "conclusion"):
            if field not in raw_step:
                raise ValidationError(
                    f"GitHub job {job.get('name')!r} step {index} is missing {field!r}"
                )
        _text(raw_step["name"], label=f"GitHub job {job.get('name')!r} step name")
        _expected(
            raw_step["status"],
            "completed",
            label=f"GitHub job {job.get('name')!r} step {raw_step['name']!r} status",
        )
        if raw_step["conclusion"] not in KNOWN_STEP_CONCLUSIONS:
            raise ValidationError(
                f"GitHub job {job.get('name')!r} step {raw_step['name']!r} "
                f"has an invalid conclusion: {raw_step['conclusion']!r}"
            )
        validated.append(raw_step)
    return validated


def validate_jobs_snapshot(
    value: Any,
    *,
    expected_run: Mapping[str, Any],
    expected_jobs: Mapping[str, str],
    known_failure: Mapping[str, str],
    skipped_mutators: Sequence[str],
) -> Counter[str]:
    """Require the complete jobs page and the one allowlisted preflight failure."""
    payload = _exact_object(
        value,
        label="GitHub jobs snapshot",
        required={"total_count", "jobs"},
        optional=set(value) if isinstance(value, dict) else frozenset(),
    )
    total_count = _integer(
        payload["total_count"],
        label="GitHub jobs total_count",
        maximum=100,
    )
    jobs = payload["jobs"]
    if not isinstance(jobs, list):
        raise ValidationError("GitHub jobs snapshot jobs must be an array")
    if total_count != len(jobs) or total_count != len(expected_jobs):
        raise ValidationError(
            "GitHub jobs snapshot is incomplete or has the wrong count: "
            f"total_count={total_count}, page_count={len(jobs)}, "
            f"expected={len(expected_jobs)}"
        )

    actual_jobs: dict[str, str] = {}
    job_ids: set[int] = set()
    failed_steps: list[tuple[str, str]] = []
    skipped_mutator_set = set(skipped_mutators)
    for index, raw_job in enumerate(jobs):
        if not isinstance(raw_job, dict):
            raise ValidationError(f"GitHub job {index} must be an object")
        for field in ("id", "name", "status", "conclusion", "steps"):
            if field not in raw_job:
                raise ValidationError(f"GitHub job {index} is missing field {field!r}")
        job_id = _integer(raw_job["id"], label=f"GitHub job {index} id", minimum=1)
        if job_id in job_ids:
            raise ValidationError(f"GitHub jobs snapshot contains a duplicate job id: {job_id}")
        job_ids.add(job_id)
        name = _text(raw_job["name"], label=f"GitHub job {index} name")
        if name in actual_jobs:
            raise ValidationError(f"GitHub jobs snapshot contains a duplicate job name: {name!r}")
        _expected(raw_job["status"], "completed", label=f"GitHub job {name!r} status")
        conclusion = raw_job["conclusion"]
        if not isinstance(conclusion, str) or conclusion not in KNOWN_JOB_CONCLUSIONS:
            raise ValidationError(f"GitHub job {name!r} has an invalid conclusion: {conclusion!r}")
        actual_jobs[name] = conclusion
        _validate_job_provenance(raw_job, expected_run=expected_run)
        steps = _validated_steps(raw_job)
        if name in skipped_mutator_set and steps:
            raise ValidationError(f"skipped mutator {name!r} must have zero steps")
        failed_steps.extend(
            (name, step["name"]) for step in steps if step["conclusion"] == "failure"
        )

    if actual_jobs != dict(expected_jobs):
        missing = sorted(set(expected_jobs) - set(actual_jobs))
        unexpected = sorted(set(actual_jobs) - set(expected_jobs))
        mismatched = sorted(
            name
            for name in set(actual_jobs) & set(expected_jobs)
            if actual_jobs[name] != expected_jobs[name]
        )
        raise ValidationError(
            "GitHub job name/conclusion set mismatch: "
            f"missing={missing!r}, unexpected={unexpected!r}, "
            f"mismatched={mismatched!r}"
        )

    expected_failed_step = (known_failure["job"], known_failure["step"])
    if failed_steps != [expected_failed_step]:
        raise ValidationError(
            "GitHub failed-step set mismatch: "
            f"actual={failed_steps!r}, expected={[expected_failed_step]!r}"
        )
    return Counter(actual_jobs.values())


def validate_artifacts_snapshot(
    value: Any,
    *,
    expected_run: Mapping[str, Any],
    expected_artifacts: Mapping[str, Mapping[str, Any]],
) -> None:
    """Require one complete, unexpired artifact page with exact identities."""
    payload = _exact_object(
        value,
        label="GitHub artifacts snapshot",
        required={"total_count", "artifacts"},
        optional=set(value) if isinstance(value, dict) else frozenset(),
    )
    total_count = _integer(
        payload["total_count"],
        label="GitHub artifacts total_count",
        maximum=100,
    )
    artifacts = payload["artifacts"]
    if not isinstance(artifacts, list):
        raise ValidationError("GitHub artifacts snapshot artifacts must be an array")
    if total_count != len(artifacts) or total_count != len(expected_artifacts):
        raise ValidationError(
            "GitHub artifacts snapshot is incomplete or has the wrong count: "
            f"total_count={total_count}, page_count={len(artifacts)}, "
            f"expected={len(expected_artifacts)}"
        )

    actual_names: set[str] = set()
    actual_ids: set[int] = set()
    for index, raw_artifact in enumerate(artifacts):
        if not isinstance(raw_artifact, dict):
            raise ValidationError(f"GitHub artifact {index} must be an object")
        for field in (
            "id",
            "name",
            "size_in_bytes",
            "digest",
            "expired",
            "workflow_run",
        ):
            if field not in raw_artifact:
                raise ValidationError(f"GitHub artifact {index} is missing field {field!r}")
        name = raw_artifact["name"]
        if not isinstance(name, str):
            raise ValidationError(f"GitHub artifact {index} name must be text")
        if name in actual_names:
            raise ValidationError(f"GitHub artifacts snapshot contains a duplicate name: {name!r}")
        actual_names.add(name)
        artifact_id = _integer(
            raw_artifact["id"],
            label=f"GitHub artifact {name!r} id",
            minimum=1,
        )
        if artifact_id in actual_ids:
            raise ValidationError(
                f"GitHub artifacts snapshot contains a duplicate id: {artifact_id}"
            )
        actual_ids.add(artifact_id)
        expected = expected_artifacts.get(name)
        if expected is None:
            raise ValidationError(f"GitHub artifacts snapshot has an unexpected artifact: {name!r}")
        for field in ("id", "size_in_bytes", "digest"):
            _expected(
                raw_artifact[field],
                expected[field],
                label=f"GitHub artifact {name!r} {field}",
            )
        if raw_artifact["expired"] is not False:
            raise ValidationError(f"GitHub artifact {name!r} is expired or ambiguously marked")

        workflow_run = raw_artifact["workflow_run"]
        if not isinstance(workflow_run, dict):
            raise ValidationError(f"GitHub artifact {name!r} workflow_run must be an object")
        provenance = {
            "id": expected_run["id"],
            "head_branch": expected_run["head_branch"],
            "head_sha": expected_run["head_sha"],
        }
        for field, expected_value in provenance.items():
            if field not in workflow_run:
                raise ValidationError(
                    f"GitHub artifact {name!r} is missing workflow_run field {field!r}"
                )
            _expected(
                workflow_run[field],
                expected_value,
                label=f"GitHub artifact {name!r} workflow_run {field}",
            )

    if actual_names != set(expected_artifacts):
        raise ValidationError(
            "GitHub artifact-name set mismatch: "
            f"missing={sorted(set(expected_artifacts) - actual_names)!r}"
        )


def _inspect_artifact_directory(path: Path, *, artifact_name: str) -> dict[str, Path]:
    """Return every regular file below one artifact, rejecting special entries."""
    files: dict[str, Path] = {}
    entry_count = 0
    pending: list[tuple[Path, tuple[str, ...]]] = [(path, ())]
    while pending:
        directory, relative_parts = pending.pop()
        try:
            entries = sorted(os.scandir(directory), key=lambda entry: entry.name)
        except OSError as error:
            raise ValidationError(
                f"Could not scan artifact directory {directory}: {error}"
            ) from error
        for entry in entries:
            entry_count += 1
            if entry_count > MAX_FILESYSTEM_ENTRIES:
                raise ValidationError(
                    f"artifact {artifact_name!r} exceeds the filesystem-entry budget"
                )
            parts = (*relative_parts, entry.name)
            relative = "/".join(parts)
            _payload_path(relative, label=f"artifact {artifact_name!r} filesystem path")
            try:
                if entry.is_symlink():
                    raise ValidationError(
                        f"artifact {artifact_name!r} contains a symlink: {relative!r}"
                    )
                if entry.is_dir(follow_symlinks=False):
                    pending.append((Path(entry.path), parts))
                elif entry.is_file(follow_symlinks=False):
                    files[relative] = Path(entry.path)
                else:
                    raise ValidationError(
                        f"artifact {artifact_name!r} contains a non-regular entry: {relative!r}"
                    )
            except OSError as error:
                raise ValidationError(
                    f"Could not inspect artifact {artifact_name!r} entry {relative!r}: {error}"
                ) from error
    return files


def _hash_payload_file(path: Path, *, expected_size: int, label: str) -> str:
    """Hash one regular payload through a no-follow descriptor."""
    flags = os.O_RDONLY | getattr(os, "O_BINARY", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        raise ValidationError(f"Could not open {label}: {error}") from error
    digest = hashlib.sha256()
    try:
        with os.fdopen(descriptor, "rb") as stream:
            opened_stat = os.fstat(stream.fileno())
            if not stat.S_ISREG(opened_stat.st_mode):
                raise ValidationError(f"{label} is non-regular")
            if opened_stat.st_size != expected_size:
                raise ValidationError(
                    f"{label} size mismatch: actual={opened_stat.st_size}, expected={expected_size}"
                )
            while chunk := stream.read(1024 * 1024):
                digest.update(chunk)
            finished_stat = os.fstat(stream.fileno())
    except OSError as error:
        raise ValidationError(f"Could not read {label}: {error}") from error
    if (
        opened_stat.st_size != finished_stat.st_size
        or opened_stat.st_mtime_ns != finished_stat.st_mtime_ns
    ):
        raise ValidationError(f"{label} changed while it was hashed")
    return digest.hexdigest()


def validate_artifact_root(
    root: Path,
    *,
    expected_artifacts: Mapping[str, Mapping[str, Any]],
) -> int:
    """Validate the exact unmerged directory and payload inventory."""
    try:
        root_stat = root.lstat()
    except OSError as error:
        raise ValidationError(f"Could not inspect artifact root {root}: {error}") from error
    if stat.S_ISLNK(root_stat.st_mode) or not stat.S_ISDIR(root_stat.st_mode):
        raise ValidationError(f"artifact root is linked or non-directory: {root}")
    try:
        entries = sorted(os.scandir(root), key=lambda entry: entry.name)
    except OSError as error:
        raise ValidationError(f"Could not scan artifact root {root}: {error}") from error

    actual_directories: dict[str, Path] = {}
    for entry in entries:
        try:
            if entry.is_symlink() or not entry.is_dir(follow_symlinks=False):
                raise ValidationError(
                    f"artifact root contains a linked or non-directory entry: {entry.name!r}"
                )
        except OSError as error:
            raise ValidationError(
                f"Could not inspect artifact-root entry {entry.name!r}: {error}"
            ) from error
        actual_directories[entry.name] = Path(entry.path)
    expected_names = set(expected_artifacts)
    if set(actual_directories) != expected_names:
        raise ValidationError(
            "artifact-root directory set mismatch: "
            f"missing={sorted(expected_names - set(actual_directories))!r}, "
            f"unexpected={sorted(set(actual_directories) - expected_names)!r}"
        )

    declared_file_count = 0
    for artifact_name in sorted(expected_artifacts):
        artifact = expected_artifacts[artifact_name]
        if "files" not in artifact:
            raise ValidationError(f"artifact {artifact_name!r} has no manifest file inventory")
        expected_files = {file["path"]: file for file in artifact["files"]}
        actual_files = _inspect_artifact_directory(
            actual_directories[artifact_name],
            artifact_name=artifact_name,
        )
        if set(actual_files) != set(expected_files):
            raise ValidationError(
                f"artifact {artifact_name!r} file set mismatch: "
                f"missing={sorted(set(expected_files) - set(actual_files))!r}, "
                f"unexpected={sorted(set(actual_files) - set(expected_files))!r}"
            )
        for relative_path in sorted(expected_files):
            expected_file = expected_files[relative_path]
            actual_digest = _hash_payload_file(
                actual_files[relative_path],
                expected_size=expected_file["size_in_bytes"],
                label=f"artifact {artifact_name!r} file {relative_path!r}",
            )
            if actual_digest != expected_file["sha256"]:
                raise ValidationError(
                    f"artifact {artifact_name!r} file {relative_path!r} SHA-256 mismatch: "
                    f"actual={actual_digest!r}, expected={expected_file['sha256']!r}"
                )
        declared_file_count += len(expected_files)

    if declared_file_count != EXPECTED_PAYLOAD_FILE_COUNT:
        raise ValidationError(
            "manifest payload-file count mismatch: "
            f"actual={declared_file_count}, expected={EXPECTED_PAYLOAD_FILE_COUNT}"
        )
    return declared_file_count


def validate_payload_selection(
    root: Path,
    *,
    expected_artifacts: Mapping[str, Mapping[str, Any]],
    artifact_names: Sequence[str],
) -> int:
    """Validate one directory containing an exact selected payload union."""
    try:
        root_stat = root.lstat()
    except OSError as error:
        raise ValidationError(
            f"Could not inspect selected artifact root {root}: {error}"
        ) from error
    if stat.S_ISLNK(root_stat.st_mode) or not stat.S_ISDIR(root_stat.st_mode):
        raise ValidationError(f"selected artifact root is linked or non-directory: {root}")

    names = tuple(artifact_names)
    if not names:
        raise ValidationError("selected artifact list must not be empty")
    if len(set(names)) != len(names):
        raise ValidationError("selected artifact list contains a duplicate name")

    expected_files: dict[str, Mapping[str, Any]] = {}
    for name in names:
        artifact = expected_artifacts.get(name)
        if artifact is None:
            raise ValidationError(f"selected artifact is not in the recovery manifest: {name!r}")
        if "files" not in artifact:
            raise ValidationError(f"selected artifact {name!r} has no manifest file inventory")
        for file in artifact["files"]:
            relative_path = file["path"]
            if relative_path in expected_files:
                raise ValidationError(
                    f"selected artifacts contain a colliding payload path: {relative_path!r}"
                )
            expected_files[relative_path] = file

    selection_label = ", ".join(sorted(names))
    actual_files = _inspect_artifact_directory(
        root,
        artifact_name=f"selected artifacts [{selection_label}]",
    )
    if set(actual_files) != set(expected_files):
        raise ValidationError(
            "selected artifact file set mismatch: "
            f"missing={sorted(set(expected_files) - set(actual_files))!r}, "
            f"unexpected={sorted(set(actual_files) - set(expected_files))!r}"
        )
    for relative_path in sorted(expected_files):
        expected_file = expected_files[relative_path]
        actual_digest = _hash_payload_file(
            actual_files[relative_path],
            expected_size=expected_file["size_in_bytes"],
            label=f"selected artifact file {relative_path!r}",
        )
        if actual_digest != expected_file["sha256"]:
            raise ValidationError(
                f"selected artifact file {relative_path!r} SHA-256 mismatch: "
                f"actual={actual_digest!r}, expected={expected_file['sha256']!r}"
            )
    return len(expected_files)


def validate_recovery(
    manifest_value: Any,
    run_value: Any,
    jobs_value: Any,
    artifacts_value: Any,
    *,
    artifact_root: Path | None = None,
) -> dict[str, Any]:
    """Validate all recovery evidence and return a deterministic summary."""
    manifest = validate_manifest(manifest_value)
    validate_run_snapshot(run_value, expected_run=manifest["run"])
    job_counts = validate_jobs_snapshot(
        jobs_value,
        expected_run=manifest["run"],
        expected_jobs=manifest["jobs"],
        known_failure=manifest["known_failure"],
        skipped_mutators=manifest["skipped_mutators"],
    )
    validate_artifacts_snapshot(
        artifacts_value,
        expected_run=manifest["run"],
        expected_artifacts=manifest["artifacts"],
    )
    verified_payload_file_count = 0
    if artifact_root is not None:
        verified_payload_file_count = validate_artifact_root(
            artifact_root,
            expected_artifacts=manifest["artifacts"],
        )

    return {
        "artifact_count": len(manifest["artifacts"]),
        "artifact_digests": {
            name: artifact["digest"] for name, artifact in sorted(manifest["artifacts"].items())
        },
        "declared_payload_file_count": sum(
            len(artifact.get("files", ())) for artifact in manifest["artifacts"].values()
        ),
        "head_sha": manifest["run"]["head_sha"],
        "job_conclusion_counts": dict(sorted(job_counts.items())),
        "known_failure": dict(manifest["known_failure"]),
        "manifest_sha256": canonical_json_sha256(manifest_value),
        "payloads_validated": artifact_root is not None,
        "run_attempt": manifest["run"]["run_attempt"],
        "run_id": manifest["run"]["id"],
        "schema_version": SCHEMA_VERSION,
        "verified_payload_file_count": verified_payload_file_count,
        "workflow": dict(manifest["run"]["workflow"]),
    }


def build_parser() -> argparse.ArgumentParser:
    """Build the command-line parser."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--expected-manifest-sha256", required=True)
    parser.add_argument("--run", type=Path, required=True)
    parser.add_argument("--jobs", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    parser.add_argument(
        "--artifact-root",
        type=Path,
        required=True,
        help="unmerged output directory created by `gh run download --dir`",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Validate evidence files and print one canonical JSON summary."""
    arguments = build_parser().parse_args(argv)
    manifest_value = read_json(arguments.manifest, label="manifest")
    validate_manifest_digest(
        manifest_value,
        expected_sha256=arguments.expected_manifest_sha256,
    )
    run_value = read_json(arguments.run, label="run")
    jobs_value = read_json(arguments.jobs, label="jobs")
    artifacts_value = read_json(arguments.artifacts, label="artifacts")
    summary = validate_recovery(
        manifest_value,
        run_value,
        jobs_value,
        artifacts_value,
        artifact_root=arguments.artifact_root,
    )
    print(json.dumps(summary, ensure_ascii=False, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValidationError as error:
        print(f"release recovery validation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
