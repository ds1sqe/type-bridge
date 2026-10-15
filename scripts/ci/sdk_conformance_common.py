#!/usr/bin/env python3
"""Shared fail-closed helpers for SDK conformance comparers."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import stat
import sys
from collections.abc import Callable
from dataclasses import dataclass, replace
from pathlib import Path, PurePosixPath
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))

import sdk_profile_matrix as profile_matrix  # noqa: E402

MAX_JSON_BYTES = 1024 * 1024

SHA256_RE = re.compile(r"^[0-9a-f]{64}$")

CONTRACT_NAME_RE = re.compile(r"^[a-z0-9][a-z0-9._/-]*$")

OBSERVATION_KEY_RE = re.compile(r"^[a-z][a-z0-9_]*$")

TYPEDB_IID_RE = re.compile(r"^0x[0-9a-f]{2,}$")

FORBIDDEN_OBSERVATION_KEYS = frozenset(
    {
        "address",
        "database",
        "database_name",
        "iid",
        "nonce",
        "pid",
        "port",
        "process_id",
        "runtime_identity",
        "timestamp",
        "wall_clock",
    }
)


class ContractError(ValueError):
    """A stable fail-closed sdk contract rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code


@dataclass(frozen=True)
class LoadedJson:
    path: Path
    raw: bytes
    value: Any

    @property
    def sha256(self) -> str:
        return hashlib.sha256(self.raw).hexdigest()


def _duplicate_key_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise ContractError("duplicate_json_key", f"duplicate JSON key {key!r}")
        value[key] = item
    return value


def _load_json(path: Path, label: str, *, require_canonical: bool = False) -> LoadedJson:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise ContractError("invalid_source_file", f"{label} cannot be inspected") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise ContractError("invalid_source_file", f"{label} is not a regular non-symlink file")
    if metadata.st_size > MAX_JSON_BYTES:
        raise ContractError(
            "json_size_limit_exceeded",
            f"{label} exceeds the {MAX_JSON_BYTES}-byte limit",
        )
    with path.open("rb") as source:
        raw = source.read(MAX_JSON_BYTES + 1)
    if len(raw) > MAX_JSON_BYTES:
        raise ContractError(
            "json_size_limit_exceeded",
            f"{label} exceeds the {MAX_JSON_BYTES}-byte limit",
        )
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ContractError("invalid_json_utf8", f"{label} is not UTF-8") from error
    try:
        value = json.loads(text, object_pairs_hook=_duplicate_key_object)
    except ContractError:
        raise
    except (json.JSONDecodeError, RecursionError) as error:
        raise ContractError("malformed_json", f"{label} is not valid bounded JSON") from error
    if require_canonical and raw != canonical_json_bytes(value):
        raise ContractError(
            "noncanonical_report_json",
            f"{label} must be compact key-sorted UTF-8 JSON with one trailing newline",
        )
    return LoadedJson(path=path, raw=raw, value=value)


def canonical_json_bytes(value: Any) -> bytes:
    """Return the report contract's deterministic JSON spelling."""

    try:
        encoded = json.dumps(
            value,
            ensure_ascii=False,
            allow_nan=False,
            separators=(",", ":"),
            sort_keys=True,
        )
    except (TypeError, ValueError, RecursionError) as error:
        raise ContractError("invalid_json_value", "value cannot be canonicalized") from error
    return f"{encoded}\n".encode()


def _exact_object(value: Any, keys: set[str], label: str) -> dict[str, Any]:
    if type(value) is not dict:
        raise ContractError("invalid_object", f"{label} must be an object")
    actual = set(value)
    if actual != keys:
        missing = sorted(keys - actual)
        extra = sorted(actual - keys)
        raise ContractError(
            "invalid_object_fields",
            f"{label} fields differ; missing={missing}, extra={extra}",
        )
    return value


def _exact_list(value: Any, label: str) -> list[Any]:
    if type(value) is not list:
        raise ContractError("invalid_list", f"{label} must be an array")
    return value


def _string(value: Any, label: str) -> str:
    if type(value) is not str or not value:
        raise ContractError("invalid_string", f"{label} must be non-empty text")
    return value


def _integer(value: Any, label: str) -> int:
    if type(value) is not int:
        raise ContractError("invalid_integer", f"{label} must be an integer")
    return value


