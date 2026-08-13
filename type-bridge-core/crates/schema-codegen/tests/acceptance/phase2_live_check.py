#!/usr/bin/env python3
"""Produce Python's exact-TypeDB-3.12.1 Phase-2 live subset report."""

from __future__ import annotations

import hashlib
import importlib
import json
import os
import stat
import struct
import sys
from collections.abc import Callable, Mapping, Sequence
from datetime import UTC, date, datetime, timedelta
from decimal import Decimal
from pathlib import Path
from typing import Any

from type_bridge import Database

REPORT_FORMAT = "typebridge.phase2-projected-live-report/v1"
SEMANTIC_PROFILE = "typedb-3.12.1/v1"
SEMANTIC_SCHEMA_FINGERPRINT = {
    "algorithm": "sha256",
    "canonicalization": "typebridge.schema-canonical-json/v1",
    "digest": "3c8d072b60c575b4c0381c1ea44088a9c18e01ed52e90730311aadccb72cd0c8",
    "domain": "typebridge.schema.semantic",
    "semantic_profile": SEMANTIC_PROFILE,
}
OUTPUT_ENV = "TYPE_BRIDGE_PHASE2_LIVE_REPORT"
ADDRESS_ENV = "TYPE_BRIDGE_PHASE2_LIVE_ADDRESS"
DATABASE_ENV = "TYPE_BRIDGE_PHASE2_LIVE_DATABASE"
HTTP_PORT_ENV = "TYPE_BRIDGE_PHASE2_LIVE_HTTP_PORT"
PACKAGE_ROOT_ENV = "TYPE_BRIDGE_PHASE2_PYTHON_PACKAGE_ROOT"
REPOSITORY_ROOT_ENV = "TYPE_BRIDGE_PHASE2_REPOSITORY_ROOT"
MAX_REPORT_BYTES = 256 * 1024
MAX_AUTHORITY_BYTES = 1024 * 1024

SOURCE_PATHS = {
    "schema": "tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml",
    "journey": "tests/contracts/sdk_conformance/workforce-v3/journey-v3.json",
    "provider": ("tests/contracts/sdk_conformance/workforce-v3/provider-3.12.1-v3.tql"),
}
SOURCE_SHA256 = {
    "schema": "74b9161e3d8fd70a1f22c3a8b7fc70a823b18cb07973064e2e10ce0940f76f1f",
    "journey": "912130753fe7938a38c054cff16e202b312551a6aa65265148628bfbd2abbbef",
    "provider": "af61aaece22d666e19d2c1357f8ebc39b8cbcf4bff7b98a19a56daf171a1faba",
}
OBSERVATION_REFS = (
    "canonical_scalar_values",
    "cleanup",
    "inherited_plain_activity_role_lifecycle",
    "integer_key_polymorphic_optional_role",
    "relation_as_player",
)
LIVE_REFS = frozenset(
    {
        "container-event",
        "data-ada",
        "data-dana",
        "event-ada",
        "interaction-absent",
        "interaction-person",
        "interaction-robot",
        "plain-activity-ada",
        "robot-7",
        "robot-negative-7",
    }
)
SCALAR_FIELDS = {
    "boolean": "val_bool",
    "date": "val_date",
    "datetime": "val_datetime",
    "datetime_tz": "val_datetime_tz",
    "decimal": "val_decimal",
    "double": "val_double",
    "duration": "val_duration",
    "long": "score",
    "string": "nickname",
}


