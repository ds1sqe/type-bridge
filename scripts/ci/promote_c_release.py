#!/usr/bin/env python3
"""Capture, seal, and promote exact C archives without rebuilding release bytes."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import io
import json
import os
import re
import shutil
import stat
import subprocess
import tarfile
import zipfile
from pathlib import Path
from typing import Any

import c_release_policy as policy

ROOT = policy.ROOT
REPOSITORY = "ds1sqe/type-bridge"
MAX_FILE = 512 * 1024 * 1024
MAX_ARCHIVE = 1024 * 1024 * 1024


class PromotionError(ValueError):
    """A source, evidence, signature, or destination identity was rejected."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise PromotionError(message)


def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    result: dict[str, Any] = {}
    for key, value in pairs:
        require(key not in result, f"Duplicate JSON key: {key}")
        result[key] = value
    return result


def read(path: Path) -> bytes:
    metadata = path.lstat()
    require(stat.S_ISREG(metadata.st_mode) and metadata.st_size <= MAX_FILE, f"Unsafe file: {path}")
    return path.read_bytes()


def load(path: Path) -> dict[str, Any]:
    result = json.loads(read(path), object_pairs_hook=unique_object)
    require(isinstance(result, dict), f"Expected JSON object: {path}")
    return result


def canonical(value: Any) -> bytes:
    return (
        json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False).encode() + b"\n"
    )


def write(path: Path, value: Any) -> None:
    with path.open("xb") as output:
        output.write(canonical(value))


def digest(path: Path) -> str:
    return hashlib.sha256(read(path)).hexdigest()


def record(path: Path) -> dict[str, Any]:
    return {"name": path.name, "size": len(read(path)), "sha256": digest(path)}


def command(arguments: list[str], *, output: Path | None = None) -> str:
    if output is not None:
        with output.open("xb") as stream:
            subprocess.run(arguments, cwd=ROOT, stdout=stream, check=True, timeout=300)
        return ""
    return subprocess.run(
        arguments, cwd=ROOT, capture_output=True, text=True, check=True, timeout=300
    ).stdout.strip()


def api(endpoint: str, destination: Path) -> dict[str, Any]:
    command(["gh", "api", f"repos/{REPOSITORY}/{endpoint}"], output=destination)
    return load(destination)


def control(*, publishing: bool = False) -> tuple[str, str, dict[str, Any]]:
    selected = load(policy.POLICY)
    require(
        selected["version"] == policy.VERSION and selected["repository"] == REPOSITORY,
        "Wrong release policy",
    )
    require(
        selected["ci_artifacts"] == policy.ARTIFACTS
        and selected["public_files"] == policy.PUBLIC_FILES,
        "Changed artifact selection",
    )
    require(selected["ci_workflow_sha256"] == digest(policy.CI), "Stale CI policy")
    source = command(["git", "rev-parse", "HEAD"])
    tree = command(["git", "rev-parse", "HEAD^{tree}"])
    require(re.fullmatch(r"[0-9a-f]{40}", source) is not None, "Malformed source")
    require(
        not command(["git", "status", "--porcelain=v1", "--untracked-files=no"]),
        "Dirty release source",
    )
    for key, expected in {
        "GITHUB_REPOSITORY": REPOSITORY,
        "GITHUB_SHA": source,
        "GITHUB_WORKFLOW_SHA": source,
        "GITHUB_EVENT_NAME": "workflow_dispatch",
        "GITHUB_RUN_ATTEMPT": "1",
    }.items():
        require(os.environ.get(key) == expected, f"Wrong control {key}")
    ref = os.environ.get("GITHUB_REF", "")
    allowed = {
        "refs/heads/master",
        "refs/heads/release/c-sdk-readiness",
        f"refs/tags/v{policy.VERSION}",
    }
    require(ref in allowed, "Unselected verification ref")
    require(
        os.environ.get("GITHUB_WORKFLOW_REF") == f"{REPOSITORY}/{policy.WORKFLOW}@{ref}",
        "Wrong workflow identity",
    )
    if publishing:
        require(ref == f"refs/tags/v{policy.VERSION}", "Promotion requires the selected stable tag")
    return source, tree, selected


