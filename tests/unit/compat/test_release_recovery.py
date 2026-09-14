"""Reject changed acceptance, publication failures, and tag identities during recovery."""

import hashlib
import importlib.util
import json
import zipfile
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location(
    "validate_release_recovery", ROOT / "scripts/ci/validate_release_recovery.py"
)
assert spec and spec.loader
validator = importlib.util.module_from_spec(spec)
spec.loader.exec_module(validator)


def accepted_state():
    policy = json.loads(validator.POLICY.read_text())
    repository = {"full_name": validator.REPOSITORY, "id": 1085407082}
    run = {
        "id": policy["run_id"],
        "run_attempt": 1,
        "head_sha": policy["source"],
        "head_branch": "v2.2.1",
        "path": ".github/workflows/release.yml",
        "event": "push",
        "status": "completed",
        "conclusion": "failure",
        "repository": repository,
        "head_repository": repository.copy(),
    }
    jobs = {
        "total_count": len(policy["jobs"]),
        "jobs": [
            {
                "id": index,
                "name": name,
                "conclusion": conclusion,
                "run_id": policy["run_id"],
                "run_attempt": 1,
                "head_sha": policy["source"],
                "status": "completed",
                "steps": [
                    {"name": step, "conclusion": result, "status": "completed"}
                    for step, result in policy["steps"][name].items()
                ],
            }
            for index, (name, conclusion) in enumerate(policy["jobs"].items())
        ],
    }
    reference = {"ref": "refs/tags/v2.2.1", "object": {"type": "tag", "sha": policy["tag_object"]}}
    tag = {
        "sha": policy["tag_object"],
        "tag": "v2.2.1",
        "object": {"type": "commit", "sha": policy["source"]},
    }
    return policy, run, jobs, reference, tag


def test_original_partial_publication_is_admitted():
    validator.validate(*accepted_state())


@pytest.mark.parametrize(
    "field,value",
    [
        ("head_sha", "0" * 40),
        ("run_attempt", 2),
        ("event", "workflow_dispatch"),
        ("status", "in_progress"),
        ("conclusion", "success"),
        ("id", 1),
    ],
)
def test_wrong_run_is_rejected(field, value):
    state = accepted_state()
    state[1][field] = value
    with pytest.raises(validator.RecoveryError):
        validator.validate(*state)


def test_missing_acceptance_job_is_rejected():
    state = accepted_state()
    state[2]["jobs"].pop()
    with pytest.raises(validator.RecoveryError):
        validator.validate(*state)


def test_different_oci_failure_is_rejected():
    state = accepted_state()
    job = next(j for j in state[2]["jobs"] if j["conclusion"] == "failure")
    step = next(s for s in job["steps"] if s["name"] == "Publish exact accepted platform manifests")
    step["conclusion"] = "failure"
    with pytest.raises(validator.RecoveryError, match="Step result drift"):
        validator.validate(*state)


@pytest.mark.parametrize("target", [3, 4])
def test_moved_tag_is_rejected(target):
    state = accepted_state()
    state[target]["object"]["sha"] = "0" * 40
    with pytest.raises(validator.RecoveryError):
        validator.validate(*state)


@pytest.mark.parametrize("mutation", ["digest", "id", "run", "source", "expired", "missing"])
def test_replaced_or_unavailable_artifact_is_rejected(mutation):
    policy = accepted_state()[0]
    artifacts = [
        {
            "name": name,
            **record,
            "expired": False,
            "workflow_run": {"id": policy["run_id"], "head_sha": policy["source"]},
        }
        for name, record in policy["artifacts"].items()
    ]
    snapshot = {"total_count": len(artifacts), "artifacts": artifacts}
    validator.validate_artifacts(policy, snapshot)
    first = artifacts[0]
    if mutation == "digest":
        first["digest"] = "sha256:" + "0" * 64
    elif mutation == "id":
        first["id"] += 1
    elif mutation == "run":
        first["workflow_run"]["id"] += 1
    elif mutation == "source":
        first["workflow_run"]["head_sha"] = "0" * 40
    elif mutation == "expired":
        first["expired"] = True
    else:
        artifacts.pop()
    with pytest.raises(validator.RecoveryError):
        validator.validate_artifacts(policy, snapshot)


def test_downloaded_bytes_must_match_original_archive(tmp_path):
    archive = tmp_path / "original.zip"
    with zipfile.ZipFile(archive, "w") as bundle:
        bundle.writestr("package.whl", b"accepted")
    expected = {
        "size_in_bytes": archive.stat().st_size,
        "digest": "sha256:" + hashlib.sha256(archive.read_bytes()).hexdigest(),
    }
    directory = tmp_path / "dist"
    directory.mkdir()
    package = directory / "package.whl"
    package.write_bytes(b"accepted")
    assert validator.verify_download(archive, expected, directory) == {"package.whl"}
    package.write_bytes(b"modified")
    with pytest.raises(validator.RecoveryError, match="digest drift"):
        validator.verify_download(archive, expected, directory)
    package.write_bytes(b"accepted")
    expected["digest"] = "sha256:" + "0" * 64
    with pytest.raises(validator.RecoveryError, match="Archive digest drift"):
        validator.verify_download(archive, expected, directory)