class ProducerError(RuntimeError):
    """A stable fail-closed live-producer rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code


def _duplicate_key_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            raise ProducerError("duplicate_json_key", f"duplicate JSON key {key!r}")
        value[key] = item
    return value


def _canonical(value: object) -> bytes:
    try:
        text = json.dumps(
            value,
            allow_nan=False,
            ensure_ascii=False,
            separators=(",", ":"),
            sort_keys=True,
        )
    except (RecursionError, TypeError, ValueError) as error:
        raise ProducerError("invalid_report_value", "report cannot be canonicalized") from error
    return f"{text}\n".encode()


def _bounded_regular_bytes(path: Path, label: str) -> bytes:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise ProducerError("invalid_authority", f"{label} cannot be inspected") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
        raise ProducerError("invalid_authority", f"{label} must be a regular file")
    if metadata.st_size > MAX_AUTHORITY_BYTES:
        raise ProducerError("authority_size_limit", f"{label} is too large")
    try:
        raw = path.read_bytes()
    except OSError as error:
        raise ProducerError("invalid_authority", f"{label} cannot be read") from error
    if len(raw) > MAX_AUTHORITY_BYTES:
        raise ProducerError("authority_size_limit", f"{label} is too large")
    return raw


def _source_authority(root: Path) -> tuple[dict[str, dict[str, str]], dict[str, bytes]]:
    authority: dict[str, dict[str, str]] = {}
    sources: dict[str, bytes] = {}
    for label, relative in SOURCE_PATHS.items():
        raw = _bounded_regular_bytes(root / relative, f"Phase-2 {label}")
        digest = hashlib.sha256(raw).hexdigest()
        if digest != SOURCE_SHA256[label]:
            raise ProducerError(
                "authority_hash_mismatch",
                f"Phase-2 {label} is not the frozen exact-3.12.1 authority",
            )
        authority[label] = {"path": relative, "sha256": digest}
        sources[label] = raw
    return authority, sources


def _required_environment(name: str, environment: Mapping[str, str]) -> str:
    value = environment.get(name)
    if value is None or not value:
        raise ProducerError("missing_environment", f"{name} must be set and non-empty")
    try:
        encoded = value.encode("utf-8")
    except UnicodeEncodeError as error:
        raise ProducerError("invalid_environment", f"{name} must be UTF-8") from error
    if len(encoded) > 4096 or any(ord(character) < 32 for character in value):
        raise ProducerError("invalid_environment", f"{name} is not a bounded text value")
    return value


def _real_directory(value: str, label: str) -> Path:
    path = Path(value)
    if not path.is_absolute():
        raise ProducerError("invalid_directory", f"{label} must be absolute")
    try:
        metadata = path.lstat()
    except OSError as error:
        raise ProducerError("invalid_directory", f"{label} cannot be inspected") from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
        raise ProducerError("invalid_directory", f"{label} must be a real directory")
    return path


def _http_port(environment: Mapping[str, str]) -> int:
    value = _required_environment(HTTP_PORT_ENV, environment)
    if not value or any(character < "0" or character > "9" for character in value):
        raise ProducerError("invalid_http_port", f"{HTTP_PORT_ENV} must be an ASCII integer")
    port = int(value)
    if port < 1 or port > 65535:
        raise ProducerError("invalid_http_port", f"{HTTP_PORT_ENV} must be in 1..65535")
    return port


def _output_path(environment: Mapping[str, str] = os.environ) -> Path:
    value = _required_environment(OUTPUT_ENV, environment)
    path = Path(value)
    if not path.is_absolute() or path.name in {"", ".", ".."}:
        raise ProducerError("invalid_output_path", "report path must be an absolute file path")
    _validate_output_target(path)
    return path


def _validate_output_target(path: Path) -> None:
    try:
        parent = path.parent.lstat()
    except OSError as error:
        raise ProducerError("invalid_output_parent", "report parent cannot be inspected") from error
    if stat.S_ISLNK(parent.st_mode) or not stat.S_ISDIR(parent.st_mode):
        raise ProducerError("invalid_output_parent", "report parent must be a real directory")
    try:
        path.lstat()
    except FileNotFoundError:
        return
    except OSError as error:
        raise ProducerError("invalid_output_path", "report path cannot be inspected") from error
    raise ProducerError("output_exists", "report destination already exists")


def _publish_report(path: Path, report: object) -> None:
    payload = _canonical(report)
    if len(payload) > MAX_REPORT_BYTES:
        raise ProducerError("report_size_limit", "canonical report is too large")
    _validate_output_target(path)
    try:
        with path.open("xb") as destination:
            destination.write(payload)
            destination.flush()
            os.fsync(destination.fileno())
    except FileExistsError as error:
        raise ProducerError("output_exists", "report destination already exists") from error
    except OSError as error:
        raise ProducerError("output_write_failed", "report could not be written") from error


def _load_package(package_root: Path) -> Any:
    package_directory = package_root / "generated_phase2"
    initializer = package_directory / "__init__.py"
    for path, label, directory in (
        (package_directory, "generated package", True),
        (initializer, "generated package initializer", False),
    ):
        try:
            metadata = path.lstat()
        except OSError as error:
            raise ProducerError("generated_package_import", f"{label} is absent") from error
        expected = stat.S_ISDIR(metadata.st_mode) if directory else stat.S_ISREG(metadata.st_mode)
        if stat.S_ISLNK(metadata.st_mode) or not expected:
            raise ProducerError(
                "generated_package_import", f"{label} must be a real filesystem object"
            )
    sys.path.insert(0, str(package_root))
    try:
        package = importlib.import_module("generated_phase2")
    except ImportError as error:
        raise ProducerError(
            "generated_package_import", "generated Python package cannot be imported"
        ) from error
    package_file = getattr(package, "__file__", None)
    if package_file is None or Path(package_file).resolve() != initializer.resolve():
        raise ProducerError(
            "generated_package_import", "generated Python package resolved outside its root"
        )
    try:
        semantic = json.loads(package.SEMANTIC_SCHEMA_FINGERPRINT_JSON)
    except (AttributeError, json.JSONDecodeError, TypeError) as error:
        raise ProducerError(
            "generated_package_identity", "generated package has no semantic identity"
        ) from error
    if type(semantic) is not dict or semantic != SEMANTIC_SCHEMA_FINGERPRINT:
        raise ProducerError(
            "generated_package_identity",
            "generated package is not the frozen Workforce V3 schema projection",
        )
    installed = getattr(package.Person, "__runtime_projection__", None)
    if installed is None or any(
        getattr(model, "__runtime_projection__", None) is not installed
        for model in (
            package.Container,
            package.Event,
            package.Interaction,
            package.Person,
            package.PlainActivity,
            package.Robot,
        )
    ):
        raise ProducerError(
            "generated_package_identity", "generated models do not share one runtime projection"
        )
    return package


def _json_object(raw: bytes, label: str) -> dict[str, Any]:
    try:
        value = json.loads(raw.decode("utf-8"), object_pairs_hook=_duplicate_key_object)
    except ProducerError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError, RecursionError) as error:
        raise ProducerError("invalid_journey", f"{label} is not valid UTF-8 JSON") from error
    if type(value) is not dict:
        raise ProducerError("invalid_journey", f"{label} must be an object")
    return value


def _record_index(
    journey: dict[str, Any],
) -> tuple[dict[str, dict[str, Any]], list[str], list[str]]:
    if (
        journey.get("format") != "typebridge.workforce-journey/v3"
        or journey.get("fixture_id") != "workforce-v3"
        or journey.get("version") != 3
        or journey.get("semantic_profile") != SEMANTIC_PROFILE
    ):
        raise ProducerError("invalid_journey", "workforce-v3 journey identity drifted")
    groups = journey.get("records")
    if type(groups) is not dict:
        raise ProducerError("invalid_journey", "workforce-v3 records are malformed")
    index: dict[str, dict[str, Any]] = {}
    for group in (
        "people",
        "robots",
        "counters",
        "memberships",
        "network_links",
        "interactions",
    ):
        records = groups.get(group)
        if type(records) is not list:
            raise ProducerError("invalid_journey", f"journey group {group!r} is malformed")
        for record in records:
            _index_record(index, record, group)
    for group in ("plain_activity", "event", "container"):
        _index_record(index, groups.get(group), group)
    create_order = journey.get("create_order")
    cleanup_order = journey.get("cleanup_order")
    if (
        type(create_order) is not list
        or type(cleanup_order) is not list
        or any(type(ref) is not str for ref in create_order + cleanup_order)
        or len(set(create_order)) != len(create_order)
        or set(create_order) != set(index)
        or cleanup_order != list(reversed(create_order))
        or not LIVE_REFS.issubset(index)
    ):
        raise ProducerError("invalid_journey", "journey operation order is not exact")
    return index, create_order, cleanup_order


def _index_record(index: dict[str, dict[str, Any]], record: object, group: str) -> None:
    if type(record) is not dict:
        raise ProducerError("invalid_journey", f"journey record in {group!r} is malformed")
    ref = record.get("ref")
    model = record.get("model")
    if type(ref) is not str or not ref or type(model) is not str or not model:
        raise ProducerError("invalid_journey", f"journey record in {group!r} has no identity")
    if ref in index:
        raise ProducerError("invalid_journey", f"duplicate journey record {ref!r}")
    index[ref] = record


def _record(records: dict[str, dict[str, Any]], ref: str, model: str) -> dict[str, Any]:
    record = records.get(ref)
    if type(record) is not dict or record.get("model") != model:
        raise ProducerError("invalid_journey", f"journey record {ref!r} is absent or changed")
    return record


def _fields(record: dict[str, Any], label: str) -> dict[str, Any]:
    fields = record.get("fields")
    if type(fields) is not dict:
        raise ProducerError("invalid_journey", f"{label} fields are malformed")
    return fields


def _field(fields: dict[str, Any], name: str, kind: str) -> dict[str, Any]:
    value = fields.get(name)
    required = {"kind", "bits"} if kind == "double" else {"kind", "value"}
    if type(value) is not dict or set(value) != required or value.get("kind") != kind:
        raise ProducerError("invalid_journey", f"field {name!r} is not an exact {kind} value")
    return value


def _primitive(field: dict[str, Any]) -> object:
    kind = field["kind"]
    if kind == "double":
        bits = field["bits"]
        if type(bits) is not str or len(bits) != 16:
            raise ProducerError("invalid_journey", "double bits are malformed")
        try:
            return struct.unpack(">d", bytes.fromhex(bits))[0]
        except (ValueError, struct.error) as error:
            raise ProducerError("invalid_journey", "double bits are malformed") from error
    value = field["value"]
    if kind == "boolean" and type(value) is bool:
        return value
    if kind == "long" and type(value) is str:
        try:
            return int(value)
        except ValueError as error:
            raise ProducerError("invalid_journey", "long value is malformed") from error
    if kind == "string" and type(value) is str:
        return value
    if kind == "decimal" and type(value) is str:
        return Decimal(value)
    if kind == "date" and type(value) is str:
        return date.fromisoformat(value)
    if kind == "datetime" and type(value) is str:
        parsed = datetime.fromisoformat(value)
        if parsed.tzinfo is not None:
            raise ProducerError("invalid_journey", "datetime must be timezone-naive")
        return parsed
    if kind == "datetime_tz" and type(value) is str and value.endswith("Z"):
        return datetime.fromisoformat(value[:-1]).replace(tzinfo=UTC)
    if kind == "duration" and type(value) is str and value.startswith("PT") and value.endswith("S"):
        try:
            return timedelta(seconds=int(value[2:-1]))
        except ValueError as error:
            raise ProducerError("invalid_journey", "duration value is malformed") from error
    raise ProducerError("invalid_journey", f"unsupported authored scalar {kind!r}")


def _attribute(package: Any, class_name: str, field: dict[str, Any]) -> object:
    return getattr(package, class_name)(_primitive(field))


def _person(package: Any, record: dict[str, Any]) -> object:
    fields = _fields(record, "person")
    if type(fields.get("aliases")) is not list:
        raise ProducerError("invalid_journey", "person list field is malformed")
    values: dict[str, object] = {
        "identifier": _attribute(package, "Identifier", _field(fields, "identifier", "string")),
        "score": _attribute(package, "Score", _field(fields, "score", "long")),
        "val_bool": _attribute(package, "ValBool", _field(fields, "val_bool", "boolean")),
        "val_constrained": _attribute(
            package,
            "ValConstrained",
            _field(fields, "val_constrained", "long"),
        ),
        "val_date": _attribute(package, "ValDate", _field(fields, "val_date", "date")),
        "val_datetime": _attribute(
            package,
            "ValDatetime",
            _field(fields, "val_datetime", "datetime"),
        ),
        "val_datetime_tz": _attribute(
            package,
            "ValDatetimeTz",
            _field(fields, "val_datetime_tz", "datetime_tz"),
        ),
        "val_decimal": _attribute(
            package,
            "ValDecimal",
            _field(fields, "val_decimal", "decimal"),
        ),
        "val_double": _attribute(
            package,
            "ValDouble",
            _field(fields, "val_double", "double"),
        ),
        "val_duration": _attribute(
            package,
            "ValDuration",
            _field(fields, "val_duration", "duration"),
        ),
    }
    optional = (
        ("nickname", "Nickname", "string"),
        ("foo__bar", "FooBar", "long"),
        ("score__gte", "ScoreGte", "long"),
    )
    for field_name, class_name, kind in optional:
        if field_name in fields:
            values[field_name] = _attribute(package, class_name, _field(fields, field_name, kind))
    return package.Person(**values)


def _robot(package: Any, record: dict[str, Any]) -> object:
    fields = _fields(record, "robot")
    return package.Robot(
        robot_id=_attribute(package, "RobotId", _field(fields, "robot_id", "long")),
        val_constrained=_attribute(
            package,
            "ValConstrained",
            _field(fields, "val_constrained", "long"),
        ),
    )


def _model_label(value: object) -> str:
    try:
        identity = json.loads(type(value).__type_id__)
    except (AttributeError, json.JSONDecodeError, TypeError) as error:
        raise ProducerError("invalid_hydration", "hydrated value has no model identity") from error
    label = identity.get("label") if type(identity) is dict else None
    if type(label) is not str:
        raise ProducerError("invalid_hydration", "hydrated value has no model label")
    return label


def _report_scalar(kind: str, value: object) -> dict[str, object]:
    if kind == "double" and type(value) is float:
        return {"bits": struct.pack(">d", value).hex(), "kind": kind}
    if kind == "long" and type(value) is int:
        return {"kind": kind, "value": str(value)}
    if kind == "boolean" and type(value) is bool:
        return {"kind": kind, "value": value}
    if kind == "date" and type(value) is date and not isinstance(value, datetime):
        return {"kind": kind, "value": value.isoformat()}
    if kind == "datetime" and type(value) is datetime and value.tzinfo is None:
        return {"kind": kind, "value": value.isoformat()}
    if kind == "datetime_tz" and type(value) is datetime and value.utcoffset() == timedelta(0):
        return {"kind": kind, "value": value.isoformat().replace("+00:00", "Z")}
    if kind == "decimal" and type(value) is Decimal:
        return {"kind": kind, "value": str(value)}
    if kind == "duration" and type(value) is timedelta:
        seconds = value.total_seconds()
        if seconds.is_integer() and seconds >= 0:
            return {"kind": kind, "value": f"PT{int(seconds)}S"}
    if kind == "string" and type(value) is str:
        return {"kind": kind, "value": value}
    raise ProducerError("invalid_hydration", f"hydrated {kind} scalar has the wrong type")


def _canonical_scalars(package: Any, person: object) -> dict[str, object]:
    if type(person) is not package.Person:
        raise ProducerError("invalid_hydration", "Ada did not hydrate as Person")
    hydrated: dict[str, dict[str, object]] = {}
    for kind, field_name in SCALAR_FIELDS.items():
        attribute = getattr(person, field_name)
        hydrated[kind] = _report_scalar(kind, attribute.value)
    return {"hydrated": hydrated, "model": _model_label(person), "ref": "data-ada"}


def _actor_identity(package: Any, actor: object | None) -> dict[str, str] | None:
    if actor is None:
        return None
    model = _model_label(actor)
    if type(actor) is package.Person:
        key = actor.identifier.value
    elif type(actor) is package.Robot:
        key = str(actor.robot_id.value)
    else:
        raise ProducerError("invalid_hydration", "Interaction actor has an unknown model")
    return {"key": key, "model": model}


def _insert(
    ref: str,
    manager: Any,
    value: object,
    created: dict[str, tuple[Any, object, str]],
    actual_order: list[str],
) -> object:
    if manager.insert(value) is not value:
        raise ProducerError("invalid_crud_result", f"insert changed result identity for {ref}")
    iid = getattr(value, "iid", None)
    if type(iid) is not str or not iid:
        raise ProducerError("invalid_crud_result", f"insert did not attach an IID for {ref}")
    created[ref] = (manager, value, iid)
    actual_order.append(ref)
    hydrated = manager.get_by_iid(iid)
    if hydrated is None:
        raise ProducerError("invalid_crud_result", f"inserted value {ref} did not hydrate")
    return hydrated


def _cleanup_created(
    created: dict[str, tuple[Any, object, str]],
    cleanup_order: Sequence[str],
    managers: Mapping[str, Any],
) -> tuple[list[str], list[dict[str, object]], dict[str, int], dict[str, bool]]:
    actual_order: list[str] = []
    counts_at_delete: dict[str, int] = {}
    reads_after_delete: dict[str, bool] = {}
    failures: list[BaseException] = []
    for ref in cleanup_order:
        entry = created.get(ref)
        if entry is None:
            continue
        manager, value, iid = entry
        try:
            manager.delete(value)
            reads_after_delete[ref] = manager.get_by_iid(iid) is not None
            if reads_after_delete[ref]:
                raise ProducerError("cleanup_failed", f"deleted value {ref} remained readable")
            actual_order.append(ref)
            counts_at_delete[ref] = manager.count()
        except BaseException as error:
            failures.append(error)
    zero_checks: list[dict[str, object]] = []
    for model in sorted(managers):
        try:
            count = managers[model].count()
            zero_checks.append({"count_after_cleanup": count, "model": model})
            if count != 0:
                raise ProducerError("cleanup_failed", f"model {model} retained {count} live values")
        except BaseException as error:
            failures.append(error)
    if failures:
        raise ProducerError(
            "cleanup_failed", f"{len(failures)} generated CRUD cleanup operation(s) failed"
        ) from failures[0]
    return actual_order, zero_checks, counts_at_delete, reads_after_delete


def _run_journey(
    package: Any,
    database: Database,
    records: dict[str, dict[str, Any]],
    create_order: Sequence[str],
    cleanup_order: Sequence[str],
) -> dict[str, object]:
    managers = {
        "container": package.Container.manager(database),
        "event": package.Event.manager(database),
        "interaction": package.Interaction.manager(database),
        "person": package.Person.manager(database),
        "plain-activity": package.PlainActivity.manager(database),
        "robot": package.Robot.manager(database),
    }
    for model, manager in managers.items():
        if manager.count() != 0:
            raise ProducerError("database_not_empty", f"new database contains {model} values")

    created: dict[str, tuple[Any, object, str]] = {}
    actual_create_order: list[str] = []
    observations: dict[str, object] = {}
    plain_read_after_create = False
    plain_role_preserved = False
    cleanup_observation: (
        tuple[list[str], list[dict[str, object]], dict[str, int], dict[str, bool]] | None
    ) = None
    try:
        ada = _person(package, _record(records, "data-ada", "person"))
        dana = _person(package, _record(records, "data-dana", "person"))
        hydrated_ada = _insert("data-ada", managers["person"], ada, created, actual_create_order)
        hydrated_dana = _insert("data-dana", managers["person"], dana, created, actual_create_order)
        observations["canonical_scalar_values"] = _canonical_scalars(package, hydrated_ada)

        robot_7 = _robot(package, _record(records, "robot-7", "robot"))
        robot_negative_7 = _robot(package, _record(records, "robot-negative-7", "robot"))
        hydrated_robot_7 = _insert(
            "robot-7", managers["robot"], robot_7, created, actual_create_order
        )
        hydrated_robot_negative_7 = _insert(
            "robot-negative-7",
            managers["robot"],
            robot_negative_7,
            created,
            actual_create_order,
        )

        interaction_records = {
            ref: _record(records, ref, "interaction")
            for ref in (
                "interaction-robot",
                "interaction-absent",
                "interaction-person",
            )
        }
        interactions = {
            "interaction-robot": package.Interaction(
                identifier=_attribute(
                    package,
                    "Identifier",
                    _field(
                        _fields(interaction_records["interaction-robot"], "Interaction"),
                        "identifier",
                        "string",
                    ),
                ),
                actor=hydrated_robot_7,
                target=hydrated_ada,
            ),
            "interaction-absent": package.Interaction(
                identifier=_attribute(
                    package,
                    "Identifier",
                    _field(
                        _fields(interaction_records["interaction-absent"], "Interaction"),
                        "identifier",
                        "string",
                    ),
                ),
                target=hydrated_dana,
            ),
            "interaction-person": package.Interaction(
                identifier=_attribute(
                    package,
                    "Identifier",
                    _field(
                        _fields(interaction_records["interaction-person"], "Interaction"),
                        "identifier",
                        "string",
                    ),
                ),
                actor=hydrated_ada,
                target=hydrated_dana,
            ),
        }
        hydrated_interactions: dict[str, object] = {}
        for ref in ("interaction-robot", "interaction-absent", "interaction-person"):
            hydrated_interactions[ref] = _insert(
                ref,
                managers["interaction"],
                interactions[ref],
                created,
                actual_create_order,
            )

        plain = package.PlainActivity(participant=hydrated_ada)
        hydrated_plain = _insert(
            "plain-activity-ada",
            managers["plain-activity"],
            plain,
            created,
            actual_create_order,
        )
        role_fact = package.PlainActivity.participant.fact.get("role")
        if type(role_fact) is not dict:
            raise ProducerError("invalid_hydration", "PlainActivity role fact is absent")
        plain_read_after_create = (
            type(hydrated_plain) is package.PlainActivity
            and type(hydrated_plain.participant) is package.Person
            and hydrated_plain.participant.identifier.value == "data-ada"
        )
        plain_role_preserved = (
            role_fact.get("declaring_relation") == "base-activity"
            and role_fact.get("label") == "participant"
        )
        if not plain_read_after_create or not plain_role_preserved:
            raise ProducerError("invalid_hydration", "inherited PlainActivity role drifted")

        event = package.Event(subject=hydrated_ada)
        hydrated_event = _insert(
            "event-ada", managers["event"], event, created, actual_create_order
        )
        event_iid = getattr(hydrated_event, "iid", None)
        if type(event_iid) is not str:
            raise ProducerError("invalid_hydration", "Event hydration lost its IID")
        container = package.Container(item=[package.EventRef(event_iid)])
        hydrated_container = _insert(
            "container-event",
            managers["container"],
            container,
            created,
            actual_create_order,
        )
        items = hydrated_container.item
        relation_player_preserved = (
            type(hydrated_container) is package.Container
            and type(items) is tuple
            and len(items) == 1
            and type(items[0]) is package.EventRef
            and items[0].iid == event_iid
        )
        if not relation_player_preserved:
            raise ProducerError("invalid_hydration", "Event-as-Container player drifted")
        observations["relation_as_player"] = {
            "owner": {"model": _model_label(hydrated_container), "ref": "container-event"},
            "player": {"model": _model_label(items[0]), "ref": "event-ada"},
            "preserved": relation_player_preserved,
            "role": package.Container.item.fact["role"]["label"],
        }

        expected_live_create = [ref for ref in create_order if ref in LIVE_REFS]
        if actual_create_order != expected_live_create:
            raise ProducerError("invalid_operation_order", "live creation order drifted")

        robot_values = sorted(
            (
                ("robot-7", hydrated_robot_7),
                ("robot-negative-7", hydrated_robot_negative_7),
            ),
            key=lambda item: item[1].robot_id.value,
        )
        observations["integer_key_polymorphic_optional_role"] = {
            "integer_keys": [
                {
                    "model": _model_label(robot),
                    "ref": ref,
                    "value": str(robot.robot_id.value),
                }
                for ref, robot in robot_values
            ],
            "optional_role": package.Interaction.actor.fact["role"]["label"],
            "relation": _model_label(hydrated_interactions["interaction-person"]),
            "states": [
                {
                    "actor": _actor_identity(package, hydrated_interactions[ref].actor),
                    "relation_ref": ref,
                }
                for ref in (
                    "interaction-absent",
                    "interaction-person",
                    "interaction-robot",
                )
            ],
        }
    finally:
        cleanup_observation = _cleanup_created(
            created,
            [ref for ref in cleanup_order if ref in LIVE_REFS],
            managers,
        )

    actual_cleanup_order, zero_checks, counts_at_delete, reads_after_delete = cleanup_observation
    expected_live_cleanup = [ref for ref in cleanup_order if ref in LIVE_REFS]
    if actual_cleanup_order != expected_live_cleanup:
        raise ProducerError("invalid_operation_order", "live cleanup order drifted")
    observations["inherited_plain_activity_role_lifecycle"] = {
        "count_after_delete": counts_at_delete["plain-activity-ada"],
        "created": "plain-activity-ada" in created,
        "deleted": "plain-activity-ada" in actual_cleanup_order,
        "inherited_relation": "base-activity",
        "model": "plain-activity",
        "participant": {"key": "data-ada", "model": "person"},
        "read_after_create": plain_read_after_create,
        "read_after_delete": reads_after_delete["plain-activity-ada"],
        "ref": "plain-activity-ada",
        "role": "participant",
        "role_identity_preserved": plain_role_preserved,
    }
    observations["cleanup"] = {
        "order": actual_cleanup_order,
        "zero_checks": zero_checks,
    }
    if tuple(sorted(observations)) != tuple(sorted(OBSERVATION_REFS)):
        raise ProducerError("invalid_observation_ledger", "live observation ledger is incomplete")
    return observations


def _run_owned_database(
    package: Any,
    address: str,
    database_name: str,
    http_port: int,
    provider_schema: str,
    records: dict[str, dict[str, Any]],
    create_order: Sequence[str],
    cleanup_order: Sequence[str],
    *,
    database_factory: Callable[..., Database] = Database,
    journey_runner: Callable[
        [Any, Database, dict[str, dict[str, Any]], Sequence[str], Sequence[str]],
        dict[str, object],
    ] = _run_journey,
) -> dict[str, object]:
    database = database_factory(
        address=address,
        database=database_name,
        http_port=http_port,
    )
    owns_database = False
    try:
        database.connect()
        detected = database.detected_server_version()
        if detected != "3.12.1":
            raise ProducerError(
                "server_version_mismatch",
                "live evidence requires the actual detected TypeDB server version 3.12.1; "
                f"detected {detected!r}",
            )
        if database.database_exists():
            raise ProducerError(
                "database_preexisting",
                f"{DATABASE_ENV} must name an absent isolated database",
            )
        owns_database = True
        database.create_database()
        if not database.database_exists():
            raise ProducerError("database_create_failed", "isolated database was not created")
        database.execute_query(provider_schema, transaction_type="schema")
        return journey_runner(package, database, records, create_order, cleanup_order)
    finally:
        try:
            if owns_database:
                database.delete_database()
                if database.database_exists():
                    raise ProducerError(
                        "database_teardown_failed", "isolated database remained after cleanup"
                    )
        finally:
            database.close()


def _repository_root(environment: Mapping[str, str]) -> Path:
    return _real_directory(
        _required_environment(REPOSITORY_ROOT_ENV, environment),
        REPOSITORY_ROOT_ENV,
    )


def _package_root(environment: Mapping[str, str]) -> Path:
    return _real_directory(
        _required_environment(PACKAGE_ROOT_ENV, environment),
        PACKAGE_ROOT_ENV,
    )


def _build_report(
    authority: dict[str, dict[str, str]], observations: dict[str, object]
) -> dict[str, object]:
    return {
        "authority": authority,
        "binding": "python",
        "format": REPORT_FORMAT,
        "observations": observations,
        "semantic_profile": SEMANTIC_PROFILE,
    }


def main(environment: Mapping[str, str] = os.environ) -> int:
    try:
        output = _output_path(environment)
        root = _repository_root(environment)
        package_root = _package_root(environment)
        address = _required_environment(ADDRESS_ENV, environment)
        database_name = _required_environment(DATABASE_ENV, environment)
        http_port = _http_port(environment)
        authority, sources = _source_authority(root)
        package = _load_package(package_root)
        journey = _json_object(sources["journey"], "Phase-2 journey")
        records, create_order, cleanup_order = _record_index(journey)
        try:
            provider_schema = sources["provider"].decode("utf-8")
        except UnicodeDecodeError as error:
            raise ProducerError("invalid_authority", "provider schema is not UTF-8") from error
        observations = _run_owned_database(
            package,
            address,
            database_name,
            http_port,
            provider_schema,
            records,
            create_order,
            cleanup_order,
        )
        _publish_report(output, _build_report(authority, observations))
    except (AssertionError, OSError, ProducerError, RuntimeError, ValueError) as error:
        print(f"Python Phase-2 live producer rejected: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
