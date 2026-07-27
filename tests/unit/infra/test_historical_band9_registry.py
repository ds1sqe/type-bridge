"""Hermetic tests for the read-only historical band-9 registry monitor."""

from __future__ import annotations

import importlib.util
import io
import json
import sys
from pathlib import Path
from types import ModuleType
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[3]
VALIDATOR_PATH = ROOT / "scripts/ci/validate_historical_band9_registry.py"
RELEASE_WORKFLOW = ROOT / ".github/workflows/release.yml"


def load_module(name: str, path: Path) -> ModuleType:
    """Load one standalone CI validator without making scripts a package."""
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


validator = load_module("validate_historical_band9_registry", VALIDATOR_PATH)


class FakeResponse:
    """Minimal bounded response for registry fixtures."""

    status = 200

    def __init__(
        self,
        url: str,
        body: bytes,
        *,
        content_length: str | None = None,
        final_url: str | None = None,
        status: int = 200,
    ) -> None:
        self.status = status
        self._url = url if final_url is None else final_url
        self._body = io.BytesIO(body)
        self.headers = {} if content_length is None else {"Content-Length": content_length}

    def __enter__(self) -> FakeResponse:
        return self

    def __exit__(self, *_: object) -> None:
        return None

    def geturl(self) -> str:
        return self._url

    def read(self, size: int) -> bytes:
        return self._body.read(size)


def api_body(key: Any, **overrides: object) -> bytes:
    """Build one exact-version API fixture."""
    entry: dict[str, object] = {
        "checksum": key.checksum,
        "crate": key.name,
        "license": key.license,
        "num": key.version,
        "yanked": False,
    }
    entry.update(overrides)
    return json.dumps({"version": entry}).encode()


def sparse_body(key: Any, *entries: dict[str, object]) -> bytes:
    """Build one newline-delimited sparse-index fixture."""
    default: dict[str, object] = {
        "cksum": key.checksum,
        "deps": [],
        "name": key.name,
        "vers": key.version,
        "yanked": False,
    }
    if key.name == validator.HISTORICAL_BAND9_DRIVER_NAME:
        default["deps"] = [dict(validator.HISTORICAL_BAND9_PROTOCOL_EDGE)]
    rows = list(entries) or [default]
    return b"\n".join(json.dumps(row).encode() for row in rows) + b"\n"


def fixture_opener(
    overrides: dict[str, bytes | FakeResponse] | None = None,
) -> tuple[Any, list[str]]:
    """Return a no-network opener for every pinned API and sparse endpoint."""
    responses: dict[str, bytes | FakeResponse] = {}
    for key in validator.HISTORICAL_BAND9_REGISTRY_KEYS:
        responses[key.api_url] = api_body(key)
        responses[key.sparse_index_url] = sparse_body(key)
    responses.update({} if overrides is None else overrides)
    requested: list[str] = []

    def opener(request: Any, *, timeout: int) -> FakeResponse:
        assert timeout == validator.REGISTRY_HTTP_TIMEOUT_SECONDS
        assert request.method == "GET"
        requested.append(request.full_url)
        fixture = responses[request.full_url]
        if isinstance(fixture, FakeResponse):
            return fixture
        return FakeResponse(request.full_url, fixture)

    return opener, requested


def test_exact_historical_keys_are_checked_through_both_registry_views() -> None:
    opener, requested = fixture_opener()

    report = validator.validate_historical_band9_registry(opener=opener)

    expected = {
        key.name: {
            "checksum": key.checksum,
            "license": key.license,
            "version": "3.12.0",
            "yanked": False,
        }
        for key in validator.HISTORICAL_BAND9_REGISTRY_KEYS
    }
    expected[validator.HISTORICAL_BAND9_DRIVER_NAME]["protocol_dependency"] = dict(
        validator.HISTORICAL_BAND9_PROTOCOL_EDGE
    )
    assert report == expected
    assert requested == [
        endpoint
        for key in validator.HISTORICAL_BAND9_REGISTRY_KEYS
        for endpoint in (key.api_url, key.sparse_index_url)
    ]


