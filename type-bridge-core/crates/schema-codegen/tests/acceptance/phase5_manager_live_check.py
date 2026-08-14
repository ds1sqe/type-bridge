#!/usr/bin/env python3
"""Produce Python's exact-TypeDB-3.12.1 Phase-5 manager-filter report."""

from __future__ import annotations

import hashlib
import importlib
import json
import os
import stat
import struct
import sys
from collections.abc import Mapping
from datetime import UTC, date, datetime, timedelta
from decimal import Decimal
from pathlib import Path
from types import ModuleType
from typing import Any

from type_bridge_core import MatchRequestError

from type_bridge import Database

REPORT_FORMAT = "typebridge.phase5-manager-filter-live-report/v1"
SEMANTIC_PROFILE = "typedb-3.12.1/v1"
SEMANTIC_SCHEMA_FINGERPRINT = {
    "algorithm": "sha256",
    "canonicalization": "typebridge.schema-canonical-json/v1",
    "digest": "3c8d072b60c575b4c0381c1ea44088a9c18e01ed52e90730311aadccb72cd0c8",
    "domain": "typebridge.schema.semantic",
    "semantic_profile": SEMANTIC_PROFILE,
}

OUTPUT_ENV = "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_REPORT"
ADDRESS_ENV = "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_ADDRESS"
DATABASE_ENV = "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_DATABASE"
HTTP_PORT_ENV = "TYPE_BRIDGE_PHASE5_MANAGER_LIVE_HTTP_PORT"
PACKAGE_ROOT_ENV = "TYPE_BRIDGE_PHASE5_MANAGER_PYTHON_PACKAGE_ROOT"
REPOSITORY_ROOT_ENV = "TYPE_BRIDGE_PHASE5_MANAGER_REPOSITORY_ROOT"
LOCAL_PACKAGE = "generated_phase5_manager"
FOREIGN_PACKAGE = "generated_phase5_manager_foreign"

MAX_REPORT_BYTES = 256 * 1024
MAX_AUTHORITY_BYTES = 1024 * 1024
SOURCE_PATHS = {
    "schema": "tests/contracts/sdk_conformance/workforce-v3/schema-v3.yaml",
    "journey": "tests/contracts/sdk_conformance/workforce-v3/journey-v3.json",
    "provider": "tests/contracts/sdk_conformance/workforce-v3/provider-3.12.1-v3.tql",
}
SOURCE_SHA256 = {
    "schema": "74b9161e3d8fd70a1f22c3a8b7fc70a823b18cb07973064e2e10ce0940f76f1f",
    "journey": "912130753fe7938a38c054cff16e202b312551a6aa65265148628bfbd2abbbef",
    "provider": "af61aaece22d666e19d2c1357f8ebc39b8cbcf4bff7b98a19a56daf171a1faba",
}


class ProducerError(RuntimeError):
    """A stable fail-closed manager-filter producer rejection."""

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
        encoded = json.dumps(
            value,
            allow_nan=False,
            ensure_ascii=False,
            separators=(",", ":"),
            sort_keys=True,
        )
    except (RecursionError, TypeError, ValueError) as error:
        raise ProducerError("invalid_report_value", "report cannot be canonicalized") from error
    return f"{encoded}\n".encode()


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
        raw = _bounded_regular_bytes(root / relative, f"Phase-5 {label}")
        digest = hashlib.sha256(raw).hexdigest()
        if digest != SOURCE_SHA256[label]:
            raise ProducerError(
                "authority_hash_mismatch",
                f"Phase-5 {label} is not the frozen exact-3.12.1 authority",
            )
        authority[label] = {"path": relative, "sha256": digest}
        sources[label] = raw
    return authority, sources


def _required_environment(name: str, environment: Mapping[str, str]) -> str:
    value = environment.get(name)
    if value is None or not value:
        raise ProducerError("missing_environment", f"{name} must be set and non-empty")
    if len(value.encode("utf-8")) > 4096 or any(ord(character) < 32 for character in value):
        raise ProducerError("invalid_environment", f"{name} is not bounded text")
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


def _output_path(environment: Mapping[str, str]) -> Path:
    path = Path(_required_environment(OUTPUT_ENV, environment))
    if not path.is_absolute() or path.name in {"", ".", ".."}:
        raise ProducerError("invalid_output_path", "report path must be absolute")
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


