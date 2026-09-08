#!/usr/bin/env python3
"""Preflight or publish an accepted Cargo candidate without rebuilding it."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import sys
import time
import urllib.error
import urllib.request
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

try:
    from cargo_release_candidate import (
        CandidateBundle,
        CandidateError,
        CandidatePackage,
        validate_candidate_bundle,
    )
except ModuleNotFoundError:
    from scripts.ci.cargo_release_candidate import (
        CandidateBundle,
        CandidateError,
        CandidatePackage,
        validate_candidate_bundle,
    )

API_BASE_URL = "https://crates.io"
SPARSE_BASE_URL = "https://index.crates.io"
PUBLISH_ENDPOINT = f"{API_BASE_URL}/api/v1/crates/new"
MAX_RESPONSE_BYTES = 4 * 1024 * 1024
Transport = Callable[[str, str, Mapping[str, str], bytes | None], "HttpResponse"]
Sleeper = Callable[[float], None]


class PublicationError(RuntimeError):
    """The registry state or upload response failed closed."""


@dataclass(frozen=True)
class HttpResponse:
    """One bounded HTTP response used by real and hermetic transports."""

    status: int
    body: bytes


@dataclass(frozen=True)
class AuthorityState:
    """One exact version's state at a crates.io authority."""

    kind: str
    checksum: str | None = None


def urllib_transport(
    method: str,
    url: str,
    headers: Mapping[str, str],
    body: bytes | None,
) -> HttpResponse:
    """Issue one bounded crates.io request without logging credentials."""
    request = urllib.request.Request(
        url,
        data=body,
        headers=dict(headers),
        method=method,
    )
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            response_body = response.read(MAX_RESPONSE_BYTES + 1)
            status = response.status
    except urllib.error.HTTPError as error:
        response_body = error.read(MAX_RESPONSE_BYTES + 1)
        status = error.code
    except (OSError, urllib.error.URLError) as error:
        raise PublicationError(f"registry request failed: {method} {url}: {error}") from error
    if len(response_body) > MAX_RESPONSE_BYTES:
        raise PublicationError(f"registry response exceeded its byte budget: {method} {url}")
    return HttpResponse(status=status, body=response_body)


def sparse_index_path(crate: str) -> str:
    """Return the crates.io sparse-index path for one normalized name."""
    name = crate.lower()
    if re.fullmatch(r"[a-z0-9_-]+", name) is None:
        raise PublicationError(f"invalid crate name for sparse index: {crate!r}")
    if len(name) == 1:
        return f"1/{name}"
    if len(name) == 2:
        return f"2/{name}"
    if len(name) == 3:
        return f"3/{name[0]}/{name}"
    return f"{name[:2]}/{name[2:4]}/{name}"


