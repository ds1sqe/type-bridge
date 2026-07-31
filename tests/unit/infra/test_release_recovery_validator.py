"""Hostile coverage for the fixed 2.0.0 release-recovery evidence ledger."""

from __future__ import annotations

import copy
import hashlib
import importlib.util
import json
import subprocess
import sys
from collections.abc import Callable, Sequence
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
VALIDATOR_PATH = ROOT / "scripts/ci/validate_release_recovery.py"
PAYLOAD_VALIDATOR_PATH = ROOT / "scripts/ci/validate_release_recovery_payloads.py"
RECOVERY_MANIFEST = ROOT / ".github/release/v2.0.0-recovery.json"
RUN_ID = 30_612_912_483
WORKFLOW_ID = 229_807_619
HEAD_SHA = "aacf4d16486a3a3bae47c3b10c1d526c587dd7a7"


def load_module(name: str, path: Path) -> ModuleType:
    """Load the standalone validator without making scripts a package."""
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


validator = load_module("validate_release_recovery", VALIDATOR_PATH)


def _workflow_run() -> dict[str, Any]:
    return {
        "id": RUN_ID,
        "head_branch": validator.RELEASE_TAG,
        "head_sha": HEAD_SHA,
    }


def _run_manifest() -> dict[str, Any]:
    return {
        "id": RUN_ID,
        "event": "push",
        "head_branch": validator.RELEASE_TAG,
        "head_sha": HEAD_SHA,
        "run_attempt": 1,
        "status": "completed",
        "conclusion": "failure",
        "workflow": {
            "id": WORKFLOW_ID,
            "name": validator.RELEASE_WORKFLOW_NAME,
            "path": validator.RELEASE_WORKFLOW_PATH,
        },
    }


def _run_snapshot() -> dict[str, Any]:
    run = _run_manifest()
    workflow = run.pop("workflow")
    return {
        **run,
        "workflow_id": workflow["id"],
        "name": workflow["name"],
        "path": workflow["path"],
        "html_url": f"https://github.example/actions/runs/{RUN_ID}",
    }


def _jobs_manifest() -> dict[str, str]:
    jobs = {f"Accepted recovery job {index:02d}": "success" for index in range(34)}
    jobs[validator.KNOWN_FAILURE_JOB] = "failure"
    jobs.update({name: "skipped" for name in sorted(validator.SKIPPED_MUTATORS)})
    assert len(jobs) == validator.EXPECTED_JOB_COUNT
    return jobs


def _job_steps(name: str, conclusion: str) -> list[dict[str, Any]]:
    if conclusion == "skipped":
        return []
    if name == validator.KNOWN_FAILURE_JOB:
        return [
            {
                "name": "Set up job",
                "status": "completed",
                "conclusion": "success",
            },
            {
                "name": validator.KNOWN_FAILURE_STEP,
                "status": "completed",
                "conclusion": "failure",
            },
            {
                "name": "Complete job",
                "status": "completed",
                "conclusion": "success",
            },
        ]
    return [
        {
            "name": "Complete job",
            "status": "completed",
            "conclusion": "success",
        }
    ]


def _jobs_snapshot(expected_jobs: dict[str, str]) -> dict[str, Any]:
    jobs = []
    for index, (name, conclusion) in enumerate(expected_jobs.items()):
        jobs.append(
            {
                "id": 90_000_000_000 + index,
                "run_id": RUN_ID,
                "run_attempt": 1,
                "workflow_name": validator.RELEASE_WORKFLOW_NAME,
                "head_branch": validator.RELEASE_TAG,
                "head_sha": HEAD_SHA,
                "name": name,
                "status": "completed",
                "conclusion": conclusion,
                "steps": _job_steps(name, conclusion),
            }
        )
    return {"total_count": len(jobs), "jobs": jobs}


def _artifact_digest(name: str) -> str:
    return f"sha256:{hashlib.sha256(('archive:' + name).encode()).hexdigest()}"


def _artifact_payloads(name: str, count: int) -> list[tuple[str, bytes]]:
    payloads = []
    for index in range(count):
        relative = f"nested/payload-{index}.bin" if index == 0 else f"payload-{index}.bin"
        payloads.append((relative, f"{name}:{index}:accepted-bytes".encode()))
    return payloads


