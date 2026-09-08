"""Reject substituted sources, incomplete acceptance and changed C release bytes."""

from __future__ import annotations

import copy
import hashlib
import importlib
import io
import json
import stat
import sys
import tomllib
import zipfile
from pathlib import Path
from typing import Any

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "scripts/ci"))
promotion = importlib.import_module("promote_c_release")
policy = importlib.import_module("c_release_policy")
SOURCE = "a" * 40
TREE = "b" * 40


def test_saved_consumer_locks_use_current_first_party_versions() -> None:
    packages = [
        tomllib.loads(path.read_text())["package"]
        for path in (ROOT / "type-bridge-core/crates").glob("*/Cargo.toml")
    ]
    versions = {package["name"]: package["version"] for package in packages}
    locks = [
        *ROOT.glob("tests/contracts/*Cargo.lock"),
        *ROOT.glob("tests/support/*/Cargo.lock"),
        *ROOT.glob("type-bridge-core/crates/**/tests/**/*Cargo.lock"),
    ]
    assert len(locks) == 6
    for path in locks:
        for package in tomllib.loads(path.read_text())["package"]:
            if package["name"] in versions and "source" not in package:
                assert package["version"] == versions[package["name"]], (path, package["name"])


def run_snapshot() -> dict[str, Any]:
    return {
        "id": 123,
        "head_sha": SOURCE,
        "path": ".github/workflows/ci.yml",
        "event": "push",
        "head_branch": "master",
        "run_attempt": 1,
        "status": "completed",
        "conclusion": "success",
        "repository": {"full_name": "ds1sqe/type-bridge", "id": 1085407082},
        "head_repository": {"full_name": "ds1sqe/type-bridge", "id": 1085407082},
    }


def test_selected_policy_matches_every_current_ci_step_and_workflow() -> None:
    selected = policy.selected_policy()
    assert json.loads(policy.POLICY.read_text()) == selected
    assert len(selected["ci_jobs"]) == 52
    assert (
        selected["ci_jobs"]["Rust windows-latest"]["Check C foundation on MSRV 1.88"] == "skipped"
    )
    assert selected["ci_jobs"]["Rust ubuntu-latest"]["Check C foundation on MSRV 1.88"] == "success"
    assert (
        selected["ci_jobs"]["Python Integration (schema, typedb/typedb:3.12.3)"][
            "Upload Python sdk report"
        ]
        == "success"
    )
    assert (
        selected["ci_jobs"]["Python Integration (schema, typedb/typedb:3.11.5)"][
            "Upload Python sdk report"
        ]
        == "skipped"
    )


@pytest.mark.parametrize(
    ("key", "value"),
    [
        ("id", 124),
        ("id", True),
        ("head_sha", "c" * 40),
        ("head_branch", "develop"),
        ("path", ".github/workflows/release.yml"),
        ("event", "pull_request"),
        ("run_attempt", 2),
        ("status", "in_progress"),
        ("conclusion", "failure"),
        ("repository", {"full_name": "ds1sqe/type-bridge", "id": 1}),
        ("head_repository", {"full_name": "attacker/type-bridge", "id": 1085407082}),
    ],
)
def test_source_run_rejects_identity_and_acceptance_changes(key: str, value: Any) -> None:
    run = run_snapshot()
    promotion.validate_run(
        run, run_id=123, source=SOURCE, workflow=".github/workflows/ci.yml", event="push"
    )
    run[key] = value
    with pytest.raises(promotion.PromotionError):
        promotion.validate_run(
            run, run_id=123, source=SOURCE, workflow=".github/workflows/ci.yml", event="push"
        )


def jobs_snapshot() -> dict[str, Any]:
    return {
        "total_count": 2,
        "jobs": [
            {
                "id": 1,
                "name": "Verify",
                "run_id": 123,
                "run_attempt": 1,
                "head_sha": SOURCE,
                "status": "completed",
                "conclusion": "success",
                "steps": [
                    {"name": "FULL audit", "status": "completed", "conclusion": "success"},
                    {"name": "Failure logs", "status": "completed", "conclusion": "skipped"},
                    {"name": "Complete job", "status": "completed", "conclusion": "success"},
                ],
            },
            {
                "id": 2,
                "name": "Publish",
                "run_id": 123,
                "run_attempt": 1,
                "head_sha": SOURCE,
                "status": "completed",
                "conclusion": "skipped",
                "steps": [],
            },
        ],
    }