@pytest.mark.parametrize("registry_view", ("api", "sparse"))
def test_yanked_historical_key_fails_closed(registry_view: str) -> None:
    key = validator.HISTORICAL_BAND9_REGISTRY_KEYS[0]
    overrides: dict[str, bytes | FakeResponse]
    if registry_view == "api":
        overrides = {key.api_url: api_body(key, yanked=True)}
    else:
        overrides = {
            key.sparse_index_url: sparse_body(
                key,
                {
                    "cksum": key.checksum,
                    "name": key.name,
                    "vers": key.version,
                    "yanked": True,
                },
            )
        }
    opener, _ = fixture_opener(overrides)

    with pytest.raises(validator.ValidationError, match="as yanked"):
        validator.validate_historical_band9_registry(opener=opener)


@pytest.mark.parametrize("registry_view", ("api", "sparse"))
def test_historical_checksum_drift_fails_closed(registry_view: str) -> None:
    key = validator.HISTORICAL_BAND9_REGISTRY_KEYS[0]
    overrides: dict[str, bytes | FakeResponse]
    if registry_view == "api":
        overrides = {key.api_url: api_body(key, checksum="0" * 64)}
    else:
        overrides = {
            key.sparse_index_url: sparse_body(
                key,
                {
                    "cksum": "0" * 64,
                    "name": key.name,
                    "vers": key.version,
                    "yanked": False,
                },
            )
        }
    opener, _ = fixture_opener(overrides)

    with pytest.raises(validator.ValidationError, match="checksum drifted"):
        validator.validate_historical_band9_registry(opener=opener)


def test_sparse_index_requires_one_exact_record() -> None:
    key = validator.HISTORICAL_BAND9_REGISTRY_KEYS[0]
    exact = {
        "cksum": key.checksum,
        "name": key.name,
        "vers": key.version,
        "yanked": False,
    }
    opener, _ = fixture_opener({key.sparse_index_url: sparse_body(key, exact, exact)})

    with pytest.raises(validator.ValidationError, match="exactly one.*found 2"):
        validator.validate_historical_band9_registry(opener=opener)


@pytest.mark.parametrize(
    ("field", "value"),
    (
        ("name", "hostile-alias"),
        ("package", "typedb-protocol"),
        ("req", "^3.12.0"),
        ("kind", "dev"),
        ("optional", True),
        ("target", "cfg(windows)"),
        ("default_features", False),
        ("features", ["hostile"]),
    ),
)
def test_historical_driver_protocol_edge_drift_fails_closed(
    field: str,
    value: object,
) -> None:
    key = validator.HISTORICAL_BAND9_REGISTRY_KEYS[0]
    dependency = dict(validator.HISTORICAL_BAND9_PROTOCOL_EDGE)
    dependency[field] = value
    entry = {
        "cksum": key.checksum,
        "deps": [dependency],
        "name": key.name,
        "vers": key.version,
        "yanked": False,
    }
    opener, _ = fixture_opener({key.sparse_index_url: sparse_body(key, entry)})

    with pytest.raises(
        validator.ValidationError,
        match="driver-to-protocol edge drifted|exactly one.*driver-to-protocol",
    ):
        validator.validate_historical_band9_registry(opener=opener)


@pytest.mark.parametrize("field", tuple(validator.HISTORICAL_BAND9_PROTOCOL_EDGE))
def test_historical_driver_protocol_edge_requires_every_resolution_field(field: str) -> None:
    key = validator.HISTORICAL_BAND9_REGISTRY_KEYS[0]
    dependency = dict(validator.HISTORICAL_BAND9_PROTOCOL_EDGE)
    del dependency[field]
    entry = {
        "cksum": key.checksum,
        "deps": [dependency],
        "name": key.name,
        "vers": key.version,
        "yanked": False,
    }
    opener, _ = fixture_opener({key.sparse_index_url: sparse_body(key, entry)})

    with pytest.raises(
        validator.ValidationError,
        match="driver-to-protocol edge drifted|exactly one.*driver-to-protocol",
    ):
        validator.validate_historical_band9_registry(opener=opener)


