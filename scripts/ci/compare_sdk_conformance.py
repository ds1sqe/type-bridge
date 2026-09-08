#!/usr/bin/env python3
"""Validate and compare the generated Python, Node, and Rust sdk reports."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import stat
import sys
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
MANIFEST_RELATIVE = "tests/contracts/sdk_conformance/manifest-v1.json"
CATALOG_RELATIVE = "tests/contracts/sdk_conformance/sdk-v1/catalog-v1.json"
JOURNEY_RELATIVE = "tests/contracts/sdk_conformance/sdk-v1/journey-v1.json"
REPORT_SCHEMA_RELATIVE = "tests/contracts/sdk_conformance/sdk-v1/report-schema-v1.json"
SCHEMA_RELATIVE = "type-bridge-core/crates/schema-codegen/tests/acceptance/schema.yaml"
PROVIDER_SCHEMA_RELATIVE = (
    "type-bridge-core/crates/schema-codegen/tests/acceptance/provider-3.12.1.tql"
)

REPORT_FORMAT = "typebridge.sdk-conformance-report/v1"
SUMMARY_FORMAT = "typebridge.sdk-conformance-summary/v1"
CATALOG_FORMAT = "typebridge.sdk-catalog/v1"
JOURNEY_FORMAT = "typebridge.sdk-journey/v1"
REPORT_BINDINGS = ("python", "node", "rust")
FINAL_BROAD_SUCCESSOR_CASES = frozenset(
    {
        "sdk.runtime.cancellation",
        "sdk.runtime.timeout-resource-limits",
        "sdk.diagnostic.all-workflows",
        "sdk.runtime.explicit-close",
    }
)
FINAL_DISTRIBUTION_SUCCESSOR_CASE = "sdk.distribution.standalone-cli"
CURRENT_BINDINGS = frozenset(REPORT_BINDINGS)
PROJECTION_TARGETS = {"python": "python", "node": "typescript", "rust": "rust"}
MAX_JSON_BYTES = 1024 * 1024
PROOF_KINDS = (
    "compile_positive",
    "compile_negative",
    "direct_runtime",
    "remote_runtime",
    "diagnostic",
    "lifecycle",
    "artifact",
)
EXPECTED_SELECTED_PROOFS = (
    ("sdk.crud.entity-single", "direct_runtime", "entity_lifecycle"),
    ("sdk.crud.relation-single", "direct_runtime", "relation_lifecycle"),
    (
        "sdk.model.values-and-references",
        "direct_runtime",
        "model_values_and_references",
    ),
    (
        "sdk.model.values-and-references",
        "remote_runtime",
        "model_values_and_references",
    ),
    (
        "sdk.query.remote-hydration",
        "direct_runtime",
        "hydrated_role_result",
    ),
    (
        "sdk.query.remote-hydration",
        "remote_runtime",
        "hydrated_role_result",
    ),
    (
        "sdk.query.remote-one-exchange",
        "remote_runtime",
        "remote_one_exchange",
    ),
    ("sdk.query.roles", "direct_runtime", "role_traversal"),
    ("sdk.query.roles", "remote_runtime", "role_traversal"),
)

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


@dataclass(frozen=True)
class Contracts:
    manifest: LoadedJson
    catalog: LoadedJson
    journey: LoadedJson
    report_schema: LoadedJson
    schema: LoadedJson
    provider_schema: LoadedJson
    capabilities: tuple[dict[str, Any], ...]
    cases: dict[str, dict[str, Any]]
    selected: tuple[tuple[str, str, str], ...]
    observations: dict[str, dict[str, Any]]
    projection_targets: dict[str, str]
    semantic_fingerprint: dict[str, Any]
    projection_fingerprints: dict[str, dict[str, Any]]


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


def _contract_path(value: Any, expected: str, label: str) -> Path:
    text = _string(value, label)
    relative = PurePosixPath(text)
    if (
        relative.is_absolute()
        or relative.as_posix() != text
        or "." in relative.parts
        or ".." in relative.parts
        or not CONTRACT_NAME_RE.fullmatch(text)
    ):
        raise ContractError("invalid_contract_path", f"{label} is not a canonical relative path")
    if text != expected:
        raise ContractError("contract_path_mismatch", f"{label} is {text!r}, expected {expected!r}")
    path = ROOT.joinpath(*relative.parts)
    if path.is_symlink() or not path.is_file():
        raise ContractError("invalid_source_file", f"{label} target is not a regular file")
    resolved = path.resolve()
    try:
        resolved.relative_to(ROOT.resolve())
    except ValueError as error:
        raise ContractError("contract_path_escape", f"{label} escapes the repository") from error
    return path


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


def _validate_report_schema(value: Any) -> None:
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
    format_property = properties.get("format")
    if type(format_property) is not dict or format_property.get("const") != REPORT_FORMAT:
        raise ContractError("invalid_report_schema", "report format is not frozen")
    results = properties.get("results")
    if type(results) is not dict or results.get("minItems") != len(EXPECTED_SELECTED_PROOFS):
        raise ContractError("invalid_report_schema", "report result minimum is not frozen")
    if results.get("maxItems") != len(EXPECTED_SELECTED_PROOFS):
        raise ContractError("invalid_report_schema", "report result maximum is not frozen")


def _validate_journey(value: Any) -> dict[str, dict[str, Any]]:
    journey = _exact_object(
        value,
        {
            "format",
            "fixture_id",
            "version",
            "semantic_profile",
            "records",
            "operation_order",
            "expected_observations",
        },
        "journey",
    )
    if journey["format"] != JOURNEY_FORMAT:
        raise ContractError("invalid_journey_format", "journey format is not v1")
    if journey["fixture_id"] != "sdk-v1" or journey["version"] != 1:
        raise ContractError("invalid_fixture_identity", "journey fixture identity is not v1")
    if journey["semantic_profile"] != "typedb-3.12.1/v1":
        raise ContractError("semantic_profile_mismatch", "journey profile is not 3.12.1/v1")

    records = _exact_object(journey["records"], {"person", "membership"}, "journey records")
    person = _exact_object(records["person"], {"model", "fields", "update"}, "person record")
    if person["model"] != "person":
        raise ContractError("invalid_journey_model", "person record has the wrong model")
    expected_fields = {
        "identifier",
        "nickname",
        "aliases",
        "score",
        "foo__bar",
        "score__gte",
        "val_double",
        "val_decimal",
        "val_bool",
        "val_date",
        "val_datetime",
        "val_datetime_tz",
        "val_duration",
        "val_constrained",
    }
    fields = _exact_object(person["fields"], expected_fields, "person fields")
    _validate_scalar(fields["identifier"], "string", "person identifier")
    _validate_scalar(fields["nickname"], "string", "person nickname")
    aliases = _exact_list(fields["aliases"], "person aliases")
    alias_values = [_validate_scalar(alias, "string", "person alias") for alias in aliases]
    if alias_values != sorted(set(alias_values)) or len(alias_values) != 2:
        raise ContractError("invalid_journey_aliases", "aliases must be two sorted unique values")
    for field, kind in {
        "score": "long",
        "foo__bar": "long",
        "score__gte": "long",
        "val_double": "double",
        "val_decimal": "decimal",
        "val_bool": "boolean",
        "val_date": "date",
        "val_datetime": "datetime",
        "val_datetime_tz": "datetime_tz",
        "val_duration": "duration",
        "val_constrained": "long",
    }.items():
        _validate_scalar(fields[field], kind, f"person {field}")
    update = _exact_object(person["update"], {"nickname"}, "person update")
    _validate_scalar(update["nickname"], "string", "updated nickname")

    membership = _exact_object(
        records["membership"], {"model", "role", "player"}, "membership record"
    )
    if membership["model"] != "membership" or membership["role"] != "member":
        raise ContractError("invalid_journey_relation", "membership identity is not canonical")
    player = _exact_object(membership["player"], {"model", "key"}, "membership player")
    identifier = fields["identifier"]["value"]
    if player != {"model": "person", "key": identifier}:
        raise ContractError(
            "invalid_journey_reference", "membership does not reference the person key"
        )

    expected_operations = [
        "insert_person",
        "read_person",
        "update_person",
        "insert_membership",
        "read_membership",
        "direct_role_query_one",
        "remote_role_query_one",
        "delete_membership",
        "delete_person",
    ]
    if journey["operation_order"] != expected_operations:
        raise ContractError("invalid_operation_order", "journey operation order is not frozen")

    observations = _exact_object(
        journey["expected_observations"],
        {
            "entity_lifecycle",
            "hydrated_role_result",
            "model_values_and_references",
            "relation_lifecycle",
            "remote_one_exchange",
            "role_traversal",
        },
        "expected observations",
    )
    for name, observation in observations.items():
        if type(observation) is not dict:
            raise ContractError("invalid_observation", f"observation {name!r} must be an object")
        _inspect_observation(observation, f"journey observation {name}")
    return observations


def _validate_scalar(value: Any, expected_kind: str, label: str) -> Any:
    value = _exact_object(
        value,
        {"kind", "bits"} if expected_kind == "double" else {"kind", "value"},
        label,
    )
    if value["kind"] != expected_kind:
        raise ContractError("invalid_scalar_kind", f"{label} is not {expected_kind!r}")
    if expected_kind == "double":
        bits = value["bits"]
        if type(bits) is not str or re.fullmatch(r"[0-9a-f]{16}", bits) is None:
            raise ContractError("invalid_scalar_value", f"{label} has invalid double bits")
        return bits
    scalar = value["value"]
    if expected_kind == "boolean":
        if type(scalar) is not bool:
            raise ContractError("invalid_scalar_value", f"{label} must contain a boolean")
    elif type(scalar) is not str or not scalar:
        raise ContractError("invalid_scalar_value", f"{label} must contain non-empty text")
    if expected_kind == "long" and re.fullmatch(r"-?(0|[1-9][0-9]*)", scalar) is None:
        raise ContractError("invalid_scalar_value", f"{label} has a noncanonical integer")
    return scalar


def _validate_catalog(
    value: Any,
    manifest: dict[str, Any],
) -> tuple[
    dict[str, dict[str, Any]],
    tuple[tuple[str, str, str], ...],
    dict[str, str],
    dict[str, Any],
    dict[str, dict[str, Any]],
]:
    catalog = _exact_object(
        value,
        {
            "format",
            "fixture",
            "journey_path",
            "report_schema_path",
            "report_bindings",
            "projection_targets",
            "expected_fingerprints",
            "selected_proofs",
            "cases",
        },
        "catalog",
    )
    if catalog["format"] != CATALOG_FORMAT:
        raise ContractError("invalid_catalog_format", "catalog format is not v1")
    fixture = _exact_object(
        catalog["fixture"],
        {"id", "version", "semantic_profile", "schema_path", "provider_schema_path"},
        "catalog fixture",
    )
    if fixture != {
        "id": "sdk-v1",
        "version": 1,
        "semantic_profile": "typedb-3.12.1/v1",
        "schema_path": SCHEMA_RELATIVE,
        "provider_schema_path": PROVIDER_SCHEMA_RELATIVE,
    }:
        raise ContractError(
            "invalid_fixture_identity", "catalog fixture is not the frozen v1 fixture"
        )
    if catalog["journey_path"] != JOURNEY_RELATIVE:
        raise ContractError("contract_path_mismatch", "catalog journey path is not frozen")
    if catalog["report_schema_path"] != REPORT_SCHEMA_RELATIVE:
        raise ContractError("contract_path_mismatch", "catalog report-schema path is not frozen")
    if catalog["report_bindings"] != list(REPORT_BINDINGS):
        raise ContractError("invalid_report_bindings", "catalog report bindings are not frozen")
    projection_targets = _exact_object(
        catalog["projection_targets"], set(REPORT_BINDINGS), "projection targets"
    )
    if projection_targets != PROJECTION_TARGETS:
        raise ContractError("invalid_projection_targets", "projection target mapping is not frozen")
    expected_fingerprints = _exact_object(
        catalog["expected_fingerprints"],
        {"semantic", "projections"},
        "expected fingerprints",
    )
    semantic_fingerprint = _validate_fingerprint(
        expected_fingerprints["semantic"],
        label="expected semantic fingerprint",
        domain="typebridge.schema.semantic",
        canonicalization="typebridge.schema-canonical-json/v1",
    )
    projection_fingerprints_value = _exact_object(
        expected_fingerprints["projections"],
        set(REPORT_BINDINGS),
        "expected projection fingerprints",
    )
    projection_fingerprints: dict[str, dict[str, Any]] = {}
    for binding in REPORT_BINDINGS:
        projection_fingerprints[binding] = _validate_fingerprint(
            projection_fingerprints_value[binding],
            label=f"expected {binding} projection fingerprint",
            domain="typebridge.binding.projection",
            canonicalization="typebridge.binding-projection/v1",
        )
    if len({item["digest"] for item in projection_fingerprints.values()}) != len(REPORT_BINDINGS):
        raise ContractError(
            "projection_fingerprint_collision",
            "expected binding projection fingerprints must differ",
        )

    capabilities = _exact_list(manifest.get("capabilities"), "manifest capabilities")
    manifest_cases: list[tuple[str, str, dict[str, Any]]] = []
    for capability in capabilities:
        if type(capability) is not dict:
            raise ContractError(
                "invalid_manifest_capability", "manifest capability is not an object"
            )
        case_ids = capability.get("case_ids")
        if type(case_ids) is not list or len(case_ids) != 1 or type(case_ids[0]) is not str:
            raise ContractError("invalid_manifest_case", "each manifest capability needs one case")
        capability_id = _string(capability.get("id"), "manifest capability ID")
        manifest_cases.append((case_ids[0], capability_id, capability))

    cases_list = _exact_list(catalog["cases"], "catalog cases")
    if len(cases_list) != len(manifest_cases):
        raise ContractError("catalog_coverage_mismatch", "catalog case count differs from manifest")
    cases: dict[str, dict[str, Any]] = {}
    selected_case_ids = {case_id for case_id, _, _ in EXPECTED_SELECTED_PROOFS}
    for index, (catalog_case, manifest_case) in enumerate(
        zip(cases_list, manifest_cases, strict=True)
    ):
        case = _exact_object(
            catalog_case, {"id", "capability_id", "disposition"}, f"catalog case {index}"
        )
        case_id, capability_id, capability = manifest_case
        if case["id"] != case_id or case["capability_id"] != capability_id:
            raise ContractError(
                "catalog_coverage_mismatch", f"catalog case {index} does not match the manifest"
            )
        if case_id in cases:
            raise ContractError("duplicate_catalog_case", f"duplicate catalog case {case_id!r}")
        statuses = _expand_binding_profile(manifest, capability["binding_profile"])
        if case_id in selected_case_ids:
            expected_disposition = "shared_smoke"
        elif case_id in FINAL_BROAD_SUCCESSOR_CASES or case_id == FINAL_DISTRIBUTION_SUCCESSOR_CASE:
            expected_disposition = "known_gap"
        elif all(statuses.get(binding) == "gap" for binding in REPORT_BINDINGS):
            expected_disposition = "known_gap"
        else:
            expected_disposition = "retained_evidence"
        if case["disposition"] != expected_disposition:
            raise ContractError(
                "invalid_case_disposition",
                f"{case_id!r} must be {expected_disposition!r}",
            )
        cases[case_id] = {"catalog": case, "capability": capability}

    for case_id in FINAL_BROAD_SUCCESSOR_CASES:
        if cases[case_id]["capability"]["binding_profile"] not in {
            "current_gap_future_planned",
            "terminal_broad_live_future_planned",
        }:
            raise ContractError(
                "invalid_successor_profile", f"{case_id!r} successor profile drifted"
            )
    if cases[FINAL_DISTRIBUTION_SUCCESSOR_CASE]["capability"]["binding_profile"] not in {
        "current_gap_future_planned",
        "standalone_distribution_offline_future_planned",
    }:
        raise ContractError(
            "invalid_successor_profile",
            f"{FINAL_DISTRIBUTION_SUCCESSOR_CASE!r} successor profile drifted",
        )

    selected_list = _exact_list(catalog["selected_proofs"], "selected proofs")
    selected: list[tuple[str, str, str]] = []
    for index, item in enumerate(selected_list):
        proof = _exact_object(
            item, {"case_id", "proof_kind", "observation_ref"}, f"selected proof {index}"
        )
        selected.append((proof["case_id"], proof["proof_kind"], proof["observation_ref"]))
    if tuple(selected) != EXPECTED_SELECTED_PROOFS:
        raise ContractError("selected_proof_mismatch", "selected proof set or order is not frozen")
    for case_id, proof_kind, _ in selected:
        entry = cases[case_id]
        profile_name = entry["capability"]["proof_profile"]
        profile = manifest["proof_profiles"][profile_name]
        if profile.get(proof_kind) != "required":
            raise ContractError(
                "proof_not_applicable", f"{case_id!r}/{proof_kind!r} is not required"
            )
        statuses = _expand_binding_profile(manifest, entry["capability"]["binding_profile"])
        if any(statuses.get(binding) != "accepted_live" for binding in REPORT_BINDINGS):
            raise ContractError(
                "selected_proof_not_live",
                f"{case_id!r} is not accepted_live for every report binding",
            )
    return (
        cases,
        tuple(selected),
        dict(projection_targets),
        semantic_fingerprint,
        projection_fingerprints,
    )


def load_contracts(
    manifest_path: Path | None = None,
    catalog_path: Path | None = None,
) -> Contracts:
    """Load and validate the committed manifest, catalog, journey, and fixture."""

    expected_manifest = ROOT / MANIFEST_RELATIVE
    expected_catalog = ROOT / CATALOG_RELATIVE
    manifest_path = expected_manifest if manifest_path is None else manifest_path
    catalog_path = expected_catalog if catalog_path is None else catalog_path
    if manifest_path.resolve() != expected_manifest.resolve():
        raise ContractError(
            "contract_path_mismatch", "manifest path is not the committed authority"
        )
    if catalog_path.resolve() != expected_catalog.resolve():
        raise ContractError("contract_path_mismatch", "catalog path is not the committed authority")

    manifest = _load_json(manifest_path, "manifest")
    catalog = _load_json(catalog_path, "catalog")
    manifest_value = _exact_object(
        manifest.value,
        {
            "format",
            "scope",
            "baseline",
            "seed_inventory",
            "canonical_case_catalog",
            "bindings",
            "implementation_order",
            "statuses",
            "proof_kinds",
            "proof_profiles",
            "binding_profiles",
            "owners",
            "capabilities",
            "seed_workflows",
            "non_operations",
        },
        "manifest",
    )
    if manifest_value["format"] != "typebridge.sdk-conformance/v1":
        raise ContractError("invalid_manifest_format", "manifest format is not v1")
    if manifest_value["proof_kinds"] != list(PROOF_KINDS):
        raise ContractError("invalid_proof_kinds", "manifest proof kinds are not frozen")

    (
        cases,
        selected,
        projection_targets,
        semantic_fingerprint,
        projection_fingerprints,
    ) = _validate_catalog(catalog.value, manifest_value)
    catalog_fixture = catalog.value["fixture"]
    journey_path = _contract_path(catalog.value["journey_path"], JOURNEY_RELATIVE, "journey path")
    report_schema_path = _contract_path(
        catalog.value["report_schema_path"], REPORT_SCHEMA_RELATIVE, "report schema path"
    )
    schema_path = _contract_path(catalog_fixture["schema_path"], SCHEMA_RELATIVE, "schema path")
    provider_path = _contract_path(
        catalog_fixture["provider_schema_path"],
        PROVIDER_SCHEMA_RELATIVE,
        "provider schema path",
    )
    journey = _load_json(journey_path, "journey")
    report_schema = _load_json(report_schema_path, "report schema")
    schema = LoadedJson(schema_path, schema_path.read_bytes(), None)
    provider_schema = LoadedJson(provider_path, provider_path.read_bytes(), None)
    observations = _validate_journey(journey.value)
    _validate_report_schema(report_schema.value)
    for _, _, observation_ref in selected:
        if observation_ref not in observations:
            raise ContractError(
                "unknown_observation_ref", f"selected proof references {observation_ref!r}"
            )
    return Contracts(
        manifest=manifest,
        catalog=catalog,
        journey=journey,
        report_schema=report_schema,
        schema=schema,
        provider_schema=provider_schema,
        capabilities=tuple(manifest_value["capabilities"]),
        cases=cases,
        selected=selected,
        observations=observations,
        projection_targets=projection_targets,
        semantic_fingerprint=semantic_fingerprint,
        projection_fingerprints=projection_fingerprints,
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
    if fingerprint["semantic_profile"] != "typedb-3.12.1/v1":
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


def _validate_report(report: LoadedJson, contracts: Contracts) -> dict[str, Any]:
    value = _exact_object(
        report.value,
        {"format", "binding", "manifest", "catalog", "fixture", "results"},
        f"report {report.path}",
    )
    if value["format"] != REPORT_FORMAT:
        raise ContractError("invalid_report_format", f"{report.path} has the wrong format")
    binding = _string(value["binding"], "report binding")
    if binding not in REPORT_BINDINGS:
        raise ContractError("unknown_report_binding", f"unknown report binding {binding!r}")
    _validate_source_identity(
        value["manifest"],
        expected_path=MANIFEST_RELATIVE,
        expected_sha256=contracts.manifest.sha256,
        label="manifest identity",
        digest_code="manifest_digest_mismatch",
    )
    _validate_source_identity(
        value["catalog"],
        expected_path=CATALOG_RELATIVE,
        expected_sha256=contracts.catalog.sha256,
        label="catalog identity",
        digest_code="catalog_digest_mismatch",
    )
    fixture = _exact_object(
        value["fixture"],
        {
            "id",
            "version",
            "semantic_profile",
            "schema",
            "provider_schema",
            "journey",
            "semantic_fingerprint",
            "projection_target",
            "projection_fingerprint",
        },
        "report fixture",
    )
    if fixture["id"] != "sdk-v1" or _integer(fixture["version"], "fixture version") != 1:
        raise ContractError("invalid_fixture_identity", f"{binding} report fixture is not v1")
    if fixture["semantic_profile"] != "typedb-3.12.1/v1":
        raise ContractError("semantic_profile_mismatch", f"{binding} report profile is invalid")
    _validate_source_identity(
        fixture["schema"],
        expected_path=SCHEMA_RELATIVE,
        expected_sha256=contracts.schema.sha256,
        label="schema identity",
        digest_code="schema_digest_mismatch",
    )
    _validate_source_identity(
        fixture["provider_schema"],
        expected_path=PROVIDER_SCHEMA_RELATIVE,
        expected_sha256=contracts.provider_schema.sha256,
        label="provider schema identity",
        digest_code="provider_schema_digest_mismatch",
    )
    _validate_source_identity(
        fixture["journey"],
        expected_path=JOURNEY_RELATIVE,
        expected_sha256=contracts.journey.sha256,
        label="journey identity",
        digest_code="journey_digest_mismatch",
    )
    semantic_fingerprint = _validate_fingerprint(
        fixture["semantic_fingerprint"],
        label="semantic fingerprint",
        domain="typebridge.schema.semantic",
        canonicalization="typebridge.schema-canonical-json/v1",
    )
    if semantic_fingerprint != contracts.semantic_fingerprint:
        raise ContractError(
            "semantic_fingerprint_authority_mismatch",
            f"{binding} report semantic fingerprint does not match the frozen fixture",
        )
    expected_target = contracts.projection_targets[binding]
    if fixture["projection_target"] != expected_target:
        raise ContractError(
            "projection_target_mismatch",
            f"{binding} report target is {fixture['projection_target']!r}, expected {expected_target!r}",
        )
    projection_fingerprint = _validate_fingerprint(
        fixture["projection_fingerprint"],
        label="projection fingerprint",
        domain="typebridge.binding.projection",
        canonicalization="typebridge.binding-projection/v1",
    )
    if projection_fingerprint != contracts.projection_fingerprints[binding]:
        raise ContractError(
            "projection_fingerprint_authority_mismatch",
            f"{binding} report projection fingerprint does not match the frozen fixture",
        )

    results = _exact_list(value["results"], "report results")
    observed_keys: list[tuple[str, str]] = []
    normalized_results: list[dict[str, Any]] = []
    for index, result_value in enumerate(results):
        result = _exact_object(
            result_value,
            {"case_id", "capability_id", "proof_kind", "outcome", "observation"},
            f"report result {index}",
        )
        case_id = _string(result["case_id"], f"result {index} case ID")
        proof_kind = _string(result["proof_kind"], f"result {index} proof kind")
        if case_id not in contracts.cases:
            raise ContractError("unknown_result_case", f"report contains unknown case {case_id!r}")
        if proof_kind not in PROOF_KINDS:
            raise ContractError(
                "unknown_proof_kind", f"report contains unknown proof {proof_kind!r}"
            )
        entry = contracts.cases[case_id]
        capability = entry["capability"]
        if result["capability_id"] != capability["id"]:
            raise ContractError(
                "capability_mapping_mismatch", f"{case_id!r} maps to the wrong capability"
            )
        statuses = _expand_binding_profile(contracts.manifest.value, capability["binding_profile"])
        status = statuses.get(binding)
        if status == "gap":
            raise ContractError(
                "manifest_gap_pass", f"{binding} cannot pass manifest gap {capability['id']!r}"
            )
        if status == "planned":
            raise ContractError(
                "planned_binding_pass",
                f"{binding} cannot pass planned capability {capability['id']!r}",
            )
        proof_profile = contracts.manifest.value["proof_profiles"][capability["proof_profile"]]
        if proof_profile.get(proof_kind) != "required":
            raise ContractError(
                "proof_not_applicable", f"{case_id!r}/{proof_kind!r} is not applicable"
            )
        if result["outcome"] != "passed":
            raise ContractError("nonpassing_result", f"{case_id!r}/{proof_kind!r} did not pass")
        if type(result["observation"]) is not dict:
            raise ContractError("invalid_observation", "result observation must be an object")
        _inspect_observation(result["observation"], f"{binding} {case_id}/{proof_kind}")
        key = (case_id, proof_kind)
        if key in observed_keys:
            raise ContractError("duplicate_result", f"duplicate result {key!r}")
        observed_keys.append(key)
        normalized_results.append(result)

    if observed_keys != sorted(observed_keys):
        raise ContractError("result_order_mismatch", f"{binding} results are not sorted")
    expected_keys = [(case_id, proof_kind) for case_id, proof_kind, _ in contracts.selected]
    if observed_keys != expected_keys:
        raise ContractError(
            "result_coverage_mismatch",
            f"{binding} result keys differ from the selected proof set",
        )
    observations_by_ref: dict[str, dict[str, Any]] = {}
    for result, (_, _, observation_ref) in zip(normalized_results, contracts.selected, strict=True):
        previous = observations_by_ref.setdefault(observation_ref, result["observation"])
        if result["observation"] != previous:
            raise ContractError(
                "observation_ref_mismatch",
                f"{binding} rows for {observation_ref!r} do not agree",
            )
    for observation_ref, observation in observations_by_ref.items():
        if observation != contracts.observations[observation_ref]:
            raise ContractError(
                "observation_mismatch",
                f"{binding} observation {observation_ref!r} differs from the journey",
            )
    return {
        "binding": binding,
        "semantic_fingerprint": semantic_fingerprint,
        "projection_target": expected_target,
        "projection_fingerprint": projection_fingerprint,
        "results": normalized_results,
    }


def _derive_summary(reports: dict[str, dict[str, Any]], contracts: Contracts) -> dict[str, Any]:
    first = reports[REPORT_BINDINGS[0]]
    passed_proofs = [
        {
            "case_id": result["case_id"],
            "capability_id": result["capability_id"],
            "proof_kind": result["proof_kind"],
            "observation": result["observation"],
        }
        for result in first["results"]
    ]
    covered = {(item["case_id"], item["proof_kind"]) for item in passed_proofs}

    current_gaps: list[dict[str, Any]] = []
    uncovered: list[dict[str, Any]] = []
    for capability in contracts.capabilities:
        case_id = capability["case_ids"][0]
        statuses = _expand_binding_profile(contracts.manifest.value, capability["binding_profile"])
        gap_bindings = [binding for binding in REPORT_BINDINGS if statuses.get(binding) == "gap"]
        if gap_bindings:
            reason = capability.get("gap_reason")
            if type(reason) is not str or not reason.strip():
                raise ContractError(
                    "missing_gap_reason", f"manifest gap {capability['id']!r} has no reason"
                )
            current_gaps.append(
                {
                    "bindings": gap_bindings,
                    "capability_id": capability["id"],
                    "case_id": case_id,
                    "reason": reason,
                }
            )
        profile = contracts.manifest.value["proof_profiles"][capability["proof_profile"]]
        for proof_kind in PROOF_KINDS:
            if profile[proof_kind] != "required" or (case_id, proof_kind) in covered:
                continue
            bindings = [
                binding
                for binding in REPORT_BINDINGS
                if statuses.get(binding) in {"accepted_offline", "accepted_live"}
            ]
            if bindings:
                uncovered.append(
                    {
                        "bindings": bindings,
                        "capability_id": capability["id"],
                        "case_id": case_id,
                        "proof_kind": proof_kind,
                    }
                )

    return {
        "format": SUMMARY_FORMAT,
        "bindings": list(REPORT_BINDINGS),
        "fixture": {
            "id": "sdk-v1",
            "version": 1,
            "semantic_profile": "typedb-3.12.1/v1",
            "manifest": {"path": MANIFEST_RELATIVE, "sha256": contracts.manifest.sha256},
            "catalog": {"path": CATALOG_RELATIVE, "sha256": contracts.catalog.sha256},
            "schema": {"path": SCHEMA_RELATIVE, "sha256": contracts.schema.sha256},
            "provider_schema": {
                "path": PROVIDER_SCHEMA_RELATIVE,
                "sha256": contracts.provider_schema.sha256,
            },
            "journey": {"path": JOURNEY_RELATIVE, "sha256": contracts.journey.sha256},
            "semantic_fingerprint": first["semantic_fingerprint"],
            "projection_fingerprints": {
                binding: {
                    "target": reports[binding]["projection_target"],
                    "fingerprint": reports[binding]["projection_fingerprint"],
                }
                for binding in REPORT_BINDINGS
            },
        },
        "passed_proofs": passed_proofs,
        "current_gaps": current_gaps,
        "uncovered_required_proofs": uncovered,
    }


def compare_reports(report_paths: list[Path]) -> dict[str, Any]:
    """Validate three reports and return a deterministic derived summary."""

    contracts = load_contracts()
    loaded = [_load_json(path, f"report {path}", require_canonical=True) for path in report_paths]
    reports: dict[str, dict[str, Any]] = {}
    for report in loaded:
        validated = _validate_report(report, contracts)
        binding = validated["binding"]
        if binding in reports:
            raise ContractError("duplicate_binding", f"received two {binding!r} reports")
        reports[binding] = validated
    missing = sorted(CURRENT_BINDINGS - reports.keys())
    extra = sorted(reports.keys() - CURRENT_BINDINGS)
    if missing:
        raise ContractError("missing_binding", f"missing reports for {missing}")
    if extra:
        raise ContractError("unexpected_binding", f"unexpected reports for {extra}")

    semantic_fingerprints = {
        canonical_json_bytes(reports[binding]["semantic_fingerprint"])
        for binding in REPORT_BINDINGS
    }
    if len(semantic_fingerprints) != 1:
        raise ContractError(
            "semantic_fingerprint_mismatch", "report semantic fingerprints do not match"
        )
    projection_fingerprints = {
        canonical_json_bytes(reports[binding]["projection_fingerprint"])
        for binding in REPORT_BINDINGS
    }
    if len(projection_fingerprints) != len(REPORT_BINDINGS):
        raise ContractError(
            "projection_fingerprint_collision", "binding projection fingerprints must differ"
        )
    first_results = reports[REPORT_BINDINGS[0]]["results"]
    for binding in REPORT_BINDINGS[1:]:
        if reports[binding]["results"] != first_results:
            raise ContractError(
                "cross_binding_observation_mismatch",
                f"{binding} results differ from {REPORT_BINDINGS[0]}",
            )
    return _derive_summary(reports, contracts)


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "reports",
        nargs=3,
        type=Path,
        metavar="REPORT",
        help="one canonical report each for Python, Node, and Rust (order-independent)",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    arguments = _parser().parse_args(argv)
    try:
        summary = compare_reports(arguments.reports)
    except (ContractError, OSError) as error:
        print(f"sdk conformance rejected: {error}", file=sys.stderr)
        return 1
    sys.stdout.buffer.write(canonical_json_bytes(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