def _artifacts_manifest(
    *,
    include_files: bool = False,
    artifact_root: Path | None = None,
) -> list[dict[str, Any]]:
    artifacts = []
    file_count = 0
    for index in range(validator.EXPECTED_ARTIFACT_COUNT):
        name = f"release-artifact-{index:02d}"
        artifact: dict[str, Any] = {
            "id": 8_780_000_000 + index,
            "name": name,
            "size_in_bytes": 1_000 + index,
            "digest": _artifact_digest(name),
        }
        if include_files:
            payload_count = 2 if index < 17 else 1
            payloads = _artifact_payloads(name, payload_count)
            artifact["files"] = [
                {
                    "path": path,
                    "size_in_bytes": len(body),
                    "sha256": hashlib.sha256(body).hexdigest(),
                }
                for path, body in payloads
            ]
            file_count += len(payloads)
            if artifact_root is not None:
                directory = artifact_root / name
                directory.mkdir(parents=True)
                for path, body in payloads:
                    destination = directory / path
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    destination.write_bytes(body)
        artifacts.append(artifact)
    if include_files:
        assert file_count == validator.EXPECTED_PAYLOAD_FILE_COUNT
    return artifacts


def _artifacts_snapshot(expected_artifacts: list[dict[str, Any]]) -> dict[str, Any]:
    artifacts = [
        {
            "id": artifact["id"],
            "name": artifact["name"],
            "size_in_bytes": artifact["size_in_bytes"],
            "digest": artifact["digest"],
            "expired": False,
            "workflow_run": _workflow_run(),
            "archive_download_url": (
                f"https://api.github.example/actions/artifacts/{artifact['id']}/zip"
            ),
        }
        for artifact in expected_artifacts
    ]
    return {"total_count": len(artifacts), "artifacts": artifacts}


def evidence(
    *,
    include_files: bool = False,
    artifact_root: Path | None = None,
) -> tuple[dict[str, Any], dict[str, Any], dict[str, Any], dict[str, Any]]:
    """Return one complete internally consistent recovery evidence set."""
    jobs = _jobs_manifest()
    artifacts = _artifacts_manifest(
        include_files=include_files,
        artifact_root=artifact_root,
    )
    manifest = {
        "schema_version": validator.SCHEMA_VERSION,
        "run": _run_manifest(),
        "jobs": jobs,
        "known_failure": {
            "job": validator.KNOWN_FAILURE_JOB,
            "step": validator.KNOWN_FAILURE_STEP,
        },
        "skipped_mutators": sorted(validator.SKIPPED_MUTATORS),
        "artifacts": artifacts,
    }
    return (
        manifest,
        _run_snapshot(),
        _jobs_snapshot(jobs),
        _artifacts_snapshot(artifacts),
    )


def validate(
    values: Sequence[dict[str, Any]],
    *,
    artifact_root: Path | None = None,
) -> dict[str, Any]:
    assert len(values) == 4
    return validator.validate_recovery(
        values[0],
        values[1],
        values[2],
        values[3],
        artifact_root=artifact_root,
    )


def test_complete_api_ledger_returns_a_deterministic_summary() -> None:
    values = evidence()

    first = validate(values)
    second = validate(copy.deepcopy(values))

    assert first == second
    assert first["run_id"] == RUN_ID
    assert first["head_sha"] == HEAD_SHA
    assert first["job_conclusion_counts"] == {
        "failure": 1,
        "skipped": 5,
        "success": 34,
    }
    assert first["artifact_count"] == 19
    assert first["payloads_validated"] is False
    assert first["verified_payload_file_count"] == 0


def test_cli_prints_one_canonical_json_object(
    tmp_path: Path, capsys: pytest.CaptureFixture
) -> None:
    artifact_root = tmp_path / "payloads"
    artifact_root.mkdir()
    values = evidence(include_files=True, artifact_root=artifact_root)
    expected_manifest_sha256 = validator.canonical_json_sha256(values[0])
    paths = []
    for label, value in zip(("manifest", "run", "jobs", "artifacts"), values, strict=True):
        path = tmp_path / f"{label}.json"
        path.write_text(json.dumps(value), encoding="utf-8")
        paths.append(path)

    result = validator.main(
        [
            "--manifest",
            str(paths[0]),
            "--expected-manifest-sha256",
            expected_manifest_sha256,
            "--run",
            str(paths[1]),
            "--jobs",
            str(paths[2]),
            "--artifacts",
            str(paths[3]),
            "--artifact-root",
            str(artifact_root),
        ]
    )

    assert result == 0
    output = capsys.readouterr().out
    assert output.count("\n") == 1
    assert output == (json.dumps(json.loads(output), sort_keys=True, separators=(",", ":")) + "\n")
    assert json.loads(output)["verified_payload_file_count"] == 36