def validate_jobs(snapshot: dict[str, Any]) -> None:
    promotion.validate_jobs(
        snapshot,
        run_id=123,
        source=SOURCE,
        expected={"Verify": {"FULL audit": "success", "Failure logs": "skipped"}},
        skipped={"Publish"},
    )


@pytest.mark.parametrize(
    "change",
    [
        "missing-job",
        "missing-step",
        "skipped-audit",
        "failed-post",
        "duplicate-job",
        "duplicate-step",
        "publisher-ran",
        "wrong-source",
        "rerun",
        "truncated",
    ],
)
def test_job_acceptance_fails_closed(change: str) -> None:
    snapshot = jobs_snapshot()
    validate_jobs(snapshot)
    job = snapshot["jobs"][0]
    if change == "missing-job":
        snapshot["jobs"].pop()
    elif change == "missing-step":
        job["steps"].pop(0)
    elif change == "skipped-audit":
        job["steps"][0]["conclusion"] = "skipped"
    elif change == "failed-post":
        job["steps"][-1]["conclusion"] = "failure"
    elif change == "duplicate-job":
        snapshot["jobs"][1]["id"] = 1
    elif change == "duplicate-step":
        job["steps"].append(copy.deepcopy(job["steps"][0]))
    elif change == "publisher-ran":
        snapshot["jobs"][1]["conclusion"] = "success"
    elif change == "wrong-source":
        job["head_sha"] = "c" * 40
    elif change == "rerun":
        job["run_attempt"] = 2
    else:
        snapshot["total_count"] = 3
    with pytest.raises(promotion.PromotionError):
        validate_jobs(snapshot)


def archive(
    tmp_path: Path, *, member: str = "payload.tar.gz", mode: int = stat.S_IFREG
) -> tuple[Path, dict[str, Any]]:
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as bundle:
        entry = zipfile.ZipInfo(member)
        entry.external_attr = (mode | 0o644) << 16
        bundle.writestr(entry, b"accepted bytes")
    path = tmp_path / "artifact.zip"
    path.write_bytes(buffer.getvalue())
    return path, {
        "size_in_bytes": path.stat().st_size,
        "digest": "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest(),
    }


def test_actions_archive_is_verified_before_extraction(tmp_path: Path) -> None:
    path, metadata = archive(tmp_path)
    promotion.extract(path, metadata, ["payload.tar.gz"], tmp_path / "extracted")
    assert (tmp_path / "extracted/payload.tar.gz").read_bytes() == b"accepted bytes"
    metadata["digest"] = "sha256:" + "0" * 64
    with pytest.raises(promotion.PromotionError, match="digest"):
        promotion.extract(path, metadata, ["payload.tar.gz"], tmp_path / "rejected")
    assert not (tmp_path / "rejected").exists()


@pytest.mark.parametrize(
    ("member", "mode"),
    [
        ("../payload.tar.gz", stat.S_IFREG),
        ("/payload.tar.gz", stat.S_IFREG),
        ("payload.tar.gz", stat.S_IFLNK),
        ("payload.tar.gz", stat.S_IFIFO),
    ],
)
def test_actions_archive_rejects_path_escape_and_special_files(
    tmp_path: Path, member: str, mode: int
) -> None:
    path, metadata = archive(tmp_path, member=member, mode=mode)
    with pytest.raises(promotion.PromotionError):
        promotion.extract(path, metadata, ["payload.tar.gz"], tmp_path / "rejected")
    assert not (tmp_path / "rejected").exists()


