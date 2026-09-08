"""GitHub-only notice finalization preserves original bytes and publisher history."""

import copy
import hashlib
import importlib.util
import json
import sys
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[3]
for name in (
    "validate_release_recovery",
    "verify_pypi_release_hashes",
    "recover_notice_release",
    "finalize_notice_release",
):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts/ci" / f"{name}.py")
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
finalize = sys.modules["finalize_notice_release"]


def publisher():
    manifest = finalize.manifest_from(ROOT / ".github/release/v2.0.2-finalization.json")
    run = copy.deepcopy(manifest["publication"]["run"])
    workflow = run.pop("workflow")
    run.update(
        workflow_id=workflow["id"],
        name=workflow["name"],
        path=workflow["path"],
        repository={"full_name": "ds1sqe/type-bridge"},
        head_repository={"full_name": "ds1sqe/type-bridge"},
    )
    jobs = [
        {
            "id": index,
            "name": name,
            "run_id": run["id"],
            "run_attempt": 1,
            "workflow_name": "Release",
            "status": "completed",
            "head_sha": run["head_sha"],
            "head_branch": run["head_branch"],
            **copy.deepcopy(outcome),
        }
        for index, (name, outcome) in enumerate(manifest["publication"]["jobs"].items(), 1)
    ]
    return manifest, run, {"total_count": len(jobs), "jobs": jobs}


@pytest.mark.parametrize(
    "mutation", [None, "source", "rerun", "fork", "missing", "duplicate", "step"]
)
def test_only_exact_successful_facade_recovery_passes(mutation):
    manifest, run, jobs = publisher()
    if mutation == "source":
        run["head_sha"] = "a" * 40
    elif mutation == "rerun":
        run["run_attempt"] = 2
    elif mutation == "fork":
        run["head_repository"]["full_name"] = "other/type-bridge"
    elif mutation == "missing":
        jobs["jobs"].pop()
    elif mutation == "duplicate":
        jobs["jobs"][1] = jobs["jobs"][0]
    elif mutation == "step":
        next(job for job in jobs["jobs"] if job["steps"])["steps"].pop()
    if mutation is None:
        finalize.validate_publisher(manifest, run, jobs)
    else:
        with pytest.raises(finalize.recovery.ValidationError):
            finalize.validate_publisher(manifest, run, jobs)


def test_notice_body_and_asset_authority_are_frozen(tmp_path, monkeypatch):
    manifest, _, _ = publisher()
    assert len(manifest["assets"]) == 13

    def original_notice(command, *, cwd):
        assert cwd == ROOT
        assert command == ["git", "show", f"{finalize.recovery.SOURCE}:docs/guide/v2.0.2-notice.md"]
        return (ROOT / "docs/guide/v2.0.2-notice.md").read_bytes()

    # Unit checks also run in shallow checkouts; the real finalizer fetches
    # full history and reads this exact original source, verified above.
    monkeypatch.setattr(finalize.subprocess, "check_output", original_notice)
    body = finalize.body_from(ROOT, manifest)
    assert hashlib.sha256(body.encode()).hexdigest() == manifest["body_sha256"]
    assert "## Exact Public Inventory\n" in body
    assert "No previously successful registry lane was republished" in body
    manifest["assets"]["recovery-promotion.json"]["sha256"] = "a" * 64
    path = tmp_path / "manifest.json"
    path.write_text(json.dumps(manifest))
    with pytest.raises(finalize.recovery.ValidationError):
        finalize.manifest_from(path)


@pytest.mark.parametrize("mutation", [None, "body", "extra", "bytes", "symlink"])
def test_packet_rejects_drift(tmp_path, mutation):
    assets = tmp_path / "assets"
    assets.mkdir()
    (assets / "file.whl").write_bytes(b"original")
    (tmp_path / "body.md").write_bytes(b"notice")
    manifest = {
        "assets": {
            "file.whl": {"size_in_bytes": 8, "sha256": hashlib.sha256(b"original").hexdigest()}
        },
        "body_sha256": hashlib.sha256(b"notice").hexdigest(),
    }
    if mutation == "body":
        (tmp_path / "body.md").write_bytes(b"changed")
    elif mutation == "extra":
        (assets / "extra.whl").write_bytes(b"original")
    elif mutation == "bytes":
        (assets / "file.whl").write_bytes(b"modified")
    elif mutation == "symlink":
        (assets / "file.whl").unlink()
        (assets / "file.whl").symlink_to(tmp_path / "body.md")
    if mutation is None:
        finalize.validate_packet(tmp_path, manifest)
    else:
        with pytest.raises(finalize.recovery.ValidationError):
            finalize.validate_packet(tmp_path, manifest)


def test_finalization_is_verify_default_and_has_no_registry_writes():
    workflow = yaml.load(
        (ROOT / ".github/workflows/release.yml").read_text(), Loader=yaml.BaseLoader
    )
    inputs = workflow["on"]["workflow_dispatch"]["inputs"]
    assert inputs["notice_finalize_mode"]["default"] == "verify"
    jobs = workflow["jobs"]
    assert "inputs.release_channel != 'notice-finalize'" in jobs["test"]["if"]
    for key in ("notice-finalize-verify", "notice-finalize-write"):
        job = jobs[key]
        assert "inputs.release_channel == 'notice-finalize'" in job["if"]
        assert "github.ref == 'refs/heads/release/2.0.2-notice'" in job["if"]
        assert "id-token" not in job["permissions"] and "packages" not in job["permissions"]
        assert "pypa/" not in json.dumps(job) and "npm publish" not in json.dumps(job)
    assert jobs["notice-finalize-verify"]["permissions"]["contents"] == "read"
    write = jobs["notice-finalize-write"]
    assert "inputs.notice_finalize_verify_run_id != ''" in write["if"]
    publish = next(step for step in write["steps"] if "gh release edit" in step.get("run", ""))
    assert publish["if"] == "inputs.notice_finalize_mode == 'publish'"
    assert publish["run"].index("--verify-draft") < publish["run"].index("gh release edit")
