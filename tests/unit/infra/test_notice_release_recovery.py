"""Fail-closed coverage for the separately frozen 2.0.2 publisher recovery."""

import copy
import hashlib
import importlib.util
import json
import stat
import subprocess
import sys
import zipfile
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]


def load(name: str) -> ModuleType:
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts/ci" / f"{name}.py")
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


load("validate_release_recovery")
load("verify_pypi_release_hashes")
recovery = load("recover_notice_release")
MANIFEST = ROOT / ".github/release/v2.0.2-recovery.json"


def snapshots() -> tuple[dict[str, Any], dict[str, Any], dict[str, Any], dict[str, Any]]:
    manifest = recovery.load_manifest(MANIFEST)
    run = copy.deepcopy(manifest["run"])
    workflow = run.pop("workflow")
    run.update(
        workflow_id=workflow["id"],
        name=workflow["name"],
        path=workflow["path"],
        repository={"full_name": recovery.REPOSITORY},
        head_repository={"full_name": recovery.REPOSITORY},
    )
    jobs = []
    for index, (name, conclusion) in enumerate(manifest["jobs"].items(), 1):
        jobs.append(
            {
                "id": index,
                "name": name,
                "conclusion": conclusion,
                "status": "completed",
                "run_id": run["id"],
                "run_attempt": 1,
                "workflow_name": "Release",
                "head_branch": "v2.0.2",
                "head_sha": recovery.SOURCE,
                "steps": copy.deepcopy(manifest["job_steps"][name]),
            }
        )
    artifacts = copy.deepcopy(manifest["artifacts"])
    for artifact in artifacts:
        artifact.update(
            expired=False,
            workflow_run={
                "id": run["id"],
                "head_branch": "v2.0.2",
                "head_sha": recovery.SOURCE,
                "repository_id": 1085407082,
                "head_repository_id": 1085407082,
            },
        )
    return (
        manifest,
        run,
        {"total_count": len(jobs), "jobs": jobs},
        {"total_count": len(artifacts), "artifacts": artifacts},
    )


def test_exact_original_partial_run_is_accepted() -> None:
    recovery.validate_source(*snapshots())


@pytest.mark.parametrize(
    "field,value",
    [
        ("head_sha", "a" * 40),
        ("run_attempt", 2),
        ("run_attempt", True),
        ("event", "workflow_dispatch"),
        ("conclusion", "success"),
        ("id", 34073617384),
        ("repository", {"full_name": "attacker/type-bridge"}),
        ("head_repository", {"full_name": "attacker/type-bridge"}),
    ],
)
def test_wrong_source_fails(field: str, value: Any) -> None:
    manifest, run, jobs, artifacts = snapshots()
    run[field] = value
    with pytest.raises(recovery.ValidationError):
        recovery.validate_source(manifest, run, jobs, artifacts)


@pytest.mark.parametrize("mutation", ["missing", "duplicate", "skip", "omit-step", "failure"])
def test_changed_jobs_or_steps_fail(mutation: str) -> None:
    manifest, run, jobs, artifacts = snapshots()
    if mutation == "missing":
        jobs["jobs"].pop()
    elif mutation == "duplicate":
        jobs["jobs"][1] = jobs["jobs"][0]
    elif mutation == "skip":
        jobs["jobs"][0]["steps"][0]["conclusion"] = "skipped"
    elif mutation == "omit-step":
        jobs["jobs"][0]["steps"].pop()
    else:
        jobs["jobs"][0]["steps"][0]["conclusion"] = "failure"
    with pytest.raises(recovery.ValidationError):
        recovery.validate_source(manifest, run, jobs, artifacts)


@pytest.mark.parametrize("mutation", ["missing", "duplicate", "expired", "digest", "origin"])
def test_changed_artifacts_fail(mutation: str) -> None:
    manifest, run, jobs, artifacts = snapshots()
    if mutation == "missing":
        artifacts["artifacts"].pop()
    elif mutation == "duplicate":
        artifacts["artifacts"][1] = artifacts["artifacts"][0]
    elif mutation == "expired":
        artifacts["artifacts"][0]["expired"] = True
    elif mutation == "digest":
        artifacts["artifacts"][0]["digest"] = "sha256:" + "a" * 64
    else:
        artifacts["artifacts"][0]["workflow_run"]["head_repository_id"] = 1
    with pytest.raises(recovery.ValidationError):
        recovery.validate_source(manifest, run, jobs, artifacts)


