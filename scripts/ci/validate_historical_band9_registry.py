#!/usr/bin/env python3
"""Require the historical 1.5.x band-9 registry keys to remain usable."""

from __future__ import annotations

import copy
import json
import re
import sys
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from typing import Any
from urllib import error as urllib_error
from urllib import request as urllib_request


class ValidationError(RuntimeError):
    """The external registry no longer preserves the historical compatibility key."""


SHA256_PATTERN = re.compile(r"^[0-9a-f]{64}$")
MAX_REGISTRY_METADATA_BYTES = 1024 * 1024
REGISTRY_HTTP_TIMEOUT_SECONDS = 20


@dataclass(frozen=True)
class HistoricalRegistryKey:
    """One immutable namespaced key retained only for released 1.5.x users."""

    checksum: str
    license: str
    name: str
    version: str

    @property
    def api_url(self) -> str:
        """Return the exact-version crates.io API endpoint."""
        return f"https://crates.io/api/v1/crates/{self.name}/{self.version}"

    @property
    def sparse_index_url(self) -> str:
        """Return the canonical crates.io sparse-index endpoint."""
        normalized = self.name.lower()
        if len(normalized) == 1:
            relative = f"1/{normalized}"
        elif len(normalized) == 2:
            relative = f"2/{normalized}"
        elif len(normalized) == 3:
            relative = f"3/{normalized[0]}/{normalized}"
        else:
            relative = f"{normalized[:2]}/{normalized[2:4]}/{normalized}"
        return f"https://index.crates.io/{relative}"


HISTORICAL_BAND9_REGISTRY_KEYS = (
    HistoricalRegistryKey(
        checksum="b8890ea7f1fee733d57f6b5610dc7cb28a68e489221d39aa703773c039048e95",
        license="Apache-2.0",
        name="type-bridge-typedb-driver-b9",
        version="3.12.0",
    ),
    HistoricalRegistryKey(
        checksum="f4a88d59ff55b0600fcd7474ea2cff445179eed293a38255e59fee474923debe",
        license="MPL-2.0",
        name="type-bridge-typedb-protocol-b9",
        version="3.12.0",
    ),
)
HISTORICAL_BAND9_DRIVER_NAME = "type-bridge-typedb-driver-b9"
HISTORICAL_BAND9_PROTOCOL_EDGE = {
    "default_features": True,
    "features": [],
    "kind": "normal",
    "name": "typedb-protocol",
    "optional": False,
    "package": "type-bridge-typedb-protocol-b9",
    "req": "=3.12.0",
    "target": None,
}


def _download_registry_metadata(
    url: str,
    *,
    opener: Callable[..., Any],
) -> bytes:
    """Read one exact registry endpoint with redirect, time, and byte bounds."""
    request = urllib_request.Request(
        url,
        headers={
            "Accept": "application/json",
            "User-Agent": "ds1sqe/type-bridge historical-band9-compatibility-monitor",
        },
        method="GET",
    )
    try:
        with opener(request, timeout=REGISTRY_HTTP_TIMEOUT_SECONDS) as response:
            status = getattr(response, "status", None)
            if status != 200:
                raise ValidationError(
                    f"Historical band-9 registry endpoint returned HTTP status {status!r}: {url}"
                )
            final_url = response.geturl()
            if final_url != url:
                raise ValidationError(
                    "Historical band-9 registry endpoint redirected away from its canonical URL: "
                    f"actual={final_url!r}, expected={url!r}"
                )
            declared_length = response.headers.get("Content-Length")
            if declared_length is not None:
                try:
                    parsed_length = int(declared_length, 10)
                except ValueError as error:
                    raise ValidationError(
                        "Historical band-9 registry response has an invalid Content-Length: "
                        f"{declared_length!r}"
                    ) from error
                if parsed_length < 0 or parsed_length > MAX_REGISTRY_METADATA_BYTES:
                    raise ValidationError(
                        "Historical band-9 registry response exceeds the byte budget: "
                        f"declared={parsed_length}, maximum={MAX_REGISTRY_METADATA_BYTES}"
                    )

            chunks: list[bytes] = []
            total = 0
            while True:
                remaining = MAX_REGISTRY_METADATA_BYTES - total
                chunk = response.read(min(64 * 1024, remaining + 1))
                if not chunk:
                    break
                total += len(chunk)
                if total > MAX_REGISTRY_METADATA_BYTES:
                    raise ValidationError(
                        "Historical band-9 registry response exceeds the byte budget while "
                        f"reading: maximum={MAX_REGISTRY_METADATA_BYTES}"
                    )
                chunks.append(chunk)
    except ValidationError:
        raise
    except (OSError, urllib_error.URLError) as error:
        raise ValidationError(
            f"Could not query historical band-9 registry endpoint {url}: {error}"
        ) from error
    return b"".join(chunks)


def _json_object(body: bytes, *, label: str) -> dict[str, Any]:
    """Decode one bounded JSON object."""
    try:
        payload = json.loads(body)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValidationError(f"{label} returned invalid JSON: {error}") from error
    if not isinstance(payload, dict):
        raise ValidationError(f"{label} must return a JSON object")
    return payload