def _load_package(package_root: Path, name: str) -> ModuleType:
    package_directory = package_root / name
    initializer = package_directory / "__init__.py"
    for path, label, directory in (
        (package_directory, f"{name} package", True),
        (initializer, f"{name} initializer", False),
    ):
        try:
            metadata = path.lstat()
        except OSError as error:
            raise ProducerError("generated_package_import", f"{label} is absent") from error
        expected = stat.S_ISDIR(metadata.st_mode) if directory else stat.S_ISREG(metadata.st_mode)
        if stat.S_ISLNK(metadata.st_mode) or not expected:
            raise ProducerError("generated_package_import", f"{label} is not a real object")
    if str(package_root) not in sys.path:
        sys.path.insert(0, str(package_root))
    try:
        package = importlib.import_module(name)
    except ImportError as error:
        raise ProducerError("generated_package_import", f"{name} cannot be imported") from error
    package_file = getattr(package, "__file__", None)
    if package_file is None or Path(package_file).resolve() != initializer.resolve():
        raise ProducerError("generated_package_import", f"{name} resolved outside its root")
    return package


def _semantic_fingerprint(package: ModuleType) -> dict[str, object]:
    try:
        value = json.loads(package.SEMANTIC_SCHEMA_FINGERPRINT_JSON)
    except (AttributeError, json.JSONDecodeError, TypeError) as error:
        raise ProducerError(
            "generated_package_identity", "package has no semantic identity"
        ) from error
    if type(value) is not dict:
        raise ProducerError("generated_package_identity", "semantic identity is malformed")
    return value


def _require_packages(local: ModuleType, foreign: ModuleType) -> None:
    local_semantic = _semantic_fingerprint(local)
    foreign_semantic = _semantic_fingerprint(foreign)
    if local_semantic != SEMANTIC_SCHEMA_FINGERPRINT:
        raise ProducerError(
            "generated_package_identity",
            "local package is not the frozen Workforce V3 projection",
        )
    if foreign_semantic == local_semantic:
        raise ProducerError(
            "generated_package_identity",
            "foreign package is not a genuinely different projection",
        )
    local_projection = getattr(local.Person, "__runtime_projection__", None)
    foreign_projection = getattr(foreign.Person, "__runtime_projection__", None)
    if (
        local_projection is None
        or foreign_projection is None
        or local_projection is foreign_projection
    ):
        raise ProducerError(
            "generated_package_identity",
            "local and foreign packages do not have isolated runtime projections",
        )
    for package in (local, foreign):
        for member in ("foo__bar", "identifier", "score", "val_bool"):
            if member not in vars(package.Person):
                raise ProducerError(
                    "generated_package_identity",
                    f"generated Person field token {member!r} is absent",
                )


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


def _person_records(journey: dict[str, Any]) -> tuple[dict[str, Any], dict[str, Any]]:
    if (
        journey.get("format") != "typebridge.workforce-journey/v3"
        or journey.get("fixture_id") != "workforce-v3"
        or journey.get("version") != 3
        or journey.get("semantic_profile") != SEMANTIC_PROFILE
    ):
        raise ProducerError("invalid_journey", "workforce-v3 identity drifted")
    records = journey.get("records")
    people = records.get("people") if type(records) is dict else None
    if type(people) is not list:
        raise ProducerError("invalid_journey", "person records are malformed")
    indexed = {
        record.get("ref"): record
        for record in people
        if type(record) is dict and type(record.get("ref")) is str
    }
    if set(indexed) != {"data-ada", "data-dana"}:
        raise ProducerError("invalid_journey", "manager person records drifted")
    return indexed["data-ada"], indexed["data-dana"]


def _field(record: dict[str, Any], name: str, kind: str) -> dict[str, Any]:
    fields = record.get("fields")
    value = fields.get(name) if type(fields) is dict else None
    if type(value) is not dict or value.get("kind") != kind:
        raise ProducerError("invalid_journey", f"person field {name!r} drifted")
    return value