def test_edited_manifest_cannot_change_release_authority(tmp_path: Path) -> None:
    manifest = json.loads(MANIFEST.read_text())
    manifest["run"]["head_sha"] = "a" * 40
    path = tmp_path / "ledger.json"
    path.write_text(json.dumps(manifest))
    with pytest.raises(recovery.ValidationError, match="SHA-256"):
        recovery.load_manifest(path)


@pytest.mark.parametrize("mutation", [None, "lightweight", "tag-object", "source", "nested"])
def test_tag_cannot_move(mutation: str | None) -> None:
    reference = {"ref": "refs/tags/v2.0.2", "object": {"type": "tag", "sha": recovery.TAG_OBJECT}}
    tag = {
        "sha": recovery.TAG_OBJECT,
        "tag": "v2.0.2",
        "object": {"type": "commit", "sha": recovery.SOURCE},
    }
    if mutation == "lightweight":
        reference["object"]["type"] = "commit"
    elif mutation == "tag-object":
        reference["object"]["sha"] = "a" * 40
    elif mutation == "source":
        tag["object"]["sha"] = "a" * 40
    elif mutation == "nested":
        tag["object"]["type"] = "tag"
    if mutation is None:
        recovery.validate_tag(reference, tag)
    else:
        with pytest.raises(recovery.ValidationError):
            recovery.validate_tag(reference, tag)


def rehearsal() -> tuple[dict[str, Any], dict[str, Any]]:
    run = {
        "id": 1234,
        "event": "workflow_dispatch",
        "head_branch": "release/2.0.2-notice",
        "head_sha": "b" * 40,
        "run_attempt": 1,
        "status": "completed",
        "conclusion": "success",
        "workflow_id": 229807619,
        "path": ".github/workflows/release.yml",
        "name": "Release",
        "repository": {"full_name": recovery.REPOSITORY},
        "head_repository": {"full_name": recovery.REPOSITORY},
    }
    jobs = []
    for index, name in enumerate((recovery.VERIFY_JOB, recovery.PUBLISH_JOB, "Test"), 1):
        jobs.append(
            {
                "id": index,
                "run_id": 1234,
                "run_attempt": 1,
                "head_sha": "b" * 40,
                "head_branch": "release/2.0.2-notice",
                "workflow_name": "Release",
                "status": "completed",
                "name": name,
                "conclusion": "skipped",
                "steps": [],
            }
        )
    jobs[0].update(
        conclusion="success",
        steps=[
            {
                "name": "Validate metadata in the exact publisher image",
                "status": "completed",
                "conclusion": "success",
            }
        ],
    )
    return run, {"total_count": len(jobs), "jobs": jobs}


@pytest.mark.parametrize(
    "mutation", [None, "source", "rerun", "branch", "publisher", "skip", "empty"]
)
def test_only_same_control_nonpublishing_rehearsal_passes(mutation: str | None) -> None:
    run, jobs = rehearsal()
    if mutation == "source":
        run["head_sha"] = "c" * 40
    elif mutation == "rerun":
        run["run_attempt"] = 2
    elif mutation == "branch":
        run["head_branch"] = "master"
    elif mutation == "publisher":
        jobs["jobs"][1].update(conclusion="success", steps=[{}])
    elif mutation == "skip":
        jobs["jobs"][0]["steps"][0]["conclusion"] = "skipped"
    elif mutation == "empty":
        jobs["jobs"][0]["steps"] = []
    if mutation is None:
        recovery.validate_rehearsal(run, jobs, "b" * 40)
    else:
        with pytest.raises(recovery.ValidationError):
            recovery.validate_rehearsal(run, jobs, "b" * 40)


