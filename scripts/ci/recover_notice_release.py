#!/usr/bin/env python3
"""Verify and stage original 2.0.2 bytes for publisher-only recovery.

This program never publishes, rebuilds, or rewrites a release. The committed
ledger and its canonical digest bind the known partial stable run, including
all skipped steps. Fresh API snapshots, archive bytes and payloads must agree.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import zipfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any

from validate_release_recovery import (
    ValidationError,
    read_json,
    validate_artifacts_snapshot,
    validate_jobs_snapshot,
    validate_manifest_digest,
    validate_payload_selection,
    validate_run_snapshot,
)
from verify_pypi_release_hashes import verify_remote_release

REPOSITORY = "ds1sqe/type-bridge"
SOURCE = "f94703f4c9b44a965a089f85b47c17933f4d9be6"
TAG_OBJECT = "e8f7a9c5c26dc1e415ae5707c337e79dddcafde6"
SOURCE_RUN = 34_077_021_385
CONTROL_REF = "refs/heads/release/2.0.2-notice"
MANIFEST_SHA256 = "9ed0cffae15440f5b47b7eb9f140d5c1c80f11629e118e10d2256bf15900cad4"
VERIFY_JOB = "Verify 2.0.2 publisher recovery"
PUBLISH_JOB = "Recover 2.0.2 Python facade"
PUBLISHER_COMMIT = "dc37677b2e1c63e2034f94d8a5b11f265b73ba33"
PUBLISHER_DIGEST = "sha256:a68d05519f6d7e47372aeaddab80b851b69afa89be179ec41775c72c4e3ab2d5"
RELEASE_ARTIFACTS = frozenset(
    {
        "python-dist",
        "core-sdist",
        "core-wheels-linux-aarch64",
        "core-wheels-linux-x86_64",
        "core-wheels-macos-aarch64",
        "core-wheels-macos-x86_64",
        "core-wheels-windows-x86_64",
        "node-package",
        "server-oci-release-metadata",
    }
)


def require(condition: bool, message: str) -> None:
    """Fail closed even when Python assertions are disabled."""
    if not condition:
        raise ValidationError(message)


def sha256(path: Path) -> str:
    """Hash a captured regular file incrementally."""
    require(path.is_file() and not path.is_symlink(), f"Unsafe file: {path}")
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def load_manifest(path: Path) -> dict[str, Any]:
    """Accept only the exact, independently captured 2.0.2 ledger."""
    manifest = read_json(path, label="manifest")
    validate_manifest_digest(manifest, expected_sha256=MANIFEST_SHA256)
    return manifest


def validate_tag(reference: dict[str, Any], tag: dict[str, Any]) -> None:
    """Bind both the annotated tag object and its original commit."""
    require(reference.get("ref") == "refs/tags/v2.0.2", "Wrong tag ref")
    require(reference.get("object", {}).get("type") == "tag", "Not an annotated tag")
    require(reference["object"].get("sha") == TAG_OBJECT, "Tag object changed")
    require(tag.get("sha") == TAG_OBJECT and tag.get("tag") == "v2.0.2", "Wrong tag object")
    require(tag.get("object", {}).get("type") == "commit", "Tag must directly target a commit")
    require(tag["object"].get("sha") == SOURCE, "Original source changed")


def validate_source(
    manifest: dict[str, Any], run: dict[str, Any], jobs: dict[str, Any], artifacts: dict[str, Any]
) -> None:
    """Require the original complete evidence, not just an aggregate conclusion."""
    require(run.get("repository", {}).get("full_name") == REPOSITORY, "Wrong source repository")
    require(run.get("head_repository", {}).get("full_name") == REPOSITORY, "Fork source rejected")
    validate_run_snapshot(run, expected_run=manifest["run"])
    counts = validate_jobs_snapshot(
        jobs,
        expected_run=manifest["run"],
        expected_jobs=manifest["jobs"],
        known_failure=manifest["known_failure"],
        skipped_mutators=manifest["skipped_mutators"],
    )
    require(dict(counts) == {"success": 39, "failure": 1, "skipped": 2}, "Wrong partial outcome")
    for job in jobs["jobs"]:
        steps = job["steps"]
        normalized = [
            {key: step.get(key) for key in ("name", "status", "conclusion", "number")}
            for step in steps
        ]
        require(normalized == manifest["job_steps"][job["name"]], f"Changed steps: {job['name']}")
        if job["conclusion"] != "skipped":
            require(bool(steps), f"Missing steps: {job['name']}")
        skipped = sorted(step["name"] for step in steps if step["conclusion"] == "skipped")
        require(skipped == manifest["skipped_steps"][job["name"]], f"Changed skips: {job['name']}")
    validate_artifacts_snapshot(
        artifacts,
        expected_run=manifest["run"],
        expected_artifacts={item["name"]: item for item in manifest["artifacts"]},
    )
    for artifact in artifacts["artifacts"]:
        origin = artifact["workflow_run"]
        require(origin.get("repository_id") == 1085407082, "Wrong artifact repository")
        require(origin.get("head_repository_id") == 1085407082, "Fork artifact rejected")


def validate_rehearsal(
    run: dict[str, Any],
    jobs: dict[str, Any],
    control_sha: str,
    *,
    verify_job: str = VERIFY_JOB,
    publish_job: str = PUBLISH_JOB,
) -> None:
    """Require same-control, attempt-one verification with no executed publisher."""
    require(run.get("repository", {}).get("full_name") == REPOSITORY, "Wrong rehearsal repository")
    require(run.get("head_repository", {}).get("full_name") == REPOSITORY, "Fork rehearsal")
    expected = {
        "event": "workflow_dispatch",
        "head_branch": CONTROL_REF.removeprefix("refs/heads/"),
        "head_sha": control_sha,
        "run_attempt": 1,
        "status": "completed",
        "conclusion": "success",
        "workflow_id": 229807619,
        "path": ".github/workflows/release.yml",
        "name": "Release",
    }
    for key, value in expected.items():
        require(type(run.get(key)) is type(value) and run[key] == value, f"Wrong rehearsal {key}")
    items = jobs.get("jobs", [])
    require(jobs.get("total_count") == len(items) and len(items) > 2, "Incomplete rehearsal jobs")
    require(len({job["id"] for job in items}) == len(items), "Duplicate rehearsal job ID")
    require(len({job["name"] for job in items}) == len(items), "Duplicate rehearsal job name")
    require({verify_job, publish_job} <= {job["name"] for job in items}, "Missing recovery jobs")
    for job in items:
        for key, value in {
            "run_id": run["id"],
            "run_attempt": 1,
            "head_sha": control_sha,
            "head_branch": expected["head_branch"],
            "workflow_name": "Release",
            "status": "completed",
        }.items():
            require(
                type(job.get(key)) is type(value) and job[key] == value,
                f"Wrong rehearsal job {key}",
            )
        if job["name"] == verify_job:
            require(job["conclusion"] == "success" and bool(job["steps"]), "Rehearsal not accepted")
            require(
                all(
                    step["status"] == "completed" and step["conclusion"] == "success"
                    for step in job["steps"]
                ),
                "Skipped or failed rehearsal step",
            )
            require(
                "Validate metadata in the exact publisher image"
                in {step["name"] for step in job["steps"]},
                "Metadata rehearsal missing",
            )
        else:
            require(
                job["conclusion"] == "skipped" and job["steps"] == [],
                f"Unexpected executed rehearsal job: {job['name']}",
            )


def api(path: str, destination: Path, *, label: str = "run") -> Any:
    """Capture authenticated read-only GitHub evidence, then parse it strictly."""
    with destination.open("xb") as stream:
        subprocess.run(
            ["gh", "api", f"repos/{REPOSITORY}/{path}"], stdout=stream, check=True, timeout=120
        )
    return read_json(destination, label=label)


def extract_archive(archive: Path, artifact: dict[str, Any], destination: Path) -> None:
    """Extract only an exact flat inventory after validating archive digest."""
    require(archive.stat().st_size == artifact["size_in_bytes"], "Archive size mismatch")
    require("sha256:" + sha256(archive) == artifact["digest"], "Archive digest mismatch")
    files = {item["path"]: item for item in artifact["files"]}
    require(
        all(re.fullmatch(r"[A-Za-z0-9_.-]+", name) and name not in {".", ".."} for name in files),
        "Non-flat payload rejected",
    )
    with zipfile.ZipFile(archive) as bundle:
        entries = bundle.infolist()
        require(
            len(entries) == len(files) and {item.filename for item in entries} == set(files),
            "Archive inventory mismatch",
        )
        for entry in entries:
            require(
                not entry.is_dir() and stat.S_IFMT(entry.external_attr >> 16) in (0, stat.S_IFREG),
                "Linked or special ZIP entry",
            )
            require(entry.file_size == files[entry.filename]["size_in_bytes"], "ZIP size mismatch")
            require(not entry.flag_bits & 1, "Encrypted archive rejected")
        destination.mkdir()
        for entry in entries:
            with bundle.open(entry) as source, (destination / entry.filename).open("xb") as output:
                shutil.copyfileobj(source, output)
    validate_payload_selection(
        destination,
        expected_artifacts={artifact["name"]: artifact},
        artifact_names=[artifact["name"]],
    )


def capture_artifact(artifact: dict[str, Any], output: Path) -> None:
    """Download an exact artifact ID and verify archive and extracted bytes."""
    archive = output / "archives" / f"{artifact['id']}.zip"
    with archive.open("xb") as stream:
        subprocess.run(
            ["gh", "api", f"repos/{REPOSITORY}/actions/artifacts/{artifact['id']}/zip"],
            stdout=stream,
            check=True,
            timeout=300,
        )
    extract_archive(archive, artifact, output / "payloads" / artifact["name"])


def prepare(manifest: dict[str, Any], output: Path) -> None:
    """Stage exact accepted facade and GitHub assets, with no recompression."""
    artifacts = {item["name"]: item for item in manifest["artifacts"]}
    for target, selection in (("dist", {"python-dist"}), ("release-assets", RELEASE_ARTIFACTS)):
        destination = output / target
        destination.mkdir()
        for name in sorted(selection):
            for item in artifacts[name]["files"]:
                source = output / "payloads" / name / item["path"]
                require(not (destination / item["path"]).exists(), "Colliding release assets")
                shutil.copyfile(source, destination / item["path"])
        validate_payload_selection(
            destination, expected_artifacts=artifacts, artifact_names=sorted(selection)
        )


def check_pypi(manifest: dict[str, Any], output: Path) -> None:
    """Reject facade conflicts and require the already-published native core."""
    for project, names, existing in (
        ("type-bridge", {"python-dist"}, False),
        (
            "type-bridge-core",
            {item["name"] for item in manifest["artifacts"] if item["name"].startswith("core-")},
            True,
        ),
    ):
        files = {
            file["path"]: output / "payloads" / item["name"] / file["path"]
            for item in manifest["artifacts"]
            if item["name"] in names
            for file in item["files"]
        }
        report = verify_remote_release(
            local_files=files,
            repository_url="https://pypi.org",
            project=project,
            version="2.0.2",
            require_existing=existing,
            attempts=1,
            retry_delay_seconds=0,
        )
        (output / "evidence" / f"{project}-preflight.json").write_text(json.dumps(report, indent=2))


def main() -> None:
    """Capture fresh evidence and emit a transparent promotion predicate."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--manifest", type=Path, default=Path(".github/release/v2.0.2-recovery.json")
    )
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--control-sha", required=True)
    parser.add_argument("--mode", choices=("verify", "publish"), default="verify")
    parser.add_argument("--verify-run", default="")
    parser.add_argument("--recheck-only", action="store_true")
    args = parser.parse_args()
    require(re.fullmatch(r"[0-9a-f]{40}", args.control_sha) is not None, "Invalid control SHA")
    if os.environ.get("GITHUB_ACTIONS") == "true":
        for key, expected in {
            "GITHUB_REPOSITORY": REPOSITORY,
            "GITHUB_REF": CONTROL_REF,
            "GITHUB_EVENT_NAME": "workflow_dispatch",
            "GITHUB_SHA": args.control_sha,
            "GITHUB_RUN_ATTEMPT": "1",
        }.items():
            require(os.environ.get(key) == expected, f"Unexpected recovery execution {key}")
    require(
        args.mode != "publish" or re.fullmatch(r"[1-9][0-9]{0,19}", args.verify_run) is not None,
        "Publish requires an exact accepted verification run",
    )
    manifest = load_manifest(args.manifest)
    if args.recheck_only:
        evidence = args.output / "evidence"
        predicate = read_json(evidence / "recovery-promotion.json", label="manifest")
        require(predicate.get("control_commit") == args.control_sha, "Changed publisher controls")
        require(predicate.get("recovery_mode") == "publish", "Not a publishing recovery")
        reference = api("git/ref/tags/v2.0.2", evidence / "prepublish-tag-ref.json")
        tag = api(f"git/tags/{TAG_OBJECT}", evidence / "prepublish-tag-object.json")
        validate_tag(reference, tag)
        artifacts = {item["name"]: item for item in manifest["artifacts"]}
        validate_payload_selection(
            args.output / "dist", expected_artifacts=artifacts, artifact_names=["python-dist"]
        )
        check_pypi(manifest, args.output)
        return
    args.output.mkdir(parents=True, exist_ok=False)
    evidence = args.output / "evidence"
    evidence.mkdir()
    for name in ("archives", "payloads"):
        (args.output / name).mkdir()
    reference = api("git/ref/tags/v2.0.2", evidence / "tag-ref.json")
    tag = api(f"git/tags/{TAG_OBJECT}", evidence / "tag-object.json")
    validate_tag(reference, tag)
    run = api(f"actions/runs/{SOURCE_RUN}", evidence / "source-run.json")
    jobs = api(
        f"actions/runs/{SOURCE_RUN}/attempts/1/jobs?per_page=100",
        evidence / "source-jobs.json",
        label="jobs",
    )
    artifacts = api(
        f"actions/runs/{SOURCE_RUN}/artifacts?per_page=100",
        evidence / "source-artifacts.json",
        label="artifacts",
    )
    validate_source(manifest, run, jobs, artifacts)
    if args.mode == "publish":
        rehearsal = api(f"actions/runs/{args.verify_run}", evidence / "rehearsal-run.json")
        require(rehearsal.get("id") == int(args.verify_run), "Wrong rehearsal ID")
        rehearsal_jobs = api(
            f"actions/runs/{args.verify_run}/attempts/1/jobs?per_page=100",
            evidence / "rehearsal-jobs.json",
            label="jobs",
        )
        validate_rehearsal(rehearsal, rehearsal_jobs, args.control_sha)
    with ThreadPoolExecutor(max_workers=4) as executor:
        list(executor.map(lambda item: capture_artifact(item, args.output), manifest["artifacts"]))
    prepare(manifest, args.output)
    check_pypi(manifest, args.output)
    shutil.copyfile(args.manifest, evidence / "v2.0.2-recovery-manifest.json")
    predicate = {
        "release": "2.0.2",
        "tag_object": TAG_OBJECT,
        "original_source": SOURCE,
        "source_run": SOURCE_RUN,
        "source_run_attempt": 1,
        "control_commit": args.control_sha,
        "control_ref": CONTROL_REF,
        "recovery_run": os.environ.get("GITHUB_RUN_ID"),
        "recovery_mode": args.mode,
        "verified_rehearsal_run": args.verify_run or None,
        "manifest_sha256": MANIFEST_SHA256,
        "publisher_commit": PUBLISHER_COMMIT,
        "publisher_image_digest": PUBLISHER_DIGEST,
        "verified_original_archive_count": 20,
        "verified_original_payload_count": 38,
        "publication_scope": ["type-bridge facade wheel", "type-bridge facade sdist"],
        "artifact_rebuild": False,
    }
    (evidence / "recovery-promotion.json").write_text(json.dumps(predicate, indent=2) + "\n")
    print(json.dumps(predicate, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (ValidationError, OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"Notice recovery verification failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