def validate_run(
    run: dict[str, Any], *, run_id: int, source: str, workflow: str, event: str
) -> None:
    for key, expected in {
        "id": run_id,
        "head_sha": source,
        "path": workflow,
        "event": event,
        "run_attempt": 1,
        "status": "completed",
        "conclusion": "success",
    }.items():
        require(type(run.get(key)) is type(expected) and run[key] == expected, f"Wrong run {key}")
    for key in ("repository", "head_repository"):
        require(run.get(key, {}).get("full_name") == REPOSITORY, "Fork or wrong repository")
        require(run[key].get("id") == 1085407082, "Wrong repository ID")
    if workflow == ".github/workflows/ci.yml":
        require(run.get("head_branch") == "master", "CI must accept the exact master commit")
    else:
        require(
            run.get("head_branch") in {"master", "release/c-sdk-readiness", f"v{policy.VERSION}"},
            "Unselected verification branch",
        )


def validate_ci_run(run: dict[str, Any], *, run_id: int, source: str) -> None:
    event = run.get("event")
    if not isinstance(event, str) or event not in {"push", "workflow_dispatch"}:
        raise PromotionError("Unselected CI event")
    validate_run(
        run, run_id=run_id, source=source, workflow=".github/workflows/ci.yml", event=event
    )


def validate_jobs(
    snapshot: dict[str, Any],
    *,
    run_id: int,
    source: str,
    expected: dict[str, dict[str, str]],
    skipped: set[str] | None = None,
) -> None:
    jobs = snapshot.get("jobs", [])
    skipped = skipped or set()
    require(snapshot.get("total_count") == len(jobs), "Truncated jobs snapshot")
    require(len(jobs) == len(expected) + len(skipped), "Wrong job count")
    require({job["name"] for job in jobs} == set(expected) | skipped, "Wrong job inventory")
    require(len({job["id"] for job in jobs}) == len(jobs), "Duplicate job ID")
    for job in jobs:
        for key, value in {
            "run_id": run_id,
            "run_attempt": 1,
            "head_sha": source,
            "status": "completed",
        }.items():
            require(type(job.get(key)) is type(value) and job[key] == value, f"Wrong job {key}")
        if job["name"] in skipped:
            require(
                job["conclusion"] == "skipped" and job["steps"] == [],
                "Publisher executed during verification",
            )
            continue
        require(
            job["conclusion"] == "success" and bool(job["steps"]), f"Unaccepted job: {job['name']}"
        )
        required = expected[job["name"]]
        observed = {step["name"]: step for step in job["steps"]}
        require(len(observed) == len(job["steps"]), "Duplicate step")
        require(set(required) <= set(observed), f"Missing steps: {job['name']}")
        for name, step in observed.items():
            require(step["status"] == "completed", "Unfinished step")
            require(
                step["conclusion"] == required.get(name, "success"),
                f"Unaccepted step: {job['name']}/{name}",
            )


def extract(archive: Path, artifact: dict[str, Any], files: list[str], destination: Path) -> None:
    metadata = archive.lstat()
    require(
        stat.S_ISREG(metadata.st_mode) and metadata.st_size <= MAX_ARCHIVE, "Unsafe Actions archive"
    )
    require(metadata.st_size == artifact["size_in_bytes"], "Actions archive size mismatch")
    with archive.open("rb") as stream:
        require(
            "sha256:" + hashlib.file_digest(stream, "sha256").hexdigest() == artifact["digest"],
            "Actions archive digest mismatch",
        )
    require(
        all(re.fullmatch(r"[A-Za-z0-9_.-]+", name) and name not in {".", ".."} for name in files),
        "Unsafe selected filename",
    )
    with zipfile.ZipFile(archive) as bundle:
        entries = bundle.infolist()
        require(
            len(entries) == len(files) and {entry.filename for entry in entries} == set(files),
            "Actions payload inventory mismatch",
        )
        require(
            sum(entry.file_size for entry in entries) <= MAX_ARCHIVE, "Oversized Actions payload"
        )
        for entry in entries:
            require(
                not entry.is_dir() and stat.S_IFMT(entry.external_attr >> 16) in {0, stat.S_IFREG},
                "Linked or special Actions payload",
            )
            require(
                not entry.flag_bits & 1 and entry.file_size <= MAX_FILE,
                "Encrypted or oversized payload",
            )
        destination.mkdir()
        for entry in entries:
            with bundle.open(entry) as source, (destination / entry.filename).open("xb") as output:
                shutil.copyfileobj(source, output)