@pytest.mark.parametrize("mutation", [None, "digest", "payload", "extra", "traversal", "symlink"])
def test_archive_and_payload_must_both_match(tmp_path: Path, mutation: str | None) -> None:
    archive = tmp_path / "capture.zip"
    data = b"accepted bytes"
    filename = "accepted.whl"
    with zipfile.ZipFile(archive, "w") as bundle:
        entry = zipfile.ZipInfo("../escape.whl" if mutation == "traversal" else filename)
        if mutation == "symlink":
            entry.external_attr = (stat.S_IFLNK | 0o777) << 16
        bundle.writestr(entry, b"changed bytes" if mutation == "payload" else data)
        if mutation == "extra":
            bundle.writestr("unexpected.whl", data)
    artifact = {
        "name": "python-dist",
        "size_in_bytes": archive.stat().st_size,
        "digest": "sha256:" + recovery.sha256(archive),
        "files": [
            {
                "path": filename,
                "size_in_bytes": len(data),
                "sha256": hashlib.sha256(data).hexdigest(),
            }
        ],
    }
    if mutation == "digest":
        artifact["digest"] = "sha256:" + "a" * 64
    if mutation is None:
        recovery.extract_archive(archive, artifact, tmp_path / "dist")
        assert (tmp_path / "dist" / filename).read_bytes() == data
    else:
        with pytest.raises(recovery.ValidationError):
            recovery.extract_archive(archive, artifact, tmp_path / "dist")


def test_checks_remain_active_under_python_optimization() -> None:
    code = "import recover_notice_release as r; r.require(False, 'fail closed')"
    result = subprocess.run(
        [sys.executable, "-O", "-c", code],
        cwd=ROOT / "scripts/ci",
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode != 0 and "fail closed" in result.stderr


def test_workflow_is_verify_default_and_cannot_republish_other_lanes() -> None:
    workflow = yaml.load(
        (ROOT / ".github/workflows/release.yml").read_text(), Loader=yaml.BaseLoader
    )
    inputs = workflow["on"]["workflow_dispatch"]["inputs"]
    assert inputs["recovery_mode"]["default"] == "verify"
    assert inputs["notice_verify_run_id"]["default"] == ""
    jobs = workflow["jobs"]
    assert "inputs.release_channel != 'notice-recovery'" in jobs["test"]["if"]
    for key, job in jobs.items():
        if key in {"notice-recovery-verify", "notice-recovery-publish"}:
            assert "inputs.release_channel == 'notice-recovery'" in job["if"]
            assert "github.ref == 'refs/heads/release/2.0.2-notice'" in job["if"]
        elif key.startswith("publish-") or key == "github-release":
            assert "notice-recovery" not in job["if"]
    verify = jobs["notice-recovery-verify"]
    assert verify["permissions"] == {"actions": "read", "contents": "read"}
    publish = jobs["notice-recovery-publish"]
    assert "inputs.recovery_mode == 'publish'" in publish["if"]
    assert "inputs.notice_verify_run_id != ''" in publish["if"]
    assert publish["environment"] == "release"
    publishers = [step for step in publish["steps"] if "pypa/" in step.get("uses", "")]
    assert len(publishers) == 1
    assert publishers[0]["uses"].split("@")[1] == recovery.PUBLISHER_COMMIT
    assert publishers[0]["with"]["verify-metadata"] == "true"
    assert publishers[0]["with"]["attestations"] == "true"
    all_publishers = [
        step["uses"]
        for job in jobs.values()
        for step in job["steps"]
        if step.get("uses", "").startswith("pypa/gh-action-pypi-publish@")
    ]
    assert all_publishers == [f"pypa/gh-action-pypi-publish@{recovery.PUBLISHER_COMMIT}"] * 3
    parser_check = (ROOT / "scripts/ci/check_pypi_publisher_metadata.sh").read_text()
    assert recovery.PUBLISHER_COMMIT in parser_check
    assert recovery.PUBLISHER_DIGEST in parser_check
    assert "--network none --read-only" in parser_check
    assert "strict=True" in parser_check
    build = jobs["build-python"]["steps"]
    assert any(
        step.get("run") == "bash scripts/ci/check_pypi_publisher_metadata.sh dist" for step in build
    )
