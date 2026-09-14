#!/usr/bin/env python3
"""Admit the fixed 2.2.1 publication recovery without replacing accepted bytes."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import stat
import subprocess
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
POLICY = ROOT / ".github/release/recovery-2.2.1.json"
REPOSITORY = "ds1sqe/type-bridge"


class RecoveryError(ValueError):
    """The original release or immutable tag no longer matches recovery policy."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise RecoveryError(message)


def validate_artifacts(policy: dict, snapshot: dict) -> None:
    """Bind every consumed artifact to its immutable original upload."""
    artifacts = snapshot.get("artifacts", [])
    require(snapshot.get("total_count") == len(artifacts), "Truncated artifact inventory")
    require(len({a["name"] for a in artifacts}) == len(artifacts), "Duplicate artifact names")
    by_name = {a["name"]: a for a in artifacts}
    for name, expected in policy["artifacts"].items():
        require(name in by_name, f"Missing artifact: {name}")
        artifact = by_name[name]
        require(artifact.get("expired") is False, f"Expired artifact: {name}")
        for key, value in expected.items():
            require(artifact.get(key) == value, f"Artifact {name} changed {key}")
        require(
            artifact.get("workflow_run", {}).get("id") == policy["run_id"], "Wrong artifact run"
        )
        require(
            artifact["workflow_run"].get("head_sha") == policy["source"], "Wrong artifact source"
        )


def verify_download(archive: Path, expected: dict, directory: Path) -> set[str]:
    """Compare consumed files with the digest-bound ZIP without extracting paths."""
    require(archive.stat().st_size == expected["size_in_bytes"], "Archive size drift")
    with archive.open("rb") as stream:
        digest = "sha256:" + hashlib.file_digest(stream, "sha256").hexdigest()
    require(digest == expected["digest"], "Archive digest drift")
    names: set[str] = set()
    with zipfile.ZipFile(archive) as bundle:
        for entry in bundle.infolist():
            path = Path(entry.filename)
            require(not path.is_absolute() and ".." not in path.parts, "Unsafe archive path")
            require(
                stat.S_IFMT(entry.external_attr >> 16) in {0, stat.S_IFREG, stat.S_IFDIR},
                "Special archive entry",
            )
            if entry.is_dir():
                continue
            require(entry.filename not in names, "Duplicate archive file")
            names.add(entry.filename)
            local = directory / path
            require(
                all(not parent.is_symlink() for parent in [local, *local.parents]),
                "Linked consumed file",
            )
            require(
                local.is_file() and local.stat().st_size == entry.file_size,
                "Consumed file size drift",
            )
            with bundle.open(entry) as original, local.open("rb") as consumed:
                require(
                    hashlib.file_digest(original, "sha256").digest()
                    == hashlib.file_digest(consumed, "sha256").digest(),
                    "Consumed file digest drift",
                )
    require(bool(names), "Empty artifact")
    return names


def verify_inventory(directory: Path, expected: set[str]) -> None:
    """Reject extra publisher inputs, including links and special files."""
    require(directory.is_dir() and not directory.is_symlink(), "Invalid artifact directory")
    observed = set()
    for path in directory.rglob("*"):
        require(not path.is_symlink(), "Linked publisher input")
        if path.is_dir():
            continue
        require(path.is_file(), "Special publisher input")
        observed.add(path.relative_to(directory).as_posix())
    require(observed == expected, "Publisher file inventory drift")


def validate(policy: dict, run: dict, jobs: dict, reference: dict, tag: dict) -> None:
    """Require the complete original acceptance and exact partial publication state."""
    expected = {
        "id": policy["run_id"],
        "run_attempt": policy["run_attempt"],
        "head_sha": policy["source"],
        "head_branch": policy["release_tag"],
        "path": ".github/workflows/release.yml",
        "event": "push",
        "status": "completed",
        "conclusion": "failure",
    }
    for key, value in expected.items():
        require(type(run.get(key)) is type(value) and run[key] == value, f"Wrong run {key}")
    for key in ("repository", "head_repository"):
        require(run.get(key, {}).get("full_name") == REPOSITORY, "Wrong repository")
        require(run[key].get("id") == 1085407082, "Wrong repository ID")
    observed = jobs.get("jobs", [])
    require(jobs.get("total_count") == len(observed) == len(policy["jobs"]), "Job count drift")
    require(len({job["id"] for job in observed}) == len(observed), "Duplicate jobs")
    require(
        {job["name"]: job["conclusion"] for job in observed} == policy["jobs"], "Job result drift"
    )
    for job in observed:
        require(job["run_id"] == policy["run_id"], "Wrong job run")
        require(job["run_attempt"] == policy["run_attempt"], "Wrong job attempt")
        require(job["head_sha"] == policy["source"], "Wrong job source")
        require(job["status"] == "completed", "Unfinished job")
        steps = job.get("steps", [])
        require(len({step["name"] for step in steps}) == len(steps), "Duplicate steps")
        require(
            {step["name"]: step["conclusion"] for step in steps} == policy["steps"][job["name"]],
            "Step result drift",
        )
        require(all(step["status"] == "completed" for step in steps), "Unfinished step")
    require(reference.get("ref") == f"refs/tags/{policy['release_tag']}", "Wrong tag ref")
    require(reference.get("object", {}).get("type") == "tag", "Lightweight tag")
    require(reference["object"].get("sha") == policy["tag_object"], "Moved tag")
    require(tag.get("sha") == policy["tag_object"], "Wrong tag object")
    require(tag.get("tag") == policy["release_tag"], "Wrong tag name")
    require(tag.get("object", {}).get("type") == "commit", "Wrong tag target kind")
    require(tag["object"].get("sha") == policy["source"], "Wrong tag source")