def _primitive(field: dict[str, Any]) -> object:
    kind = field["kind"]
    if kind == "double":
        bits = field.get("bits")
        if type(bits) is not str:
            raise ProducerError("invalid_journey", "double bits are malformed")
        try:
            return struct.unpack(">d", bytes.fromhex(bits))[0]
        except (ValueError, struct.error) as error:
            raise ProducerError("invalid_journey", "double bits are malformed") from error
    value = field.get("value")
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
            raise ProducerError("invalid_journey", "datetime is not timezone-naive")
        return parsed
    if kind == "datetime_tz" and type(value) is str and value.endswith("Z"):
        return datetime.fromisoformat(value[:-1]).replace(tzinfo=UTC)
    if kind == "duration" and type(value) is str and value.startswith("PT") and value.endswith("S"):
        try:
            return timedelta(seconds=int(value[2:-1]))
        except ValueError as error:
            raise ProducerError("invalid_journey", "duration is malformed") from error
    raise ProducerError("invalid_journey", f"unsupported scalar domain {kind!r}")


def _person(package: ModuleType, record: dict[str, Any]) -> object:
    values = {
        "foo__bar": package.FooBar(_primitive(_field(record, "foo__bar", "long"))),
        "identifier": package.Identifier(_primitive(_field(record, "identifier", "string"))),
        "score": package.Score(_primitive(_field(record, "score", "long"))),
        "score__gte": package.ScoreGte(_primitive(_field(record, "score__gte", "long"))),
        "val_bool": package.ValBool(_primitive(_field(record, "val_bool", "boolean"))),
        "val_constrained": package.ValConstrained(
            _primitive(_field(record, "val_constrained", "long"))
        ),
        "val_date": package.ValDate(_primitive(_field(record, "val_date", "date"))),
        "val_datetime": package.ValDatetime(_primitive(_field(record, "val_datetime", "datetime"))),
        "val_datetime_tz": package.ValDatetimeTz(
            _primitive(_field(record, "val_datetime_tz", "datetime_tz"))
        ),
        "val_decimal": package.ValDecimal(_primitive(_field(record, "val_decimal", "decimal"))),
        "val_double": package.ValDouble(_primitive(_field(record, "val_double", "double"))),
        "val_duration": package.ValDuration(_primitive(_field(record, "val_duration", "duration"))),
    }
    if "nickname" in record.get("fields", {}):
        values["nickname"] = package.Nickname(_primitive(_field(record, "nickname", "string")))
    return package.Person(**values)


def _expect_diagnostic(
    operation: Any,
    *,
    kind: str,
    category: str,
    code: str,
) -> dict[str, object]:
    try:
        operation()
    except MatchRequestError as error:
        if error.sdk_category != category or error.code != code:
            raise ProducerError(
                "diagnostic_mismatch",
                f"{kind} returned {error.sdk_category!r}/{error.code!r}",
            ) from error
        return {
            "kind": kind,
            "category": error.sdk_category,
            "code": error.code,
            "rejected_before_provider_io": True,
        }
    except BaseException as error:
        raise ProducerError(
            "diagnostic_type_mismatch",
            f"{kind} raised {type(error).__name__} instead of MatchRequestError",
        ) from error
    raise ProducerError("missing_rejection", f"{kind} unexpectedly succeeded")


def _pre_io_rejections(
    package: ModuleType,
    foreign: ModuleType,
    database: Database,
) -> tuple[list[dict[str, object]], dict[str, object]]:
    comparison = package.ProjectedManagerComparison
    manager = package.Person.manager(database)
    rejections = [
        _expect_diagnostic(
            lambda: manager.where(
                package.Robot.robot_id,
                comparison.EQ,
                package.RobotId(7),
            ),
            kind="wrong_field_owner",
            category="integrity",
            code="field_owner_mismatch",
        ),
        _expect_diagnostic(
            lambda: manager.where(
                package.Person.foo__bar,
                comparison.EQ,
                package.Identifier("wrong-domain"),
            ),
            kind="wrong_scalar_domain",
            category="invalid_input",
            code="wrong_scalar_domain",
        ),
        _expect_diagnostic(
            lambda: manager.where(
                foreign.Person.foo__bar,
                comparison.EQ,
                foreign.FooBar(7),
            ),
            kind="wrong_package",
            category="integrity",
            code="generated_token_package_mismatch",
        ),
        _expect_diagnostic(
            lambda: manager.where(
                package.Person.val_bool,
                comparison.GT,
                package.ValBool(True),
            ),
            kind="boolean_ordering",
            category="invalid_input",
            code="invalid_operator_for_type",
        ),
    ]
    first = _expect_diagnostic(
        lambda: manager.where(
            package.Person.foo__bar,
            comparison.GTE,
            package.FooBar(7),
        ).first(),
        kind="nonsingular_first",
        category="invalid_input",
        code="manager_first_requires_identity",
    )
    return rejections, {
        "category": first["category"],
        "code": first["code"],
        "rejected_before_provider_io": first["rejected_before_provider_io"],
    }