def capture_artifacts(
    run_id: int, source: str, selection: dict[str, list[str]], output: Path
) -> dict[str, Any]:
    snapshot = api(f"actions/runs/{run_id}/artifacts?per_page=100", output / "artifacts.json")
    items = snapshot.get("artifacts", [])
    require(snapshot.get("total_count") == len(items), "Truncated artifacts snapshot")
    require(len({item["id"] for item in items}) == len(items), "Duplicate artifact ID")
    require(len({item["name"] for item in items}) == len(items), "Duplicate artifact name")
    selected = {item["name"]: item for item in items if item["name"] in selection}
    require(set(selected) == set(selection), "Missing selected artifact")
    records = {}
    for name, item in selected.items():
        require(item.get("expired") is False, "Expired artifact")
        origin = item.get("workflow_run", {})
        require(
            origin.get("id") == run_id and origin.get("head_sha") == source, "Wrong artifact source"
        )
        require(
            origin.get("repository_id") == 1085407082
            and origin.get("head_repository_id") == 1085407082,
            "Fork artifact",
        )
        archive = output / f"{item['id']}.zip"
        command(
            ["gh", "api", f"repos/{REPOSITORY}/actions/artifacts/{item['id']}/zip"], output=archive
        )
        destination = output / name
        extract(archive, item, selection[name], destination)
        records[name] = {
            "id": item["id"],
            "digest": item["digest"],
            "size_in_bytes": item["size_in_bytes"],
            "files": [record(destination / filename) for filename in sorted(selection[name])],
        }
    return records


def capture(ci_run: int, output: Path) -> None:
    source, tree, selected = control()
    output.mkdir(parents=True, exist_ok=False)
    run = api(f"actions/runs/{ci_run}", output / "ci-run.json")
    jobs = api(f"actions/runs/{ci_run}/attempts/1/jobs?per_page=100", output / "ci-jobs.json")
    validate_ci_run(run, run_id=ci_run, source=source)
    validate_jobs(jobs, run_id=ci_run, source=source, expected=selected["ci_jobs"])
    artifacts = capture_artifacts(ci_run, source, policy.ARTIFACTS, output)
    write(
        output / "capture.json",
        {
            "source": source,
            "tree": tree,
            "ci_run": ci_run,
            "artifacts": artifacts,
            "policy_sha256": digest(policy.POLICY),
        },
    )