def _expand_binding_profile(manifest: dict[str, Any], profile_name: str) -> dict[str, str]:
    profiles = manifest.get("binding_profiles")
    if type(profiles) is not dict or profile_name not in profiles:
        raise ContractError("unknown_binding_profile", f"unknown profile {profile_name!r}")
    profile = profiles[profile_name]
    if type(profile) is not dict:
        raise ContractError("invalid_binding_profile", f"profile {profile_name!r} is not an object")
    statuses = manifest.get("statuses")
    if type(statuses) is not list or any(type(status) is not str for status in statuses):
        raise ContractError("invalid_manifest_statuses", "manifest statuses are malformed")
    expanded: dict[str, str] = {}
    for status in statuses:
        bindings = profile.get(status)
        if type(bindings) is not list or any(type(binding) is not str for binding in bindings):
            raise ContractError(
                "invalid_binding_profile", f"profile {profile_name!r} has malformed {status!r}"
            )
        for binding in bindings:
            if binding in expanded:
                raise ContractError(
                    "duplicate_binding_status", f"{binding!r} has two statuses in {profile_name!r}"
                )
            expanded[binding] = status
    return expanded


def catalog_profile(fixture: dict[str, Any], version: int) -> str:
    """Return the catalog's frozen profile once it is a matrix entry."""

    profile = fixture["semantic_profile"]
    if profile not in profile_matrix.supported_profiles(version):
        raise ContractError(
            "semantic_profile_mismatch", f"sdk-v{version} catalog profile is not a matrix profile"
        )
    return profile


def for_report_profile(contracts: Any, reports: list[LoadedJson], version: int) -> Any:
    """Rebind ``contracts`` to the matrix entry of the profile the reports share.

    The catalog's top-level identity is the entry for its own profile, so a
    report set on that profile keeps ``contracts`` unchanged. Reports that
    declare no readable profile also keep it, so exact report validation
    reports them with its usual codes.
    """

    profile = profile_matrix.shared_value(
        (
            profile_matrix.report_field(report.value, "fixture", "semantic_profile")
            for report in reports
        ),
        "semantic profile",
        error=ContractError,
    )
    return contracts if profile is None else for_profile(contracts, profile, version)


def for_profile(contracts: Any, profile: str, version: int) -> Any:
    """Rebind ``contracts`` to the ``sdk-v{version}`` matrix entry for ``profile``.

    The catalog's own profile keeps ``contracts`` unchanged; any other profile
    must be a matrix entry and replaces the expected fingerprints.
    """

    if profile == contracts.semantic_profile:
        return contracts
    identity = profile_matrix.select(version, profile, error=ContractError)
    fingerprints = identity.expected_fingerprints
    if fingerprints is None:
        raise ContractError("invalid_profile_matrix", f"sdk-v{version} pins no fingerprints")
    semantic = _validate_fingerprint(
        fingerprints["semantic"],
        label="matrix semantic fingerprint",
        domain="typebridge.schema.semantic",
        canonicalization="typebridge.schema-canonical-json/v1",
        semantic_profile=identity.semantic_profile,
    )
    projections = _exact_object(
        fingerprints["projections"],
        set(contracts.projection_fingerprints),
        "matrix projection fingerprints",
    )
    return replace(
        contracts,
        semantic_profile=identity.semantic_profile,
        semantic_fingerprint=semantic,
        projection_fingerprints={
            binding: _validate_fingerprint(
                projections[binding],
                label=f"matrix {binding} projection fingerprint",
                domain="typebridge.binding.projection",
                canonicalization="typebridge.binding-projection/v1",
                semantic_profile=identity.semantic_profile,
            )
            for binding in contracts.projection_fingerprints
        },
    )


def _validate_source_identity(
    value: Any,
    *,
    expected_path: str,
    expected_sha256: str,
    label: str,
    digest_code: str,
) -> None:
    identity = _exact_object(value, {"path", "sha256"}, label)
    if identity["path"] != expected_path:
        raise ContractError("source_path_mismatch", f"{label} path is not exact")
    digest = identity["sha256"]
    if type(digest) is not str or SHA256_RE.fullmatch(digest) is None:
        raise ContractError("invalid_sha256", f"{label} SHA-256 is malformed")
    if digest != expected_sha256:
        raise ContractError(digest_code, f"{label} SHA-256 does not match the source")


