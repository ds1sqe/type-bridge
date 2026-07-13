#!/usr/bin/env python3
"""Verify idempotent PyPI uploads by comparing same-filename SHA-256s."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from collections.abc import Sequence
from pathlib import Path
from typing import Any

DEFAULT_REPOSITORY_URL = "https://pypi.org"
MAX_RESPONSE_BYTES = 10 * 1024 * 1024


class VerificationError(RuntimeError):
    """A PyPI preflight could not prove an idempotent upload is byte-identical."""


class ReleaseNotVisibleError(VerificationError):
    """A post-publish candidate is not yet visible through PyPI's JSON API."""


def sha256_path(path: Path) -> str:
    """Hash one distribution without loading it into memory."""
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def distribution_files(directory: Path) -> dict[str, Path]:
    """Return one non-empty flat wheel/sdist directory."""
    if not directory.is_dir() or directory.is_symlink():
        raise VerificationError(f"Distribution directory is missing or unsafe: {directory}")
    files: dict[str, Path] = {}
    for path in sorted(directory.iterdir(), key=lambda candidate: candidate.name):
        if not path.is_file() or path.is_symlink() or not path.name.endswith((".whl", ".tar.gz")):
            raise VerificationError(f"Unexpected distribution directory entry: {path}")
        files[path.name] = path
    if not files:
        raise VerificationError(f"Distribution directory is empty: {directory}")
    return files


def release_url(repository_url: str, project: str, version: str) -> str:
    """Build the release-specific PyPI JSON API URL."""
    if not project or not version:
        raise VerificationError("PyPI project and version must be non-empty")
    base = repository_url.rstrip("/")
    return (
        f"{base}/pypi/{urllib.parse.quote(project, safe='')}/"
        f"{urllib.parse.quote(version, safe='')}/json"
    )


def fetch_release_json(
    repository_url: str,
    project: str,
    version: str,
) -> dict[str, Any] | None:
    """Fetch one PyPI release; return None only for an authoritative 404."""
    url = release_url(repository_url, project, version)
    request = urllib.request.Request(
        url,
        headers={
            "Accept": "application/json",
            "User-Agent": "ds1sqe/type-bridge release hash preflight",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            payload = response.read(MAX_RESPONSE_BYTES + 1)
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return None
        raise VerificationError(f"PyPI JSON request failed ({error.code}) for {url}") from error
    except (OSError, urllib.error.URLError) as error:
        raise VerificationError(f"PyPI JSON request failed for {url}: {error}") from error
    if len(payload) > MAX_RESPONSE_BYTES:
        raise VerificationError(f"PyPI JSON response exceeded size limit for {url}")
    try:
        parsed = json.loads(payload.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise VerificationError(f"PyPI returned invalid JSON for {url}: {error}") from error
    if not isinstance(parsed, dict):
        raise VerificationError(f"PyPI returned a non-object JSON response for {url}")
    return parsed


def remote_hashes(payload: dict[str, Any] | None) -> dict[str, str]:
    """Return the release's unique filename-to-SHA256 mapping."""
    if payload is None:
        return {}
    urls = payload.get("urls")
    if not isinstance(urls, list):
        raise VerificationError("PyPI release JSON has no urls list")
    hashes: dict[str, str] = {}
    for item in urls:
        if not isinstance(item, dict):
            raise VerificationError("PyPI release JSON contains a non-object file entry")
        filename = item.get("filename")
        digests = item.get("digests")
        digest = digests.get("sha256") if isinstance(digests, dict) else None
        if not isinstance(filename, str) or not filename:
            raise VerificationError("PyPI release JSON contains an invalid filename")
        if filename in hashes:
            raise VerificationError(f"PyPI release JSON repeats filename: {filename}")
        if not isinstance(digest, str) or re.fullmatch(r"[0-9a-fA-F]{64}", digest) is None:
            raise VerificationError(f"PyPI release JSON has no valid SHA256 for {filename}")
        hashes[filename] = digest.lower()
    return hashes


def verify_release_hashes(
    local_files: dict[str, Path],
    payload: dict[str, Any] | None,
    *,
    require_existing: bool = False,
) -> dict[str, Any]:
    """Hard-fail any same-name remote file whose bytes differ."""
    published = remote_hashes(payload)
    unexpected_remote = sorted(published.keys() - local_files.keys())
    if unexpected_remote:
        raise VerificationError(
            "PyPI release contains files outside the exact local candidate set: "
            f"{unexpected_remote}"
        )
    artifacts: list[dict[str, Any]] = []
    missing: list[str] = []
    for filename, path in sorted(local_files.items()):
        local_digest = sha256_path(path)
        remote_digest = published.get(filename)
        if remote_digest is not None and remote_digest != local_digest:
            raise VerificationError(
                f"PyPI already has different bytes for {filename}: "
                f"remote={remote_digest}, local={local_digest}"
            )
        if remote_digest is None:
            missing.append(filename)
        artifacts.append(
            {
                "filename": filename,
                "sha256": local_digest,
                "status": "already-published-identical" if remote_digest else "new",
            }
        )
    if require_existing and missing:
        raise ReleaseNotVisibleError(
            f"PyPI has not exposed the published candidates yet: {missing}"
        )
    return {"artifacts": artifacts, "status": "ok"}


def verify_remote_release(
    *,
    local_files: dict[str, Path],
    repository_url: str,
    project: str,
    version: str,
    require_existing: bool,
    attempts: int,
    retry_delay_seconds: float,
) -> dict[str, Any]:
    """Fetch and verify a release, retrying only post-publish visibility lag."""
    if attempts < 1:
        raise VerificationError("Verification attempts must be at least one")
    if retry_delay_seconds < 0:
        raise VerificationError("Retry delay must be non-negative")
    for attempt in range(1, attempts + 1):
        payload = fetch_release_json(repository_url, project, version)
        try:
            return verify_release_hashes(
                local_files,
                payload,
                require_existing=require_existing,
            )
        except ReleaseNotVisibleError:
            if attempt == attempts:
                raise
            time.sleep(retry_delay_seconds)
    raise AssertionError("unreachable verification retry loop")


def build_parser() -> argparse.ArgumentParser:
    """Build the PyPI preflight CLI."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--dist-dir", type=Path, required=True)
    parser.add_argument("--repository-url", default=DEFAULT_REPOSITORY_URL)
    parser.add_argument("--require-existing", action="store_true")
    parser.add_argument("--attempts", type=int, default=1)
    parser.add_argument("--retry-delay-seconds", type=float, default=0)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Fetch remote hashes, compare local candidates, and print a JSON report."""
    args = build_parser().parse_args(argv)
    files = distribution_files(args.dist_dir.resolve())
    report = verify_remote_release(
        local_files=files,
        repository_url=args.repository_url,
        project=args.project,
        version=args.version,
        require_existing=args.require_existing,
        attempts=args.attempts,
        retry_delay_seconds=args.retry_delay_seconds,
    )
    report["project"] = args.project
    report["version"] = args.version
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except VerificationError as error:
        print(f"PyPI release hash verification failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