def test_cli_requires_payload_hash_validation(tmp_path: Path) -> None:
    values = evidence()
    expected_manifest_sha256 = validator.canonical_json_sha256(values[0])
    paths = []
    for label, value in zip(("manifest", "run", "jobs", "artifacts"), values, strict=True):
        path = tmp_path / f"{label}.json"
        path.write_text(json.dumps(value), encoding="utf-8")
        paths.append(path)

    with pytest.raises(SystemExit) as error:
        validator.main(
            [
                "--manifest",
                str(paths[0]),
                "--expected-manifest-sha256",
                expected_manifest_sha256,
                "--run",
                str(paths[1]),
                "--jobs",
                str(paths[2]),
                "--artifacts",
                str(paths[3]),
            ]
        )

    assert error.value.code == 2


def test_duplicate_json_keys_fail_closed(tmp_path: Path) -> None:
    snapshot = tmp_path / "run.json"
    snapshot.write_text('{"id":1,"id":2}', encoding="utf-8")

    with pytest.raises(validator.ValidationError, match="duplicate key"):
        validator.read_json(snapshot, label="run")


def test_json_byte_budget_and_final_symlink_are_rejected(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    oversized = tmp_path / "oversized.json"
    oversized.write_bytes(b"12345")
    monkeypatch.setitem(validator.MAX_JSON_BYTES, "run", 4)
    with pytest.raises(validator.ValidationError, match="byte budget"):
        validator.read_json(oversized, label="run")

    target = tmp_path / "target.json"
    target.write_text("{}", encoding="utf-8")
    linked = tmp_path / "linked.json"
    linked.symlink_to(target)
    with pytest.raises(validator.ValidationError, match="linked or non-regular"):
        validator.read_json(linked, label="run")


@pytest.mark.parametrize("body", ("NaN", "Infinity", "1.5"))
def test_non_integer_json_numbers_are_rejected(tmp_path: Path, body: str) -> None:
    snapshot = tmp_path / "number.json"
    snapshot.write_text(body, encoding="utf-8")

    with pytest.raises(validator.ValidationError, match="numeric|floating"):
        validator.read_json(snapshot, label="run")


@pytest.mark.parametrize(
    ("field", "replacement"),
    (
        ("id", RUN_ID + 1),
        ("event", "workflow_dispatch"),
        ("head_branch", "master"),
        ("head_sha", "f" * 40),
        ("run_attempt", 2),
        ("status", "in_progress"),
        ("conclusion", "success"),
        ("workflow_id", WORKFLOW_ID + 1),
        ("name", "Another workflow"),
        ("path", ".github/workflows/other.yml"),
    ),
)
def test_every_run_identity_field_is_exact(field: str, replacement: Any) -> None:
    values = list(evidence())
    values[1][field] = replacement

    with pytest.raises(validator.ValidationError, match=field):
        validate(tuple(values))


@pytest.mark.parametrize(
    ("mutation", "message"),
    (
        (
            lambda manifest: manifest.update({"unexpected": True}),
            "unexpected",
        ),
        (
            lambda manifest: manifest["run"].update({"event": "workflow_dispatch"}),
            "manifest run event",
        ),
        (
            lambda manifest: manifest["known_failure"].update({"step": "A different step"}),
            "known failure step",
        ),
        (
            lambda manifest: manifest["skipped_mutators"].pop(),
            "skipped mutators mismatch",
        ),
        (
            lambda manifest: manifest["jobs"].pop("Accepted recovery job 00"),
            "jobs count mismatch",
        ),
        (
            lambda manifest: manifest["artifacts"].pop(),
            "artifact count mismatch",
        ),
    ),
)
def test_manifest_cannot_relax_the_fixed_recovery_contract(
    mutation: Callable[[dict[str, Any]], Any],
    message: str,
) -> None:
    values = list(evidence())
    mutation(values[0])

    with pytest.raises(validator.ValidationError, match=message):
        validate(tuple(values))


def test_coherent_run_identity_substitution_is_rejected() -> None:
    values = list(evidence())
    replacement_id = RUN_ID + 1
    replacement_sha = "f" * 40
    replacement_workflow_id = WORKFLOW_ID + 1
    values[0]["run"]["id"] = replacement_id
    values[0]["run"]["head_sha"] = replacement_sha
    values[0]["run"]["workflow"]["id"] = replacement_workflow_id
    values[1]["id"] = replacement_id
    values[1]["head_sha"] = replacement_sha
    values[1]["workflow_id"] = replacement_workflow_id
    for job in values[2]["jobs"]:
        job["run_id"] = replacement_id
        job["head_sha"] = replacement_sha
    for artifact in values[3]["artifacts"]:
        artifact["workflow_run"]["id"] = replacement_id
        artifact["workflow_run"]["head_sha"] = replacement_sha

    with pytest.raises(validator.ValidationError, match="manifest run id mismatch"):
        validate(tuple(values))


def test_committed_recovery_manifest_is_valid_and_digest_pinned() -> None:
    manifest_value = validator.read_json(RECOVERY_MANIFEST, label="manifest")

    manifest = validator.validate_manifest(manifest_value)
    canonical = json.dumps(
        manifest_value,
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")

    assert manifest["run"]["id"] == validator.RELEASE_RUN_ID
    assert manifest["run"]["head_sha"] == validator.RELEASE_HEAD_SHA
    assert len(manifest["artifacts"]) == validator.EXPECTED_ARTIFACT_COUNT
    assert hashlib.sha256(canonical).hexdigest() == (
        "f8d5b2d04ad01a45694aecdd171846443bfd511a9363ab771e5f182c6bd17d2d"
    )
    assert (
        validator.validate_manifest_digest(
            manifest_value,
            expected_sha256="f8d5b2d04ad01a45694aecdd171846443bfd511a9363ab771e5f182c6bd17d2d",
        )
        == "f8d5b2d04ad01a45694aecdd171846443bfd511a9363ab771e5f182c6bd17d2d"
    )

    mutated = copy.deepcopy(manifest_value)
    mutated["artifacts"][0]["files"][0]["sha256"] = "f" * 64
    with pytest.raises(validator.ValidationError, match="manifest SHA-256 mismatch"):
        validator.validate_manifest_digest(
            mutated,
            expected_sha256="f8d5b2d04ad01a45694aecdd171846443bfd511a9363ab771e5f182c6bd17d2d",
        )


def _job(snapshot: dict[str, Any], name: str) -> dict[str, Any]:
    return next(job for job in snapshot["jobs"] if job["name"] == name)


@pytest.mark.parametrize(
    ("mutation", "message"),
    (
        (
            lambda jobs: jobs.update({"total_count": jobs["total_count"] - 1}),
            "incomplete",
        ),
        (
            lambda jobs: jobs["jobs"][0].update({"name": "unexpected replacement"}),
            "name/conclusion set mismatch",
        ),
        (
            lambda jobs: jobs["jobs"][0].update({"conclusion": "failure"}),
            "name/conclusion set mismatch",
        ),
        (
            lambda jobs: jobs["jobs"][0].update({"run_id": RUN_ID + 1}),
            "run_id",
        ),
        (
            lambda jobs: jobs["jobs"][0].update({"status": "in_progress"}),
            "status mismatch",
        ),
        (
            lambda jobs: jobs["jobs"][1].update({"id": jobs["jobs"][0]["id"]}),
            "duplicate job id",
        ),
    ),
)
def test_job_page_tampering_is_rejected(
    mutation: Callable[[dict[str, Any]], Any],
    message: str,
) -> None:
    values = list(evidence())
    mutation(values[2])

    with pytest.raises(validator.ValidationError, match=message):
        validate(tuple(values))


def test_only_the_named_preflight_step_may_fail() -> None:
    values = list(evidence())
    failure_job = _job(values[2], validator.KNOWN_FAILURE_JOB)
    failure_step = next(step for step in failure_job["steps"] if step["conclusion"] == "failure")
    failure_step["name"] = "An unapproved failure"

    with pytest.raises(validator.ValidationError, match="failed-step set mismatch"):
        validate(tuple(values))

    values = list(evidence())
    _job(values[2], "Accepted recovery job 00")["steps"][0]["conclusion"] = "failure"
    with pytest.raises(validator.ValidationError, match="failed-step set mismatch"):
        validate(tuple(values))


def test_skipped_mutators_must_have_no_executed_steps() -> None:
    values = list(evidence())
    skipped = _job(values[2], sorted(validator.SKIPPED_MUTATORS)[0])
    skipped["steps"].append(
        {"name": "Mutated registry", "status": "completed", "conclusion": "success"}
    )

    with pytest.raises(validator.ValidationError, match="zero steps"):
        validate(tuple(values))


@pytest.mark.parametrize(
    ("field", "replacement", "message"),
    (
        ("id", 123, "id mismatch"),
        ("size_in_bytes", 123, "size_in_bytes mismatch"),
        ("digest", f"sha256:{'f' * 64}", "digest mismatch"),
        ("expired", True, "expired"),
    ),
)
def test_artifact_identity_and_freshness_are_exact(
    field: str,
    replacement: Any,
    message: str,
) -> None:
    values = list(evidence())
    values[3]["artifacts"][0][field] = replacement

    with pytest.raises(validator.ValidationError, match=message):
        validate(tuple(values))


def test_artifact_set_ids_and_source_run_provenance_are_exact() -> None:
    values = list(evidence())
    values[3]["artifacts"][0]["name"] = "unexpected"
    with pytest.raises(validator.ValidationError, match="unexpected artifact"):
        validate(tuple(values))

    values = list(evidence())
    values[3]["artifacts"][1]["id"] = values[3]["artifacts"][0]["id"]
    with pytest.raises(validator.ValidationError, match="duplicate id"):
        validate(tuple(values))

    values = list(evidence())
    values[3]["artifacts"][0]["workflow_run"]["head_sha"] = "f" * 40
    with pytest.raises(validator.ValidationError, match="workflow_run head_sha"):
        validate(tuple(values))


def test_complete_unmerged_payload_inventory_is_hashed(tmp_path: Path) -> None:
    artifact_root = tmp_path / "artifacts"
    artifact_root.mkdir()
    values = evidence(include_files=True, artifact_root=artifact_root)

    summary = validate(values, artifact_root=artifact_root)

    assert summary["payloads_validated"] is True
    assert summary["declared_payload_file_count"] == 36
    assert summary["verified_payload_file_count"] == 36


def test_artifact_root_requires_exact_top_level_directories(tmp_path: Path) -> None:
    artifact_root = tmp_path / "artifacts"
    artifact_root.mkdir()
    values = evidence(include_files=True, artifact_root=artifact_root)
    (artifact_root / "unexpected-artifact").mkdir()

    with pytest.raises(validator.ValidationError, match="directory set mismatch"):
        validate(values, artifact_root=artifact_root)


def test_artifact_root_rejects_extra_missing_and_linked_payloads(tmp_path: Path) -> None:
    artifact_root = tmp_path / "artifacts"
    artifact_root.mkdir()
    values = evidence(include_files=True, artifact_root=artifact_root)
    first_artifact = values[0]["artifacts"][0]["name"]
    first_directory = artifact_root / first_artifact
    extra = first_directory / "extra.bin"
    extra.write_bytes(b"not declared")
    with pytest.raises(validator.ValidationError, match="file set mismatch"):
        validate(values, artifact_root=artifact_root)

    extra.unlink()
    missing = first_directory / values[0]["artifacts"][0]["files"][0]["path"]
    missing.unlink()
    with pytest.raises(validator.ValidationError, match="file set mismatch"):
        validate(values, artifact_root=artifact_root)

    missing.parent.mkdir(parents=True, exist_ok=True)
    missing.symlink_to(first_directory / values[0]["artifacts"][0]["files"][1]["path"])
    with pytest.raises(validator.ValidationError, match="symlink"):
        validate(values, artifact_root=artifact_root)


def test_payload_size_digest_and_manifest_path_are_fail_closed(tmp_path: Path) -> None:
    artifact_root = tmp_path / "artifacts"
    artifact_root.mkdir()
    values = evidence(include_files=True, artifact_root=artifact_root)
    artifact = values[0]["artifacts"][0]
    payload = artifact["files"][0]
    path = artifact_root / artifact["name"] / payload["path"]
    path.write_bytes(b"x" * payload["size_in_bytes"])
    with pytest.raises(validator.ValidationError, match="SHA-256 mismatch"):
        validate(values, artifact_root=artifact_root)

    values = list(evidence())
    values[0]["artifacts"][0]["files"] = [
        {
            "path": "../escape.bin",
            "size_in_bytes": 1,
            "sha256": "0" * 64,
        }
    ]
    with pytest.raises(validator.ValidationError, match="normalized bounded relative path"):
        validate(tuple(values))


def test_artifact_root_requires_all_file_inventories_and_rejects_root_symlink(
    tmp_path: Path,
) -> None:
    artifact_root = tmp_path / "artifacts"
    artifact_root.mkdir()
    values = evidence(include_files=True, artifact_root=artifact_root)
    del values[0]["artifacts"][0]["files"]
    with pytest.raises(validator.ValidationError, match="no manifest file inventory"):
        validate(values, artifact_root=artifact_root)

    real_root = tmp_path / "real"
    real_root.mkdir()
    linked_root = tmp_path / "linked"
    linked_root.symlink_to(real_root, target_is_directory=True)
    with pytest.raises(validator.ValidationError, match="linked or non-directory"):
        validator.validate_artifact_root(
            linked_root,
            expected_artifacts=validator.validate_manifest(evidence()[0])["artifacts"],
        )


def test_selected_payload_directory_is_exact_and_hash_bound(tmp_path: Path) -> None:
    artifact_root = tmp_path / "artifacts"
    artifact_root.mkdir()
    values = evidence(include_files=True, artifact_root=artifact_root)
    manifest = validator.validate_manifest(values[0])
    selected = values[0]["artifacts"][0]["name"]
    selected_root = artifact_root / selected

    assert (
        validator.validate_payload_selection(
            selected_root,
            expected_artifacts=manifest["artifacts"],
            artifact_names=[selected],
        )
        == 2
    )

    payload = values[0]["artifacts"][0]["files"][0]
    path = selected_root / payload["path"]
    path.write_bytes(b"x" * payload["size_in_bytes"])
    with pytest.raises(validator.ValidationError, match="SHA-256 mismatch"):
        validator.validate_payload_selection(
            selected_root,
            expected_artifacts=manifest["artifacts"],
            artifact_names=[selected],
        )


def test_selected_payload_directory_rejects_unknown_duplicate_and_extra(
    tmp_path: Path,
) -> None:
    artifact_root = tmp_path / "artifacts"
    artifact_root.mkdir()
    values = evidence(include_files=True, artifact_root=artifact_root)
    manifest = validator.validate_manifest(values[0])
    selected = values[0]["artifacts"][0]["name"]
    selected_root = artifact_root / selected

    with pytest.raises(validator.ValidationError, match="not in the recovery manifest"):
        validator.validate_payload_selection(
            selected_root,
            expected_artifacts=manifest["artifacts"],
            artifact_names=["unknown"],
        )
    with pytest.raises(validator.ValidationError, match="duplicate name"):
        validator.validate_payload_selection(
            selected_root,
            expected_artifacts=manifest["artifacts"],
            artifact_names=[selected, selected],
        )

    (selected_root / "extra.bin").write_bytes(b"unexpected")
    with pytest.raises(validator.ValidationError, match="file set mismatch"):
        validator.validate_payload_selection(
            selected_root,
            expected_artifacts=manifest["artifacts"],
            artifact_names=[selected],
        )


def test_selected_payload_cli_imports_validator_and_fails_on_substitution(
    tmp_path: Path,
) -> None:
    artifact_root = tmp_path / "artifacts"
    artifact_root.mkdir()
    values = evidence(include_files=True, artifact_root=artifact_root)
    manifest_path = tmp_path / "manifest.json"
    manifest_path.write_text(json.dumps(values[0]), encoding="utf-8")
    selected = values[0]["artifacts"][0]["name"]
    selected_root = artifact_root / selected
    command = [
        sys.executable,
        str(PAYLOAD_VALIDATOR_PATH),
        "--manifest",
        str(manifest_path),
        "--expected-manifest-sha256",
        validator.canonical_json_sha256(values[0]),
        "--artifact-root",
        str(selected_root),
        "--artifact",
        selected,
    ]

    accepted = subprocess.run(command, check=False, capture_output=True, text=True)
    assert accepted.returncode == 0, accepted.stderr
    assert json.loads(accepted.stdout)["verified_payload_file_count"] == 2

    payload = values[0]["artifacts"][0]["files"][0]
    (selected_root / payload["path"]).write_bytes(b"x" * payload["size_in_bytes"])
    rejected = subprocess.run(command, check=False, capture_output=True, text=True)
    assert rejected.returncode == 1
    assert "SHA-256 mismatch" in rejected.stderr