def _validate_exact_record(
    entry: dict[str, Any],
    key: HistoricalRegistryKey,
    *,
    checksum_field: str,
    name_field: str,
    source: str,
    version_field: str,
) -> None:
    """Require one registry record to match the immutable compatibility identity."""
    if entry.get(name_field) != key.name or entry.get(version_field) != key.version:
        raise ValidationError(
            f"{source} historical band-9 identity mismatch for {key.name}@{key.version}"
        )
    if entry.get("yanked") is not False:
        raise ValidationError(
            f"{source} reports historical band-9 key {key.name}@{key.version} as yanked"
        )
    checksum = entry.get(checksum_field)
    if not isinstance(checksum, str) or SHA256_PATTERN.fullmatch(checksum) is None:
        raise ValidationError(
            f"{source} reports no canonical checksum for {key.name}@{key.version}"
        )
    if checksum != key.checksum:
        raise ValidationError(
            f"{source} checksum drifted for historical band-9 key {key.name}@{key.version}: "
            f"actual={checksum!r}, expected={key.checksum!r}"
        )


def _validate_historical_driver_protocol_edge(entry: dict[str, Any]) -> None:
    """Bind the Cargo-resolution edge that makes the historical driver usable."""
    dependencies = entry.get("deps")
    if not isinstance(dependencies, list) or not all(
        isinstance(dependency, dict) for dependency in dependencies
    ):
        raise ValidationError(
            "crates.io sparse index has no valid dependency array for historical band-9 driver"
        )
    candidates = [
        dependency
        for dependency in dependencies
        if dependency.get("name") == HISTORICAL_BAND9_PROTOCOL_EDGE["name"]
        or dependency.get("package") == HISTORICAL_BAND9_PROTOCOL_EDGE["package"]
    ]
    if len(candidates) != 1:
        raise ValidationError(
            "crates.io sparse index must contain exactly one historical band-9 driver-to-"
            f"protocol edge, found {len(candidates)}"
        )
    dependency = candidates[0]
    actual_fields = set(dependency)
    expected_fields = set(HISTORICAL_BAND9_PROTOCOL_EDGE)
    if actual_fields != expected_fields:
        raise ValidationError(
            "crates.io sparse-index historical band-9 driver-to-protocol edge drifted: "
            f"actual_fields={sorted(actual_fields)!r}, expected_fields={sorted(expected_fields)!r}"
        )
    for field, expected in HISTORICAL_BAND9_PROTOCOL_EDGE.items():
        actual = dependency.get(field)
        matches = actual is expected if isinstance(expected, bool) else actual == expected
        if not matches:
            raise ValidationError(
                "crates.io sparse-index historical band-9 driver-to-protocol edge drifted: "
                f"field={field!r}, actual={actual!r}, expected={expected!r}"
            )


def validate_historical_registry_key(
    key: HistoricalRegistryKey,
    *,
    opener: Callable[..., Any] = urllib_request.urlopen,
) -> dict[str, object]:
    """Cross-check one historical key through the API and sparse index."""
    api_payload = _json_object(
        _download_registry_metadata(key.api_url, opener=opener),
        label="crates.io exact-version API",
    )
    api_entry = api_payload.get("version")
    if not isinstance(api_entry, dict):
        raise ValidationError(
            f"crates.io API has no exact historical band-9 record for {key.name}@{key.version}"
        )
    _validate_exact_record(
        api_entry,
        key,
        checksum_field="checksum",
        name_field="crate",
        source="crates.io API",
        version_field="num",
    )
    if api_entry.get("license") != key.license:
        raise ValidationError(
            f"crates.io API license drifted for historical band-9 key {key.name}@{key.version}: "
            f"actual={api_entry.get('license')!r}, expected={key.license!r}"
        )

    sparse_body = _download_registry_metadata(key.sparse_index_url, opener=opener)
    sparse_entries: list[dict[str, Any]] = []
    for line_number, line in enumerate(sparse_body.splitlines(), start=1):
        try:
            entry = json.loads(line)
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise ValidationError(
                "crates.io sparse index returned invalid JSON for historical band-9 key "
                f"{key.name} on line {line_number}: {error}"
            ) from error
        if not isinstance(entry, dict):
            raise ValidationError(
                "crates.io sparse index returned a non-object record for historical band-9 "
                f"key {key.name} on line {line_number}"
            )
        if entry.get("name") == key.name and entry.get("vers") == key.version:
            sparse_entries.append(entry)
    if len(sparse_entries) != 1:
        raise ValidationError(
            "crates.io sparse index must contain exactly one historical band-9 record for "
            f"{key.name}@{key.version}, found {len(sparse_entries)}"
        )
    _validate_exact_record(
        sparse_entries[0],
        key,
        checksum_field="cksum",
        name_field="name",
        source="crates.io sparse index",
        version_field="vers",
    )
    result: dict[str, object] = {
        "checksum": key.checksum,
        "license": key.license,
        "version": key.version,
        "yanked": False,
    }
    if key.name == HISTORICAL_BAND9_DRIVER_NAME:
        _validate_historical_driver_protocol_edge(sparse_entries[0])
        result["protocol_dependency"] = copy.deepcopy(HISTORICAL_BAND9_PROTOCOL_EDGE)
    return result


def validate_historical_band9_registry(
    *,
    opener: Callable[..., Any] = urllib_request.urlopen,
) -> dict[str, dict[str, object]]:
    """Require both external 1.5.x compatibility keys to remain non-yanked."""
    return {
        key.name: validate_historical_registry_key(key, opener=opener)
        for key in HISTORICAL_BAND9_REGISTRY_KEYS
    }


def main(argv: Sequence[str] | None = None) -> int:
    """Run the read-only external compatibility monitor."""
    if argv:
        raise ValidationError("This validator accepts no arguments")
    report = validate_historical_band9_registry()
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main(sys.argv[1:]))
    except ValidationError as error:
        print(f"Historical band-9 registry validation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