def _validate_fingerprint(
    value: Any,
    *,
    label: str,
    domain: str,
    canonicalization: str,
    semantic_profile: str,
) -> dict[str, Any]:
    fingerprint = _exact_object(
        value,
        {"domain", "algorithm", "canonicalization", "semantic_profile", "digest"},
        label,
    )
    if fingerprint["domain"] != domain:
        raise ContractError("fingerprint_domain_mismatch", f"{label} domain is invalid")
    if fingerprint["algorithm"] != "sha256":
        raise ContractError("fingerprint_algorithm_mismatch", f"{label} algorithm is invalid")
    if fingerprint["canonicalization"] != canonicalization:
        raise ContractError(
            "fingerprint_canonicalization_mismatch", f"{label} canonicalization is invalid"
        )
    if fingerprint["semantic_profile"] != semantic_profile:
        raise ContractError("semantic_profile_mismatch", f"{label} profile is invalid")
    digest = fingerprint["digest"]
    if type(digest) is not str or SHA256_RE.fullmatch(digest) is None:
        raise ContractError("invalid_fingerprint_digest", f"{label} digest is malformed")
    return fingerprint


def _inspect_observation(value: Any, label: str, *, depth: int = 0) -> int:
    if depth > 16:
        raise ContractError("observation_limit_exceeded", f"{label} exceeds nesting limit")
    if type(value) is dict:
        if len(value) > 128:
            raise ContractError("observation_limit_exceeded", f"{label} has too many fields")
        total = 1
        for key, item in value.items():
            if type(key) is not str or OBSERVATION_KEY_RE.fullmatch(key) is None:
                raise ContractError("invalid_observation_key", f"{label} contains key {key!r}")
            if key in FORBIDDEN_OBSERVATION_KEYS or key.endswith("_iid"):
                raise ContractError(
                    "runtime_identity_leak", f"{label} contains forbidden key {key!r}"
                )
            total += _inspect_observation(item, f"{label}.{key}", depth=depth + 1)
    elif type(value) is list:
        if len(value) > 128:
            raise ContractError("observation_limit_exceeded", f"{label} has too many items")
        total = 1 + sum(
            _inspect_observation(item, f"{label}[{index}]", depth=depth + 1)
            for index, item in enumerate(value)
        )
    elif type(value) is str:
        if len(value.encode()) > 4096:
            raise ContractError("observation_limit_exceeded", f"{label} text is too long")
        if TYPEDB_IID_RE.fullmatch(value):
            raise ContractError("runtime_identity_leak", f"{label} contains a TypeDB IID")
        total = 1
    elif type(value) is bool or value is None:
        total = 1
    elif type(value) is int:
        if not -(2**63) <= value < 2**63:
            raise ContractError("observation_limit_exceeded", f"{label} integer is outside i64")
        total = 1
    else:
        raise ContractError(
            "invalid_observation_value", f"{label} contains a noncanonical JSON value"
        )
    if total > 4096:
        raise ContractError("observation_limit_exceeded", f"{label} is too large")
    return total


def contract_path(value: Any, expected: str, label: str, root: Path) -> Path:
    text = _string(value, label)
    relative = PurePosixPath(text)
    if (
        relative.is_absolute()
        or relative.as_posix() != text
        or "." in relative.parts
        or (".." in relative.parts)
        or (CONTRACT_NAME_RE.fullmatch(text) is None)
    ):
        raise ContractError("invalid_contract_path", f"{label} is not a canonical relative path")
    if text != expected:
        raise ContractError("contract_path_mismatch", f"{label} is {text!r}, expected {expected!r}")
    path = root.joinpath(*relative.parts)
    if path.is_symlink() or not path.is_file():
        raise ContractError("invalid_source_file", f"{label} target is not a regular file")
    try:
        path.resolve().relative_to(root.resolve())
    except ValueError as error:
        raise ContractError("contract_path_escape", f"{label} escapes the repository") from error
    return path


def validate_report_schema(
    value: Any, report_format: str, bindings: tuple[str, ...], result_count: int
) -> None:
    schema = _exact_object(
        value,
        {
            "$schema",
            "$id",
            "title",
            "type",
            "additionalProperties",
            "required",
            "properties",
            "$defs",
        },
        "report schema",
    )
    if schema["$schema"] != "https://json-schema.org/draft/2020-12/schema":
        raise ContractError("invalid_report_schema", "unexpected JSON Schema dialect")
    if schema["type"] != "object" or schema["additionalProperties"] is not False:
        raise ContractError("invalid_report_schema", "report root must be a closed object")
    properties = schema["properties"]
    if type(properties) is not dict:
        raise ContractError("invalid_report_schema", "report properties are malformed")
    if properties.get("format") != {"const": report_format}:
        raise ContractError("invalid_report_schema", "report format is not frozen")
    if properties.get("binding") != {"enum": list(bindings)}:
        raise ContractError("invalid_report_schema", "report bindings are not frozen")
    results = properties.get("results")
    if type(results) is not dict:
        raise ContractError("invalid_report_schema", "report results are malformed")
    if results.get("minItems") != result_count:
        raise ContractError("invalid_report_schema", "report result minimum is not frozen")
    if results.get("maxItems") != result_count:
        raise ContractError("invalid_report_schema", "report result maximum is not frozen")