def seal(inputs: Path, reports: Path, output: Path) -> None:
    # The independent FULL-C audit is the acceptance authority. It reopens all
    # archive, security, Artifact acceptance, predecessor, and V6 inputs before sealing.
    import audit_full_c_artifact as audit

    source, tree, _ = control()
    captured = load(inputs / "capture.json")
    require(
        captured["source"] == source
        and captured["tree"] == tree
        and captured["policy_sha256"] == digest(policy.POLICY),
        "Capture source or policy drift",
    )
    paths = {
        filename: inputs / artifact / filename
        for artifact, files in policy.ARTIFACTS.items()
        for filename in files
    }
    require(set(captured["artifacts"]) == set(policy.ARTIFACTS), "Incomplete capture inventory")
    for artifact, item in captured["artifacts"].items():
        require(
            item["files"]
            == [record(inputs / artifact / name) for name in sorted(policy.ARTIFACTS[artifact])],
            "Captured bytes changed",
        )
    result, summary = audit.audit(
        reports_root=reports,
        acceptance_path=paths["c-artifact-acceptance-report.json"],
        provider=paths["c-artifact-clean-consumer.json"],
        live=paths["c-artifact-live-journey.json"],
        cli=paths[policy.CLI],
        runtime=paths[policy.RUNTIME],
        generated=paths[policy.GENERATED],
        security_evidence=inputs / "c-distribution-security-linux-x86_64-gnu",
    )
    require(
        result["source_commit"] == source and result["authority_state"] == "accepted-artifact",
        "FULL-C source or acceptance mismatch",
    )
    output.mkdir(parents=True, exist_ok=False)
    for original, public in policy.PUBLIC_FILES.items():
        shutil.copyfile(paths[original], output / public)
    evidence_files = {
        f"ci/{name}": read(inputs / name)
        for name in ("ci-run.json", "ci-jobs.json", "artifacts.json", "capture.json")
    }
    evidence_files.update(
        {
            f"inputs/{name}": read(path)
            for name, path in paths.items()
            if name not in policy.PUBLIC_FILES
        }
    )
    for version, bindings in audit.REPORT_BINDINGS.items():
        for binding in bindings:
            evidence_files[f"reports/v{version}/{binding}.json"] = read(
                reports / f"v{version}" / f"{binding}.json"
            )
    evidence_files["full-c-audit.json"] = canonical(result)
    evidence_files["full-c-summary.md"] = summary.encode()
    evidence_files["release-policy.json"] = read(policy.POLICY)
    with (
        (output / policy.EVIDENCE).open("xb") as raw,
        gzip.GzipFile(fileobj=raw, mode="wb", filename="", mtime=0) as compressed,
        tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive,
    ):
        for name, body in sorted(evidence_files.items()):
            entry = tarfile.TarInfo(name)
            entry.size = len(body)
            entry.mode = 0o644
            archive.addfile(entry, io.BytesIO(body))
    write(
        output / policy.RECEIPT,
        {
            "format": "typebridge.c-release-verification/v1",
            "status": "accepted-full-c-artifacts",
            "source": source,
            "tree": tree,
            "ci_run": captured["ci_run"],
            "verification_run": int(os.environ["GITHUB_RUN_ID"]),
            "verification_attempt": 1,
            "policy_sha256": digest(policy.POLICY),
            "artifact_set_id": result["artifact_set_id"],
            "full_c_audit_sha256": hashlib.sha256(evidence_files["full-c-audit.json"]).hexdigest(),
            "files": [record(path) for path in sorted(output.iterdir())],
            "public_name_mapping": policy.PUBLIC_FILES,
            "publication_authority": False,
        },
    )


def validate_receipt(directory: Path, *, source: str, tree: str, verify_run: int) -> dict[str, Any]:
    receipt = load(directory / policy.RECEIPT)
    for key, expected in {
        "format": "typebridge.c-release-verification/v1",
        "status": "accepted-full-c-artifacts",
        "source": source,
        "tree": tree,
        "verification_run": verify_run,
        "verification_attempt": 1,
        "policy_sha256": digest(policy.POLICY),
        "public_name_mapping": policy.PUBLIC_FILES,
        "publication_authority": False,
    }.items():
        require(
            type(receipt.get(key)) is type(expected) and receipt[key] == expected,
            f"Wrong verification receipt {key}",
        )
    names = sorted([*policy.PUBLIC_FILES.values(), policy.EVIDENCE])
    require(
        receipt["files"] == [record(directory / name) for name in names], "Verified payload changed"
    )
    require(
        {path.name for path in directory.iterdir()} == set(names) | {policy.RECEIPT},
        "Unexpected verified payload",
    )
    return receipt


def validate_tag(reference: dict[str, Any], tag: dict[str, Any], source: str) -> str:
    require(
        reference.get("ref") == f"refs/tags/v{policy.VERSION}"
        and reference.get("object", {}).get("type") == "tag",
        "Wrong or lightweight tag",
    )
    require(
        tag.get("sha") == reference["object"].get("sha") and tag.get("tag") == f"v{policy.VERSION}",
        "Tag object drift",
    )
    require(
        tag.get("object", {})
        == {
            "type": "commit",
            "sha": source,
            "url": f"https://api.github.com/repos/{REPOSITORY}/git/commits/{source}",
        },
        "Tag source drift",
    )
    return tag["sha"]