def accepted_receipt(tmp_path: Path) -> dict[str, Any]:
    names = sorted([*policy.PUBLIC_FILES.values(), policy.EVIDENCE])
    for name in names:
        (tmp_path / name).write_bytes(name.encode())
    receipt = {
        "format": "typebridge.c-release-verification/v1",
        "status": "accepted-full-c-artifacts",
        "source": SOURCE,
        "tree": TREE,
        "verification_run": 123,
        "verification_attempt": 1,
        "policy_sha256": promotion.digest(policy.POLICY),
        "public_name_mapping": policy.PUBLIC_FILES,
        "publication_authority": False,
        "files": [promotion.record(tmp_path / name) for name in names],
    }
    (tmp_path / policy.RECEIPT).write_bytes(promotion.canonical(receipt))
    return receipt


@pytest.mark.parametrize(
    "change",
    [
        "bytes",
        "extra-file",
        "source",
        "tree",
        "verification_run",
        "verification_attempt",
        "policy_sha256",
        "publication_authority",
    ],
)
def test_verified_bundle_rejects_substitution(tmp_path: Path, change: str) -> None:
    receipt = accepted_receipt(tmp_path)
    promotion.validate_receipt(tmp_path, source=SOURCE, tree=TREE, verify_run=123)
    if change == "bytes":
        (tmp_path / policy.EVIDENCE).write_bytes(b"rebuilt")
    elif change == "extra-file":
        (tmp_path / "unselected").write_bytes(b"extra")
    else:
        receipt[change] = True if change == "publication_authority" else "different"
        (tmp_path / policy.RECEIPT).write_bytes(promotion.canonical(receipt))
    with pytest.raises(promotion.PromotionError):
        promotion.validate_receipt(tmp_path, source=SOURCE, tree=TREE, verify_run=123)


def test_tag_requires_an_annotated_direct_same_source_target() -> None:
    reference = {"ref": "refs/tags/v2.2.0", "object": {"type": "tag", "sha": "c" * 40}}
    tag = {
        "sha": "c" * 40,
        "tag": "v2.2.0",
        "object": {
            "type": "commit",
            "sha": SOURCE,
            "url": f"https://api.github.com/repos/ds1sqe/type-bridge/git/commits/{SOURCE}",
        },
    }
    assert promotion.validate_tag(reference, tag, SOURCE) == "c" * 40
    tag["object"]["sha"] = "d" * 40
    with pytest.raises(promotion.PromotionError, match="source"):
        promotion.validate_tag(reference, tag, SOURCE)
    reference["object"]["type"] = "commit"
    with pytest.raises(promotion.PromotionError, match="lightweight"):
        promotion.validate_tag(reference, tag, SOURCE)


def test_workflow_keeps_publish_protected_and_without_builders() -> None:
    workflow = yaml.load((ROOT / policy.WORKFLOW).read_text(), Loader=yaml.BaseLoader)
    assert set(workflow["on"]) == {"workflow_dispatch"}
    assert workflow["permissions"] == {"contents": "read", "actions": "read"}
    publish = workflow["jobs"]["publish"]
    assert publish["environment"] == "release"
    assert publish["if"] == "inputs.mode == 'publish' && github.ref == 'refs/tags/v2.2.0'"
    assert publish["permissions"] == {"contents": "write", "actions": "read", "id-token": "write"}
    assert all(job["runs-on"] == "ubuntu-24.04" for job in workflow["jobs"].values())
    source = json.dumps(publish)
    for forbidden in (
        "cargo ",
        "npm ",
        "uv ",
        " seal ",
        " capture ",
        "--clobber",
        "release create",
        "release edit",
    ):
        assert forbidden not in source
    assert "--verify-run" in source
    assert policy.selected_policy()["verification_steps"] == {
        step["name"]: "success" for step in workflow["jobs"]["verify"]["steps"]
    }


