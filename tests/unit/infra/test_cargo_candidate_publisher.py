"""Hermetic registry tests for exact prebuilt Cargo request publication."""

from __future__ import annotations

import hashlib
import json
from collections import deque
from collections.abc import Mapping
from pathlib import Path

import pytest

from scripts.ci.cargo_release_candidate import (
    CandidateBundle,
    CandidatePackage,
)
from scripts.ci.cargo_release_inventory import CargoInventoryPackage
from scripts.ci.publish_cargo_release_candidate import (
    HttpResponse,
    PublicationError,
    process_candidate,
)


def package(name: str, *, immutable: bool = False, order: int = 1) -> CargoInventoryPackage:
    return CargoInventoryPackage(
        name=name,
        manifest=f"crates/{name}/Cargo.toml",
        classification="public-immutable" if immutable else "public-first-party",
        role="supporting",
        version_policy="fixed",
        version="1.2.3",
        publish_order=order,
        docs_target="lib",
        registry_checksum=("0" * 64 if immutable else None),
    )


def bundled(tmp_path: Path, *, immutable: bool = False) -> tuple[CandidateBundle, bytes, str]:
    archive = b"exact accepted archive"
    checksum = hashlib.sha256(archive).hexdigest()
    request = b"exact prebuilt PUT body"
    request_path = tmp_path / "candidate.put"
    request_path.write_bytes(request)
    item = package("demo-crate", immutable=immutable)
    candidate = CandidatePackage(
        package=item,
        archive=tmp_path / "demo.crate",
        archive_sha256=checksum,
        request_body=None if immutable else request_path,
        request_body_sha256=None if immutable else hashlib.sha256(request).hexdigest(),
        metadata_sha256=None if immutable else "1" * 64,
    )
    bundle = CandidateBundle(
        root=tmp_path,
        manifest=tmp_path / "manifest.json",
        manifest_sha256="2" * 64,
        packages=(candidate,),
    )
    return bundle, request, checksum


def api(
    checksum: str,
    *,
    name: str = "demo-crate",
    yanked: bool = False,
) -> HttpResponse:
    return HttpResponse(
        200,
        json.dumps(
            {
                "version": {
                    "checksum": checksum,
                    "crate": name,
                    "num": "1.2.3",
                    "yanked": yanked,
                }
            }
        ).encode(),
    )


def index(
    checksum: str,
    *,
    name: str = "demo-crate",
    yanked: bool = False,
) -> HttpResponse:
    return HttpResponse(
        200,
        (
            json.dumps(
                {
                    "cksum": checksum,
                    "name": name,
                    "vers": "1.2.3",
                    "yanked": yanked,
                }
            )
            + "\n"
        ).encode(),
    )


class Transport:
    def __init__(self, *responses: HttpResponse) -> None:
        self.responses = deque(responses)
        self.calls: list[tuple[str, str, dict[str, str], bytes | None]] = []

    def __call__(
        self,
        method: str,
        url: str,
        headers: Mapping[str, str],
        body: bytes | None,
    ) -> HttpResponse:
        self.calls.append((method, url, dict(headers), body))
        assert self.responses, f"unexpected request: {method} {url}"
        return self.responses.popleft()


def test_existing_identical_candidate_skips_without_put(tmp_path: Path) -> None:
    bundle, _, checksum = bundled(tmp_path)
    transport = Transport(api(checksum), index(checksum))

    report = process_candidate(
        bundle,
        mode="publish",
        transport=transport,
        sleeper=lambda _: None,
    )

    assert report["packages"][0]["status"] == "already-published-identical"
    assert [call[0] for call in transport.calls] == ["GET", "GET"]


def test_preflight_absent_key_never_uploads(tmp_path: Path) -> None:
    bundle, _, _ = bundled(tmp_path)
    transport = Transport(HttpResponse(404, b""), HttpResponse(404, b""))

    report = process_candidate(
        bundle,
        mode="preflight",
        transport=transport,
        sleeper=lambda _: None,
    )

    assert report["packages"][0]["status"] == "upload-eligible"
    assert [call[0] for call in transport.calls] == ["GET", "GET"]


def test_publish_sends_exact_prebuilt_body_and_verifies_both_authorities(
    tmp_path: Path,
) -> None:
    bundle, request, checksum = bundled(tmp_path)
    transport = Transport(
        HttpResponse(404, b""),
        HttpResponse(404, b""),
        HttpResponse(404, b""),
        HttpResponse(404, b""),
        HttpResponse(200, b'{"warnings":{}}'),
        api(checksum),
        index(checksum),
    )

    report = process_candidate(
        bundle,
        mode="publish",
        token="secret-token",
        transport=transport,
        sleeper=lambda _: None,
    )

    put = [call for call in transport.calls if call[0] == "PUT"]
    assert len(put) == 1
    assert put[0][2]["Authorization"] == "secret-token"
    assert put[0][2]["Content-Type"] == "application/octet-stream"
    assert put[0][3] == request
    assert report["packages"][0]["status"] == "published-identical"