def stage(verify_run: int, output: Path) -> None:
    source, tree, selected = control(publishing=True)
    output.mkdir(parents=True, exist_ok=False)
    run = api(f"actions/runs/{verify_run}", output / "verification-run.json")
    jobs = api(
        f"actions/runs/{verify_run}/attempts/1/jobs?per_page=100", output / "verification-jobs.json"
    )
    validate_run(
        run, run_id=verify_run, source=source, workflow=policy.WORKFLOW, event="workflow_dispatch"
    )
    validate_jobs(
        jobs,
        run_id=verify_run,
        source=source,
        expected={policy.VERIFY_JOB: selected["verification_steps"]},
        skipped={policy.PUBLISH_JOB},
    )
    files = [*policy.PUBLIC_FILES.values(), policy.EVIDENCE, policy.RECEIPT]
    capture_artifacts(verify_run, source, {policy.VERIFY_ARTIFACT: files}, output)
    payloads = output / policy.VERIFY_ARTIFACT
    receipt = validate_receipt(payloads, source=source, tree=tree, verify_run=verify_run)
    ci_run = api(f"actions/runs/{receipt['ci_run']}", output / "ci-run.json")
    ci_jobs = api(
        f"actions/runs/{receipt['ci_run']}/attempts/1/jobs?per_page=100", output / "ci-jobs.json"
    )
    validate_ci_run(ci_run, run_id=receipt["ci_run"], source=source)
    validate_jobs(ci_jobs, run_id=receipt["ci_run"], source=source, expected=selected["ci_jobs"])
    reference = api(f"git/ref/tags/v{policy.VERSION}", output / "tag-ref.json")
    require(reference.get("object", {}).get("type") == "tag", "Annotated tag required")
    tag = api(f"git/tags/{reference['object']['sha']}", output / "tag-object.json")
    tag_object = validate_tag(reference, tag, source)
    write(
        payloads / policy.PROMOTION,
        {
            "format": "typebridge.c-release-promotion/v1",
            "version": policy.VERSION,
            "repository": REPOSITORY,
            "source": source,
            "tree": tree,
            "tag": f"v{policy.VERSION}",
            "tag_object": tag_object,
            "workflow_identity": policy.IDENTITY,
            "policy_sha256": digest(policy.POLICY),
            "ci_run": receipt["ci_run"],
            "verification_run": verify_run,
            "verification_attempt": 1,
            "artifact_set_id": receipt["artifact_set_id"],
            "abi": "1.6.0",
            "target": policy.TARGET,
            "runner": "ubuntu-24.04",
            "linkage": "shared",
            "files": [record(payloads / name) for name in sorted(files)],
            "public_name_mapping": policy.PUBLIC_FILES,
            "publication_authority": True,
        },
    )


def verify_signature(payload: Path, bundle: Path, source: str) -> None:
    command(
        [
            "cosign",
            "verify-blob",
            "--bundle",
            str(bundle),
            "--certificate-identity",
            policy.IDENTITY,
            "--certificate-oidc-issuer",
            "https://token.actions.githubusercontent.com",
            "--certificate-github-workflow-sha",
            source,
            str(payload),
        ]
    )