def test_sealing_preserves_all_accepted_archive_bytes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    audit = importlib.import_module("audit_full_c_artifact")
    monkeypatch.setattr(promotion, "control", lambda: (SOURCE, TREE, policy.selected_policy()))
    monkeypatch.setenv("GITHUB_RUN_ID", "123")
    inputs = tmp_path / "inputs"
    inputs.mkdir()
    artifacts = {}
    for artifact, names in policy.ARTIFACTS.items():
        directory = inputs / artifact
        directory.mkdir()
        for name in names:
            (directory / name).write_bytes(b"immutable accepted bytes: " + name.encode())
        artifacts[artifact] = {
            "files": [promotion.record(directory / name) for name in sorted(names)]
        }
    for name in ("ci-run.json", "ci-jobs.json", "artifacts.json"):
        (inputs / name).write_text("{}")
    promotion.write(
        inputs / "capture.json",
        {
            "source": SOURCE,
            "tree": TREE,
            "ci_run": 456,
            "artifacts": artifacts,
            "policy_sha256": promotion.digest(policy.POLICY),
        },
    )
    reports = tmp_path / "reports"
    for version, bindings in audit.REPORT_BINDINGS.items():
        (reports / f"v{version}").mkdir(parents=True)
        for binding in bindings:
            (reports / f"v{version}" / f"{binding}.json").write_text("{}")
    calls: list[dict[str, Any]] = []

    def accepted_audit(**arguments: Any) -> tuple[dict[str, Any], str]:
        calls.append(arguments)
        return {
            "source_commit": SOURCE,
            "authority_state": "accepted-artifact",
            "artifact_set_id": "sha256:" + "d" * 64,
        }, "Accepted audit\n"

    monkeypatch.setattr(audit, "audit", accepted_audit)
    output = tmp_path / "sealed"
    promotion.seal(inputs, reports, output)
    assert len(calls) == 1
    assert set(calls[0]) == {
        "reports_root",
        "acceptance_path",
        "provider",
        "live",
        "cli",
        "runtime",
        "generated",
        "security_evidence",
    }
    for original, public in policy.PUBLIC_FILES.items():
        assert (output / public).read_bytes() == b"immutable accepted bytes: " + original.encode()
    promotion.validate_receipt(output, source=SOURCE, tree=TREE, verify_run=123)


def test_conflicting_draft_bytes_stop_before_signing_or_upload(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(promotion, "control", lambda **_: (SOURCE, TREE, policy.selected_policy()))
    payloads = tmp_path / policy.VERIFY_ARTIFACT
    payloads.mkdir()
    accepted_receipt(payloads)
    files = sorted([*policy.PUBLIC_FILES.values(), policy.EVIDENCE, policy.RECEIPT])
    promotion.write(
        payloads / policy.PROMOTION,
        {
            "source": SOURCE,
            "tree": TREE,
            "policy_sha256": promotion.digest(policy.POLICY),
            "tag_object": "c" * 40,
            "files": [promotion.record(payloads / name) for name in files],
        },
    )
    reference = {"ref": "refs/tags/v2.2.0", "object": {"type": "tag", "sha": "c" * 40}}
    tag = {
        "sha": "c" * 40,
        "tag": "v2.2.0",
        "object": {
            "type": "commit",
            "sha": SOURCE,
            "url": f"https://api.github.com/repos/ds1sqe/type-bridge/git/commits/{SOURCE}",
        },
    }
    monkeypatch.setattr(
        promotion, "api", lambda endpoint, _: reference if endpoint.startswith("git/ref/") else tag
    )
    conflicting = b"different accepted destination bytes"
    release = {
        "id": 999,
        "draft": True,
        "prerelease": False,
        "tag_name": "v2.2.0",
        "target_commitish": SOURCE,
        "body": "Ordinary release",
        "assets": [
            {
                "id": 88,
                "name": files[0],
                "size": len(conflicting),
                "digest": "sha256:" + hashlib.sha256(conflicting).hexdigest(),
            }
        ],
    }
    calls: list[list[str]] = []

    def command(arguments: list[str], *, output: Path | None = None) -> str:
        calls.append(arguments)
        assert arguments[:2] == ["gh", "api"]
        if output is None:
            assert "--paginate" in arguments
            return json.dumps([[release]])
        output.write_bytes(conflicting)
        return ""

    monkeypatch.setattr(promotion, "command", command)
    with pytest.raises(promotion.PromotionError, match="bytes conflict"):
        promotion.publish(tmp_path)
    assert len(calls) == 2
