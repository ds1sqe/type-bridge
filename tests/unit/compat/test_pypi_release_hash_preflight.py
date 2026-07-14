"""Hostile tests for idempotent PyPI filename/hash preflights."""

from __future__ import annotations

import hashlib
import importlib.util
import sys
from pathlib import Path
from types import ModuleType

import pytest

ROOT = Path(__file__).resolve().parents[3]
HELPER_PATH = ROOT / "scripts/ci/verify_pypi_release_hashes.py"


def load_module(name: str, path: Path) -> ModuleType:
    """Load one standalone CI helper without adding scripts to the package."""
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


preflight = load_module("verify_pypi_release_hashes", HELPER_PATH)


def write_distribution(tmp_path: Path, name: str, payload: bytes) -> Path:
    """Write one local release candidate."""
    path = tmp_path / name
    path.write_bytes(payload)
    return path


def remote_payload(filename: str, digest: str) -> dict[str, object]:
    """Return a minimal release-specific PyPI JSON response."""
    return {"urls": [{"filename": filename, "digests": {"sha256": digest}}]}


def test_identical_existing_filename_is_safe_to_skip(tmp_path: Path) -> None:
    artifact = write_distribution(tmp_path, "type_bridge-1.5.7.tar.gz", b"same bytes")
    digest = hashlib.sha256(artifact.read_bytes()).hexdigest()

    report = preflight.verify_release_hashes(
        {artifact.name: artifact},
        remote_payload(artifact.name, digest),
    )

    assert report["status"] == "ok"
    assert report["artifacts"][0]["status"] == "already-published-identical"


def test_same_filename_with_different_bytes_hard_fails(tmp_path: Path) -> None:
    artifact = write_distribution(tmp_path, "type_bridge-1.5.7.tar.gz", b"local bytes")
    remote_digest = hashlib.sha256(b"hostile remote bytes").hexdigest()

    with pytest.raises(preflight.VerificationError, match="different bytes"):
        preflight.verify_release_hashes(
            {artifact.name: artifact},
            remote_payload(artifact.name, remote_digest),
        )


def test_missing_remote_filename_remains_publishable(tmp_path: Path) -> None:
    artifact = write_distribution(
        tmp_path,
        "type_bridge-1.5.7-py3-none-any.whl",
        b"new wheel",
    )

    report = preflight.verify_release_hashes({artifact.name: artifact}, {"urls": []})

    assert report["artifacts"][0]["status"] == "new"


@pytest.mark.parametrize("require_existing", [False, True])
def test_remote_extra_filename_breaks_exact_release_inventory(
    tmp_path: Path,
    require_existing: bool,
) -> None:
    artifact = write_distribution(tmp_path, "type_bridge-1.5.7.tar.gz", b"same bytes")
    digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
    payload = {
        "urls": [
            {"filename": artifact.name, "digests": {"sha256": digest}},
            {"filename": "hostile-extra.whl", "digests": {"sha256": "a" * 64}},
        ]
    }

    with pytest.raises(preflight.VerificationError, match="outside the exact local candidate set"):
        preflight.verify_release_hashes(
            {artifact.name: artifact},
            payload,
            require_existing=require_existing,
        )


def test_post_publish_verification_requires_every_candidate(tmp_path: Path) -> None:
    artifact = write_distribution(
        tmp_path,
        "type_bridge-1.5.7-py3-none-any.whl",
        b"new wheel",
    )

    with pytest.raises(preflight.ReleaseNotVisibleError, match="not exposed"):
        preflight.verify_release_hashes(
            {artifact.name: artifact},
            {"urls": []},
            require_existing=True,
        )


def test_post_publish_verification_retries_visibility_only(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    artifact = write_distribution(tmp_path, "type_bridge-1.5.7.tar.gz", b"same bytes")
    digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
    responses = iter([None, remote_payload(artifact.name, digest)])
    calls = 0

    def fake_fetch(repository_url: str, project: str, version: str) -> dict[str, object] | None:
        nonlocal calls
        assert (repository_url, project, version) == ("https://pypi.org", "type-bridge", "1.5.7")
        calls += 1
        return next(responses)

    monkeypatch.setattr(preflight, "fetch_release_json", fake_fetch)

    report = preflight.verify_remote_release(
        local_files={artifact.name: artifact},
        repository_url="https://pypi.org",
        project="type-bridge",
        version="1.5.7",
        require_existing=True,
        attempts=2,
        retry_delay_seconds=0,
    )

    assert calls == 2
    assert report["artifacts"][0]["status"] == "already-published-identical"


@pytest.mark.parametrize(
    "payload",
    [
        {"urls": [{"filename": "duplicate.whl", "digests": {"sha256": "a" * 64}}] * 2},
        {"urls": [{"filename": "bad.whl", "digests": {"sha256": "not-a-digest"}}]},
        {"urls": "not-a-list"},
    ],
)
def test_malformed_remote_release_metadata_hard_fails(payload: dict[str, object]) -> None:
    with pytest.raises(preflight.VerificationError):
        preflight.remote_hashes(payload)


def test_distribution_directory_rejects_unexpected_files(tmp_path: Path) -> None:
    write_distribution(tmp_path, "type_bridge-1.5.7.tar.gz", b"sdist")
    write_distribution(tmp_path, "manifest.json", b"not publishable")

    with pytest.raises(preflight.VerificationError, match="Unexpected"):
        preflight.distribution_files(tmp_path)


def test_release_url_quotes_project_and_version() -> None:
    assert preflight.release_url("https://pypi.org/", "type bridge", "1.5.7+local") == (
        "https://pypi.org/pypi/type%20bridge/1.5.7%2Blocal/json"
    )