def compare_report_set(
    report_paths: list[Path],
    bindings: tuple[str, ...],
    validate: Callable[[Path], dict[str, Any]],
    summarize: Callable[[dict[str, dict[str, Any]]], dict[str, Any]],
) -> dict[str, Any]:
    reports: dict[str, dict[str, Any]] = {}
    for path in report_paths:
        validated = validate(path)
        binding = validated["binding"]
        if binding in reports:
            raise ContractError("duplicate_binding", f"received two {binding!r} reports")
        reports[binding] = validated
    missing = sorted(set(bindings) - reports.keys())
    extra = sorted(reports.keys() - set(bindings))
    if missing:
        raise ContractError("missing_binding", f"missing reports for {missing}")
    if extra:
        raise ContractError("unexpected_binding", f"unexpected reports for {extra}")
    semantic = {
        canonical_json_bytes(reports[binding]["semantic_fingerprint"]) for binding in bindings
    }
    if len(semantic) != 1:
        raise ContractError(
            "semantic_fingerprint_mismatch", "report semantic fingerprints do not match"
        )
    projections = {
        canonical_json_bytes(reports[binding]["projection_fingerprint"]) for binding in bindings
    }
    if len(projections) != len(bindings):
        raise ContractError(
            "projection_fingerprint_collision", "binding projection fingerprints must differ"
        )
    first_results = reports[bindings[0]]["results"]
    for binding in bindings[1:]:
        if reports[binding]["results"] != first_results:
            raise ContractError(
                "cross_binding_observation_mismatch", f"{binding} results differ from {bindings[0]}"
            )
    return summarize(reports)


def report_parser(description: str | None, bindings: tuple[str, ...]) -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=description)
    parser.add_argument(
        "reports",
        nargs=len(bindings),
        type=Path,
        metavar="REPORT",
        help="one canonical report per binding (order-independent): " + ", ".join(bindings),
    )
    return parser


def _regular_bytes(path: Path, label: str, limit: int) -> bytes:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise ContractError("invalid_source_file", f"{label} cannot be inspected") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise ContractError("invalid_source_file", f"{label} must be a regular non-symlink file")
    if metadata.st_size > limit:
        raise ContractError("source_size_limit", f"{label} exceeds {limit} bytes")
    try:
        with path.open("rb") as source:
            raw = source.read(limit + 1)
    except OSError as error:
        raise ContractError("invalid_source_file", f"{label} cannot be read") from error
    if len(raw) > limit:
        raise ContractError("source_size_limit", f"{label} exceeds {limit} bytes")
    return raw


def _json_value(raw: bytes, label: str) -> Any:
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as error:
        raise ContractError("invalid_json_utf8", f"{label} is not UTF-8") from error
    try:
        return json.loads(text, object_pairs_hook=_duplicate_key_object)
    except ContractError:
        raise
    except (json.JSONDecodeError, RecursionError) as error:
        raise ContractError("malformed_json", f"{label} is not valid bounded JSON") from error


def validate_producer_source(
    source: str | bytes, maximum: int, forbidden_markers: tuple[str, ...]
) -> None:
    """Reject a producer that imports or names comparator-side expectations."""
    if type(source) is bytes:
        if len(source) > maximum:
            raise ContractError(
                "producer_source_size_limit", f"producer source exceeds {maximum} bytes"
            )
        try:
            text = source.decode("utf-8")
        except UnicodeDecodeError as error:
            raise ContractError(
                "invalid_producer_source_utf8", "producer source is not UTF-8"
            ) from error
    elif type(source) is str:
        if len(source.encode("utf-8")) > maximum:
            raise ContractError(
                "producer_source_size_limit", f"producer source exceeds {maximum} bytes"
            )
        text = source
    else:
        raise ContractError("invalid_producer_source", "producer source must be text")
    for marker in forbidden_markers:
        if marker in text:
            raise ContractError(
                "expected_observation_import",
                f"producer source contains forbidden marker {marker!r}",
            )