def _keys(package: ModuleType, values: object) -> list[str]:
    if type(values) is not list:
        raise ProducerError("invalid_terminal_result", "manager all did not return a list")
    keys = []
    for value in values:
        if type(value) is not package.Person:
            raise ProducerError("invalid_terminal_result", "manager all returned another model")
        identifier = getattr(value, "identifier", None)
        key = getattr(identifier, "value", None)
        if type(key) is not str:
            raise ProducerError("invalid_terminal_result", "hydrated person has no string key")
        keys.append(key)
    if len(keys) != len(set(keys)):
        raise ProducerError("invalid_terminal_result", "manager all returned duplicate identities")
    return sorted(keys)


def _field_token_observation(package: ModuleType) -> dict[str, str]:
    token = package.Person.foo__bar
    owner_identity = json.loads(token.owner.__type_id__)
    fact = token.fact
    fact_id = fact.get("id") if type(fact) is dict else None
    attribute = fact_id.get("attribute") if type(fact_id) is dict else None
    descriptor = vars(package.Person).get("foo__bar")
    binding_name = getattr(descriptor, "name", None)
    if owner_identity != {"kind": "entity", "label": "person"} or attribute != "foo__bar":
        raise ProducerError("generated_token_identity", "foo__bar field token identity drifted")
    if binding_name != "foo__bar":
        raise ProducerError("generated_token_identity", "foo__bar binding name is ambiguous")
    return {
        "owner": f"{owner_identity['kind']}:{owner_identity['label']}",
        "attribute": f"attribute:{attribute}",
        "binding_name": binding_name,
    }


def _run_manager_journey(package: ModuleType, database: Database) -> dict[str, object]:
    comparison = package.ProjectedManagerComparison
    operator_outcomes = []
    with database.transaction("read") as transaction:
        manager = package.Person.manager(transaction)
        root = manager.where()
        for operator, member in (
            ("eq", comparison.EQ),
            ("ne", comparison.NE),
            ("gt", comparison.GT),
            ("gte", comparison.GTE),
            ("lt", comparison.LT),
            ("lte", comparison.LTE),
        ):
            selected = manager.where(package.Person.foo__bar, member, package.FooBar(7)).all()
            operator_outcomes.append(
                {"operator": operator, "normalized_keys": _keys(package, selected)}
            )

        authored_order = ["foo__bar:gte:7", "score:gt:40"]
        conjunction = manager.where(
            package.Person.foo__bar,
            comparison.GTE,
            package.FooBar(7),
        ).where(package.Person.score, comparison.GT, package.Score(40))
        conjunction_keys = _keys(package, conjunction.all())

        all_keys = _keys(package, root.all())
        count = root.count()
        exists = root.exists()
        if all_keys != ["data-ada", "data-dana"] or count != 2 or exists is not True:
            raise ProducerError("terminal_mismatch", "empty manager terminals drifted")

        identity = manager.where(
            package.Person.identifier,
            comparison.EQ,
            package.Identifier("data-ada"),
        )
        first = identity.first()
        if type(first) is not package.Person or first.identifier.value != "data-ada":
            raise ProducerError("terminal_mismatch", "strict first returned the wrong person")

        session = package.Person.query(transaction)
        person = session.exact(package.Person)
        plan04 = session.query(person).where(
            person.field(package.Person.identifier).eq(package.Identifier("data-ada"))
        )
        if plan04.count_by(person) != 1:
            raise ProducerError("borrowed_read_reuse", "Plan04 sibling query did not remain usable")
        sibling = manager.where(
            package.Person.foo__bar,
            comparison.EQ,
            package.FooBar(9),
        )
        if _keys(package, sibling.all()) != ["data-dana"]:
            raise ProducerError("borrowed_read_reuse", "sibling manager filter was not reusable")

        return {
            "model": "person",
            "field_token": _field_token_observation(package),
            "operator_literal": {"kind": "long", "value": "7"},
            "operator_outcomes": operator_outcomes,
            "conjunction": {
                "authored_order": authored_order,
                "normalized_keys": conjunction_keys,
            },
            "terminals": {
                "all_normalization": "reference_key_ascending",
                "count": count,
                "exists": exists,
            },
            "first": {
                "identity_predicate": "identifier:eq:data-ada",
                "strict_singular": True,
                "result": first.identifier.value,
            },
            "borrowed_read": {
                "reusable_after_each_terminal": True,
                "sibling_filter_usable": True,
                "final_state": "active",
            },
        }