def test_rate_limit_retries_byte_identical_body_with_bounded_backoff(tmp_path: Path) -> None:
    bundle, request, checksum = bundled(tmp_path)
    transport = Transport(
        HttpResponse(404, b""),
        HttpResponse(404, b""),
        HttpResponse(404, b""),
        HttpResponse(404, b""),
        HttpResponse(429, b"{}"),
        HttpResponse(429, b"{}"),
        HttpResponse(200, b"{}"),
        api(checksum),
        index(checksum),
    )
    sleeps: list[float] = []

    process_candidate(
        bundle,
        mode="publish",
        token="secret-token",
        transport=transport,
        upload_attempts=3,
        upload_backoff=10,
        sleeper=sleeps.append,
    )

    puts = [call for call in transport.calls if call[0] == "PUT"]
    assert [call[3] for call in puts] == [request, request, request]
    assert sleeps == [10, 20]


def test_concurrent_publish_race_requires_exact_dual_authority_match(tmp_path: Path) -> None:
    bundle, _, checksum = bundled(tmp_path)
    transport = Transport(
        HttpResponse(404, b""),
        HttpResponse(404, b""),
        HttpResponse(404, b""),
        HttpResponse(404, b""),
        HttpResponse(409, b'{"errors":[{"detail":"already exists"}]}'),
        api(checksum),
        index(checksum),
    )

    report = process_candidate(
        bundle,
        mode="publish",
        token="secret-token",
        transport=transport,
        sleeper=lambda _: None,
    )

    assert report["packages"][0]["status"] == "already-published-identical"


def test_checksum_mismatch_fails_before_put(tmp_path: Path) -> None:
    bundle, _, checksum = bundled(tmp_path)
    transport = Transport(api("0" * 64), index(checksum))

    with pytest.raises(PublicationError, match="checksum mismatch"):
        process_candidate(
            bundle,
            mode="publish",
            token="secret-token",
            transport=transport,
            sleeper=lambda _: None,
        )

    assert all(call[0] != "PUT" for call in transport.calls)


def test_partial_visibility_is_polled_before_deciding(tmp_path: Path) -> None:
    bundle, _, checksum = bundled(tmp_path)
    transport = Transport(
        api(checksum),
        HttpResponse(404, b""),
        api(checksum),
        index(checksum),
    )
    sleeps: list[float] = []

    report = process_candidate(
        bundle,
        mode="preflight",
        transport=transport,
        verify_attempts=2,
        verify_delay=7,
        sleeper=sleeps.append,
    )

    assert report["packages"][0]["status"] == "already-published-identical"
    assert sleeps == [7]


def test_immutable_b8_key_must_exist_non_yanked_and_is_never_uploaded(
    tmp_path: Path,
) -> None:
    bundle, _, _ = bundled(tmp_path, immutable=True)
    transport = Transport(HttpResponse(404, b""), HttpResponse(404, b""))

    with pytest.raises(PublicationError, match="matching visibility"):
        process_candidate(
            bundle,
            mode="publish",
            token="secret-token",
            transport=transport,
            verify_attempts=1,
            sleeper=lambda _: None,
        )

    assert all(call[0] != "PUT" for call in transport.calls)


def test_complete_key_preflight_fails_before_first_put(tmp_path: Path) -> None:
    bundle, _, checksum = bundled(tmp_path)
    second_request = tmp_path / "second.put"
    second_request.write_bytes(b"second exact body")
    second_archive_sha = hashlib.sha256(b"second archive").hexdigest()
    second = CandidatePackage(
        package=package("second-crate", order=2),
        archive=tmp_path / "second.crate",
        archive_sha256=second_archive_sha,
        request_body=second_request,
        request_body_sha256=hashlib.sha256(second_request.read_bytes()).hexdigest(),
        metadata_sha256="3" * 64,
    )
    bundle = CandidateBundle(
        root=bundle.root,
        manifest=bundle.manifest,
        manifest_sha256=bundle.manifest_sha256,
        packages=(*bundle.packages, second),
    )
    transport = Transport(
        HttpResponse(404, b""),
        HttpResponse(404, b""),
        api("0" * 64, name="second-crate"),
        index(second_archive_sha, name="second-crate"),
    )

    with pytest.raises(PublicationError, match="checksum mismatch"):
        process_candidate(
            bundle,
            mode="publish",
            token="secret-token",
            transport=transport,
            sleeper=lambda _: None,
        )

    assert checksum == bundle.packages[0].archive_sha256
    assert all(call[0] != "PUT" for call in transport.calls)