@pytest.mark.parametrize("extra", ["extra.whl", "nested/extra.whl"])
def test_extra_publisher_inputs_are_rejected(tmp_path, extra):
    (tmp_path / "accepted.whl").write_bytes(b"accepted")
    validator.verify_inventory(tmp_path, {"accepted.whl"})
    unexpected = tmp_path / extra
    unexpected.parent.mkdir(parents=True, exist_ok=True)
    unexpected.write_bytes(b"unexpected")
    with pytest.raises(validator.RecoveryError, match="inventory drift"):
        validator.verify_inventory(tmp_path, {"accepted.whl"})


def test_linked_publisher_input_is_rejected(tmp_path):
    (tmp_path / "accepted.whl").write_bytes(b"accepted")
    (tmp_path / "linked.whl").symlink_to(tmp_path / "accepted.whl")
    with pytest.raises(validator.RecoveryError, match="Linked publisher input"):
        validator.verify_inventory(tmp_path, {"accepted.whl", "linked.whl"})


def recovery_environment(policy):
    return {
        "SERVER_OCI_DIGEST": policy["oci_digests"]["index"],
        "SERVER_OCI_AMD64_DIGEST": policy["oci_digests"]["linux/amd64"],
        "SERVER_OCI_ARM64_DIGEST": policy["oci_digests"]["linux/arm64"],
        "GITHUB_REPOSITORY": validator.REPOSITORY,
        "GITHUB_REF": "refs/heads/master",
        "GITHUB_EVENT_NAME": "workflow_dispatch",
        "GITHUB_SHA": "a" * 40,
        "GITHUB_RUN_ID": "123",
        "GITHUB_RUN_ATTEMPT": "1",
    }


def test_recovery_attestation_distinguishes_build_and_promotion():
    policy = accepted_state()[0]
    predicate = validator.recovery_predicate(policy, recovery_environment(policy))
    assert predicate["release"]["source"] == policy["source"]
    assert predicate["originalBuild"]["runId"] == policy["run_id"]
    assert predicate["originalBuild"]["artifacts"] == policy["artifacts"]
    assert predicate["recovery"]["source"] == "a" * 40
    assert predicate["recovery"]["runId"] == "123"


@pytest.mark.parametrize(
    "field",
    [
        "SERVER_OCI_DIGEST",
        "SERVER_OCI_AMD64_DIGEST",
        "SERVER_OCI_ARM64_DIGEST",
        "GITHUB_REPOSITORY",
        "GITHUB_REF",
        "GITHUB_EVENT_NAME",
    ],
)
def test_recovery_attestation_rejects_changed_identity(field):
    policy = accepted_state()[0]
    environment = recovery_environment(policy)
    environment[field] = "changed"
    with pytest.raises(validator.RecoveryError):
        validator.recovery_predicate(policy, environment)


def test_recovery_workflow_preserves_publication_dependencies():
    import yaml

    workflow = yaml.safe_load((ROOT / ".github/workflows/release.yml").read_text())
    jobs = workflow["jobs"]
    assert jobs["test"]["if"] == "${{ !inputs.recover_publication }}"
    admission = (
        "inputs.recover_publication && github.ref == 'refs/heads/master' && "
        "inputs.release_channel == 'stable'"
    )
    assert jobs["recovery-preflight"]["if"] == admission
    previous = {
        "release-tag-preflight": None,
        "publish-server-oci": "release-tag-preflight",
        "publish-core-pypi": "publish-server-oci",
        "publish-python-pypi": "publish-core-pypi",
        "github-release": "publish-python-pypi",
    }
    for name, prerequisite in previous.items():
        job = jobs[name]
        recovery_guard = job["if"].split(" || ")[1]
        assert admission in recovery_guard
        assert "needs.recovery-preflight.result == 'success'" in recovery_guard
        assert "recovery-preflight" in job["needs"]
        if prerequisite:
            assert prerequisite in job["needs"]
            assert f"needs.{prerequisite}.result == 'success'" in recovery_guard
            assert "needs.release-tag-preflight.result == 'success'" in recovery_guard
    for name in ["publish-node-npm", "publish-crates"]:
        assert "github.event_name == 'push'" in jobs[name]["if"]
        assert "||" not in jobs[name]["if"]
    for name in list(previous)[1:]:
        steps = jobs[name]["steps"]
        checks = [
            step for step in steps if step.get("name") == "Verify original recovery artifact bytes"
        ]
        assert len(checks) == 1
        assert checks[0]["if"] == "inputs.recover_publication"
        assert "--check " in checks[0]["run"]
        assert steps.index(checks[0]) < next(
            i
            for i, step in enumerate(steps)
            if step.get("name") == "Revalidate immutable release tag"
        )
    steps = jobs["publish-server-oci"]["steps"]
    for step in steps:
        if step.get("id", "").startswith("attest-provenance-"):
            assert step["if"] == "${{ !inputs.recover_publication }}"
        if step.get("id", "").startswith("attest-recovery-"):
            assert step["if"] == "inputs.recover_publication"
            assert step["with"]["predicate-type"].endswith("publication-recovery/v1")