def _run_owned_database(
    package: ModuleType,
    address: str,
    database_name: str,
    http_port: int,
    provider_schema: str,
    ada: dict[str, Any],
    dana: dict[str, Any],
) -> dict[str, object]:
    database = Database(address=address, database=database_name, http_port=http_port)
    owns_database = False
    failure: BaseException | None = None
    observation: dict[str, object] | None = None
    try:
        database.connect()
        detected = database.detected_server_version()
        if detected != "3.12.1":
            raise ProducerError(
                "server_version_mismatch",
                f"exact TypeDB 3.12.1 required, detected {detected!r}",
            )
        if database.database_exists():
            raise ProducerError("database_preexisting", "isolated database must be absent")
        database.create_database()
        owns_database = True
        database.execute_query(provider_schema, transaction_type="schema")
        manager = package.Person.manager(database)
        inserted = [manager.insert(_person(package, record)) for record in (ada, dana)]
        if _keys(package, inserted) != ["data-ada", "data-dana"]:
            raise ProducerError("seed_mismatch", "manager fixture insertion drifted")
        observation = _run_manager_journey(package, database)
        for value in reversed(inserted):
            manager.delete(value)
        if manager.count() != 0:
            raise ProducerError("cleanup_failed", "person rows remained after cleanup")
    except BaseException as error:
        failure = error
    if owns_database:
        try:
            database.delete_database()
            if database.database_exists():
                raise ProducerError("database_teardown_failed", "isolated database remained")
        except BaseException as error:
            failure = failure or error
    try:
        database.close()
    except BaseException as error:
        failure = failure or error
    if failure is not None:
        raise failure
    if observation is None:
        raise ProducerError("missing_observation", "manager journey produced no observation")
    return observation


def _http_port(environment: Mapping[str, str]) -> int:
    raw = _required_environment(HTTP_PORT_ENV, environment)
    if not raw.isascii() or not raw.isdigit():
        raise ProducerError("invalid_http_port", f"{HTTP_PORT_ENV} must be an ASCII integer")
    port = int(raw)
    if port < 1 or port > 65535:
        raise ProducerError("invalid_http_port", f"{HTTP_PORT_ENV} must be in 1..65535")
    return port


def main(environment: Mapping[str, str] = os.environ) -> int:
    try:
        output = _output_path(environment)
        root = _real_directory(
            _required_environment(REPOSITORY_ROOT_ENV, environment),
            REPOSITORY_ROOT_ENV,
        )
        package_root = _real_directory(
            _required_environment(PACKAGE_ROOT_ENV, environment),
            PACKAGE_ROOT_ENV,
        )
        address = _required_environment(ADDRESS_ENV, environment)
        database_name = _required_environment(DATABASE_ENV, environment)
        http_port = _http_port(environment)
        authority, sources = _source_authority(root)
        local = _load_package(package_root, LOCAL_PACKAGE)
        foreign = _load_package(package_root, FOREIGN_PACKAGE)
        _require_packages(local, foreign)
        journey = _json_object(sources["journey"], "Phase-5 journey")
        ada, dana = _person_records(journey)
        try:
            provider_schema = sources["provider"].decode("utf-8")
        except UnicodeDecodeError as error:
            raise ProducerError("invalid_authority", "provider schema is not UTF-8") from error

        preflight_database = Database(
            address=address,
            database=database_name,
            http_port=http_port,
        )
        try:
            rejections, nonsingular = _pre_io_rejections(local, foreign, preflight_database)
        finally:
            preflight_database.close()

        observation = _run_owned_database(
            local,
            address,
            database_name,
            http_port,
            provider_schema,
            ada,
            dana,
        )
        observation["rejections"] = rejections
        first = observation.get("first")
        if type(first) is not dict:
            raise ProducerError("invalid_observation", "first observation is malformed")
        first["nonsingular_rejection"] = nonsingular
        report = {
            "authority": authority,
            "binding": "python",
            "format": REPORT_FORMAT,
            "observation": observation,
            "semantic_profile": SEMANTIC_PROFILE,
        }
        _publish_report(output, report)
    except (AssertionError, OSError, ProducerError, RuntimeError, ValueError) as error:
        print(f"Python Phase-5 manager-filter producer rejected: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