def _json_object(body: bytes, *, label: str) -> dict[str, Any]:
    try:
        payload = json.loads(body.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise PublicationError(f"{label} returned malformed JSON: {error}") from error
    if not isinstance(payload, dict):
        raise PublicationError(f"{label} did not return a JSON object")
    return payload


def query_api(
    candidate: CandidatePackage,
    *,
    transport: Transport,
) -> AuthorityState:
    """Read the exact version from the crates.io API."""
    package = candidate.package
    url = f"{API_BASE_URL}/api/v1/crates/{package.name}/{package.version}"
    response = transport(
        "GET",
        url,
        {
            "Accept": "application/json",
            "User-Agent": "ds1sqe/type-bridge exact Cargo publisher",
        },
        None,
    )
    if response.status == 404:
        return AuthorityState("absent")
    if response.status != 200:
        raise PublicationError(
            f"crates.io API lookup failed for {package.name}@{package.version}: "
            f"HTTP {response.status}"
        )
    payload = _json_object(response.body, label="crates.io API")
    version = payload.get("version")
    if not isinstance(version, dict):
        raise PublicationError("crates.io API response has no version object")
    identity = (version.get("crate"), version.get("num"))
    if not (
        isinstance(identity[0], str)
        and identity[0].lower() == package.name.lower()
        and identity[1] == package.version
    ):
        raise PublicationError(
            f"crates.io API identity drifted for {package.name}@{package.version}"
        )
    checksum = version.get("checksum")
    if not isinstance(checksum, str) or re.fullmatch(r"[0-9a-f]{64}", checksum) is None:
        raise PublicationError("crates.io API returned a malformed checksum")
    if version.get("yanked") is not False:
        return AuthorityState("yanked", checksum)
    return AuthorityState("present", checksum)


def query_sparse_index(
    candidate: CandidatePackage,
    *,
    transport: Transport,
) -> AuthorityState:
    """Read the exact version from the crates.io sparse index."""
    package = candidate.package
    url = f"{SPARSE_BASE_URL}/{sparse_index_path(package.name)}"
    response = transport(
        "GET",
        url,
        {
            "Accept": "text/plain",
            "User-Agent": "ds1sqe/type-bridge exact Cargo publisher",
        },
        None,
    )
    if response.status == 404:
        return AuthorityState("absent")
    if response.status != 200:
        raise PublicationError(
            f"crates.io sparse-index lookup failed for {package.name}@{package.version}: "
            f"HTTP {response.status}"
        )
    try:
        lines = response.body.decode("utf-8").splitlines()
    except UnicodeDecodeError as error:
        raise PublicationError("crates.io sparse index returned non-UTF-8 data") from error
    matches: list[dict[str, Any]] = []
    for line in lines:
        try:
            entry = json.loads(line)
        except json.JSONDecodeError as error:
            raise PublicationError(
                f"crates.io sparse index returned malformed JSON: {error}"
            ) from error
        if not isinstance(entry, dict):
            raise PublicationError("crates.io sparse index entry is not an object")
        if entry.get("vers") == package.version:
            matches.append(entry)
    if not matches:
        return AuthorityState("absent")
    if len(matches) != 1:
        raise PublicationError("crates.io sparse index contains duplicate exact-version entries")
    entry = matches[0]
    name = entry.get("name")
    if not isinstance(name, str) or name.lower() != package.name.lower():
        raise PublicationError("crates.io sparse-index package identity drifted")
    checksum = entry.get("cksum")
    if not isinstance(checksum, str) or re.fullmatch(r"[0-9a-f]{64}", checksum) is None:
        raise PublicationError("crates.io sparse index returned a malformed checksum")
    if entry.get("yanked") is not False:
        return AuthorityState("yanked", checksum)
    return AuthorityState("present", checksum)


def _classify_pair(
    candidate: CandidatePackage,
    api: AuthorityState,
    index: AuthorityState,
) -> str:
    expected = candidate.archive_sha256
    for label, state in (("API", api), ("sparse index", index)):
        if state.kind == "yanked":
            raise PublicationError(
                f"crates.io {label} exposes yanked {candidate.package.name}@"
                f"{candidate.package.version}"
            )
        if state.kind == "present" and state.checksum != expected:
            raise PublicationError(
                f"crates.io {label} checksum mismatch for {candidate.package.name}@"
                f"{candidate.package.version}: candidate={expected}, registry={state.checksum}"
            )
        if state.kind not in {"absent", "present"}:
            raise PublicationError(f"unknown crates.io authority state: {state.kind!r}")
    if api.kind == "present" and index.kind == "present":
        return "matching"
    if api.kind == "absent" and index.kind == "absent":
        return "absent"
    return "partial"


def registry_state(
    candidate: CandidatePackage,
    *,
    transport: Transport,
) -> str:
    """Return absent, matching, or partial after checksum validation."""
    return _classify_pair(
        candidate,
        query_api(candidate, transport=transport),
        query_sparse_index(candidate, transport=transport),
    )


def settle_registry_state(
    candidate: CandidatePackage,
    *,
    transport: Transport,
    attempts: int,
    retry_delay: float,
    sleeper: Sleeper,
    require_matching: bool,
) -> str:
    """Bound partial or post-upload visibility convergence."""
    if attempts < 1 or attempts > 100:
        raise PublicationError("registry verification attempts must be from 1 through 100")
    if retry_delay < 0 or retry_delay > 600:
        raise PublicationError("registry retry delay must be from 0 through 600 seconds")
    last = "unknown"
    for attempt in range(1, attempts + 1):
        last = registry_state(candidate, transport=transport)
        if last == "matching":
            return last
        if last == "absent" and not require_matching:
            return last
        if attempt < attempts:
            sleeper(retry_delay)
    expectation = "matching visibility" if require_matching else "consistent visibility"
    raise PublicationError(
        f"crates.io did not reach {expectation} for {candidate.package.name}@"
        f"{candidate.package.version} after {attempts} attempts (last={last})"
    )


def _upload_response_error(response: HttpResponse) -> str:
    if not response.body:
        return f"HTTP {response.status}"
    try:
        payload = json.loads(response.body.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        return f"HTTP {response.status}"
    if isinstance(payload, dict) and isinstance(payload.get("errors"), list):
        details = [
            value.get("detail")
            for value in payload["errors"]
            if isinstance(value, dict) and isinstance(value.get("detail"), str)
        ]
        if details:
            return f"HTTP {response.status}: {'; '.join(details)}"
    return f"HTTP {response.status}"


def upload_exact_request(
    candidate: CandidatePackage,
    *,
    token: str,
    transport: Transport,
    attempts: int,
    initial_backoff: float,
    sleeper: Sleeper,
) -> None:
    """PUT one prebuilt request, retrying 429 with byte-identical content."""
    if candidate.request_body is None:
        raise PublicationError(f"candidate {candidate.package.name} has no publish request")
    if not token or token != token.strip():
        raise PublicationError("CARGO_REGISTRY_TOKEN is missing or malformed")
    if attempts < 1 or attempts > 10:
        raise PublicationError("upload attempts must be from 1 through 10")
    if initial_backoff < 0 or initial_backoff > 600:
        raise PublicationError("upload backoff must be from 0 through 600 seconds")
    try:
        request_stat = candidate.request_body.lstat()
        request_body = candidate.request_body.read_bytes()
    except OSError as error:
        raise PublicationError(
            f"could not reread accepted publish request for {candidate.package.name}: {error}"
        ) from error
    if stat.S_ISLNK(request_stat.st_mode) or not stat.S_ISREG(request_stat.st_mode):
        raise PublicationError(
            f"accepted publish request became linked or non-regular for {candidate.package.name}"
        )
    if (
        len(request_body) != request_stat.st_size
        or hashlib.sha256(request_body).hexdigest() != candidate.request_body_sha256
    ):
        raise PublicationError(
            f"accepted publish request changed before upload for {candidate.package.name}"
        )
    backoff = initial_backoff
    for attempt in range(1, attempts + 1):
        response = transport(
            "PUT",
            PUBLISH_ENDPOINT,
            {
                "Accept": "application/json",
                "Authorization": token,
                "Content-Type": "application/octet-stream",
                "User-Agent": "ds1sqe/type-bridge exact Cargo publisher",
            },
            request_body,
        )
        if 200 <= response.status < 300:
            if response.body:
                payload = _json_object(response.body, label="crates.io publish endpoint")
                errors = payload.get("errors")
                if isinstance(errors, list) and errors:
                    raise PublicationError(
                        f"crates.io rejected {candidate.package.name}@"
                        f"{candidate.package.version}: {_upload_response_error(response)}"
                    )
            return
        if response.status != 429:
            raise PublicationError(
                f"crates.io upload failed for {candidate.package.name}@"
                f"{candidate.package.version}: {_upload_response_error(response)}"
            )
        if attempt == attempts:
            raise PublicationError(
                f"crates.io upload remained rate-limited for {candidate.package.name}@"
                f"{candidate.package.version} after {attempts} attempts"
            )
        sleeper(backoff)
        backoff *= 2


def process_candidate(
    bundle: CandidateBundle,
    *,
    mode: str,
    token: str | None = None,
    transport: Transport = urllib_transport,
    verify_attempts: int = 12,
    verify_delay: float = 5,
    upload_attempts: int = 5,
    upload_backoff: float = 10,
    sleeper: Sleeper = time.sleep,
) -> dict[str, Any]:
    """Preflight all 19 keys, then publish absent first-party keys in order."""
    if mode not in {"preflight", "publish"}:
        raise PublicationError(f"unknown candidate mode: {mode!r}")

    # This pass is deliberately complete before the first irreversible PUT.
    # It prevents a bad later key (including either immutable b8 input) from
    # being discovered only after an earlier first-party package was uploaded.
    initial_states: list[tuple[CandidatePackage, str]] = []
    for candidate in bundle.packages:
        if candidate.package.immutable:
            state = settle_registry_state(
                candidate,
                transport=transport,
                attempts=verify_attempts,
                retry_delay=verify_delay,
                sleeper=sleeper,
                require_matching=True,
            )
        else:
            state = settle_registry_state(
                candidate,
                transport=transport,
                attempts=verify_attempts,
                retry_delay=verify_delay,
                sleeper=sleeper,
                require_matching=False,
            )
        initial_states.append((candidate, state))

    if mode == "publish" and any(
        state == "absent" for candidate, state in initial_states if not candidate.package.immutable
    ):
        if token is None or not token or token != token.strip():
            raise PublicationError("CARGO_REGISTRY_TOKEN is missing or malformed")

    reports: list[dict[str, str]] = []
    for candidate, initial_state in initial_states:
        if candidate.package.immutable:
            status = "verified-preexisting-identical"
        elif initial_state == "matching":
            status = "already-published-identical"
        elif mode == "preflight":
            status = "upload-eligible"
        else:
            # Recheck immediately before PUT so a concurrent identical winner
            # is skipped and any intervening mismatch still fails closed.
            current_state = settle_registry_state(
                candidate,
                transport=transport,
                attempts=verify_attempts,
                retry_delay=verify_delay,
                sleeper=sleeper,
                require_matching=False,
            )
            if current_state == "matching":
                status = "already-published-identical"
                reports.append(
                    {
                        "archive_sha256": candidate.archive_sha256,
                        "name": candidate.package.name,
                        "status": status,
                        "version": candidate.package.version,
                    }
                )
                continue
            try:
                upload_exact_request(
                    candidate,
                    token=token or "",
                    transport=transport,
                    attempts=upload_attempts,
                    initial_backoff=upload_backoff,
                    sleeper=sleeper,
                )
            except PublicationError as upload_error:
                # A concurrent publisher can win after the absence check. The
                # failed PUT is success only when both authorities converge on
                # the already accepted archive checksum.
                try:
                    settle_registry_state(
                        candidate,
                        transport=transport,
                        attempts=verify_attempts,
                        retry_delay=verify_delay,
                        sleeper=sleeper,
                        require_matching=True,
                    )
                except PublicationError:
                    raise upload_error
                status = "already-published-identical"
            else:
                settle_registry_state(
                    candidate,
                    transport=transport,
                    attempts=verify_attempts,
                    retry_delay=verify_delay,
                    sleeper=sleeper,
                    require_matching=True,
                )
                status = "published-identical"
        reports.append(
            {
                "archive_sha256": candidate.archive_sha256,
                "name": candidate.package.name,
                "status": status,
                "version": candidate.package.version,
            }
        )
    return {
        "manifest_sha256": bundle.manifest_sha256,
        "mode": mode,
        "packages": reports,
        "status": "ok",
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("preflight", "publish"))
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--expected-release-version", required=True)
    parser.add_argument("--expected-manifest-sha256", required=True)
    parser.add_argument("--verify-attempts", type=int, default=12)
    parser.add_argument("--verify-delay-seconds", type=float, default=5)
    parser.add_argument("--upload-attempts", type=int, default=5)
    parser.add_argument("--upload-initial-backoff-seconds", type=float, default=10)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    bundle = validate_candidate_bundle(
        args.bundle,
        expected_release_version=args.expected_release_version,
        expected_manifest_sha256=args.expected_manifest_sha256,
    )
    report = process_candidate(
        bundle,
        mode=args.mode,
        token=os.environ.get("CARGO_REGISTRY_TOKEN"),
        verify_attempts=args.verify_attempts,
        verify_delay=args.verify_delay_seconds,
        upload_attempts=args.upload_attempts,
        upload_backoff=args.upload_initial_backoff_seconds,
    )
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (CandidateError, PublicationError) as error:
        print(f"Cargo candidate publication failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
