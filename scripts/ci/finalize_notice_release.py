#!/usr/bin/env python3
"""Stage and verify the exact 2.0.2 GitHub notice; never publish packages.

The original stable bytes and successful facade recovery are separate frozen
authorities. This helper performs only authenticated reads and local staging.
GitHub draft creation/publication remains an explicit workflow step.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any

import recover_notice_release as recovery
from validate_release_recovery import (
    read_json,
    validate_artifacts_snapshot,
    validate_manifest_digest,
    validate_run_snapshot,
)
from verify_pypi_release_hashes import distribution_files, verify_remote_release

MANIFEST_SHA256 = "c69f482a636d464f6be6fe07fbe4068c5a5a5d014d74c0ce3cfc6ac0d4c470d3"
PUBLISH_RUN = 34_086_312_651
VERIFY_JOB = "Verify 2.0.2 GitHub notice finalization"
FINALIZE_JOB = "Finalize 2.0.2 GitHub notice"
require = recovery.require


def manifest_from(path: Path) -> dict[str, Any]:
    """Accept only the exact reviewed packet and successful publisher run."""
    manifest = read_json(path, label="manifest")
    validate_manifest_digest(manifest, expected_sha256=MANIFEST_SHA256)
    return manifest


def validate_publisher(manifest: dict[str, Any], run: dict[str, Any], jobs: dict[str, Any]) -> None:
    """Require every original recovery job/step, including zero-step skips."""
    expected = manifest["publication"]
    validate_run_snapshot(run, expected_run=expected["run"])
    for field in ("repository", "head_repository"):
        require(
            run.get(field, {}).get("full_name") == recovery.REPOSITORY, "Wrong publisher repository"
        )
    items = jobs.get("jobs", [])
    require(
        jobs.get("total_count") == len(items) == len(expected["jobs"]), "Incomplete publisher jobs"
    )
    require(len({job["id"] for job in items}) == len(items), "Duplicate publisher jobs")
    require({job["name"] for job in items} == set(expected["jobs"]), "Wrong publisher job names")
    for job in items:
        for key, value in {
            "run_id": PUBLISH_RUN,
            "run_attempt": 1,
            "head_sha": expected["run"]["head_sha"],
            "head_branch": expected["run"]["head_branch"],
            "workflow_name": "Release",
            "status": "completed",
        }.items():
            require(
                type(job.get(key)) is type(value) and job[key] == value,
                f"Wrong publisher job {key}",
            )
        outcome = {
            "conclusion": job["conclusion"],
            "steps": [
                {key: step.get(key) for key in ("name", "status", "conclusion", "number")}
                for step in job["steps"]
            ],
        }
        require(outcome == expected["jobs"][job["name"]], "Changed publisher job/steps")


def body_from(root: Path, manifest: dict[str, Any]) -> str:
    """Retain the exact original inventory and append reviewed recovery provenance."""
    original = subprocess.check_output(
        ["git", "show", f"{recovery.SOURCE}:docs/guide/v2.0.2-notice.md"], cwd=root
    ).decode()
    suffix = (root / ".github/release/v2.0.2-recovery-notice.md").read_text()
    body = original.rstrip() + suffix
    require(
        hashlib.sha256(body.encode()).hexdigest() == manifest["body_sha256"], "Notice body changed"
    )
    return body


def validate_packet(output: Path, manifest: dict[str, Any]) -> None:
    """Check all 13 flat assets and the full body without accepting extras."""
    assets = output / "assets"
    require(assets.is_dir() and not assets.is_symlink(), "Missing assets directory")
    expected = manifest["assets"]
    require({path.name for path in assets.iterdir()} == set(expected), "Wrong notice asset set")
    for name, file in expected.items():
        path = assets / name
        require(
            path.stat().st_size == file["size_in_bytes"]
            and recovery.sha256(path) == file["sha256"],
            f"Changed notice asset: {name}",
        )
    require(recovery.sha256(output / "body.md") == manifest["body_sha256"], "Changed notice body")


def stage(
    root: Path, output: Path, manifest: dict[str, Any], control: str, mode: str, verify_run: str
) -> None:
    """Reconcile original bytes and prior publication before staging GitHub-only assets."""
    output.mkdir(parents=True, exist_ok=False)
    evidence = output / "evidence"
    evidence.mkdir()
    if mode == "draft":
        releases = recovery.api("releases?per_page=100", evidence / "existing-releases.json")
        require(
            not any(item["tag_name"] == "v2.0.2" for item in releases),
            "Refusing to overwrite an existing notice release or draft",
        )
    if mode != "verify":
        require(verify_run.isdigit() and len(verify_run) <= 20, "Exact notice rehearsal required")
        run = recovery.api(f"actions/runs/{verify_run}", evidence / "verify-run.json")
        require(run.get("id") == int(verify_run), "Wrong notice rehearsal ID")
        jobs = recovery.api(
            f"actions/runs/{verify_run}/attempts/1/jobs?per_page=100",
            evidence / "verify-jobs.json",
            label="jobs",
        )
        recovery.validate_rehearsal(
            run, jobs, control, verify_job=VERIFY_JOB, publish_job=FINALIZE_JOB
        )
    subprocess.run(
        [
            sys.executable,
            str(root / "scripts/ci/recover_notice_release.py"),
            "--output",
            str(output / "original"),
            "--control-sha",
            control,
        ],
        cwd=root,
        check=True,
    )
    run = recovery.api(f"actions/runs/{PUBLISH_RUN}", evidence / "publisher-run.json")
    jobs = recovery.api(
        f"actions/runs/{PUBLISH_RUN}/attempts/1/jobs?per_page=100",
        evidence / "publisher-jobs.json",
        label="jobs",
    )
    validate_publisher(manifest, run, jobs)
    artifacts = recovery.api(
        f"actions/runs/{PUBLISH_RUN}/artifacts?per_page=100",
        evidence / "publisher-artifacts.json",
        label="artifacts",
    )
    expected = {item["name"]: item for item in manifest["publication"]["artifacts"]}
    validate_artifacts_snapshot(
        artifacts, expected_run=manifest["publication"]["run"], expected_artifacts=expected
    )
    for name in ("archives", "payloads"):
        (output / name).mkdir()
    artifact = expected["notice-recovery-publication"]
    recovery.capture_artifact(artifact, output)
    payload = output / "payloads/notice-recovery-publication"
    shutil.copytree(output / "original/release-assets", output / "assets")
    for name in ("v2.0.2-recovery-manifest.json", "recovery-promotion.json"):
        shutil.copyfile(payload / name, output / "assets" / name)
    (output / "body.md").write_text(body_from(root, manifest))
    validate_packet(output, manifest)
    verify_remote_release(
        local_files=distribution_files(output / "original/payloads/python-dist"),
        repository_url="https://pypi.org",
        project="type-bridge",
        version="2.0.2",
        require_existing=True,
        attempts=1,
        retry_delay_seconds=0,
    )
    summary = {
        "status": "verified-notice-packet",
        "mode": mode,
        "control_commit": control,
        "original_source": recovery.SOURCE,
        "publisher_run": PUBLISH_RUN,
        "manifest_sha256": MANIFEST_SHA256,
        "body_sha256": manifest["body_sha256"],
        "asset_count": 13,
        "verified_rehearsal_run": verify_run or None,
    }
    (evidence / "notice-packet.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(json.dumps(summary))


def verify_draft(output: Path, manifest: dict[str, Any]) -> None:
    """Download and verify every draft asset before the separate publish step."""
    validate_packet(output, manifest)
    evidence = output / "evidence"
    releases = recovery.api("releases?per_page=100", evidence / "releases.json")
    matches = [item for item in releases if item["tag_name"] == "v2.0.2"]
    require(len(matches) == 1, "Expected one exact notice draft")
    release = recovery.api(f"releases/{matches[0]['id']}", evidence / "draft.json")
    require(release["draft"] is True and release["prerelease"] is False, "Not a stable draft")
    require(
        release["target_commitish"] == recovery.SOURCE and release["name"] == "TypeBridge 2.0.2",
        "Wrong draft identity",
    )
    require(release["body"] == (output / "body.md").read_text(), "Changed draft body")
    expected = manifest["assets"]
    require(
        len(release["assets"]) == len(expected)
        and {asset["name"] for asset in release["assets"]} == set(expected),
        "Wrong draft assets",
    )
    downloaded = output / "draft-downloads"
    downloaded.mkdir()

    def download(asset: dict[str, Any]) -> None:
        file = expected[asset["name"]]
        require(
            asset["state"] == "uploaded"
            and asset["size"] == file["size_in_bytes"]
            and asset["digest"] == "sha256:" + file["sha256"],
            "Changed draft asset identity",
        )
        path = downloaded / asset["name"]
        with path.open("xb") as stream:
            subprocess.run(
                [
                    "gh",
                    "api",
                    f"repos/{recovery.REPOSITORY}/releases/assets/{asset['id']}",
                    "-H",
                    "Accept: application/octet-stream",
                ],
                stdout=stream,
                check=True,
                timeout=120,
            )
        require(
            path.stat().st_size == file["size_in_bytes"]
            and recovery.sha256(path) == file["sha256"],
            "Changed downloaded draft bytes",
        )

    with ThreadPoolExecutor(max_workers=4) as executor:
        list(executor.map(download, release["assets"]))
    recovery.validate_tag(
        recovery.api("git/ref/tags/v2.0.2", evidence / "final-tag-ref.json"),
        recovery.api(f"git/tags/{recovery.TAG_OBJECT}", evidence / "final-tag-object.json"),
    )
    print(json.dumps({"status": "verified-draft", "release_id": release["id"], "assets": 13}))


def main() -> None:
    """Select only local staging or read-only draft verification."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--mode", choices=("verify", "draft", "publish"), default="verify")
    parser.add_argument("--verify-run", default="")
    parser.add_argument("--verify-draft", action="store_true")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    manifest = manifest_from(root / ".github/release/v2.0.2-finalization.json")
    if args.verify_draft:
        verify_draft(args.output, manifest)
    else:
        stage(root, args.output, manifest, os.environ["GITHUB_SHA"], args.mode, args.verify_run)


if __name__ == "__main__":
    main()