def publish(output: Path) -> None:
    source, tree, _ = control(publishing=True)
    payloads = output / policy.VERIFY_ARTIFACT
    promotion = load(payloads / policy.PROMOTION)
    require(
        promotion["source"] == source
        and promotion["tree"] == tree
        and promotion["policy_sha256"] == digest(policy.POLICY),
        "Staged promotion drift",
    )
    files = sorted([*policy.PUBLIC_FILES.values(), policy.EVIDENCE, policy.RECEIPT])
    require(
        promotion["files"] == [record(payloads / name) for name in files], "Staged payload drift"
    )
    reference = api(f"git/ref/tags/v{policy.VERSION}", output / "publish-tag-ref.json")
    require(reference.get("object", {}).get("type") == "tag", "Annotated tag required")
    tag = api(f"git/tags/{reference['object']['sha']}", output / "publish-tag-object.json")
    require(validate_tag(reference, tag, source) == promotion["tag_object"], "Staged tag changed")
    files.append(policy.PROMOTION)
    # The ordinary release workflow owns draft creation and all other assets.
    # C promotion can only add its selected files to that existing draft.
    raw = command(
        ["gh", "api", "--paginate", "--slurp", f"repos/{REPOSITORY}/releases?per_page=100"]
    )
    releases = [
        release
        for page in json.loads(raw)
        for release in page
        if release.get("tag_name") == f"v{policy.VERSION}"
    ]
    require(len(releases) == 1, "Expected one existing release draft")
    release = releases[0]
    require(
        release["draft"] is True
        and release["prerelease"] is False
        and release["target_commitish"] == source,
        "Wrong release draft",
    )
    write(output / "draft-before.json", release)
    existing = {asset["name"]: asset for asset in release["assets"]}
    require(len(existing) == len(release["assets"]), "Duplicate release asset")
    selected_names = set(files) | {name + ".sigstore.json" for name in files}
    require(
        not any(
            (name.startswith("type-bridge-c-") or name.startswith("type-bridge-cli-"))
            and name not in selected_names
            for name in existing
        ),
        "Unexpected C release asset",
    )
    accepted = output / "existing"
    accepted.mkdir()
    for name in sorted(selected_names & set(existing)):
        asset = existing[name]
        command(
            [
                "gh",
                "api",
                "-H",
                "Accept: application/octet-stream",
                f"repos/{REPOSITORY}/releases/assets/{asset['id']}",
            ],
            output=accepted / name,
        )
        require(asset["size"] == len(read(accepted / name)), "Existing asset size mismatch")
        require(
            asset.get("digest") == "sha256:" + digest(accepted / name),
            "Existing asset digest mismatch",
        )
        if name in files:
            require(
                read(accepted / name) == read(payloads / name), "Existing release bytes conflict"
            )
        else:
            verify_signature(
                payloads / name.removesuffix(".sigstore.json"), accepted / name, source
            )
    # Validate every existing byte before requesting a signature or upload.
    for name in files:
        bundle = payloads / (name + ".sigstore.json")
        if bundle.name in existing:
            shutil.copyfile(accepted / bundle.name, bundle)
        else:
            command(["cosign", "sign-blob", "--yes", "--bundle", str(bundle), str(payloads / name)])
        verify_signature(payloads / name, bundle, source)
    for name in sorted(selected_names - set(existing)):
        command(
            [
                "gh",
                "release",
                "upload",
                f"v{policy.VERSION}",
                str(payloads / name),
                "--repo",
                REPOSITORY,
            ]
        )
    final = api(f"releases/{release['id']}", output / "draft-after.json")
    require(
        final["draft"] is True
        and final["tag_name"] == f"v{policy.VERSION}"
        and final["target_commitish"] == source,
        "Release identity changed",
    )
    final_assets = {asset["name"]: asset for asset in final["assets"]}
    require(set(final_assets) == set(existing) | selected_names, "Release inventory changed")
    require(final["body"] == release["body"], "Ordinary release body changed during C promotion")
    for name in set(existing) - selected_names:
        require(
            final_assets[name] == existing[name],
            "Ordinary release asset changed during C promotion",
        )
    downloaded = output / "uploaded"
    downloaded.mkdir()
    for name in sorted(selected_names):
        asset = final_assets[name]
        command(
            [
                "gh",
                "api",
                "-H",
                "Accept: application/octet-stream",
                f"repos/{REPOSITORY}/releases/assets/{asset['id']}",
            ],
            output=downloaded / name,
        )
        require(read(downloaded / name) == read(payloads / name), "Uploaded bytes mismatch")
    for name in files:
        verify_signature(downloaded / name, downloaded / (name + ".sigstore.json"), source)
    write(
        output / "promotion-result.json",
        {
            "status": "draft-c-assets-and-signatures-verified",
            "release_id": release["id"],
            "source": source,
            "assets": [record(downloaded / name) for name in sorted(selected_names)],
        },
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="mode", required=True)
    capture_parser = subparsers.add_parser("capture")
    capture_parser.add_argument("--ci-run", required=True, type=int)
    seal_parser = subparsers.add_parser("seal")
    seal_parser.add_argument("--inputs", required=True, type=Path)
    seal_parser.add_argument("--reports", required=True, type=Path)
    stage_parser = subparsers.add_parser("stage")
    stage_parser.add_argument("--verify-run", required=True, type=int)
    publish_parser = subparsers.add_parser("publish")
    for command_parser in (capture_parser, seal_parser, stage_parser, publish_parser):
        command_parser.add_argument("--output", required=True, type=Path)
    arguments = parser.parse_args()
    if arguments.mode == "capture":
        capture(arguments.ci_run, arguments.output)
    elif arguments.mode == "seal":
        seal(arguments.inputs, arguments.reports, arguments.output)
    elif arguments.mode == "stage":
        stage(arguments.verify_run, arguments.output)
    else:
        publish(arguments.output)


if __name__ == "__main__":
    main()