def test_historical_driver_protocol_edge_rejects_unexpected_resolution_field() -> None:
    key = validator.HISTORICAL_BAND9_REGISTRY_KEYS[0]
    dependency = dict(validator.HISTORICAL_BAND9_PROTOCOL_EDGE)
    dependency["hostile_resolution_field"] = True
    entry = {
        "cksum": key.checksum,
        "deps": [dependency],
        "name": key.name,
        "vers": key.version,
        "yanked": False,
    }
    opener, _ = fixture_opener({key.sparse_index_url: sparse_body(key, entry)})

    with pytest.raises(validator.ValidationError, match="driver-to-protocol edge drifted"):
        validator.validate_historical_band9_registry(opener=opener)


def test_historical_driver_protocol_edge_must_be_unique() -> None:
    key = validator.HISTORICAL_BAND9_REGISTRY_KEYS[0]
    dependency = dict(validator.HISTORICAL_BAND9_PROTOCOL_EDGE)
    entry = {
        "cksum": key.checksum,
        "deps": [dependency, dependency],
        "name": key.name,
        "vers": key.version,
        "yanked": False,
    }
    opener, _ = fixture_opener({key.sparse_index_url: sparse_body(key, entry)})

    with pytest.raises(validator.ValidationError, match="exactly one.*found 2"):
        validator.validate_historical_band9_registry(opener=opener)


@pytest.mark.parametrize("declared_length", ("invalid", "9"))
def test_registry_download_declared_length_is_bounded(
    monkeypatch: pytest.MonkeyPatch,
    declared_length: str,
) -> None:
    key = validator.HISTORICAL_BAND9_REGISTRY_KEYS[0]
    monkeypatch.setattr(validator, "MAX_REGISTRY_METADATA_BYTES", 8)
    opener, _ = fixture_opener(
        {
            key.api_url: FakeResponse(
                key.api_url,
                b"",
                content_length=declared_length,
            )
        }
    )

    with pytest.raises(validator.ValidationError, match="Content-Length|byte budget"):
        validator.validate_historical_band9_registry(opener=opener)


def test_registry_download_stream_is_bounded_without_content_length(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    key = validator.HISTORICAL_BAND9_REGISTRY_KEYS[0]
    monkeypatch.setattr(validator, "MAX_REGISTRY_METADATA_BYTES", 8)
    opener, _ = fixture_opener({key.api_url: FakeResponse(key.api_url, b"123456789")})

    with pytest.raises(validator.ValidationError, match="byte budget while reading"):
        validator.validate_historical_band9_registry(opener=opener)


def test_registry_redirect_is_rejected() -> None:
    key = validator.HISTORICAL_BAND9_REGISTRY_KEYS[0]
    opener, _ = fixture_opener(
        {
            key.api_url: FakeResponse(
                key.api_url,
                api_body(key),
                final_url="https://example.invalid/redirected",
            )
        }
    )

    with pytest.raises(validator.ValidationError, match="redirected away"):
        validator.validate_historical_band9_registry(opener=opener)


def test_release_checks_external_liveness_before_local_publication_boundary() -> None:
    workflow = RELEASE_WORKFLOW.read_text()
    liveness = "python scripts/ci/validate_historical_band9_registry.py"
    local_identity = "python scripts/ci/validate_release_identity.py"

    assert liveness in workflow
    assert workflow.index(liveness) < workflow.index(local_identity)
    source = VALIDATOR_PATH.read_text()
    assert "urllib_request.Request" in source
    assert 'method="GET"' in source
    assert "subprocess" not in source