def recovery_predicate(policy: dict, environment: dict[str, str]) -> dict:
    """Describe promotion separately from the original build and acceptance."""
    actual = {
        "index": environment["SERVER_OCI_DIGEST"],
        "linux/amd64": environment["SERVER_OCI_AMD64_DIGEST"],
        "linux/arm64": environment["SERVER_OCI_ARM64_DIGEST"],
    }
    require(actual == policy["oci_digests"], "Recovered OCI digest drift")
    require(environment["GITHUB_REPOSITORY"] == REPOSITORY, "Wrong recovery repository")
    require(environment["GITHUB_REF"] == "refs/heads/master", "Wrong recovery control ref")
    require(environment["GITHUB_EVENT_NAME"] == "workflow_dispatch", "Wrong recovery event")
    return {
        "release": {
            "tag": policy["release_tag"],
            "tagObject": policy["tag_object"],
            "source": policy["source"],
            "ociDigests": actual,
        },
        "originalBuild": {
            "repository": REPOSITORY,
            "runId": policy["run_id"],
            "runAttempt": policy["run_attempt"],
            "artifacts": policy["artifacts"],
            "jobs": policy["jobs"],
        },
        "recovery": {
            "source": environment["GITHUB_SHA"],
            "ref": environment["GITHUB_REF"],
            "runId": environment["GITHUB_RUN_ID"],
            "runAttempt": environment["GITHUB_RUN_ATTEMPT"],
            "operation": "Verify accepted bytes and original tag signatures; complete publication",
        },
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--check", action="append", default=[], metavar="ARTIFACT=DIRECTORY")
    parser.add_argument("--oci-predicate", type=Path)
    args = parser.parse_args()
    policy = json.loads(POLICY.read_text())
    args.output.mkdir(parents=True, exist_ok=False)

    def snapshot(name: str, endpoint: str) -> dict:
        result = subprocess.run(
            ["gh", "api", f"repos/{REPOSITORY}/{endpoint}"],
            check=True,
            capture_output=True,
            text=True,
            timeout=60,
        )
        (args.output / name).write_text(result.stdout)
        return json.loads(result.stdout)

    run = snapshot("run.json", f"actions/runs/{policy['run_id']}")
    jobs = snapshot("jobs.json", f"actions/runs/{policy['run_id']}/attempts/1/jobs?per_page=100")
    reference = snapshot("tag-ref.json", f"git/ref/tags/{policy['release_tag']}")
    tag = snapshot("tag.json", f"git/tags/{policy['tag_object']}")
    validate(policy, run, jobs, reference, tag)
    artifacts = snapshot(
        "artifacts.json", f"actions/runs/{policy['run_id']}/artifacts?per_page=100"
    )
    validate_artifacts(policy, artifacts)
    inventories: dict[Path, set[str]] = {}
    for selection in args.check:
        name, separator, destination = selection.partition("=")
        require(bool(separator) and name in policy["artifacts"], "Unselected artifact")
        expected = policy["artifacts"][name]
        archive = args.output / f"{expected['id']}.zip"
        with archive.open("xb") as stream:
            subprocess.run(
                [
                    "gh",
                    "api",
                    "--allow-escape-sequences",
                    f"repos/{REPOSITORY}/actions/artifacts/{expected['id']}/zip",
                ],
                check=True,
                stdout=stream,
                timeout=300,
            )
        directory = Path(destination).absolute()
        names = verify_download(archive, expected, directory)
        inventory = inventories.setdefault(directory, set())
        require(not inventory.intersection(names), "Overlapping artifact files")
        inventory.update(names)
    for directory, names in inventories.items():
        verify_inventory(directory, names)
    if args.oci_predicate:
        predicate = recovery_predicate(policy, dict(os.environ))
        args.oci_predicate.write_text(json.dumps(predicate, indent=2, sort_keys=True) + "\n")
    print("Accepted exact original release and immutable tag for publication recovery")


if __name__ == "__main__":
    main()
