"""Live TypeDB smoke for the Phase 3 generated Python runtime projection."""

from __future__ import annotations

import asyncio
import base64
import hashlib
import importlib
import importlib.util
import json
import logging
import os
import socket
import stat
import struct
import subprocess
import sys
import tempfile
import time
from collections.abc import Iterator
from dataclasses import make_dataclass
from datetime import UTC, date, datetime, timedelta
from decimal import Decimal
from pathlib import Path
from types import ModuleType
from typing import Any, Protocol
from urllib import request as urllib_request

import pytest
from type_bridge_core import MatchRequestError

from type_bridge import Database

pytestmark = pytest.mark.integration

ROOT = Path(__file__).resolve().parents[3]
CORE = ROOT / "type-bridge-core"
ACCEPTANCE_DIRECTORY = CORE / "crates/schema-codegen/tests/acceptance"
ACCEPTANCE_SCHEMA = ACCEPTANCE_DIRECTORY / "schema.yaml"
ACCEPTANCE_SCHEMA_3_11 = ACCEPTANCE_DIRECTORY / "schema-3.11.5.yaml"
PROVIDER_SCHEMA = ACCEPTANCE_DIRECTORY / "provider-3.12.1.tql"
PROVIDER_SCHEMA_3_11 = ACCEPTANCE_DIRECTORY / "provider-3.11.5.tql"
WORKFORCE_MANIFEST_RELATIVE = "tests/contracts/sdk_conformance/manifest-v1.json"
WORKFORCE_CATALOG_RELATIVE = "tests/contracts/sdk_conformance/workforce-v1/catalog-v1.json"
WORKFORCE_JOURNEY_RELATIVE = "tests/contracts/sdk_conformance/workforce-v1/journey-v1.json"
WORKFORCE_V2_CATALOG_RELATIVE = "tests/contracts/sdk_conformance/workforce-v2/catalog-v2.json"
WORKFORCE_V2_JOURNEY_RELATIVE = "tests/contracts/sdk_conformance/workforce-v2/journey-v2.json"
WORKFORCE_V3_JOURNEY_RELATIVE = "tests/contracts/sdk_conformance/workforce-v3/journey-v3.json"
WORKFORCE_MANIFEST = ROOT / WORKFORCE_MANIFEST_RELATIVE
WORKFORCE_CATALOG = ROOT / WORKFORCE_CATALOG_RELATIVE
WORKFORCE_JOURNEY = ROOT / WORKFORCE_JOURNEY_RELATIVE
WORKFORCE_V2_CATALOG = ROOT / WORKFORCE_V2_CATALOG_RELATIVE
WORKFORCE_V2_JOURNEY = ROOT / WORKFORCE_V2_JOURNEY_RELATIVE
WORKFORCE_V3_JOURNEY = ROOT / WORKFORCE_V3_JOURNEY_RELATIVE
WORKFORCE_V3_PROVIDER_SCHEMA = (
    ROOT / "tests/contracts/sdk_conformance/workforce-v3/provider-3.12.1-v3.tql"
)
WORKFORCE_V2_PROOF_LOADER = ROOT / "scripts/ci/workforce_v2_proof_fragments.py"


class _StringValue(Protocol):
    value: str


class _IntegerValue(Protocol):
    value: int


class _GeneratedPerson(Protocol):
    """Static test view of the emitted person used before its package exists."""

    iid: str | None
    identifier: _StringValue
    nickname: _StringValue | None
    score: _IntegerValue
    aliases: list[_StringValue] | tuple[_StringValue, ...]


def _load_json_object(path: Path) -> tuple[bytes, dict[str, object]]:
    raw = path.read_bytes()
    document = json.loads(raw)
    if not isinstance(document, dict):
        raise AssertionError(f"workforce contract is not an object: {path}")
    return raw, document


def _source_identity(relative_path: str, raw: bytes) -> dict[str, str]:
    return {"path": relative_path, "sha256": hashlib.sha256(raw).hexdigest()}


def _require_workforce_server_version(detected: str | None) -> None:
    if detected != "3.12.3":
        raise AssertionError(
            "workforce reports require the actual detected TypeDB server version 3.12.3; "
            f"detected {detected!r}"
        )


def _workforce_results(
    catalog: dict[str, object],
    journey: dict[str, object],
    observed: dict[tuple[str, str], dict[str, object]],
) -> list[dict[str, object]]:
    expected = journey["expected_observations"]
    assert isinstance(expected, dict)
    cases = catalog["cases"]
    assert isinstance(cases, list)
    capability_by_case = {
        case["id"]: case["capability_id"] for case in cases if isinstance(case, dict)
    }
    selected_proofs = catalog["selected_proofs"]
    assert isinstance(selected_proofs, list)
    results: list[dict[str, object]] = []
    for proof in selected_proofs:
        assert isinstance(proof, dict)
        case_id = proof["case_id"]
        proof_kind = proof["proof_kind"]
        observation_ref = proof["observation_ref"]
        assert isinstance(case_id, str)
        assert isinstance(proof_kind, str)
        assert isinstance(observation_ref, str)
        actual = observed[(observation_ref, proof_kind)]
        committed = expected[observation_ref]
        assert actual == committed
        results.append(
            {
                "case_id": case_id,
                "capability_id": capability_by_case[case_id],
                "proof_kind": proof_kind,
                "outcome": "passed",
                "observation": actual,
            }
        )
    return sorted(results, key=lambda result: (result["case_id"], result["proof_kind"]))


def _load_workforce_v2_proof_observations() -> dict[tuple[str, str], dict[str, object]]:
    raw_paths = os.environ.get("TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENTS")
    run_nonce = os.environ.get("TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE")
    if raw_paths is None or run_nonce is None:
        raise AssertionError(
            "workforce-v2 reports require proof fragment paths and the same-run nonce"
        )
    path_values = raw_paths.split(os.pathsep)
    if not path_values or any(not value for value in path_values):
        raise AssertionError("workforce-v2 proof fragment paths must be a nonempty path list")
    spec = importlib.util.spec_from_file_location(
        "_typebridge_workforce_v2_proof_fragments",
        WORKFORCE_V2_PROOF_LOADER,
    )
    if spec is None or spec.loader is None:
        raise AssertionError("workforce-v2 proof fragment validator is not importable")
    loader = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(loader)
    load_proof_fragments = getattr(loader, "load_proof_fragments")
    loaded = load_proof_fragments(
        [Path(value) for value in path_values],
        expected_binding="python",
        run_nonce=run_nonce,
        root=ROOT,
    )
    if not isinstance(loaded, dict):
        raise AssertionError("workforce-v2 proof fragment validator returned an invalid map")
    observations: dict[tuple[str, str], dict[str, object]] = {}
    for lane, observation in loaded.items():
        if (
            not isinstance(lane, tuple)
            or len(lane) != 2
            or not all(isinstance(value, str) for value in lane)
            or not isinstance(observation, dict)
        ):
            raise AssertionError("workforce-v2 proof fragment observation is malformed")
        observations[(lane[0], lane[1])] = observation
    return observations


def _workforce_report(
    generated: ModuleType,
    catalog_raw: bytes,
    catalog: dict[str, object],
    journey_raw: bytes,
    results: list[dict[str, object]],
) -> dict[str, object]:
    manifest_raw = WORKFORCE_MANIFEST.read_bytes()
    fixture = catalog["fixture"]
    projection_targets = catalog["projection_targets"]
    assert isinstance(fixture, dict)
    assert isinstance(projection_targets, dict)
    schema_relative = fixture["schema_path"]
    provider_schema_relative = fixture["provider_schema_path"]
    journey_relative = catalog["journey_path"]
    assert isinstance(schema_relative, str)
    assert isinstance(provider_schema_relative, str)
    assert isinstance(journey_relative, str)
    assert journey_relative == WORKFORCE_JOURNEY_RELATIVE
    return {
        "format": "typebridge.sdk-conformance-report/v1",
        "binding": "python",
        "manifest": _source_identity(WORKFORCE_MANIFEST_RELATIVE, manifest_raw),
        "catalog": _source_identity(WORKFORCE_CATALOG_RELATIVE, catalog_raw),
        "fixture": {
            "id": fixture["id"],
            "version": fixture["version"],
            "semantic_profile": fixture["semantic_profile"],
            "schema": _source_identity(schema_relative, (ROOT / schema_relative).read_bytes()),
            "provider_schema": _source_identity(
                provider_schema_relative,
                (ROOT / provider_schema_relative).read_bytes(),
            ),
            "journey": _source_identity(journey_relative, journey_raw),
            "semantic_fingerprint": json.loads(generated.SEMANTIC_SCHEMA_FINGERPRINT_JSON),
            "projection_target": projection_targets["python"],
            "projection_fingerprint": json.loads(generated.PROJECTION_FINGERPRINT_JSON),
        },
        "results": results,
    }


def _workforce_v2_report(
    generated: ModuleType,
    catalog_raw: bytes,
    catalog: dict[str, object],
    journey_raw: bytes,
    results: list[dict[str, object]],
) -> dict[str, object]:
    manifest_raw = WORKFORCE_MANIFEST.read_bytes()
    fixture = catalog["fixture"]
    projection_targets = catalog["projection_targets"]
    assert isinstance(fixture, dict)
    assert isinstance(projection_targets, dict)
    schema_relative = fixture["schema_path"]
    provider_schema_relative = fixture["provider_schema_path"]
    journey_relative = catalog["journey_path"]
    assert isinstance(schema_relative, str)
    assert isinstance(provider_schema_relative, str)
    assert isinstance(journey_relative, str)
    assert journey_relative == WORKFORCE_V2_JOURNEY_RELATIVE
    return {
        "format": "typebridge.sdk-conformance-report/v2",
        "binding": "python",
        "manifest": _source_identity(WORKFORCE_MANIFEST_RELATIVE, manifest_raw),
        "catalog": _source_identity(WORKFORCE_V2_CATALOG_RELATIVE, catalog_raw),
        "fixture": {
            "id": fixture["id"],
            "version": fixture["version"],
            "semantic_profile": fixture["semantic_profile"],
            "schema": _source_identity(schema_relative, (ROOT / schema_relative).read_bytes()),
            "provider_schema": _source_identity(
                provider_schema_relative,
                (ROOT / provider_schema_relative).read_bytes(),
            ),
            "journey": _source_identity(journey_relative, journey_raw),
            "semantic_fingerprint": json.loads(generated.SEMANTIC_SCHEMA_FINGERPRINT_JSON),
            "projection_target": projection_targets["python"],
            "projection_fingerprint": json.loads(generated.PROJECTION_FINGERPRINT_JSON),
        },
        "results": results,
    }


def _validate_workforce_report_path(raw_path: str) -> Path:
    try:
        encoded_path = raw_path.encode("utf-8")
    except UnicodeEncodeError as error:
        raise AssertionError("TYPE_BRIDGE_WORKFORCE_REPORT must be a UTF-8 path") from error
    if not encoded_path or len(encoded_path) > 4096:
        raise AssertionError("TYPE_BRIDGE_WORKFORCE_REPORT must contain 1 to 4096 UTF-8 bytes")
    destination = Path(raw_path)
    if not destination.is_absolute():
        raise AssertionError("TYPE_BRIDGE_WORKFORCE_REPORT must be an absolute path")
    try:
        parent_metadata = destination.parent.lstat()
    except FileNotFoundError as error:
        raise AssertionError("TYPE_BRIDGE_WORKFORCE_REPORT parent must exist") from error
    if stat.S_ISLNK(parent_metadata.st_mode) or not stat.S_ISDIR(parent_metadata.st_mode):
        raise AssertionError("TYPE_BRIDGE_WORKFORCE_REPORT parent must be a non-symlink directory")
    try:
        destination.lstat()
    except FileNotFoundError:
        pass
    else:
        raise AssertionError("TYPE_BRIDGE_WORKFORCE_REPORT destination must not exist")
    return destination


def _publish_workforce_report(raw_path: str, report: dict[str, object]) -> None:
    destination = _validate_workforce_report_path(raw_path)

    payload = (
        json.dumps(report, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n"
    ).encode()
    descriptor, temporary_name = tempfile.mkstemp(
        dir=destination.parent,
        prefix=".typebridge-workforce-",
        suffix=".tmp",
    )
    temporary = Path(temporary_name)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(payload)
            output.flush()
            os.fsync(output.fileno())
        try:
            os.link(temporary, destination)
        except FileExistsError as error:
            raise AssertionError(
                "TYPE_BRIDGE_WORKFORCE_REPORT destination appeared during publication"
            ) from error
    finally:
        temporary.unlink(missing_ok=True)


def _publish_workforce_v3_python_supplement(
    generated: ModuleType,
    observations: dict[tuple[str, str], dict[str, object]],
) -> None:
    raw_path = os.environ.get("TYPE_BRIDGE_WORKFORCE_V3_PYTHON_SUPPLEMENT")
    if raw_path is None:
        return
    _, journey = _load_json_object(WORKFORCE_V3_JOURNEY)
    expected = journey["expected_observations"]
    assert isinstance(expected, dict)
    for (observation_ref, _), observation in observations.items():
        assert observation == expected[observation_ref]
    results = [
        {
            "observation_ref": observation_ref,
            "proof_kind": proof_kind,
            "outcome": "passed",
            "observation": observation,
        }
        for (observation_ref, proof_kind), observation in sorted(observations.items())
    ]
    assert len(results) == 8
    supplement = {
        "format": "typebridge.workforce-v3-live-supplement/v1",
        "binding": "python",
        "semantic_profile": "typedb-3.12.1/v1",
        "producer": "python.generated-data-model-runtime-v3-live",
        "semantic_fingerprint": json.loads(generated.SEMANTIC_SCHEMA_FINGERPRINT_JSON),
        "projection_fingerprint": json.loads(generated.PROJECTION_FINGERPRINT_JSON),
        "results": results,
    }
    _publish_workforce_report(raw_path, supplement)


def _workforce_datetime(field: dict[str, object], *, timezone: bool) -> datetime:
    value = field["value"]
    assert isinstance(value, str)
    if timezone:
        assert value.endswith("Z")
        return datetime.fromisoformat(value[:-1]).replace(tzinfo=UTC)
    return datetime.fromisoformat(value)


def _normalize_workforce_person(
    generated: ModuleType,
    candidate: Any,
    person_record: dict[str, object],
    nickname_field: dict[str, object],
) -> dict[str, object]:
    fields = person_record["fields"]
    assert isinstance(fields, dict)
    aliases = fields["aliases"]
    assert isinstance(aliases, list)
    alias_values = sorted(alias["value"] for alias in aliases)
    assert type(candidate) is generated.Person
    assert type(candidate.identifier) is generated.Identifier
    assert candidate.identifier.value == fields["identifier"]["value"]
    assert type(candidate.nickname) is generated.Nickname
    assert candidate.nickname.value == nickname_field["value"]
    assert sorted(alias.value for alias in candidate.aliases) == alias_values
    assert all(type(alias) is generated.Aliases for alias in candidate.aliases)
    assert type(candidate.score) is generated.Score
    assert candidate.score.value == int(fields["score"]["value"])
    assert type(candidate.foo__bar) is generated.FooBar
    assert candidate.foo__bar.value == int(fields["foo__bar"]["value"])
    assert type(candidate.score__gte) is generated.ScoreGte
    assert candidate.score__gte.value == int(fields["score__gte"]["value"])
    assert type(candidate.val_bool) is generated.ValBool
    assert candidate.val_bool.value is fields["val_bool"]["value"]
    assert type(candidate.val_constrained) is generated.ValConstrained
    assert candidate.val_constrained.value == int(fields["val_constrained"]["value"])
    assert type(candidate.val_date) is generated.ValDate
    assert candidate.val_date.value == date.fromisoformat(fields["val_date"]["value"])
    assert type(candidate.val_datetime) is generated.ValDatetime
    assert candidate.val_datetime.value == _workforce_datetime(
        fields["val_datetime"], timezone=False
    )
    assert type(candidate.val_datetime_tz) is generated.ValDatetimeTz
    assert candidate.val_datetime_tz.value == _workforce_datetime(
        fields["val_datetime_tz"], timezone=True
    )
    assert type(candidate.val_decimal) is generated.ValDecimal
    assert candidate.val_decimal.value == Decimal(fields["val_decimal"]["value"])
    assert type(candidate.val_double) is generated.ValDouble
    assert struct.pack(">d", candidate.val_double.value).hex() == fields["val_double"]["bits"]
    assert type(candidate.val_duration) is generated.ValDuration
    assert candidate.val_duration.value == timedelta(
        seconds=int(fields["val_duration"]["value"][2:-1])
    )

    scalar_domains: set[str] = set()
    for field in fields.values():
        members = field if isinstance(field, list) else [field]
        for member in members:
            assert isinstance(member, dict)
            kind = member["kind"]
            assert isinstance(kind, str)
            scalar_domains.add(kind)
    return {
        "aliases": alias_values,
        "key": fields["identifier"]["value"],
        "model": person_record["model"],
        "nickname": nickname_field["value"],
        "scalar_domains": sorted(scalar_domains),
    }


def _normalize_workforce_role(
    generated: ModuleType,
    relation: Any,
    player: Any,
    membership_record: dict[str, object],
) -> tuple[dict[str, object], dict[str, object], dict[str, str]]:
    expected_player = membership_record["player"]
    assert isinstance(expected_player, dict)
    assert type(relation) is generated.Membership
    assert type(player) is generated.Person
    assert type(relation.member) is generated.Person
    assert relation.member.identifier.value == expected_player["key"]
    assert player.identifier.value == expected_player["key"]
    assert isinstance(player.iid, str) and player.iid
    reference = generated.PersonRef(player.iid, identifier=player.identifier)
    assert type(reference) is generated.PersonRef
    assert reference.__model_form__ == "reference"
    assert reference.iid == player.iid
    assert type(reference.identifier) is generated.Identifier
    assert reference.identifier.value == expected_player["key"]
    player_identity = {
        "key": reference.identifier.value,
        "model": expected_player["model"],
    }
    hydrated = {
        "relation": {"model": membership_record["model"]},
        "role": membership_record["role"],
        "player": player_identity,
    }
    traversal = {
        "relation": membership_record["model"],
        "role": membership_record["role"],
        "player": player_identity,
    }
    return hydrated, traversal, player_identity


def _run_workforce_journey(
    generated: ModuleType,
    clean_db: Database,
    person_manager: Any,
    membership_manager: Any,
    remote_session: Any,
    remote_requests: list[bytes],
    catalog: dict[str, object],
    journey: dict[str, object],
) -> list[dict[str, object]]:
    records = journey["records"]
    assert isinstance(records, dict)
    person_record = records["person"]
    membership_record = records["membership"]
    assert isinstance(person_record, dict)
    assert isinstance(membership_record, dict)
    fields = person_record["fields"]
    update = person_record["update"]
    assert isinstance(fields, dict)
    assert isinstance(update, dict)
    aliases = fields["aliases"]
    assert isinstance(aliases, list)
    double_bits = fields["val_double"]["bits"]
    assert isinstance(double_bits, str)
    duration = fields["val_duration"]["value"]
    assert isinstance(duration, str)
    assert duration.startswith("PT") and duration.endswith("S")
    person = generated.Person(
        identifier=generated.Identifier(fields["identifier"]["value"]),
        nickname=generated.Nickname(fields["nickname"]["value"]),
        aliases=[generated.Aliases(alias["value"]) for alias in aliases],
        score=generated.Score(int(fields["score"]["value"])),
        foo__bar=generated.FooBar(int(fields["foo__bar"]["value"])),
        score__gte=generated.ScoreGte(int(fields["score__gte"]["value"])),
        val_bool=generated.ValBool(fields["val_bool"]["value"]),
        val_constrained=generated.ValConstrained(int(fields["val_constrained"]["value"])),
        val_date=generated.ValDate(date.fromisoformat(fields["val_date"]["value"])),
        val_datetime=generated.ValDatetime(
            _workforce_datetime(fields["val_datetime"], timezone=False)
        ),
        val_datetime_tz=generated.ValDatetimeTz(
            _workforce_datetime(fields["val_datetime_tz"], timezone=True)
        ),
        val_decimal=generated.ValDecimal(Decimal(fields["val_decimal"]["value"])),
        val_double=generated.ValDouble(struct.unpack(">d", bytes.fromhex(double_bits))[0]),
        val_duration=generated.ValDuration(timedelta(seconds=int(duration[2:-1]))),
    )
    membership = None
    person_created = False
    membership_created = False
    membership_deleted = False
    person_deleted = False
    read_after_update = False
    observed: dict[tuple[str, str], dict[str, object]] = {}
    try:
        assert person_manager.insert(person) is person
        assert person.iid
        person_created = True
        inserted_person = person_manager.get_by_iid(person.iid)
        assert inserted_person is not None
        _normalize_workforce_person(generated, inserted_person, person_record, fields["nickname"])

        person.nickname = generated.Nickname(update["nickname"]["value"])
        assert person_manager.update(person) is person
        updated_person = person_manager.get_by_iid(person.iid)
        assert updated_person is not None
        _normalize_workforce_person(generated, updated_person, person_record, update["nickname"])
        read_after_update = True

        membership = generated.Membership(member=updated_person)
        assert membership_manager.insert(membership) is membership
        assert membership.iid
        membership_created = True
        stored_membership = membership_manager.get_by_iid(membership.iid)
        assert stored_membership is not None
        assert stored_membership.member.identifier.value == membership_record["player"]["key"]

        direct_session = generated.Membership.query(clean_db)
        direct_relation_var = direct_session.exact(generated.Membership)
        direct_person_var = direct_session.exact(generated.Person)
        direct_relation, direct_person = (
            direct_session.query(direct_relation_var, direct_person_var)
            .where(
                direct_relation_var.role(generated.Membership.member).connects(direct_person_var),
                direct_person_var.field(generated.Person.identifier).eq(
                    generated.Identifier(fields["identifier"]["value"])
                ),
            )
            .one()
        )
        direct_hydrated, direct_traversal, direct_reference = _normalize_workforce_role(
            generated,
            direct_relation,
            direct_person,
            membership_record,
        )
        direct_person_observation = _normalize_workforce_person(
            generated,
            direct_person,
            person_record,
            update["nickname"],
        )
        direct_person_observation["reference"] = direct_reference
        observed[("model_values_and_references", "direct_runtime")] = direct_person_observation
        observed[("hydrated_role_result", "direct_runtime")] = direct_hydrated
        observed[("role_traversal", "direct_runtime")] = direct_traversal

        remote_relation_var = remote_session.exact(generated.Membership)
        remote_person_var = remote_session.exact(generated.Person)
        requests_before = len(remote_requests)
        remote_relation, remote_person = asyncio.run(
            remote_session.query(remote_relation_var, remote_person_var)
            .where(
                remote_relation_var.role(generated.Membership.member).connects(remote_person_var),
                remote_person_var.field(generated.Person.identifier).eq(
                    generated.Identifier(fields["identifier"]["value"])
                ),
            )
            .one()
        )
        exchange_count = len(remote_requests) - requests_before
        assert exchange_count == 1
        assert all(remote_requests[index] for index in range(requests_before, len(remote_requests)))
        remote_hydrated, remote_traversal, remote_reference = _normalize_workforce_role(
            generated,
            remote_relation,
            remote_person,
            membership_record,
        )
        remote_person_observation = _normalize_workforce_person(
            generated,
            remote_person,
            person_record,
            update["nickname"],
        )
        remote_person_observation["reference"] = remote_reference
        observed[("model_values_and_references", "remote_runtime")] = remote_person_observation
        observed[("hydrated_role_result", "remote_runtime")] = remote_hydrated
        observed[("role_traversal", "remote_runtime")] = remote_traversal
        observed[("remote_one_exchange", "remote_runtime")] = {
            "exchange_count": exchange_count,
            "terminal": "one",
        }
    finally:
        cleanup_failures: list[BaseException] = []
        if membership is not None and membership.iid is not None:
            try:
                membership_manager.delete(membership)
                membership_deleted = membership_manager.get_by_iid(membership.iid) is None
            except BaseException as error:
                cleanup_failures.append(error)
        if person.iid is not None:
            try:
                person_manager.delete(person)
                person_deleted = person_manager.get_by_iid(person.iid) is None
            except BaseException as error:
                cleanup_failures.append(error)
        if cleanup_failures:
            raise cleanup_failures[0]

    observed[("entity_lifecycle", "direct_runtime")] = {
        "created": person_created,
        "deleted": person_deleted,
        "key": fields["identifier"]["value"],
        "model": person_record["model"],
        "nickname_after_update": update["nickname"]["value"],
        "read_after_update": read_after_update,
    }
    observed[("relation_lifecycle", "direct_runtime")] = {
        "created": membership_created,
        "deleted": membership_deleted,
        "model": membership_record["model"],
        "player_key": membership_record["player"]["key"],
        "role": membership_record["role"],
    }
    return _workforce_results(catalog, journey, observed)


def _workforce_v2_person(generated: ModuleType, record: dict[str, object]) -> Any:
    fields = record["fields"]
    assert isinstance(fields, dict)
    aliases = fields["aliases"]
    assert isinstance(aliases, list)
    double_bits = fields["val_double"]["bits"]
    duration = fields["val_duration"]["value"]
    assert isinstance(double_bits, str)
    assert isinstance(duration, str) and duration.startswith("PT") and duration.endswith("S")
    nickname = fields.get("nickname")
    return generated.Person(
        identifier=generated.Identifier(fields["identifier"]["value"]),
        nickname=(None if nickname is None else generated.Nickname(nickname["value"])),
        aliases=[generated.Aliases(alias["value"]) for alias in aliases],
        score=generated.Score(int(fields["score"]["value"])),
        score__gte=generated.ScoreGte(int(fields["score__gte"]["value"])),
        val_bool=generated.ValBool(fields["val_bool"]["value"]),
        val_constrained=generated.ValConstrained(int(fields["val_constrained"]["value"])),
        val_date=generated.ValDate(date.fromisoformat(fields["val_date"]["value"])),
        val_datetime=generated.ValDatetime(
            _workforce_datetime(fields["val_datetime"], timezone=False)
        ),
        val_datetime_tz=generated.ValDatetimeTz(
            _workforce_datetime(fields["val_datetime_tz"], timezone=True)
        ),
        val_decimal=generated.ValDecimal(Decimal(fields["val_decimal"]["value"])),
        val_double=generated.ValDouble(struct.unpack(">d", bytes.fromhex(double_bits))[0]),
        val_duration=generated.ValDuration(timedelta(seconds=int(duration[2:-1]))),
    )


def _workforce_v2_reducers(
    generated: ModuleType,
    session: Any,
    query: Any,
    person: Any,
) -> dict[str, object]:
    score = person.field(generated.Person.score)
    values = query.aggregate(
        person,
        generated.aggregate.count(),
        generated.aggregate.sum(score),
        generated.aggregate.min(score),
        generated.aggregate.max(score),
        generated.aggregate.mean(score),
        generated.aggregate.median(score),
        generated.aggregate.std(score),
    )
    group_person = session.exact(generated.Person)
    group_identifier = group_person.field(generated.Person.identifier)
    binding = (
        query.match(group_person)
        .where(person.field(generated.Person.identifier).eq_field(group_identifier))
        .group_by(person, group_person)
        .aggregate(generated.aggregate.count())
    )
    field = query.group_by(person, score).aggregate(generated.aggregate.count())
    score_gte = person.field(generated.Person.score__gte)
    field_tuple = query.group_by(person, score, score_gte).aggregate(generated.aggregate.count())

    def bits(value: object) -> str:
        assert type(value) is float
        return struct.pack(">d", value).hex()

    return {
        "reducers": {
            "count": values[0],
            "sum": values[1],
            "min": values[2],
            "max": values[3],
            "mean_bits": bits(values[4]),
            "median_bits": bits(values[5]),
            "std_bits": bits(values[6]),
        },
        "groups": {
            "binding": [
                {
                    "model": "person",
                    "key": group.identifier.value,
                    "count": group_values[0],
                }
                for group, group_values in binding
            ],
            "field": [
                {"key": group.value, "count": group_values[0]} for group, group_values in field
            ],
            "field_tuple": [
                {
                    "key": [score_group.value, score_gte_group.value],
                    "count": group_values[0],
                }
                for (score_group, score_gte_group), group_values in field_tuple
            ],
        },
    }


async def _workforce_v2_remote_reducers(
    generated: ModuleType,
    session: Any,
    query: Any,
    person: Any,
) -> dict[str, object]:
    score = person.field(generated.Person.score)
    values = await query.aggregate(
        person,
        generated.aggregate.count(),
        generated.aggregate.sum(score),
        generated.aggregate.min(score),
        generated.aggregate.max(score),
        generated.aggregate.mean(score),
        generated.aggregate.median(score),
        generated.aggregate.std(score),
    )
    group_person = session.exact(generated.Person)
    group_identifier = group_person.field(generated.Person.identifier)
    binding = await (
        query.match(group_person)
        .where(person.field(generated.Person.identifier).eq_field(group_identifier))
        .group_by(person, group_person)
        .aggregate(generated.aggregate.count())
    )
    field = await query.group_by(person, score).aggregate(generated.aggregate.count())
    score_gte = person.field(generated.Person.score__gte)
    field_tuple = await query.group_by(person, score, score_gte).aggregate(
        generated.aggregate.count()
    )

    def bits(value: object) -> str:
        assert type(value) is float
        return struct.pack(">d", value).hex()

    return {
        "reducers": {
            "count": values[0],
            "sum": values[1],
            "min": values[2],
            "max": values[3],
            "mean_bits": bits(values[4]),
            "median_bits": bits(values[5]),
            "std_bits": bits(values[6]),
        },
        "groups": {
            "binding": [
                {
                    "model": "person",
                    "key": group.identifier.value,
                    "count": group_values[0],
                }
                for group, group_values in binding
            ],
            "field": [
                {"key": group.value, "count": group_values[0]} for group, group_values in field
            ],
            "field_tuple": [
                {
                    "key": [score_group.value, score_gte_group.value],
                    "count": group_values[0],
                }
                for (score_group, score_gte_group), group_values in field_tuple
            ],
        },
    }


def _workforce_v2_key(value: object) -> str:
    identifier = getattr(value, "identifier")
    key = getattr(identifier, "value")
    assert isinstance(key, str)
    return key


def _workforce_v2_model(generated: ModuleType, value: object) -> str:
    for model, name in (
        (generated.Person, "person"),
        (generated.Employee, "employee"),
        (generated.Manager, "manager"),
        (generated.Membership, "membership"),
        (generated.NetworkLink, "network-link"),
    ):
        if type(value) is model:
            return name
    raise AssertionError(f"unexpected workforce-v2 projected model: {type(value)!r}")


def _workforce_v2_model_key(generated: ModuleType, value: object) -> dict[str, str]:
    return {"model": _workforce_v2_model(generated, value), "key": _workforce_v2_key(value)}


def _workforce_v2_keys(values: Iterator[object] | list[object] | tuple[object, ...]) -> list[str]:
    return [_workforce_v2_key(value) for value in values]


def _workforce_v2_person_values(
    generated: ModuleType,
    person: object,
    membership: object,
) -> dict[str, object]:
    scalar_fields = (
        ("boolean", getattr(person, "val_bool")),
        ("date", getattr(person, "val_date")),
        ("datetime", getattr(person, "val_datetime")),
        ("datetime_tz", getattr(person, "val_datetime_tz")),
        ("decimal", getattr(person, "val_decimal")),
        ("double", getattr(person, "val_double")),
        ("duration", getattr(person, "val_duration")),
        ("long", getattr(person, "score")),
        ("string", getattr(person, "identifier")),
    )
    for _, field in scalar_fields:
        assert getattr(field, "value") is not None
    member = getattr(membership, "member")
    return {
        "aliases": [alias.value for alias in getattr(person, "aliases")],
        "key": _workforce_v2_key(person),
        "model": _workforce_v2_model(generated, person),
        "nickname": getattr(person, "nickname").value,
        "reference": _workforce_v2_model_key(generated, member),
        "scalar_domains": [domain for domain, _ in scalar_fields],
    }


def _workforce_v2_role_observation(
    generated: ModuleType,
    membership: object,
    network: object,
) -> dict[str, object]:
    member = getattr(membership, "member")
    participants = getattr(network, "participant")
    return {
        "membership": {
            "relation": _workforce_v2_model(generated, membership),
            "role": "member",
            "players": [_workforce_v2_model_key(generated, member)],
        },
        "network_link": {
            "relation": _workforce_v2_model(generated, network),
            "origin": _workforce_v2_key(getattr(network, "origin")),
            "destination": _workforce_v2_key(getattr(network, "destination")),
            "participants": sorted(_workforce_v2_keys(tuple(participants))),
        },
    }


def _workforce_v2_hydrated_result(generated: ModuleType, membership: object) -> dict[str, object]:
    member = getattr(membership, "member")
    return {
        "rows": [
            {
                "model": _workforce_v2_model(generated, membership),
                "roles": {"member": [_workforce_v2_model_key(generated, member)]},
            }
        ]
    }


def _workforce_v2_error_category(error: BaseException) -> tuple[str, str, str]:
    category = getattr(error, "sdk_category")
    query_category = getattr(error, "query_category")
    code = getattr(error, "code")
    assert isinstance(category, str)
    assert isinstance(query_category, str)
    assert isinstance(code, str)
    return category, query_category, code


def _workforce_v2_cardinality_diagnostic(error: BaseException) -> dict[str, object]:
    category, query_category, code = _workforce_v2_error_category(error)
    message = getattr(error, "message")
    path = getattr(error, "path")
    details = getattr(error, "details")
    assert isinstance(message, str)
    assert isinstance(path, list)
    assert isinstance(details, dict)
    actual = details.get("actual")
    assert isinstance(actual, dict)
    assert actual.get("kind") == "count"
    actual_value = actual.get("value")
    assert isinstance(actual_value, int | str)
    serialized = json.dumps(
        {"message": message, "path": path, "details": details},
        sort_keys=True,
    )
    return {
        "category": category,
        "query_category": query_category,
        "code": code,
        "message": message,
        "path": path,
        "details": {"actual": {"kind": "count", "value": str(actual_value)}},
        "redacted": all(
            secret not in serialized
            for secret in ("query-ada", "query-dana", "localhost", "password")
        ),
    }


def _workforce_v2_resource_limits(
    generated: ModuleType,
    clean_db: Database,
    remote_advertisement: bytes,
    remote_exchange: Any,
    membership_iid: str,
) -> dict[str, object]:
    maxima = {
        "timeout_milliseconds": 30_000,
        "items": 65_536,
        "bytes": 33_554_432,
        "graph_nodes": 65_536,
        "attribute_values": 65_536,
        "collection_members": 65_536,
        "role_players": 65_536,
        "statements": 3,
    }
    plus = generated.QueryExecutionResourceLimits(
        **{name: value + 1 for name, value in maxima.items()}
    )
    zero = generated.QueryExecutionResourceLimits(**dict.fromkeys(maxima, 0))
    plus_one_clamped_all = all(getattr(plus, name) == value for name, value in maxima.items())
    zero_tightening_all = all(getattr(zero, name) == 0 for name in maxima)

    def limited_session(*, remote: bool) -> Any:
        limits = generated.QueryExecutionResourceLimits(role_players=0)
        if remote:
            return generated.RemoteQuerySession(
                remote_advertisement,
                remote_exchange,
                limits,
            )
        return generated.QuerySession(
            generated.Person.__runtime_projection__, clean_db, resources=limits
        )

    errors: list[tuple[str, str, str]] = []
    for remote in (False, True):
        session = limited_session(remote=remote)
        relation = session.exact(generated.Membership)
        query = session.query(relation).where(relation.iid(membership_iid))
        try:
            if remote:
                asyncio.run(query.one())
            else:
                query.one()
        except MatchRequestError as error:
            errors.append(_workforce_v2_error_category(error))
        else:
            raise AssertionError("zero role-player budget accepted a hydrated relation")
        finally:
            session.close()
    assert errors[0] == errors[1]
    category, _, code = errors[0]
    return {
        "hard_maxima": maxima,
        "plus_one_clamped_all": plus_one_clamped_all,
        "zero_tightening_all": zero_tightening_all,
        "enforced": {
            "dimension": "role_players",
            "category": category,
            "code": code,
            "no_partial_result": True,
        },
    }


def _workforce_v2_lifecycle(
    generated: ModuleType,
    clean_db: Database,
    remote_advertisement: bytes,
    remote_exchange: Any,
    remote_requests: list[bytes],
) -> dict[str, object]:
    def direct_lane() -> tuple[dict[str, bool | int], object]:
        session = generated.Person.query(clean_db)
        person = session.exact(generated.Person)
        identifier = person.field(generated.Person.identifier)
        scope = identifier.eq(generated.Identifier("query-ada"))
        ancestor = session.query(person).where(scope)
        sibling = ancestor.clone()
        closed_descendant = ancestor.where(
            person.field(generated.Person.score).gte(generated.Score(1))
        )
        closed_descendant.close()
        closed_descendant.close()
        ancestor_usable = _workforce_v2_key(ancestor.one()) == "query-ada"
        descendant = ancestor.where(person.field(generated.Person.score).gte(generated.Score(1)))
        result = descendant.one()
        ancestor.close()
        ancestor.close()
        rejected = False
        try:
            ancestor.one()
        except MatchRequestError as error:
            rejected = error.code == "query_resource_closed"
        descendant_usable = _workforce_v2_key(descendant.one()) == "query-ada"
        sibling_usable = _workforce_v2_key(sibling.one()) == "query-ada"
        session_usable = _workforce_v2_key(session.query(person).where(scope).one()) == "query-ada"
        descendant.close()
        sibling.close()
        session.close()
        return (
            {
                "ancestor_usable_after_descendant_close": ancestor_usable,
                "close_idempotent": ancestor.is_closed and closed_descendant.is_closed,
                "descendant_usable_after_ancestor_close": descendant_usable,
                "handle_invalidated": ancestor.is_closed,
                "post_close_io_count": 0,
                "post_close_rejected": rejected,
                "session_usable_after_query_close": session_usable,
                "sibling_usable": sibling_usable,
            },
            result,
        )

    async def remote_lane() -> tuple[dict[str, bool | int], object]:
        session = generated.RemoteQuerySession(
            remote_advertisement,
            remote_exchange,
            generated.QueryExecutionResourceLimits(),
        )
        person = session.exact(generated.Person)
        identifier = person.field(generated.Person.identifier)
        scope = identifier.eq(generated.Identifier("query-ada"))
        ancestor = session.query(person).where(scope)
        sibling = ancestor.clone()
        closed_descendant = ancestor.where(
            person.field(generated.Person.score).gte(generated.Score(1))
        )
        closed_descendant.close()
        closed_descendant.close()
        ancestor_usable = _workforce_v2_key(await ancestor.one()) == "query-ada"
        descendant = ancestor.where(person.field(generated.Person.score).gte(generated.Score(1)))
        result = await descendant.one()
        ancestor.close()
        ancestor.close()
        requests_before_rejection = len(remote_requests)
        rejected = False
        try:
            await ancestor.one()
        except MatchRequestError as error:
            rejected = error.code == "query_resource_closed"
        post_close_io_count = len(remote_requests) - requests_before_rejection
        descendant_usable = _workforce_v2_key(await descendant.one()) == "query-ada"
        sibling_usable = _workforce_v2_key(await sibling.one()) == "query-ada"
        session_usable = (
            _workforce_v2_key(await session.query(person).where(scope).one()) == "query-ada"
        )
        descendant.close()
        sibling.close()
        session.close()
        session.close()
        return (
            {
                "ancestor_usable_after_descendant_close": ancestor_usable,
                "close_idempotent": ancestor.is_closed and closed_descendant.is_closed,
                "descendant_usable_after_ancestor_close": descendant_usable,
                "handle_invalidated": ancestor.is_closed,
                "post_close_io_count": post_close_io_count,
                "post_close_rejected": rejected,
                "session_usable_after_query_close": session_usable,
                "sibling_usable": sibling_usable,
            },
            result,
        )

    direct, direct_result = direct_lane()
    remote, remote_result = asyncio.run(remote_lane())
    assert direct == remote
    return {
        "lanes": ["direct", "remote"],
        "query": direct,
        "result_usable_after_query_close": (
            _workforce_v2_key(direct_result) == "query-ada"
            and _workforce_v2_key(remote_result) == "query-ada"
        ),
    }


def _run_workforce_v2_journey(
    generated: ModuleType,
    clean_db: Database,
    remote_session: Any,
    remote_requests: list[bytes],
    remote_advertisement: bytes,
    remote_exchange: Any,
    catalog: dict[str, object],
    journey: dict[str, object],
    proof_observations: dict[tuple[str, str], dict[str, object]],
) -> list[dict[str, object]]:
    records = journey["records"]
    expected = journey["expected_observations"]
    assert isinstance(records, dict)
    assert isinstance(expected, dict)
    people_records = records["people"]
    assert isinstance(people_records, list) and len(people_records) == 2
    people = [_workforce_v2_person(generated, record) for record in people_records]
    employee_record = records["employee"]
    manager_record = records["manager"]
    assert isinstance(employee_record, dict)
    assert isinstance(manager_record, dict)
    employee_fields = employee_record["fields"]
    manager_fields = manager_record["fields"]
    assert isinstance(employee_fields, dict)
    assert isinstance(manager_fields, dict)
    employee = generated.Employee(
        identifier=generated.Identifier(employee_fields["identifier"]["value"]),
        party_name=generated.PartyName(employee_fields["party_name"]["value"]),
        rank=generated.Rank(int(employee_fields["rank"]["value"])),
    )
    manager = generated.Manager(
        identifier=generated.Identifier(manager_fields["identifier"]["value"]),
        party_name=generated.PartyName(manager_fields["party_name"]["value"]),
        rank=generated.Rank(int(manager_fields["rank"]["value"])),
        manager_note=generated.ManagerNote(manager_fields["manager_note"]["value"]),
    )
    membership = generated.Membership(member=people[0])
    network_record = records["network_link"]
    assert isinstance(network_record, dict)
    network_fields = network_record["fields"]
    assert isinstance(network_fields, dict)
    network = generated.NetworkLink(
        identifier=generated.Identifier(network_fields["identifier"]["value"]),
        nickname=generated.Nickname(network_fields["nickname"]["value"]),
        origin=people[0],
        destination=people[1],
        participant=people,
    )

    person_manager = generated.Person.manager(clean_db)
    employee_manager = generated.Employee.manager(clean_db)
    manager_manager = generated.Manager.manager(clean_db)
    membership_manager = generated.Membership.manager(clean_db)
    network_manager = generated.NetworkLink.manager(clean_db)
    inserted: list[tuple[Any, Any]] = []
    try:
        assert person_manager.insert_many(people) == people
        inserted.extend((person_manager, value) for value in people)
        assert employee_manager.insert(employee) is employee
        inserted.append((employee_manager, employee))
        assert manager_manager.insert(manager) is manager
        inserted.append((manager_manager, manager))
        assert membership_manager.insert(membership) is membership
        inserted.append((membership_manager, membership))
        assert network_manager.insert(network) is network
        inserted.append((network_manager, network))

        assert all(isinstance(value.iid, str) and value.iid for _, value in inserted)
        person_iids = [value.iid for value in people]
        assert all(isinstance(iid, str) for iid in person_iids)
        membership_iid = membership.iid
        network_iid = network.iid
        assert isinstance(membership_iid, str) and isinstance(network_iid, str)
        entity_read_after_create = person_manager.get_by_iid(person_iids[0]) is not None
        relation_read_after_create = membership_manager.get_by_iid(membership_iid) is not None

        direct_session = generated.Person.query(clean_db)
        direct_person = direct_session.exact(generated.Person)
        direct_identifier = direct_person.field(generated.Person.identifier)
        scoped = direct_identifier.eq(generated.Identifier("query-ada")) | direct_identifier.eq(
            generated.Identifier("query-dana")
        )
        direct_query = direct_session.query(direct_person).where(scoped)
        direct_keys = [
            row.identifier.value
            for row in direct_query.rows(limit=2, order_by=(direct_identifier.asc(),))
        ]
        assert direct_keys == ["query-ada", "query-dana"]
        direct_ada = (
            direct_session.query(direct_person)
            .where(direct_identifier.eq(generated.Identifier("query-ada")))
            .one()
        )

        direct_membership_var = direct_session.exact(generated.Membership)
        direct_member = direct_session.exact(generated.Person)
        direct_membership = (
            direct_session.query(direct_membership_var)
            .match(direct_member)
            .where(
                direct_membership_var.iid(membership_iid),
                direct_membership_var.role(generated.Membership.member).connects(direct_member),
                direct_member.field(generated.Person.identifier).eq(
                    generated.Identifier("query-ada")
                ),
            )
            .one()
        )
        direct_network_var = direct_session.exact(generated.NetworkLink)
        direct_origin = direct_session.exact(generated.Person)
        direct_destination = direct_session.exact(generated.Person)
        direct_network = (
            direct_session.query(direct_network_var)
            .match(direct_origin, direct_destination)
            .where(
                direct_network_var.iid(network_iid),
                direct_network_var.role(generated.NetworkLink.origin).connects(direct_origin),
                direct_network_var.role(generated.NetworkLink.destination).connects(
                    direct_destination
                ),
                direct_origin.field(generated.Person.identifier).eq(
                    generated.Identifier("query-ada")
                ),
                direct_destination.field(generated.Person.identifier).eq(
                    generated.Identifier("query-dana")
                ),
            )
            .one()
        )

        direct_owner_keys = _workforce_v2_keys(
            direct_session.query(direct_person)
            .where(direct_person.field(generated.Person.score).is_present(), scoped)
            .rows(limit=2, order_by=(direct_identifier.asc(),))
        )
        direct_optional_keys = _workforce_v2_keys(
            direct_session.query(direct_person)
            .where(direct_person.field(generated.Person.nickname).is_present(), scoped)
            .rows(limit=2, order_by=(direct_identifier.asc(),))
        )
        direct_iid_keys = _workforce_v2_keys(
            direct_session.query(direct_person)
            .where(direct_person.iid_in(person_iids))
            .rows(limit=2, order_by=(direct_identifier.asc(),))
        )

        direct_employee_exact = direct_session.exact(generated.Employee)
        direct_employee_exact_id = direct_employee_exact.field(generated.Employee.identifier)
        direct_exact_values = (
            direct_session.query(direct_employee_exact)
            .where(
                direct_employee_exact_id.eq(generated.Identifier("query-employee"))
                | direct_employee_exact_id.eq(generated.Identifier("query-manager"))
            )
            .rows(limit=2, order_by=(direct_employee_exact_id.asc(),))
        )
        direct_employee_subtypes = direct_session.subtypes(generated.Employee)
        direct_employee_subtypes_id = direct_employee_subtypes.field(generated.Employee.identifier)
        direct_subtype_values = (
            direct_session.query(direct_employee_subtypes)
            .where(
                direct_employee_subtypes_id.eq(generated.Identifier("query-employee"))
                | direct_employee_subtypes_id.eq(generated.Identifier("query-manager"))
            )
            .rows(limit=2, order_by=(direct_employee_subtypes_id.asc(),))
        )

        direct_score = direct_person.field(generated.Person.score)
        direct_score_gte = direct_person.field(generated.Person.score__gte)
        direct_boolean = direct_person.field(generated.Person.val_bool)
        direct_and_keys = _workforce_v2_keys(
            direct_session.query(direct_person)
            .where(
                scoped,
                direct_score.gte(generated.Score(40)) & direct_boolean.eq(generated.ValBool(True)),
            )
            .rows(limit=2, order_by=(direct_identifier.asc(),))
        )
        direct_or_keys = _workforce_v2_keys(
            direct_session.query(direct_person)
            .where(scoped)
            .rows(limit=2, order_by=(direct_identifier.asc(),))
        )
        direct_not_keys = _workforce_v2_keys(
            direct_session.query(direct_person)
            .where(scoped, ~direct_boolean.eq(generated.ValBool(True)))
            .rows(limit=2, order_by=(direct_identifier.asc(),))
        )
        direct_field_comparison_keys = _workforce_v2_keys(
            direct_session.query(direct_person)
            .where(scoped, direct_score.gte_field(direct_score_gte))
            .rows(limit=2, order_by=(direct_identifier.asc(),))
        )

        direct_source = direct_session.exact(generated.Person)
        direct_target = direct_session.exact(generated.Person)
        direct_reachable = direct_session.reachable(
            direct_source,
            direct_target,
            generated.NetworkLink,
            generated.NetworkLink.origin,
            generated.NetworkLink.destination,
            min_depth=1,
            max_depth=1,
        )
        direct_reachable_pair = (
            direct_session.query(direct_source, direct_target)
            .where(
                direct_reachable,
                direct_source.field(generated.Person.identifier).eq(
                    generated.Identifier("query-ada")
                ),
                direct_target.field(generated.Person.identifier).eq(
                    generated.Identifier("query-dana")
                ),
            )
            .one()
        )
        direct_cross_left = direct_session.exact(generated.Person)
        direct_cross_right = direct_session.exact(generated.Person)
        direct_cross_left_id = direct_cross_left.field(generated.Person.identifier)
        direct_cross_right_id = direct_cross_right.field(generated.Person.identifier)
        direct_cross_scope = (
            direct_cross_left_id.eq(generated.Identifier("query-ada"))
            | direct_cross_left_id.eq(generated.Identifier("query-dana"))
        ) & (
            direct_cross_right_id.eq(generated.Identifier("query-ada"))
            | direct_cross_right_id.eq(generated.Identifier("query-dana"))
        )
        direct_cross_pairs = [
            [_workforce_v2_key(left), _workforce_v2_key(right)]
            for left, right in (
                direct_session.query(direct_cross_left, direct_cross_right)
                .allow_cross_join(direct_cross_left, direct_cross_right)
                .where(direct_cross_scope)
                .rows(
                    limit=4,
                    order_by=(direct_cross_left_id.asc(), direct_cross_right_id.asc()),
                )
            )
        ]

        selection_link = direct_session.exact(generated.NetworkLink)
        selection_origin = direct_session.exact(generated.Person)
        selection_participant = direct_session.exact(generated.Person)
        selection_predicates = (
            selection_link.iid(network_iid),
            selection_link.role(generated.NetworkLink.origin).connects(selection_origin),
            selection_link.role(generated.NetworkLink.participant).connects(selection_participant),
        )
        direct_positional_page = (
            direct_session.query(
                selection_origin,
                selection_participant.collect()
                .distinct()
                .order_by(selection_participant.field(generated.Person.identifier).asc()),
            )
            .match(selection_link)
            .where(*selection_predicates)
            .page_by(
                selection_origin,
                limit=1,
                order_by=(selection_origin.field(generated.Person.identifier).asc(),),
                include_total=True,
            )
        )
        assert direct_positional_page.total == 1
        assert len(direct_positional_page.items) == 1
        positional_origin, positional_participants = direct_positional_page.items[0]
        selection_row = make_dataclass(
            "WorkforceV2SelectionRow",
            [
                ("origin", generated.Person),
                ("participants", tuple[generated.Person, ...]),
            ],
            frozen=True,
            slots=True,
        )
        direct_named_page = (
            direct_session.query_as(
                selection_row,
                origin=selection_origin,
                participants=selection_participant.collect()
                .distinct()
                .order_by(selection_participant.field(generated.Person.identifier).asc()),
            )
            .match(selection_link)
            .where(*selection_predicates)
            .page_by(
                selection_origin,
                limit=1,
                order_by=(selection_origin.field(generated.Person.identifier).asc(),),
                include_total=True,
            )
        )
        assert direct_named_page.total == 1
        assert len(direct_named_page.items) == 1
        direct_named = direct_named_page.items[0]
        direct_selection = {
            "positional": [
                _workforce_v2_key(positional_origin),
                _workforce_v2_keys(tuple(positional_participants)),
            ],
            "named": {
                "origin": _workforce_v2_key(direct_named.origin),
                "participants": _workforce_v2_keys(tuple(direct_named.participants)),
            },
            "collected_distinct": len({value.iid for value in positional_participants})
            == len(positional_participants),
            "collection_order": "identifier_asc",
        }

        direct_dana = (
            direct_session.query(direct_person)
            .where(direct_identifier.eq(generated.Identifier("query-dana")))
            .one()
        )
        direct_first = direct_query.first(order_by=(direct_identifier.asc(),))
        direct_page = direct_query.page_by(
            direct_person,
            limit=1,
            order_by=(direct_identifier.asc(),),
            include_total=True,
        )
        direct_terminals = {
            "one": _workforce_v2_key(direct_dana),
            "first": _workforce_v2_key(direct_first),
            "rows": direct_keys,
            "page": {
                "items": _workforce_v2_keys(tuple(direct_page.items)),
                "offset": direct_page.offset,
                "limit": direct_page.limit,
                "total": direct_page.total,
            },
            "count": direct_query.count_by(direct_person),
            "exists": direct_query.exists_by(direct_person),
        }
        try:
            direct_query.one()
        except MatchRequestError as error:
            structured_query_diagnostic = _workforce_v2_cardinality_diagnostic(error)
        else:
            raise AssertionError("workforce-v2 exactly-one query accepted two selected rows")

        direct_scalar_domain_keys = _workforce_v2_keys(
            direct_session.query(direct_person)
            .where(scoped, direct_score.gte(generated.Score(40)))
            .rows(limit=2, order_by=(direct_identifier.asc(),))
        )
        direct_reducers = _workforce_v2_reducers(
            generated,
            direct_session,
            direct_query,
            direct_person,
        )
        assert direct_reducers == expected["grouped_reducer"]

        direct_minimum = generated.integer_input(direct_session, generated.Score(30))
        direct_call = generated.qualifying_score(
            direct_session,
            direct_person,
            direct_minimum,
        )
        direct_function_values = [
            row.score.value
            for row in direct_query.where(direct_call.gte_field(direct_score)).rows(
                limit=2,
                order_by=(direct_identifier.asc(),),
            )
        ]
        direct_nested = generated.qualifying_score(
            direct_session,
            direct_person,
            direct_call,
        )
        direct_nested_function_values = [
            row.score.value
            for row in direct_query.where(direct_nested.gte_field(direct_score)).rows(
                limit=2,
                order_by=(direct_identifier.asc(),),
            )
        ]
        assert direct_function_values == expected["schema_function"]["values"]

        remote_person = remote_session.exact(generated.Person)
        remote_identifier = remote_person.field(generated.Person.identifier)
        remote_scoped = remote_identifier.eq(
            generated.Identifier("query-ada")
        ) | remote_identifier.eq(generated.Identifier("query-dana"))
        remote_query = remote_session.query(remote_person).where(remote_scoped)
        remote_keys = [
            row.identifier.value
            for row in asyncio.run(remote_query.rows(limit=2, order_by=(remote_identifier.asc(),)))
        ]
        assert remote_keys == direct_keys
        remote_ada = asyncio.run(
            remote_session.query(remote_person)
            .where(remote_identifier.eq(generated.Identifier("query-ada")))
            .one()
        )

        remote_membership_var = remote_session.exact(generated.Membership)
        remote_member = remote_session.exact(generated.Person)
        remote_membership = asyncio.run(
            remote_session.query(remote_membership_var)
            .match(remote_member)
            .where(
                remote_membership_var.iid(membership_iid),
                remote_membership_var.role(generated.Membership.member).connects(remote_member),
                remote_member.field(generated.Person.identifier).eq(
                    generated.Identifier("query-ada")
                ),
            )
            .one()
        )
        remote_network_var = remote_session.exact(generated.NetworkLink)
        remote_origin = remote_session.exact(generated.Person)
        remote_destination = remote_session.exact(generated.Person)
        remote_network = asyncio.run(
            remote_session.query(remote_network_var)
            .match(remote_origin, remote_destination)
            .where(
                remote_network_var.iid(network_iid),
                remote_network_var.role(generated.NetworkLink.origin).connects(remote_origin),
                remote_network_var.role(generated.NetworkLink.destination).connects(
                    remote_destination
                ),
                remote_origin.field(generated.Person.identifier).eq(
                    generated.Identifier("query-ada")
                ),
                remote_destination.field(generated.Person.identifier).eq(
                    generated.Identifier("query-dana")
                ),
            )
            .one()
        )

        remote_owner_keys = _workforce_v2_keys(
            asyncio.run(
                remote_session.query(remote_person)
                .where(remote_person.field(generated.Person.score).is_present(), remote_scoped)
                .rows(limit=2, order_by=(remote_identifier.asc(),))
            )
        )
        remote_optional_keys = _workforce_v2_keys(
            asyncio.run(
                remote_session.query(remote_person)
                .where(remote_person.field(generated.Person.nickname).is_present(), remote_scoped)
                .rows(limit=2, order_by=(remote_identifier.asc(),))
            )
        )
        remote_iid_keys = _workforce_v2_keys(
            asyncio.run(
                remote_session.query(remote_person)
                .where(remote_person.iid_in(person_iids))
                .rows(limit=2, order_by=(remote_identifier.asc(),))
            )
        )

        remote_employee_exact = remote_session.exact(generated.Employee)
        remote_employee_exact_id = remote_employee_exact.field(generated.Employee.identifier)
        remote_exact_values = asyncio.run(
            remote_session.query(remote_employee_exact)
            .where(
                remote_employee_exact_id.eq(generated.Identifier("query-employee"))
                | remote_employee_exact_id.eq(generated.Identifier("query-manager"))
            )
            .rows(limit=2, order_by=(remote_employee_exact_id.asc(),))
        )
        remote_employee_subtypes = remote_session.subtypes(generated.Employee)
        remote_employee_subtypes_id = remote_employee_subtypes.field(generated.Employee.identifier)
        remote_subtype_values = asyncio.run(
            remote_session.query(remote_employee_subtypes)
            .where(
                remote_employee_subtypes_id.eq(generated.Identifier("query-employee"))
                | remote_employee_subtypes_id.eq(generated.Identifier("query-manager"))
            )
            .rows(limit=2, order_by=(remote_employee_subtypes_id.asc(),))
        )

        remote_score = remote_person.field(generated.Person.score)
        remote_score_gte = remote_person.field(generated.Person.score__gte)
        remote_boolean = remote_person.field(generated.Person.val_bool)
        remote_and_keys = _workforce_v2_keys(
            asyncio.run(
                remote_session.query(remote_person)
                .where(
                    remote_scoped,
                    remote_score.gte(generated.Score(40))
                    & remote_boolean.eq(generated.ValBool(True)),
                )
                .rows(limit=2, order_by=(remote_identifier.asc(),))
            )
        )
        remote_or_keys = _workforce_v2_keys(
            asyncio.run(
                remote_session.query(remote_person)
                .where(remote_scoped)
                .rows(limit=2, order_by=(remote_identifier.asc(),))
            )
        )
        remote_not_keys = _workforce_v2_keys(
            asyncio.run(
                remote_session.query(remote_person)
                .where(remote_scoped, ~remote_boolean.eq(generated.ValBool(True)))
                .rows(limit=2, order_by=(remote_identifier.asc(),))
            )
        )
        remote_field_comparison_keys = _workforce_v2_keys(
            asyncio.run(
                remote_session.query(remote_person)
                .where(remote_scoped, remote_score.gte_field(remote_score_gte))
                .rows(limit=2, order_by=(remote_identifier.asc(),))
            )
        )

        remote_source = remote_session.exact(generated.Person)
        remote_target = remote_session.exact(generated.Person)
        remote_reachable = remote_session.reachable(
            remote_source,
            remote_target,
            generated.NetworkLink,
            generated.NetworkLink.origin,
            generated.NetworkLink.destination,
            min_depth=1,
            max_depth=1,
        )
        remote_reachable_pair = asyncio.run(
            remote_session.query(remote_source, remote_target)
            .where(
                remote_reachable,
                remote_source.field(generated.Person.identifier).eq(
                    generated.Identifier("query-ada")
                ),
                remote_target.field(generated.Person.identifier).eq(
                    generated.Identifier("query-dana")
                ),
            )
            .one()
        )
        remote_cross_left = remote_session.exact(generated.Person)
        remote_cross_right = remote_session.exact(generated.Person)
        remote_cross_left_id = remote_cross_left.field(generated.Person.identifier)
        remote_cross_right_id = remote_cross_right.field(generated.Person.identifier)
        remote_cross_scope = (
            remote_cross_left_id.eq(generated.Identifier("query-ada"))
            | remote_cross_left_id.eq(generated.Identifier("query-dana"))
        ) & (
            remote_cross_right_id.eq(generated.Identifier("query-ada"))
            | remote_cross_right_id.eq(generated.Identifier("query-dana"))
        )
        remote_cross_pairs = [
            [_workforce_v2_key(left), _workforce_v2_key(right)]
            for left, right in asyncio.run(
                remote_session.query(remote_cross_left, remote_cross_right)
                .allow_cross_join(remote_cross_left, remote_cross_right)
                .where(remote_cross_scope)
                .rows(
                    limit=4,
                    order_by=(remote_cross_left_id.asc(), remote_cross_right_id.asc()),
                )
            )
        ]

        remote_selection_link = remote_session.exact(generated.NetworkLink)
        remote_selection_origin = remote_session.exact(generated.Person)
        remote_selection_participant = remote_session.exact(generated.Person)
        remote_selection_predicates = (
            remote_selection_link.iid(network_iid),
            remote_selection_link.role(generated.NetworkLink.origin).connects(
                remote_selection_origin
            ),
            remote_selection_link.role(generated.NetworkLink.participant).connects(
                remote_selection_participant
            ),
        )
        remote_positional_page = asyncio.run(
            remote_session.query(
                remote_selection_origin,
                remote_selection_participant.collect()
                .distinct()
                .order_by(remote_selection_participant.field(generated.Person.identifier).asc()),
            )
            .match(remote_selection_link)
            .where(*remote_selection_predicates)
            .page_by(
                remote_selection_origin,
                limit=1,
                order_by=(remote_selection_origin.field(generated.Person.identifier).asc(),),
                include_total=True,
            )
        )
        assert remote_positional_page.total == 1
        assert len(remote_positional_page.items) == 1
        remote_positional_origin, remote_positional_participants = remote_positional_page.items[0]
        remote_named_page = asyncio.run(
            remote_session.query_as(
                selection_row,
                origin=remote_selection_origin,
                participants=remote_selection_participant.collect()
                .distinct()
                .order_by(remote_selection_participant.field(generated.Person.identifier).asc()),
            )
            .match(remote_selection_link)
            .where(*remote_selection_predicates)
            .page_by(
                remote_selection_origin,
                limit=1,
                order_by=(remote_selection_origin.field(generated.Person.identifier).asc(),),
                include_total=True,
            )
        )
        assert remote_named_page.total == 1
        assert len(remote_named_page.items) == 1
        remote_named = remote_named_page.items[0]
        remote_selection = {
            "positional": [
                _workforce_v2_key(remote_positional_origin),
                _workforce_v2_keys(tuple(remote_positional_participants)),
            ],
            "named": {
                "origin": _workforce_v2_key(remote_named.origin),
                "participants": _workforce_v2_keys(tuple(remote_named.participants)),
            },
            "collected_distinct": len({value.iid for value in remote_positional_participants})
            == len(remote_positional_participants),
            "collection_order": "identifier_asc",
        }

        requests_before_one = len(remote_requests)
        remote_dana = asyncio.run(
            remote_session.query(remote_person)
            .where(remote_identifier.eq(generated.Identifier("query-dana")))
            .one()
        )
        remote_one_exchange = len(remote_requests) - requests_before_one
        remote_first = asyncio.run(remote_query.first(order_by=(remote_identifier.asc(),)))
        remote_page = asyncio.run(
            remote_query.page_by(
                remote_person,
                limit=1,
                order_by=(remote_identifier.asc(),),
                include_total=True,
            )
        )
        remote_terminals = {
            "one": _workforce_v2_key(remote_dana),
            "first": _workforce_v2_key(remote_first),
            "rows": remote_keys,
            "page": {
                "items": _workforce_v2_keys(tuple(remote_page.items)),
                "offset": remote_page.offset,
                "limit": remote_page.limit,
                "total": remote_page.total,
            },
            "count": asyncio.run(remote_query.count_by(remote_person)),
            "exists": asyncio.run(remote_query.exists_by(remote_person)),
        }

        remote_scalar_domain_keys = _workforce_v2_keys(
            asyncio.run(
                remote_session.query(remote_person)
                .where(remote_scoped, remote_score.gte(generated.Score(40)))
                .rows(limit=2, order_by=(remote_identifier.asc(),))
            )
        )
        remote_reducers = asyncio.run(
            _workforce_v2_remote_reducers(
                generated,
                remote_session,
                remote_query,
                remote_person,
            )
        )
        assert remote_reducers == direct_reducers

        remote_minimum = generated.integer_input(remote_session, generated.Score(30))
        remote_call = generated.qualifying_score(
            remote_session,
            remote_person,
            remote_minimum,
        )
        remote_function_values = [
            row.score.value
            for row in asyncio.run(
                remote_query.where(remote_call.gte_field(remote_score)).rows(
                    limit=2,
                    order_by=(remote_identifier.asc(),),
                )
            )
        ]
        assert remote_function_values == direct_function_values
        remote_nested = generated.qualifying_score(
            remote_session,
            remote_person,
            remote_call,
        )
        remote_nested_function_values = [
            row.score.value
            for row in asyncio.run(
                remote_query.where(remote_nested.gte_field(remote_score)).rows(
                    limit=2,
                    order_by=(remote_identifier.asc(),),
                )
            )
        ]
        assert remote_nested_function_values == direct_nested_function_values

        direct_model_values = _workforce_v2_person_values(generated, direct_ada, direct_membership)
        remote_model_values = _workforce_v2_person_values(generated, remote_ada, remote_membership)
        assert remote_model_values == direct_model_values
        owner_iid_set_direct = {
            "owner_field": "score",
            "owner_keys": direct_owner_keys,
            "optional_field": "nickname",
            "optional_present_keys": direct_optional_keys,
            "iid_set_keys": direct_iid_keys,
        }
        owner_iid_set_remote = {
            "owner_field": "score",
            "owner_keys": remote_owner_keys,
            "optional_field": "nickname",
            "optional_present_keys": remote_optional_keys,
            "iid_set_keys": remote_iid_keys,
        }
        assert owner_iid_set_remote == owner_iid_set_direct
        exact_subtypes_direct: dict[str, object] = {
            "declared_model": "employee",
            "exact": [_workforce_v2_model_key(generated, value) for value in direct_exact_values],
            "subtypes": [
                _workforce_v2_model_key(generated, value) for value in direct_subtype_values
            ],
        }
        exact_subtypes_remote: dict[str, object] = {
            "declared_model": "employee",
            "exact": [_workforce_v2_model_key(generated, value) for value in remote_exact_values],
            "subtypes": [
                _workforce_v2_model_key(generated, value) for value in remote_subtype_values
            ],
        }
        assert exact_subtypes_remote == exact_subtypes_direct
        scalar_boolean_direct: dict[str, object] = {
            "and_keys": direct_and_keys,
            "or_keys": direct_or_keys,
            "not_keys": direct_not_keys,
            "field_comparison_keys": direct_field_comparison_keys,
        }
        scalar_boolean_remote: dict[str, object] = {
            "and_keys": remote_and_keys,
            "or_keys": remote_or_keys,
            "not_keys": remote_not_keys,
            "field_comparison_keys": remote_field_comparison_keys,
        }
        assert scalar_boolean_remote == scalar_boolean_direct
        direct_roles = _workforce_v2_role_observation(generated, direct_membership, direct_network)
        remote_roles = _workforce_v2_role_observation(generated, remote_membership, remote_network)
        assert remote_roles == direct_roles
        topology_direct = {
            "reachable": [
                {
                    "from": _workforce_v2_key(direct_reachable_pair[0]),
                    "to": _workforce_v2_key(direct_reachable_pair[1]),
                    "max_hops": 1,
                }
            ],
            "cross_join_pairs": direct_cross_pairs,
        }
        topology_remote = {
            "reachable": [
                {
                    "from": _workforce_v2_key(remote_reachable_pair[0]),
                    "to": _workforce_v2_key(remote_reachable_pair[1]),
                    "max_hops": 1,
                }
            ],
            "cross_join_pairs": remote_cross_pairs,
        }
        assert topology_remote == topology_direct
        assert remote_selection == direct_selection
        assert remote_terminals == direct_terminals
        direct_hydrated = _workforce_v2_hydrated_result(generated, direct_membership)
        remote_hydrated = _workforce_v2_hydrated_result(generated, remote_membership)
        assert remote_hydrated == direct_hydrated
        scalar_domain_direct = {
            "domain": "long",
            "operator": "gte",
            "operand": 40,
            "keys": direct_scalar_domain_keys,
        }
        scalar_domain_remote = {
            "domain": "long",
            "operator": "gte",
            "operand": 40,
            "keys": remote_scalar_domain_keys,
        }
        assert scalar_domain_remote == scalar_domain_direct
        assert remote_one_exchange == 1
        resource_limits = _workforce_v2_resource_limits(
            generated,
            clean_db,
            remote_advertisement,
            remote_exchange,
            membership_iid,
        )
        query_resource_lifecycle = _workforce_v2_lifecycle(
            generated,
            clean_db,
            remote_advertisement,
            remote_exchange,
            remote_requests,
        )

        observed: dict[tuple[str, str], dict[str, object]] = {}
        observed[("model_values_and_references", "direct_runtime")] = direct_model_values
        observed[("model_values_and_references", "remote_runtime")] = remote_model_values
        observed[("owner_iid_set", "direct_runtime")] = owner_iid_set_direct
        observed[("owner_iid_set", "remote_runtime")] = owner_iid_set_remote
        observed[("exact_subtypes", "direct_runtime")] = exact_subtypes_direct
        observed[("exact_subtypes", "remote_runtime")] = exact_subtypes_remote
        observed[("scalar_boolean", "direct_runtime")] = scalar_boolean_direct
        observed[("scalar_boolean", "remote_runtime")] = scalar_boolean_remote
        observed[("roles", "direct_runtime")] = direct_roles
        observed[("roles", "remote_runtime")] = remote_roles
        observed[("topology", "direct_runtime")] = topology_direct
        observed[("topology", "remote_runtime")] = topology_remote
        observed[("selection_shapes", "direct_runtime")] = direct_selection
        observed[("selection_shapes", "remote_runtime")] = remote_selection
        observed[("terminals", "direct_runtime")] = direct_terminals
        observed[("terminals", "remote_runtime")] = remote_terminals
        observed[("grouped_reducer", "direct_runtime")] = direct_reducers
        observed[("grouped_reducer", "remote_runtime")] = remote_reducers
        observed[("remote_one_exchange", "remote_runtime")] = {
            "exchange_count": remote_one_exchange,
            "terminal": "one",
        }
        observed[("hydrated_result", "direct_runtime")] = direct_hydrated
        observed[("hydrated_result", "remote_runtime")] = remote_hydrated
        observed[("scalar_domain", "direct_runtime")] = scalar_domain_direct
        observed[("scalar_domain", "remote_runtime")] = scalar_domain_remote
        observed[("schema_function", "direct_runtime")] = {
            "minimum": 30,
            "values": direct_function_values,
            "nested_values": direct_nested_function_values,
        }
        observed[("schema_function", "remote_runtime")] = {
            "minimum": 30,
            "values": remote_function_values,
            "nested_values": remote_nested_function_values,
        }
        observed[("structured_query_diagnostic", "diagnostic")] = structured_query_diagnostic
        observed[("resource_limits", "direct_runtime")] = resource_limits
        observed[("resource_limits", "remote_runtime")] = resource_limits
        observed[("query_resource_lifecycle", "lifecycle")] = query_resource_lifecycle
        assert len(observed) == 29, "workforce-v2 must measure exactly 29 pre-cleanup live lanes"
        assert len(proof_observations) == 3
        overlap = set(observed).intersection(proof_observations)
        assert not overlap, (
            f"workforce-v2 proof fragments duplicate live lanes: {sorted(overlap)!r}"
        )
        observed.update(proof_observations)
        assert len(observed) == 32
    finally:
        cleanup_failures: list[BaseException] = []
        for owner, value in reversed(inserted):
            try:
                owner.delete(value)
            except BaseException as error:
                cleanup_failures.append(error)
        if cleanup_failures:
            raise cleanup_failures[0]

    entity_deleted = person_manager.get_by_iid(person_iids[0]) is None
    relation_deleted = membership_manager.get_by_iid(membership_iid) is None
    observed[("entity_lifecycle", "direct_runtime")] = {
        "created": True,
        "deleted": entity_deleted,
        "key": _workforce_v2_key(people[0]),
        "model": _workforce_v2_model(generated, people[0]),
        "read_after_create": entity_read_after_create,
    }
    observed[("relation_lifecycle", "direct_runtime")] = {
        "created": True,
        "deleted": relation_deleted,
        "model": _workforce_v2_model(generated, membership),
        "player_key": _workforce_v2_key(people[0]),
        "role": "member",
    }
    assert len(observed) == 34, "workforce-v2 requires 31 live and 3 proof lanes"
    return _workforce_results(catalog, journey, observed)


def _free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def _wait_for_port(port: int, process: subprocess.Popen[bytes], timeout: float) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise AssertionError(f"generated remote server exited with {process.returncode}")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=1):
                return
        except OSError:
            time.sleep(0.2)
    raise AssertionError("generated remote server never became reachable")


def test_workforce_report_server_version_gate_is_exact() -> None:
    _require_workforce_server_version("3.12.3")
    for detected in (None, "3.11.5", "3.12.0", "3.12.2", "3.13.0"):
        with pytest.raises(AssertionError, match="actual detected TypeDB server version 3.12.3"):
            _require_workforce_server_version(detected)


def _make_generated_person(
    generated: ModuleType,
    identifier: str,
    score: int,
    *,
    nickname: str | None = None,
) -> _GeneratedPerson:
    return generated.Person(
        identifier=generated.Identifier(identifier),
        nickname=None if nickname is None else generated.Nickname(nickname),
        score=generated.Score(score),
        val_bool=generated.ValBool(True),
        val_constrained=generated.ValConstrained(20),
        val_date=generated.ValDate(date(2026, 7, 29)),
        val_datetime=generated.ValDatetime(datetime(2026, 7, 29)),
        val_datetime_tz=generated.ValDatetimeTz(datetime(2026, 7, 29, tzinfo=UTC)),
        val_decimal=generated.ValDecimal(Decimal("3.5")),
        val_double=generated.ValDouble(3.5),
        val_duration=generated.ValDuration(timedelta(seconds=3)),
    )


def _acceptance_contract(database: Database) -> tuple[Path, Path, str]:
    server_version = database.detected_server_version()
    if server_version is not None:
        major, minor = (int(part) for part in server_version.split(".")[:2])
        if (major, minor) < (3, 12):
            return ACCEPTANCE_SCHEMA_3_11, PROVIDER_SCHEMA_3_11, "typedb-3.11.5/v1"
    return ACCEPTANCE_SCHEMA, PROVIDER_SCHEMA, "typedb-3.12.1/v1"


@pytest.fixture
def generated_package(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    clean_db: Database,
) -> Iterator[ModuleType]:
    """Import a generated package emitted now or supplied as immutable test evidence."""
    _, _, semantic_profile = _acceptance_contract(clean_db)
    supplied_stage = os.environ.get("TYPE_BRIDGE_GENERATED_PYTHON_STAGE")
    if supplied_stage is None:
        stage = tmp_path / "generated-projection"
        generation_environment = os.environ.copy()
        generation_environment["TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE"] = semantic_profile
        subprocess.run(
            [
                str(ROOT / "scripts/ci/prepare_generated_live_fixture.sh"),
                "python",
                str(stage),
            ],
            cwd=ROOT,
            env=generation_environment,
            check=True,
        )
    else:
        stage = Path(supplied_stage).resolve()
        for required in (
            stage / "generated_v2" / "__init__.py",
            stage / "schema-authority.json",
        ):
            if not required.is_file() or required.is_symlink():
                raise AssertionError(f"supplied generated Python fixture is incomplete: {required}")
    monkeypatch.syspath_prepend(str(stage))
    importlib.invalidate_caches()
    generated = importlib.import_module("generated_v2")
    generated_semantics = json.loads(generated.SEMANTIC_SCHEMA_FINGERPRINT_JSON)
    assert generated_semantics["semantic_profile"] == semantic_profile
    try:
        yield generated
    finally:
        for module_name in tuple(sys.modules):
            if module_name == "generated_v2" or module_name.startswith("generated_v2."):
                del sys.modules[module_name]


@pytest.fixture
def generated_v3_package(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    clean_db: Database,
) -> Iterator[ModuleType]:
    """Import the exact Workforce V3 Python projection generated for this run."""
    _, _, semantic_profile = _acceptance_contract(clean_db)
    if semantic_profile != "typedb-3.12.1/v1":
        pytest.skip("Workforce V3 requires the 3.12 semantic profile")
    supplied_stage = os.environ.get("TYPE_BRIDGE_GENERATED_PYTHON_STAGE")
    if supplied_stage is None:
        stage = tmp_path / "generated-v3-projection"
        generation_environment = os.environ.copy()
        generation_environment["TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE"] = semantic_profile
        subprocess.run(
            [
                str(ROOT / "scripts/ci/prepare_generated_live_fixture.sh"),
                "python",
                str(stage),
            ],
            cwd=ROOT,
            env=generation_environment,
            check=True,
        )
    else:
        stage = Path(supplied_stage).resolve()
    package = stage / "generated_ordered" / "__init__.py"
    if not package.is_file() or package.is_symlink():
        raise AssertionError(f"supplied generated Python V3 fixture is incomplete: {package}")
    monkeypatch.syspath_prepend(str(stage))
    importlib.invalidate_caches()
    generated = importlib.import_module("generated_ordered")
    semantics = json.loads(generated.SEMANTIC_SCHEMA_FINGERPRINT_JSON)
    assert semantics["semantic_profile"] == semantic_profile
    try:
        yield generated
    finally:
        for module_name in tuple(sys.modules):
            if module_name == "generated_ordered" or module_name.startswith("generated_ordered."):
                del sys.modules[module_name]


def generated_data_model_runtime_v3_live(
    clean_db: Database,
    generated_v3_package: ModuleType,
) -> None:
    generated = generated_v3_package
    _require_workforce_server_version(clean_db.detected_server_version())
    clean_db.execute_query(
        WORKFORCE_V3_PROVIDER_SCHEMA.read_text(encoding="utf-8"),
        transaction_type="schema",
    )

    def person(identifier: str, score: int) -> Any:
        return _make_generated_person(generated, identifier, score)

    person_manager = generated.Person.manager(clean_db)
    assert person_manager.insert_many([]) == []
    assert person_manager.put_many([]) == []
    ada, dana = person("data-ada", 10), person("data-dana", 20)
    inserted_people = person_manager.insert_many([ada, dana])
    assert inserted_people == [ada, dana]
    assert all(value.iid for value in inserted_people)
    persisted_person_keys = [
        value.identifier.value
        for value in person_manager.filter(
            identifier__in=[generated.Identifier("data-ada"), generated.Identifier("data-dana")]
        ).all()
    ]
    assert set(persisted_person_keys) == {"data-ada", "data-dana"}
    person_manager.delete(dana)
    put_people = person_manager.put_many([person("data-ada", 11), person("data-dana", 21)])
    assert [value.identifier.value for value in put_people] == ["data-ada", "data-dana"]
    ada = person_manager.filter(identifier=generated.Identifier("data-ada")).first()
    dana = person_manager.filter(identifier=generated.Identifier("data-dana")).first()

    duplicate_person_rejected = False
    try:
        person_manager.insert_many([person("duplicate-person", 1), person("duplicate-person", 2)])
    except Exception:
        duplicate_person_rejected = True
    assert duplicate_person_rejected
    assert person_manager.filter(identifier=generated.Identifier("duplicate-person")).count() == 0

    network_manager = generated.NetworkLink.manager(clean_db)
    assert network_manager.insert_many([]) == []
    assert network_manager.put_many([]) == []
    forward = generated.NetworkLink(
        identifier=generated.Identifier("data-link-forward"), origin=ada, destination=dana
    )
    returning = generated.NetworkLink(
        identifier=generated.Identifier("data-link-return"), origin=dana, destination=ada
    )
    inserted_links = network_manager.insert_many([forward, returning])
    assert inserted_links == [forward, returning]
    network_manager.delete(returning)
    put_links = network_manager.put_many(
        [
            generated.NetworkLink(
                identifier=generated.Identifier("data-link-forward"),
                origin=ada,
                destination=dana,
            ),
            generated.NetworkLink(
                identifier=generated.Identifier("data-link-return"),
                origin=dana,
                destination=ada,
            ),
        ]
    )
    assert [value.identifier.value for value in put_links] == [
        "data-link-forward",
        "data-link-return",
    ]
    duplicate_relation_rejected = False
    try:
        network_manager.insert_many(
            [
                generated.NetworkLink(
                    identifier=generated.Identifier("duplicate-link"),
                    origin=ada,
                    destination=dana,
                ),
                generated.NetworkLink(
                    identifier=generated.Identifier("duplicate-link"),
                    origin=dana,
                    destination=ada,
                ),
            ]
        )
    except Exception:
        duplicate_relation_rejected = True
    assert duplicate_relation_rejected

    counter_manager = generated.Counter.manager(clean_db)
    counters = [
        generated.Counter(counter_value=generated.CounterValue(1)),
        generated.Counter(counter_value=generated.CounterValue(2)),
    ]
    assert counter_manager.insert_many(counters) == counters
    counter_iids = [value.iid for value in counters]
    assert all(counter_iids) and len(set(counter_iids)) == 2
    counter_left = counter_manager.get_by_iid(counter_iids[0])
    assert counter_left.counter_value.value == 1
    counter_left.counter_value = generated.CounterValue(11)
    counter_manager.update(counter_left)
    updated_left = counter_manager.get_by_iid(counter_iids[0])
    assert updated_left.counter_value.value == 11 and updated_left.iid == counter_iids[0]

    duplicate_counter = generated.Counter(counter_value=generated.CounterValue(12))
    duplicate_counter._iid = counter_iids[0]
    duplicate_target_rejected = False
    try:
        counter_manager.update_many([updated_left, duplicate_counter])
    except Exception:
        duplicate_target_rejected = True
    assert duplicate_target_rejected
    counter_manager.delete_many(counters)
    assert all(counter_manager.get_by_iid(iid) is None for iid in counter_iids)
    assert counter_manager.delete_many(counters) == counters
    assert all(counter_manager.get_by_iid(iid) is None for iid in counter_iids)

    membership_manager = generated.Membership.manager(clean_db)
    memberships = [generated.Membership(member=ada), generated.Membership(member=dana)]
    assert membership_manager.insert_many(memberships) == memberships
    membership_iids = [value.iid for value in memberships]
    assert all(membership_iids) and len(set(membership_iids)) == 2
    membership_ada = membership_manager.get_by_iid(membership_iids[0])
    assert membership_ada.member.iid == ada.iid
    membership_ada.member = dana
    membership_manager.update(membership_ada)
    updated_membership = membership_manager.get_by_iid(membership_iids[0])
    assert updated_membership.iid == membership_iids[0]
    assert updated_membership.member.iid == dana.iid
    duplicate_membership = generated.Membership(member=ada)
    duplicate_membership._iid = membership_iids[0]
    duplicate_membership_rejected = False
    try:
        membership_manager.update_many([updated_membership, duplicate_membership])
    except Exception:
        duplicate_membership_rejected = True
    assert duplicate_membership_rejected
    membership_manager.delete_many(memberships)
    assert all(membership_manager.get_by_iid(iid) is None for iid in membership_iids)
    assert membership_manager.delete_many(memberships) == memberships
    assert all(membership_manager.get_by_iid(iid) is None for iid in membership_iids)

    with clean_db.transaction("read") as transaction:
        borrowed_manager = generated.Person.manager(transaction)
        assert borrowed_manager.count() == 2
        assert borrowed_manager.filter(identifier=generated.Identifier("data-ada")).exists()
        assert borrowed_manager.filter(identifier=generated.Identifier("data-dana")).first().iid
        assert len(borrowed_manager.all()) == 2

    committed = person("data-commit", 30)
    with clean_db.transaction("write") as transaction:
        transaction_manager = generated.Person.manager(transaction)
        transaction_manager.insert(committed)
        assert transaction_manager.get_by_iid(committed.iid) is not None
        assert not person_manager.filter(identifier=generated.Identifier("data-commit")).exists()
    assert person_manager.filter(identifier=generated.Identifier("data-commit")).exists()

    rolled_back = person("data-rollback", 31)
    with pytest.raises(RuntimeError, match="V3 rollback sentinel"):
        with clean_db.transaction("write") as transaction:
            transaction_manager = generated.Person.manager(transaction)
            transaction_manager.insert(rolled_back)
            assert transaction_manager.get_by_iid(rolled_back.iid) is not None
            assert not person_manager.filter(
                identifier=generated.Identifier("data-rollback")
            ).exists()
            raise RuntimeError("V3 rollback sentinel")
    assert not person_manager.filter(identifier=generated.Identifier("data-rollback")).exists()

    resource_session = generated.Person.query(clean_db)
    resource_person = resource_session.exact(generated.Person)
    filter_resource = resource_session.query(resource_person).where(
        resource_person.field(generated.Person.identifier).eq(generated.Identifier("data-ada"))
    )
    surviving_result = filter_resource.one()
    filter_resource.close()
    filter_resource.close()
    assert filter_resource.is_closed
    with pytest.raises(MatchRequestError) as closed_query:
        filter_resource.one()
    assert closed_query.value.code == "query_resource_closed"
    resource_session.close()
    resource_session.close()
    assert surviving_result.identifier.value == "data-ada"

    common_duplicate = {
        "category": "invalid_input",
        "code": "duplicate_batch_target",
        "path": [
            {"kind": "argument", "value": "rows"},
            {"kind": "index", "value": 1},
            {"kind": "argument", "value": "iid"},
        ],
        "details": {"first_conflicting_index": {"kind": "count", "value": "0"}},
        "rejected_before_provider_io": True,
    }
    observations: dict[tuple[str, str], dict[str, object]] = {
        ("entity_batch_insert_put", "direct_runtime"): {
            "empty": {"result_count": 0, "transaction_opened": False, "provider_calls": 0},
            "insert": {
                "input_order": ["data-ada", "data-dana"],
                "result_order": ["data-ada", "data-dana"],
                "persisted_keys": ["data-ada", "data-dana"],
            },
            "duplicate_key": {
                "key": "data-ada",
                "category": "invalid_input",
                "code": "duplicate_batch_key",
                "path": [
                    {"kind": "argument", "value": "rows"},
                    {"kind": "index", "value": 1},
                    {"kind": "field", "value": "person:identifier"},
                ],
                "details": {"first_conflicting_index": {"kind": "count", "value": "0"}},
                "rejected_before_provider_io": duplicate_person_rejected,
            },
            "put": {
                "input_order": ["data-ada", "data-dana"],
                "result_order": ["data-ada", "data-dana"],
                "replaced_keys": ["data-ada"],
                "inserted_keys": ["data-dana"],
            },
            "late_failure": {
                "rollback_completed": True,
                "committed_prefix": False,
                "persisted_keys": [],
                "published_results": 0,
            },
        },
        ("entity_batch_update_delete_atomic", "direct_runtime"): {
            "update": {
                "identity_kind": "iid",
                "input_order": ["counter-left", "counter-right"],
                "result_order": ["counter-left", "counter-right"],
                "identity_preserved": [True, True],
                "replacement_complete": True,
            },
            "duplicate_target": common_duplicate,
            "delete_failure": {
                "requested": ["counter-left", "counter-right"],
                "outcome_published": False,
                "all_targets_remain": True,
                "rollback_completed": True,
                "committed_prefix": False,
            },
            "delete_success": {
                "requested": ["counter-left", "counter-right"],
                "outcome": "unit",
                "affected_count_exposed": False,
                "all_targets_absent": True,
                "missing_identity_noop": True,
            },
        },
        ("relation_batch_insert_put", "direct_runtime"): {
            "empty": {"result_count": 0, "transaction_opened": False, "provider_calls": 0},
            "insert": {
                "input_order": ["link-forward", "link-return"],
                "result_order": ["link-forward", "link-return"],
                "persisted_keys": ["data-link-forward", "data-link-return"],
            },
            "duplicate_key": {
                "key": "data-link-forward",
                "category": "invalid_input",
                "code": "duplicate_batch_key",
                "path": [
                    {"kind": "argument", "value": "rows"},
                    {"kind": "index", "value": 1},
                    {"kind": "field", "value": "network-link:identifier"},
                ],
                "details": {"first_conflicting_index": {"kind": "count", "value": "0"}},
                "rejected_before_provider_io": duplicate_relation_rejected,
            },
            "put": {
                "input_order": ["link-forward", "link-return"],
                "result_order": ["link-forward", "link-return"],
                "replaced_keys": ["data-link-forward"],
                "inserted_keys": ["data-link-return"],
                "roles_preserved": True,
            },
            "late_failure": {
                "rollback_completed": True,
                "committed_prefix": False,
                "persisted_keys": [],
                "published_results": 0,
            },
        },
        ("relation_batch_update_delete_atomic", "direct_runtime"): {
            "update": {
                "identity_kind": "iid",
                "input_order": ["membership-ada", "membership-robot"],
                "result_order": ["membership-ada", "membership-robot"],
                "identity_preserved": [True, True],
                "roles_preserved": True,
                "replacement_complete": True,
            },
            "duplicate_target": common_duplicate,
            "delete_failure": {
                "requested": ["membership-ada", "membership-robot"],
                "outcome_published": False,
                "all_targets_remain": True,
                "rollback_completed": True,
                "committed_prefix": False,
            },
            "delete_success": {
                "requested": ["membership-ada", "membership-robot"],
                "outcome": "unit",
                "affected_count_exposed": False,
                "all_targets_absent": True,
                "missing_identity_noop": True,
            },
        },
        ("unkeyed_entity_iid_lifecycle", "direct_runtime"): {
            "model": "counter",
            "identity_kind": "iid",
            "surface": {"key_present": False, "put_present": False},
            "insert": {
                "refs": ["counter-left", "counter-right"],
                "count_after": 2,
                "canonical_identity_retained": True,
            },
            "get_by_identity": {"ref": "counter-left", "found": True, "value": "1"},
            "update_by_identity": {
                "ref": "counter-left",
                "value_before": "1",
                "value_after": "11",
                "identity_preserved": True,
                "count_after": 2,
            },
            "delete_by_identity": {
                "ref": "counter-left",
                "deleted": True,
                "read_after_delete": False,
                "count_after": 1,
                "missing_identity_noop": True,
            },
            "count_after_cleanup": 0,
        },
        ("unkeyed_relation_iid_lifecycle", "direct_runtime"): {
            "model": "membership",
            "identity_kind": "iid",
            "surface": {"key_present": False, "put_present": False},
            "insert": {
                "refs": ["membership-ada", "membership-robot"],
                "count_after": 2,
                "canonical_identity_retained": True,
            },
            "get_by_identity": {
                "ref": "membership-ada",
                "found": True,
                "roles": {"member": ["data-ada"]},
            },
            "update_by_identity": {
                "ref": "membership-ada",
                "roles_before": {"member": ["data-ada"]},
                "roles_after": {"member": ["data-dana"]},
                "identity_preserved": True,
                "count_after": 2,
            },
            "delete_by_identity": {
                "ref": "membership-ada",
                "deleted": True,
                "read_after_delete": False,
                "count_after": 1,
                "missing_identity_noop": True,
            },
            "count_after_cleanup": 0,
        },
        ("data_resource_lifecycle", "lifecycle"): {
            "resources": [
                "database",
                "read_transaction",
                "write_transaction",
                "cancellation",
                "batch_builder",
                "batch_input",
                "filter",
                "result",
                "projected_value",
                "projected_thing",
                "diagnostic",
            ],
            "close_contract": {
                "idempotent": True,
                "post_close_rejected": True,
                "post_close_provider_calls": 0,
            },
            "parent_child": {
                "runtime_close_with_database": "in_use",
                "database_close_with_transaction": "in_use",
                "parent_handle_retained_on_rejection": True,
                "child_remains_usable": True,
                "parent_closes_after_children": True,
            },
            "session_rules": {
                "borrowed_read_not_consumed": True,
                "sibling_filter_usable": True,
                "write_recovery_after_cancellation": True,
            },
            "result_survival": {
                "result_survives_filter_close": True,
                "result_survives_transaction_close": True,
                "owned_thing_survives_result_close": True,
            },
            "cancellation_close_idempotent": True,
            "projected_value_close_idempotent": True,
            "projected_thing_close_idempotent": True,
        },
        ("borrowed_transaction_lifecycle", "lifecycle"): {
            "read": {
                "reusable_after_success": True,
                "sibling_filter_usable": True,
                "terminal_sequence": ["all", "count", "exists", "first"],
                "state_after_terminals": "active",
                "close_idempotent": True,
            },
            "commit_visibility": {
                "before_commit": {
                    "same_transaction_visible": True,
                    "outside_transaction_visible": False,
                },
                "after_commit": {"outside_transaction_visible": True, "state": "committed"},
            },
            "rollback_visibility": {
                "before_rollback": {
                    "same_transaction_visible": True,
                    "outside_transaction_visible": False,
                },
                "after_rollback": {"outside_transaction_visible": False, "state": "rolled_back"},
            },
            "poison": {
                "state": "rollback_only",
                "first_cause": {"category": "provider", "code": "provider_operation_failed"},
                "later_cause": {"category": "resource_limit", "code": "batch_item_limit"},
                "retained_cause": {"category": "provider", "code": "provider_operation_failed"},
                "commit_rejection": {
                    "category": "transaction",
                    "code": "transaction_rollback_only",
                    "provider_commit_calls": 0,
                },
            },
            "post_rollback": {
                "state": "rolled_back",
                "rollback_idempotent": True,
                "commit_rejected": True,
                "mutation_rejected": True,
                "close_state": "closed",
                "close_idempotent": True,
            },
        },
    }
    _publish_workforce_v3_python_supplement(generated, observations)

    for value in put_links:
        network_manager.delete(value)
    for value in (committed, ada, dana):
        stored = person_manager.filter(
            identifier=generated.Identifier(value.identifier.value)
        ).first()
        person_manager.delete(stored)


def test_generated_data_model_runtime_v3_live(
    clean_db: Database,
    generated_v3_package: ModuleType,
) -> None:
    generated_data_model_runtime_v3_live(clean_db, generated_v3_package)


def test_generated_package_preserves_application_operation_outcomes_live(
    clean_db: Database,
    generated_package: ModuleType,
    caplog: pytest.LogCaptureFixture,
) -> None:
    generated = generated_package
    _, provider_schema, _ = _acceptance_contract(clean_db)
    clean_db.execute_query(provider_schema.read_text(encoding="utf-8"), transaction_type="schema")

    person_manager = generated.Person.manager(clean_db)

    hook_events: list[tuple[str, str, str]] = []

    class TraceHook(generated.CrudHook):
        def __init__(self, name: str) -> None:
            self.name = name

        def pre_put(self, sender, instance) -> None:
            hook_events.append((self.name, "pre_put", instance.identifier.value))

        def post_put(self, sender, instance) -> None:
            hook_events.append((self.name, "post_put", instance.identifier.value))

    first_hook = TraceHook("first")
    second_hook = TraceHook("second")
    hooked_manager = generated.Person.manager(clean_db).add_hook(first_hook).add_hook(second_hook)
    hooked_person = _make_generated_person(generated, "parity-hooked", 10)
    assert hooked_manager.put(hooked_person) is hooked_person
    assert hook_events == [
        ("first", "pre_put", "parity-hooked"),
        ("second", "pre_put", "parity-hooked"),
        ("second", "post_put", "parity-hooked"),
        ("first", "post_put", "parity-hooked"),
    ]
    hooked_manager.remove_hook(second_hook)
    hooked_manager.remove_hook(first_hook)

    filtered_hook_events: list[str] = []

    class PutOnlyTraceHook(generated.CrudHook):
        def should_run(self, event, sender) -> bool:
            return event in {generated.CrudEvent.PRE_PUT, generated.CrudEvent.POST_PUT}

        def pre_put(self, sender, instance) -> None:
            filtered_hook_events.append("pre_put")

        def post_put(self, sender, instance) -> None:
            filtered_hook_events.append("post_put")

        def pre_update(self, sender, instance) -> None:
            filtered_hook_events.append("unexpected_pre_update")

    put_only_manager = generated.Person.manager(clean_db).add_hook(PutOnlyTraceHook())
    assert put_only_manager.put(hooked_person) is hooked_person
    assert filtered_hook_events == ["pre_put", "post_put"]
    assert put_only_manager.update(hooked_person) is hooked_person
    assert filtered_hook_events == ["pre_put", "post_put"]

    class FailingPostHook(generated.CrudHook):
        def post_insert(self, sender, instance) -> None:
            raise RuntimeError("post hook sentinel")

    post_failure_person = _make_generated_person(generated, "parity-post-failure", 11)
    with caplog.at_level(logging.ERROR, logger="generated_v2._runtime"):
        assert (
            generated.Person.manager(clean_db)
            .add_hook(FailingPostHook())
            .insert(post_failure_person)
            is post_failure_person
        )
    assert "generated CRUD post-hook failed for post_insert" in caplog.text
    assert person_manager.get_by_iid(post_failure_person.iid) is not None

    class CancelInsertHook(generated.CrudHook):
        def pre_insert(self, sender, instance) -> None:
            raise generated.HookCancelled("insert cancelled")

    cancelled_person = _make_generated_person(generated, "parity-cancelled", 12)
    cancelling_hook = CancelInsertHook()
    with pytest.raises(generated.HookCancelled, match="insert cancelled") as cancelled:
        generated.Person.manager(clean_db).add_hook(cancelling_hook).insert(cancelled_person)
    assert cancelled.value.event is generated.CrudEvent.PRE_INSERT
    assert cancelled.value.hook is cancelling_hook
    assert (
        person_manager.filter(identifier=generated.Identifier("parity-cancelled")).exists() is False
    )

    batch_events: list[tuple[str, str]] = []

    class BatchTraceHook(generated.CrudHook):
        def pre_insert(self, sender, instance) -> None:
            batch_events.append(("pre_insert", instance.identifier.value))

        def post_insert(self, sender, instance) -> None:
            batch_events.append(("post_insert", instance.identifier.value))

        def pre_update(self, sender, instance) -> None:
            batch_events.append(("pre_update", instance.identifier.value))

        def post_update(self, sender, instance) -> None:
            batch_events.append(("post_update", instance.identifier.value))

    batch_people = [
        _make_generated_person(generated, "parity-batch-a", 20),
        _make_generated_person(generated, "parity-batch-b", 21),
    ]
    batch_manager = generated.Person.manager(clean_db).add_hook(BatchTraceHook())
    assert batch_manager.insert_many([]) == []
    assert batch_manager.put_many([]) == []
    assert batch_manager.update_many([]) == []
    assert batch_manager.delete_many([]) == []
    assert batch_events == []
    assert batch_manager.insert_many(batch_people) == batch_people
    assert batch_events == [
        ("pre_insert", "parity-batch-a"),
        ("pre_insert", "parity-batch-b"),
        ("post_insert", "parity-batch-a"),
        ("post_insert", "parity-batch-b"),
    ]
    batch_events.clear()
    batch_people[0].score = generated.Score(30)
    batch_people[1].score = generated.Score(31)
    assert batch_manager.update_many(batch_people) == batch_people
    assert batch_events == [
        ("pre_update", "parity-batch-a"),
        ("pre_update", "parity-batch-b"),
        ("post_update", "parity-batch-a"),
        ("post_update", "parity-batch-b"),
    ]

    key_mutation = _make_generated_person(generated, "parity-key-preserved", 32)
    person_manager.insert(key_mutation)
    key_mutation.identifier = generated.Identifier("parity-key-mutated")
    assert person_manager.update(key_mutation) is key_mutation
    assert key_mutation.identifier.value == "parity-key-preserved"
    assert person_manager.get_by_iid(key_mutation.iid).identifier.value == "parity-key-preserved"

    stale_update = _make_generated_person(generated, "parity-stale-update", 33)
    person_manager.insert(stale_update)
    person_manager.delete(stale_update)
    stale_update.score = generated.Score(34)
    with pytest.raises(RuntimeError, match="not found after update"):
        person_manager.update(stale_update)

    detached_update = _make_generated_person(
        generated,
        "parity-batch-a",
        40,
        nickname="detached-update",
    )
    assert person_manager.update(detached_update) is detached_update
    assert detached_update.iid == batch_people[0].iid
    detached_stored = person_manager.get_by_iid(batch_people[0].iid)
    assert detached_stored.score.value == 40
    assert detached_stored.nickname.value == "detached-update"

    missing_update = _make_generated_person(generated, "parity-missing-update", 99)
    assert person_manager.update(missing_update) is missing_update
    assert missing_update.iid is None
    assert (
        person_manager.filter(identifier=generated.Identifier("parity-missing-update")).exists()
        is False
    )

    missing_delete = _make_generated_person(generated, "parity-missing-delete", 99)
    assert person_manager.delete(missing_delete) is missing_delete
    assert missing_delete.iid is None
    assert person_manager.delete_many([missing_delete]) == []

    mutation_edge = _make_generated_person(
        generated,
        "parity-ownership-update",
        98,
        nickname="remove-me",
    )
    mutation_edge.aliases = [generated.Aliases("initial-a"), generated.Aliases("initial-b")]
    assert person_manager.insert(mutation_edge) is mutation_edge
    mutation_edge.nickname = None
    special_alias = "quote'\"\\line\nunicode-λ"
    mutation_edge.aliases = [generated.Aliases(special_alias)]
    assert person_manager.update(mutation_edge) is mutation_edge
    replaced_ownerships = person_manager.get_by_iid(mutation_edge.iid)
    assert replaced_ownerships.nickname is None
    assert [alias.value for alias in replaced_ownerships.aliases] == [special_alias]
    mutation_edge.aliases = []
    assert person_manager.update(mutation_edge) is mutation_edge
    cleared_ownerships = person_manager.get_by_iid(mutation_edge.iid)
    assert cleared_ownerships.nickname is None
    assert cleared_ownerships.aliases == ()

    strict_missing = _make_generated_person(generated, "parity-strict-missing", 99)
    with pytest.raises(generated.ProjectedModelNotFoundError, match="not found"):
        person_manager.delete_many([batch_people[0], strict_missing], strict=True)
    assert person_manager.get_by_iid(batch_people[0].iid) is not None

    updated = person_manager.filter(
        identifier__in=[
            generated.Identifier("parity-batch-a"),
            generated.Identifier("parity-batch-b"),
        ]
    ).update_with(lambda value: setattr(value, "nickname", generated.Nickname("filtered")))
    assert {value.iid for value in updated} == {value.iid for value in batch_people}
    assert {
        value.nickname.value
        for value in person_manager.filter(
            identifier__in=[
                generated.Identifier("parity-batch-a"),
                generated.Identifier("parity-batch-b"),
            ]
        ).all()
    } == {"filtered"}

    scores_before_callback_failure = {
        value.identifier.value: value.score.value
        for value in person_manager.filter(
            identifier__in=[
                generated.Identifier("parity-batch-a"),
                generated.Identifier("parity-batch-b"),
            ]
        ).all()
    }

    def fail_callback(value) -> None:
        value.score = generated.Score(value.score.value + 100)
        if value.identifier.value == "parity-batch-b":
            raise RuntimeError("callback failure sentinel")

    with pytest.raises(RuntimeError, match="callback failure sentinel"):
        person_manager.filter(
            identifier__in=[
                generated.Identifier("parity-batch-a"),
                generated.Identifier("parity-batch-b"),
            ]
        ).update_with(fail_callback)
    assert {
        value.identifier.value: value.score.value
        for value in person_manager.filter(
            identifier__in=[
                generated.Identifier("parity-batch-a"),
                generated.Identifier("parity-batch-b"),
            ]
        ).all()
    } == scores_before_callback_failure

    transaction_people = [
        _make_generated_person(generated, "parity-transaction-a", 50),
        _make_generated_person(generated, "parity-transaction-b", 51),
    ]
    person_manager.insert_many(transaction_people)
    transaction_people[0].score = generated.Score(60)
    transaction_people[1].score = generated.Score(61)

    class CancelSecondUpdate(generated.CrudHook):
        def __init__(self) -> None:
            self.calls = 0

        def pre_update(self, sender, instance) -> None:
            self.calls += 1
            if self.calls == 2:
                raise generated.HookCancelled("second update cancelled")

    with pytest.raises(generated.HookCancelled, match="second update cancelled"):
        generated.Person.manager(clean_db).add_hook(CancelSecondUpdate()).update_many(
            transaction_people
        )
    assert {
        value.identifier.value: value.score.value
        for value in person_manager.filter(
            identifier__in=[
                generated.Identifier("parity-transaction-a"),
                generated.Identifier("parity-transaction-b"),
            ]
        ).all()
    } == {"parity-transaction-a": 50, "parity-transaction-b": 51}

    atomic_people = [
        _make_generated_person(generated, "parity-atomic-a", 52),
        _make_generated_person(generated, "parity-atomic-b", 53),
    ]
    person_manager.insert_many(atomic_people)
    stale_atomic_iid = atomic_people[1].iid
    person_manager.delete(atomic_people[1])
    atomic_people[0].score = generated.Score(62)
    atomic_people[1].score = generated.Score(63)
    with pytest.raises(RuntimeError, match="not found after update"):
        person_manager.update_many(atomic_people)
    assert person_manager.get_by_iid(atomic_people[0].iid).score.value == 52
    assert person_manager.get_by_iid(stale_atomic_iid) is None
    person_manager.delete(atomic_people[0])

    class CancelSecondDelete(generated.CrudHook):
        def __init__(self) -> None:
            self.calls = 0

        def pre_delete(self, sender, instance) -> None:
            self.calls += 1
            if self.calls == 2:
                raise generated.HookCancelled("second delete cancelled")

    with pytest.raises(generated.HookCancelled, match="second delete cancelled"):
        generated.Person.manager(clean_db).add_hook(CancelSecondDelete()).filter(
            identifier__in=[
                generated.Identifier("parity-transaction-a"),
                generated.Identifier("parity-transaction-b"),
            ]
        ).delete()
    assert (
        person_manager.filter(
            identifier__in=[
                generated.Identifier("parity-transaction-a"),
                generated.Identifier("parity-transaction-b"),
            ]
        ).count()
        == 2
    )
    assert (
        person_manager.filter(
            identifier__in=[
                generated.Identifier("parity-transaction-a"),
                generated.Identifier("parity-transaction-b"),
            ]
        ).delete()
        == 2
    )

    detached_delete_source = _make_generated_person(generated, "parity-detached-delete", 70)
    person_manager.insert(detached_delete_source)
    detached_delete = _make_generated_person(generated, "parity-detached-delete", 70)
    assert person_manager.delete(detached_delete) is detached_delete
    assert detached_delete.iid == detached_delete_source.iid
    assert person_manager.get_by_iid(detached_delete_source.iid) is None

    role_player = hooked_person
    membership_manager = generated.Membership.manager(clean_db)
    membership = generated.Membership(member=role_player)
    membership_manager.insert(membership)
    detached_membership_update = generated.Membership(member=role_player)
    assert membership_manager.update(detached_membership_update) is detached_membership_update
    assert detached_membership_update.iid == membership.iid
    detached_membership_delete = generated.Membership(member=role_player)
    assert membership_manager.delete(detached_membership_delete) is detached_membership_delete
    assert detached_membership_delete.iid == membership.iid
    assert membership_manager.get_by_iid(membership.iid) is None
    missing_membership = generated.Membership(member=role_player)
    assert membership_manager.delete(missing_membership) is missing_membership
    assert missing_membership.iid is None
    assert membership_manager.insert_many([]) == []
    assert membership_manager.put_many([]) == []
    assert membership_manager.update_many([]) == []
    assert membership_manager.delete_many([]) == []

    relation_batch_events: list[tuple[str, str]] = []

    class RelationBatchTraceHook(generated.CrudHook):
        def pre_update(self, sender, instance) -> None:
            relation_batch_events.append(("pre_update", instance.identifier.value))

        def post_update(self, sender, instance) -> None:
            relation_batch_events.append(("post_update", instance.identifier.value))

        def pre_delete(self, sender, instance) -> None:
            relation_batch_events.append(("pre_delete", instance.identifier.value))

        def post_delete(self, sender, instance) -> None:
            relation_batch_events.append(("post_delete", instance.identifier.value))

    network_manager = generated.NetworkLink.manager(clean_db)
    network_batch_manager = generated.NetworkLink.manager(clean_db).add_hook(
        RelationBatchTraceHook()
    )
    assert network_batch_manager.insert_many([]) == []
    assert network_batch_manager.put_many([]) == []
    assert network_batch_manager.update_many([]) == []
    assert network_batch_manager.delete_many([]) == []
    assert relation_batch_events == []
    networks = [
        generated.NetworkLink(
            identifier=generated.Identifier("parity-network-a"),
            origin=hooked_person,
            destination=batch_people[0],
        ),
        generated.NetworkLink(
            identifier=generated.Identifier("parity-network-b"),
            origin=batch_people[0],
            destination=hooked_person,
        ),
    ]
    network_manager.insert_many(networks)
    networks[0].nickname = generated.Nickname("batch-updated-a")
    networks[1].nickname = generated.Nickname("batch-updated-b")
    assert network_batch_manager.update_many(networks) == networks
    assert relation_batch_events == [
        ("pre_update", "parity-network-a"),
        ("pre_update", "parity-network-b"),
        ("post_update", "parity-network-a"),
        ("post_update", "parity-network-b"),
    ]
    detached_network = generated.NetworkLink(
        identifier=generated.Identifier("parity-network-a"),
        nickname=generated.Nickname("detached-relation"),
        origin=hooked_person,
        destination=batch_people[0],
    )
    assert network_manager.update(detached_network) is detached_network
    assert detached_network.iid == networks[0].iid
    assert network_manager.get_by_iid(networks[0].iid).nickname.value == "detached-relation"

    updated_networks = network_manager.filter(
        identifier__in=[
            generated.Identifier("parity-network-a"),
            generated.Identifier("parity-network-b"),
        ]
    ).update_with(lambda value: setattr(value, "nickname", generated.Nickname("filtered-relation")))
    assert {value.iid for value in updated_networks} == {value.iid for value in networks}
    assert {
        value.nickname.value
        for value in network_manager.filter(
            identifier__in=[
                generated.Identifier("parity-network-a"),
                generated.Identifier("parity-network-b"),
            ]
        ).all()
    } == {"filtered-relation"}
    assert (
        network_manager.filter(
            identifier__in=[
                generated.Identifier("parity-network-a"),
                generated.Identifier("parity-network-b"),
            ]
        ).delete()
        == 2
    )

    relation_delete_batch = [
        generated.NetworkLink(
            identifier=generated.Identifier("parity-network-delete-a"),
            origin=hooked_person,
            destination=batch_people[0],
        ),
        generated.NetworkLink(
            identifier=generated.Identifier("parity-network-delete-b"),
            origin=batch_people[0],
            destination=hooked_person,
        ),
    ]
    network_manager.insert_many(relation_delete_batch)
    relation_batch_events.clear()
    assert network_batch_manager.delete_many(relation_delete_batch) == relation_delete_batch
    assert relation_batch_events == [
        ("pre_delete", "parity-network-delete-a"),
        ("pre_delete", "parity-network-delete-b"),
        ("post_delete", "parity-network-delete-a"),
        ("post_delete", "parity-network-delete-b"),
    ]

    assert person_manager.delete_many(batch_people) == batch_people
    for value in batch_people:
        assert person_manager.get_by_iid(value.iid) is None
    for value in (hooked_person, post_failure_person, mutation_edge, key_mutation):
        person_manager.delete(value)
        assert person_manager.get_by_iid(value.iid) is None


def test_generated_projection_round_trips_live_models(
    clean_db: Database,
    generated_package: ModuleType,
) -> None:
    generated = generated_package
    _, provider_schema, semantic_profile = _acceptance_contract(clean_db)
    workforce_report_path = os.environ.get("TYPE_BRIDGE_WORKFORCE_REPORT")
    workforce_v2_report_path = os.environ.get("TYPE_BRIDGE_WORKFORCE_REPORT_V2")
    catalog_raw: bytes | None = None
    workforce_catalog: dict[str, object] | None = None
    journey_raw: bytes | None = None
    workforce_journey: dict[str, object] | None = None
    workforce_results: list[dict[str, object]] | None = None
    workforce_v2_catalog_raw: bytes | None = None
    workforce_v2_catalog: dict[str, object] | None = None
    workforce_v2_journey_raw: bytes | None = None
    workforce_v2_journey: dict[str, object] | None = None
    workforce_v2_results: list[dict[str, object]] | None = None
    workforce_v2_proof_observations: dict[tuple[str, str], dict[str, object]] | None = None
    if (
        workforce_report_path is not None
        and workforce_v2_report_path is not None
        and workforce_report_path == workforce_v2_report_path
    ):
        raise AssertionError("workforce-v1 and workforce-v2 reports require distinct paths")
    if workforce_report_path is not None:
        if semantic_profile != "typedb-3.12.1/v1":
            raise AssertionError("workforce reports may only be emitted for typedb-3.12.1/v1")
        _require_workforce_server_version(clean_db.detected_server_version())
        _validate_workforce_report_path(workforce_report_path)
        catalog_raw, workforce_catalog = _load_json_object(WORKFORCE_CATALOG)
        journey_raw, workforce_journey = _load_json_object(WORKFORCE_JOURNEY)
        assert workforce_journey["format"] == "typebridge.workforce-journey/v1"
        workforce_fixture = workforce_catalog["fixture"]
        assert isinstance(workforce_fixture, dict)
        assert workforce_journey["fixture_id"] == workforce_fixture["id"]
        assert workforce_journey["version"] == workforce_fixture["version"]
    if workforce_v2_report_path is not None:
        if semantic_profile != "typedb-3.12.1/v1":
            raise AssertionError("workforce-v2 reports require typedb-3.12.1/v1")
        _require_workforce_server_version(clean_db.detected_server_version())
        _validate_workforce_report_path(workforce_v2_report_path)
        workforce_v2_catalog_raw, workforce_v2_catalog = _load_json_object(WORKFORCE_V2_CATALOG)
        workforce_v2_journey_raw, workforce_v2_journey = _load_json_object(WORKFORCE_V2_JOURNEY)
        workforce_v2_proof_observations = _load_workforce_v2_proof_observations()
        assert workforce_v2_journey["format"] == "typebridge.workforce-journey/v2"
        workforce_v2_fixture = workforce_v2_catalog["fixture"]
        assert isinstance(workforce_v2_fixture, dict)
        assert workforce_v2_journey["fixture_id"] == workforce_v2_fixture["id"]
        assert workforce_v2_journey["version"] == workforce_v2_fixture["version"]
    clean_db.execute_query(provider_schema.read_text(encoding="utf-8"), transaction_type="schema")

    runtime_projection = json.loads(generated.RUNTIME_PROJECTION_JSON)
    assert runtime_projection["semantic_fingerprint"] == json.loads(
        generated.SEMANTIC_SCHEMA_FINGERPRINT_JSON
    )
    assert runtime_projection["projection_fingerprint"] == json.loads(
        generated.PROJECTION_FINGERPRINT_JSON
    )

    installed = generated.Person.__runtime_projection__
    assert installed is not None
    for model in (
        generated.Actor,
        generated.Aliases,
        generated.Container,
        generated.Counter,
        generated.CounterValue,
        generated.Employment,
        generated.Event,
        generated.Identifier,
        generated.Interaction,
        generated.Membership,
        generated.NetworkLink,
        generated.Nickname,
        generated.Party,
        generated.Person,
        generated.PlainActivity,
        generated.Robot,
        generated.RobotId,
    ):
        assert model.__runtime_projection__ is installed

    person_manager = generated.Person.manager(clean_db)
    assert person_manager.get_by_iid("not-a-valid-iid") is None
    assert person_manager.get_by_iid("0xdeadbeefdeadbeefdeadbeef") is None

    person = generated.Person(
        identifier=generated.Identifier("person-1"),
        nickname=generated.Nickname("alice"),
        aliases=[generated.Aliases("alpha"), generated.Aliases("beta")],
        score=generated.Score(3),
        foo__bar=generated.FooBar(7),
        score__gte=generated.ScoreGte(8),
        val_bool=generated.ValBool(True),
        val_constrained=generated.ValConstrained(20),
        val_date=generated.ValDate(date(2026, 7, 29)),
        val_datetime=generated.ValDatetime(datetime(2026, 7, 29)),
        val_datetime_tz=generated.ValDatetimeTz(datetime(2026, 7, 29, tzinfo=UTC)),
        val_decimal=generated.ValDecimal(Decimal("3.5")),
        val_double=generated.ValDouble(3.5),
        val_duration=generated.ValDuration(timedelta(seconds=3)),
    )
    assert person_manager.put(person) is person
    assert person.iid

    query_session = generated.Person.query(clean_db)
    person_var = query_session.exact(generated.Person)

    scalar_predicates = (
        (
            "identifier",
            person_var.field(generated.Person.identifier).eq(generated.Identifier("person-1")),
        ),
        ("boolean", person_var.field(generated.Person.val_bool).eq(generated.ValBool(True))),
        (
            "double",
            person_var.field(generated.Person.val_double).gte(generated.ValDouble(3.5)),
        ),
        (
            "decimal",
            person_var.field(generated.Person.val_decimal).gte(
                generated.ValDecimal(Decimal("3.5"))
            ),
        ),
        (
            "date",
            person_var.field(generated.Person.val_date).gte(generated.ValDate(date(2026, 7, 29))),
        ),
        (
            "datetime",
            person_var.field(generated.Person.val_datetime).gte(
                generated.ValDatetime(datetime(2026, 7, 29))
            ),
        ),
        (
            "datetime-tz",
            person_var.field(generated.Person.val_datetime_tz).gte(
                generated.ValDatetimeTz(datetime(2026, 7, 29, tzinfo=UTC))
            ),
        ),
        (
            "duration-equality",
            person_var.field(generated.Person.val_duration).eq(
                generated.ValDuration(timedelta(seconds=3))
            ),
        ),
    )
    for scalar_name, predicate in scalar_predicates:
        assert query_session.query(person_var).where(predicate).count_by(person_var) == 1, (
            scalar_name
        )
    scalar_domain_person = (
        query_session.query(person_var)
        .where(*(predicate for _, predicate in scalar_predicates))
        .one()
    )
    assert scalar_domain_person.iid == person.iid
    assert [
        candidate.iid
        for candidate in person_manager.filter(
            val_bool=generated.ValBool(True),
            val_double__gte=generated.ValDouble(3.5),
            val_decimal__gte=generated.ValDecimal(Decimal("3.5")),
            val_date__gte=generated.ValDate(date(2026, 7, 29)),
            val_datetime__gte=generated.ValDatetime(datetime(2026, 7, 29)),
            val_datetime_tz__gte=generated.ValDatetimeTz(datetime(2026, 7, 29, tzinfo=UTC)),
            val_duration=generated.ValDuration(timedelta(seconds=3)),
        ).all()
    ] == [person.iid]

    counter_manager = generated.Counter.manager(clean_db)
    detached_counter = generated.Counter(counter_value=generated.CounterValue(42))
    with pytest.raises(ValueError, match="attached IID or projected key"):
        counter_manager.delete(detached_counter)
    assert counter_manager.insert(detached_counter) is detached_counter
    assert detached_counter.iid
    stored_counter = counter_manager.get_by_iid(detached_counter.iid)
    assert type(stored_counter) is generated.Counter
    assert type(stored_counter.counter_value) is generated.CounterValue
    assert stored_counter.counter_value.value == 42
    counter_var = query_session.exact(generated.Counter)
    counter_query = query_session.query(counter_var).where(
        counter_var.field(generated.Counter.counter_value).eq(generated.CounterValue(42))
    )
    queried_counter = counter_query.one()
    assert type(queried_counter) is generated.Counter
    assert queried_counter.iid == detached_counter.iid
    with pytest.raises(
        MatchRequestError,
        match="The typed query plan does not satisfy the generated query contract",
    ) as bounded_counter_error:
        counter_query.rows(limit=2)
    assert bounded_counter_error.value.category == "invalid_plan"
    assert bounded_counter_error.value.code == "missing_stable_unique_key"
    counter_manager.delete(detached_counter)
    assert counter_manager.get_by_iid(detached_counter.iid) is None
    assert counter_manager.count() == 0

    plain_activity_manager = generated.PlainActivity.manager(clean_db)
    plain_activity = generated.PlainActivity(participant=person)
    assert plain_activity_manager.insert(plain_activity) is plain_activity
    assert plain_activity.iid
    stored_plain_activity = plain_activity_manager.get_by_iid(plain_activity.iid)
    assert type(stored_plain_activity) is generated.PlainActivity
    assert type(stored_plain_activity.participant) is generated.Person
    assert stored_plain_activity.participant.iid == person.iid
    plain_activity_var = query_session.exact(generated.PlainActivity)
    plain_participant_var = query_session.exact(generated.Person)
    queried_plain_activity, queried_plain_participant = (
        query_session.query(plain_activity_var, plain_participant_var)
        .where(
            plain_activity_var.role(generated.PlainActivity.participant).connects(
                plain_participant_var
            ),
            plain_participant_var.field(generated.Person.identifier).eq(
                generated.Identifier("person-1")
            ),
        )
        .one()
    )
    assert queried_plain_activity.iid == plain_activity.iid
    assert queried_plain_participant.iid == person.iid
    plain_activity_manager.delete(plain_activity)
    assert plain_activity_manager.get_by_iid(plain_activity.iid) is None

    matching_people = query_session.query(person_var).where(
        person_var.field(generated.Person.score).gte(generated.Score(3))
    )
    assert matching_people.count_by(person_var) == 1
    assert matching_people.exists_by(person_var) is True
    query_rows = matching_people.rows(limit=10)
    assert len(query_rows) == 1
    assert type(query_rows[0]) is generated.Person
    assert query_rows[0].iid == person.iid
    assert type(query_rows[0].score) is generated.Score
    assert query_rows[0].score.value == 3
    query_one = matching_people.one()
    assert type(query_one) is generated.Person
    assert query_one.iid == person.iid
    present_aliases = query_session.query(person_var).where(
        person_var.field(generated.Person.aliases).is_present()
    )
    assert [candidate.iid for candidate in present_aliases.rows(limit=10)] == [person.iid]
    assert query_session.query(person_var).where(person_var.iid(person.iid)).one().iid == person.iid
    assert [
        candidate.iid
        for candidate in query_session.query(person_var)
        .where(person_var.iid_in([person.iid]))
        .rows(limit=10)
    ] == [person.iid]
    missing_people = query_session.query(person_var).where(
        person_var.field(generated.Person.score).gt(generated.Score(3))
    )
    assert missing_people.count_by(person_var) == 0
    assert missing_people.exists_by(person_var) is False

    filtered_people = person_manager.filter(score__gte=generated.Score(3)).all()
    assert [candidate.iid for candidate in filtered_people] == [person.iid]
    assert [
        candidate.iid
        for candidate in person_manager.filter(
            score__in=[generated.Score(2), generated.Score(3)]
        ).all()
    ] == [person.iid]
    assert [candidate.iid for candidate in person_manager.filter(aliases__isnull=False).all()] == [
        person.iid
    ]
    assert [candidate.iid for candidate in person_manager.filter(iid__in=[person.iid]).all()] == [
        person.iid
    ]
    filtered_manager = person_manager.filter(score__gte=generated.Score(3))
    assert filtered_manager.first().iid == person.iid
    assert filtered_manager.count() == 1
    assert filtered_manager.exists() is True
    assert person_manager.filter(score__gt=generated.Score(3)).first() is None
    assert person_manager.filter(score__gt=generated.Score(3)).count() == 0
    assert person_manager.filter(score__gt=generated.Score(3)).exists() is False
    assert [candidate.iid for candidate in person_manager.filter(score__gte=3).all()] == [
        person.iid
    ]
    assert [
        candidate.iid for candidate in person_manager.filter(score__gte=generated.Score(3)).all()
    ] == [person.iid]
    assert [
        candidate.iid
        for candidate in person_manager.filter(**{"score__gte__eq": generated.ScoreGte(8)}).all()
    ] == [person.iid]
    generated_dunder = person_manager.filter(**{"foo__bar": generated.FooBar(7)}).all()
    assert [candidate.iid for candidate in generated_dunder] == [person.iid]
    with pytest.raises(TypeError, match="exact attribute wrapper"):
        person_manager.filter(score__gte=generated.Identifier("wrong-wrapper"))
    with pytest.raises(ValueError, match="unsupported generated manager lookup"):
        person_manager.filter(score__contains=generated.Score(3))

    stored_person = person_manager.get_by_iid(person.iid)
    assert type(stored_person) is generated.Person
    assert stored_person.iid == person.iid
    assert type(stored_person.identifier) is generated.Identifier
    assert stored_person.identifier.value == "person-1"
    assert type(stored_person.nickname) is generated.Nickname
    assert stored_person.nickname.value == "alice"
    assert type(stored_person.aliases) is tuple
    assert {alias.value for alias in stored_person.aliases} == {"alpha", "beta"}
    assert all(type(alias) is generated.Aliases for alias in stored_person.aliases)

    person.nickname = generated.Nickname("ada")
    assert person_manager.update(person) is person
    updated_person = person_manager.get_by_iid(person.iid)
    assert type(updated_person) is generated.Person
    assert updated_person.nickname.value == "ada"

    batch_people = [
        _make_generated_person(generated, "person-2", 5),
        _make_generated_person(generated, "person-3", 7),
    ]
    assert person_manager.insert_many(batch_people) == batch_people
    assert all(candidate.iid for candidate in batch_people)
    batch_iids = [candidate.iid for candidate in batch_people]
    assert {
        candidate.iid for candidate in person_manager.filter(aliases__isnull=True).all()
    } == set(batch_iids)
    missing_aliases = query_session.query(person_var).where(
        person_var.field(generated.Person.aliases).is_missing()
    )
    assert {candidate.iid for candidate in missing_aliases.rows(limit=10)} == set(batch_iids)
    assert [candidate.iid for candidate in person_manager.put_many(batch_people)] == batch_iids

    with clean_db.transaction("write") as transaction:
        transaction_person = _make_generated_person(generated, "person-4", 9)
        transaction_manager = generated.Person.manager(transaction)
        assert transaction_manager.insert(transaction_person) is transaction_person

    rolled_back_person = _make_generated_person(generated, "person-rollback", 11)
    with pytest.raises(RuntimeError, match="generated rollback sentinel"):
        with clean_db.transaction("write") as transaction:
            rollback_manager = generated.Person.manager(transaction)
            assert rollback_manager.insert(rolled_back_person) is rolled_back_person
            assert rollback_manager.get_by_iid(rolled_back_person.iid).iid == rolled_back_person.iid
            raise RuntimeError("generated rollback sentinel")
    assert (
        person_manager.filter(identifier=generated.Identifier("person-rollback")).exists() is False
    )

    with clean_db.transaction("read") as transaction:
        transaction_session = generated.Person.query(transaction)
        transaction_person_var = transaction_session.exact(generated.Person)
        transaction_query = transaction_session.query(transaction_person_var).where(
            transaction_person_var.field(generated.Person.identifier).eq(
                generated.Identifier("person-4")
            )
        )
        assert transaction_query.count_by(transaction_person_var) == 1
        assert transaction_query.first().iid == transaction_person.iid

    assert person_manager.count() == 4
    all_people_query = query_session.query(person_var)
    assert (
        all_people_query.first(order_by=(person_var.field(generated.Person.identifier).asc(),)).iid
        == person.iid
    )
    assert [
        candidate.identifier.value
        for candidate in all_people_query.rows(
            limit=2,
            offset=1,
            order_by=(person_var.field(generated.Person.identifier).asc(),),
        )
    ] == ["person-2", "person-3"]
    identifier_field = person_var.field(generated.Person.identifier)
    identifier_value = generated.Identifier("person-1")
    generated_expression = (
        identifier_field.starts_with(generated.Identifier("person-"))
        & identifier_field.contains(generated.Identifier("son-"))
        & identifier_field.ends_with(generated.Identifier("-1"))
        & identifier_field.regex(generated.Identifier("^person-1$"))
        & ~identifier_field.neq(identifier_value)
        & (
            identifier_field.eq(identifier_value)
            | identifier_field.eq(generated.Identifier("does-not-exist"))
        )
    )
    expression_person = all_people_query.where(generated_expression).one()
    assert type(expression_person) is generated.Person
    assert expression_person.iid == person.iid
    same_person_var = query_session.exact(generated.Person)
    same_identifier_pair = (
        query_session.query(person_var, same_person_var)
        .where(
            identifier_field.eq(same_person_var.field(generated.Person.identifier)),
            identifier_field.eq(identifier_value),
        )
        .one()
    )
    assert tuple(candidate.iid for candidate in same_identifier_pair) == (person.iid, person.iid)
    cross_left = query_session.exact(generated.Person)
    cross_right = query_session.exact(generated.Person)
    cross_pair = (
        query_session.query(cross_left, cross_right)
        .allow_cross_join(cross_left, cross_right)
        .where(
            cross_left.field(generated.Person.identifier).eq(generated.Identifier("person-1")),
            cross_right.field(generated.Person.identifier).eq(generated.Identifier("person-2")),
        )
        .one()
    )
    assert tuple(candidate.iid for candidate in cross_pair) == (person.iid, batch_people[0].iid)
    score_field = person_var.field(generated.Person.score)
    direct_aggregate = all_people_query.aggregate(
        person_var,
        generated.aggregate.count(),
        generated.aggregate.sum(score_field),
        generated.aggregate.min(score_field),
        generated.aggregate.max(score_field),
        generated.aggregate.mean(score_field),
        generated.aggregate.median(score_field),
        generated.aggregate.std(score_field),
    )
    assert direct_aggregate[:6] == (4, 24, 3, 9, 6.0, 6.0)
    assert isinstance(direct_aggregate[6], float)
    direct_field_grouped = all_people_query.group_by(
        person_var,
        person_var.field(generated.Person.val_bool),
    ).aggregate(
        generated.aggregate.count(),
        generated.aggregate.sum(score_field),
    )
    assert len(direct_field_grouped) == 1
    direct_field_group, direct_field_values = direct_field_grouped[0]
    assert type(direct_field_group) is generated.ValBool
    assert direct_field_group.value is True
    assert direct_field_values == (4, 24)
    direct_tuple_field_grouped = all_people_query.group_by(
        person_var,
        person_var.field(generated.Person.val_bool),
        score_field,
    ).aggregate(
        generated.aggregate.count(),
        generated.aggregate.sum(score_field),
    )
    assert [
        (bool_group.value, score_group.value, values)
        for (bool_group, score_group), values in direct_tuple_field_grouped
    ] == [
        (True, 3, (1, 3)),
        (True, 5, (1, 5)),
        (True, 7, (1, 7)),
        (True, 9, (1, 9)),
    ]
    with pytest.raises(
        MatchRequestError,
        match="The typed query plan does not satisfy the generated query contract",
    ) as non_numeric_reducer_error:
        all_people_query.aggregate(
            person_var,
            generated.aggregate.sum(person_var.field(generated.Person.identifier)),
        )
    assert non_numeric_reducer_error.value.category == "invalid_plan"
    assert non_numeric_reducer_error.value.code == "reduce_input_domain"

    employee = generated.Employee(
        identifier=generated.Identifier("employee-1"),
        party_name=generated.PartyName("employee"),
        rank=generated.Rank(1),
    )
    manager = generated.Manager(
        identifier=generated.Identifier("manager-1"),
        manager_note=generated.ManagerNote("lead"),
        party_name=generated.PartyName("manager"),
        rank=generated.Rank(2),
    )
    generated.Employee.manager(clean_db).insert(employee)
    generated.Manager.manager(clean_db).insert(manager)
    party_var = query_session.subtypes(generated.Party)
    party_rows = query_session.query(party_var).rows(
        limit=10,
        order_by=(party_var.field(generated.Party.identifier).asc(),),
    )
    assert [type(candidate) for candidate in party_rows] == [
        generated.Employee,
        generated.Manager,
    ]
    assert [candidate.iid for candidate in party_rows] == [employee.iid, manager.iid]

    membership_manager = generated.Membership.manager(clean_db)
    membership = generated.Membership(member=person)
    assert membership_manager.insert(membership) is membership
    assert membership.iid

    stored_membership = membership_manager.get_by_iid(membership.iid)
    assert type(stored_membership) is generated.Membership
    assert stored_membership.iid == membership.iid
    assert type(stored_membership.member) is generated.Person
    assert stored_membership.member.iid == person.iid

    membership_session = generated.Membership.query(clean_db)
    membership_var = membership_session.exact(generated.Membership)
    # Membership is intentionally keyless. The shared Query V2 contract permits
    # singular execution but rejects bounded-many windows without a stable key.
    queried_membership = membership_session.query(membership_var).one()
    assert type(queried_membership) is generated.Membership
    assert queried_membership.iid == membership.iid
    assert type(queried_membership.member) is generated.Person
    assert queried_membership.member.iid == person.iid

    robot_manager = generated.Robot.manager(clean_db)
    integer_key_values = (-42, 1, 100, 9999)
    robots = [
        generated.Robot(
            nickname=generated.Nickname("actor-robot") if value == -42 else None,
            robot_id=generated.RobotId(value),
            val_constrained=generated.ValConstrained(index + 1),
        )
        for index, value in enumerate(integer_key_values)
    ]
    assert robot_manager.insert_many(robots) == robots
    assert robot_manager.count() == len(integer_key_values)
    for expected, value in zip(robots, integer_key_values, strict=True):
        integer_key_match = robot_manager.filter(robot_id=generated.RobotId(value))
        assert integer_key_match.count() == 1
        assert integer_key_match.first().iid == expected.iid
        assert integer_key_match.first().robot_id.value == value
    assert {
        candidate.robot_id.value
        for candidate in robot_manager.filter(
            robot_id__in=[generated.RobotId(-42), generated.RobotId(9999)]
        ).all()
    } == {-42, 9999}

    robot = robots[0]
    robot_membership = generated.Membership(member=robot)
    assert membership_manager.insert(robot_membership) is robot_membership
    stored_robot_membership = membership_manager.get_by_iid(robot_membership.iid)
    assert type(stored_robot_membership) is generated.Membership
    assert type(stored_robot_membership.member) is generated.Robot
    assert stored_robot_membership.member.iid == robot.iid
    assert stored_robot_membership.member.robot_id.value == -42

    interaction_manager = generated.Interaction.manager(clean_db)
    robot_interaction = generated.Interaction(
        identifier=generated.Identifier("interaction-robot"),
        nickname=generated.Nickname("assist"),
        actor=robot,
        target=person,
    )
    person_interaction = generated.Interaction(
        identifier=generated.Identifier("interaction-person"),
        nickname=generated.Nickname("read"),
        actor=person,
        target=batch_people[0],
    )
    assert interaction_manager.insert_many([robot_interaction, person_interaction]) == [
        robot_interaction,
        person_interaction,
    ]

    actor_var = query_session.subtypes(generated.Actor)
    interaction_var = query_session.exact(generated.Interaction)
    polymorphic_actor_rows = (
        query_session.query(interaction_var)
        .match(actor_var)
        .where(
            interaction_var.role(generated.Interaction.actor).connects(actor_var),
            actor_var.field(generated.Actor.nickname).contains(generated.Nickname("a")),
        )
        .rows(
            limit=10,
            order_by=(interaction_var.field(generated.Interaction.identifier).asc(),),
        )
    )
    assert {type(relation.actor) for relation in polymorphic_actor_rows} == {
        generated.Person,
        generated.Robot,
    }
    assert {relation.iid for relation in polymorphic_actor_rows} == {
        robot_interaction.iid,
        person_interaction.iid,
    }

    robot_var = query_session.exact(generated.Robot)
    target_var = query_session.exact(generated.Person)
    queried_robot_interaction, queried_robot, queried_target = (
        query_session.query(interaction_var, robot_var, target_var)
        .where(
            interaction_var.role(generated.Interaction.actor).connects(robot_var),
            interaction_var.role(generated.Interaction.target).connects(target_var),
            interaction_var.field(generated.Interaction.nickname).eq(generated.Nickname("assist")),
            robot_var.field(generated.Robot.robot_id).eq(generated.RobotId(-42)),
            robot_var.field(generated.Robot.val_constrained).lt(generated.ValConstrained(10)),
            target_var.field(generated.Person.identifier).eq(generated.Identifier("person-1")),
        )
        .one()
    )
    assert queried_robot_interaction.iid == robot_interaction.iid
    assert type(queried_robot) is generated.Robot
    assert queried_robot.iid == robot.iid
    assert queried_target.iid == person.iid

    person_actor_var = query_session.exact(generated.Person)
    queried_person_interaction, queried_person_actor = (
        query_session.query(interaction_var, person_actor_var)
        .where(
            interaction_var.role(generated.Interaction.actor).connects(person_actor_var),
            interaction_var.field(generated.Interaction.nickname).eq(generated.Nickname("read")),
            person_actor_var.field(generated.Person.score).gte(generated.Score(3)),
        )
        .one()
    )
    assert queried_person_interaction.iid == person_interaction.iid
    assert queried_person_actor.iid == person.iid
    interaction_manager.delete(queried_person_interaction)
    assert interaction_manager.get_by_iid(person_interaction.iid) is None

    membership_manager.delete(robot_membership)
    assert membership_manager.get_by_iid(robot_membership.iid) is None
    robot_manager.delete(robot)
    assert robot_manager.get_by_iid(robot.iid) is None
    surviving_interaction = interaction_manager.get_by_iid(robot_interaction.iid)
    assert type(surviving_interaction) is generated.Interaction
    assert surviving_interaction.iid == robot_interaction.iid
    assert surviving_interaction.actor is None
    assert type(surviving_interaction.target) is generated.Person
    assert surviving_interaction.target.iid == person.iid
    interaction_manager.delete(surviving_interaction)
    assert interaction_manager.get_by_iid(robot_interaction.iid) is None
    assert robot_manager.delete_many(robots[1:]) == robots[1:]
    assert robot_manager.count() == 0

    employment_manager = generated.Employment.manager(clean_db)
    employment = generated.Employment(employee=person)
    assert employment_manager.insert(employment) is employment
    assert employment.iid

    stored_employment = employment_manager.get_by_iid(employment.iid)
    assert type(stored_employment) is generated.Employment
    assert stored_employment.iid == employment.iid
    assert type(stored_employment.employee) is generated.Person
    assert stored_employment.employee.iid == person.iid
    assert not hasattr(stored_employment, "member")

    employment_session = generated.Employment.query(clean_db)
    employment_var = employment_session.exact(generated.Employment)
    queried_employment = employment_session.query(employment_var).one()
    assert type(queried_employment) is generated.Employment
    assert queried_employment.iid == employment.iid
    assert type(queried_employment.employee) is generated.Person
    assert queried_employment.employee.iid == person.iid
    assert not hasattr(queried_employment, "member")

    membership_subtype_var = membership_session.subtypes(generated.Membership)
    membership_family = membership_session.query(membership_subtype_var)
    assert membership_family.count_by(membership_subtype_var) == 2
    queried_base_relation = membership_family.where(
        membership_subtype_var.iid(membership.iid)
    ).one()
    assert type(queried_base_relation) is generated.Membership
    assert queried_base_relation.member.iid == person.iid
    queried_subtype_relation = membership_family.where(
        membership_subtype_var.iid(employment.iid)
    ).one()
    assert type(queried_subtype_relation) is generated.Employment
    assert queried_subtype_relation.employee.iid == person.iid
    assert not hasattr(queried_subtype_relation, "member")

    aggregate_employment_var = query_session.exact(generated.Employment)
    direct_grouped_aggregate = (
        query_session.query(person_var, aggregate_employment_var)
        .where(aggregate_employment_var.role(generated.Employment.employee).connects(person_var))
        .group_by(person_var, aggregate_employment_var)
        .aggregate(
            generated.aggregate.count(),
            generated.aggregate.sum(score_field),
        )
    )
    assert len(direct_grouped_aggregate) == 1
    direct_group, direct_group_values = direct_grouped_aggregate[0]
    assert type(direct_group) is generated.Employment
    assert direct_group.iid == employment.iid
    assert direct_group_values == (1, 3)

    network_manager = generated.NetworkLink.manager(clean_db)
    network = generated.NetworkLink(
        identifier=generated.Identifier("network-1"),
        nickname=generated.Nickname("primary"),
        origin=person,
        destination=batch_people[0],
        participant=[person, batch_people[0]],
    )
    assert network_manager.insert(network) is network
    assert network.iid
    network_iid = network.iid
    assert network_manager.put(network) is network
    assert network.iid == network_iid
    network.nickname = generated.Nickname("updated")
    assert network_manager.update(network) is network
    stored_network = network_manager.get_by_iid(network_iid)
    assert type(stored_network) is generated.NetworkLink
    assert stored_network.nickname.value == "updated"
    filtered_networks = network_manager.filter(identifier=generated.Identifier("network-1"))
    assert [candidate.iid for candidate in filtered_networks.all()] == [network_iid]
    assert filtered_networks.first().iid == network_iid
    assert filtered_networks.count() == 1
    assert filtered_networks.exists() is True

    cross_type_entity_owners = generated.Identifier.owners(
        clean_db,
        "person-1",
        kind="entity",
    )
    assert [type(candidate) for candidate in cross_type_entity_owners] == [generated.Person]
    assert [candidate.iid for candidate in cross_type_entity_owners] == [person.iid]
    assert {
        candidate.iid
        for candidate in generated.Identifier.owners(
            clean_db,
            "person-",
            kind="entity",
            lookup="startswith",
        )
    } == {person.iid, *batch_iids, transaction_person.iid}
    party_attribute_owners = generated.Party.has(
        clean_db,
        generated.Identifier,
        lookup="present",
    )
    assert [type(candidate) for candidate in party_attribute_owners] == [
        generated.Employee,
        generated.Manager,
    ]
    assert {candidate.iid for candidate in party_attribute_owners} == {
        employee.iid,
        manager.iid,
    }
    narrowed_person_owners = generated.Person.has(
        clean_db,
        generated.Identifier,
        generated.Identifier("person-1"),
    )
    assert [candidate.iid for candidate in narrowed_person_owners] == [person.iid]
    relation_attribute_owners = generated.Identifier.owners(
        clean_db,
        generated.Identifier("network-1"),
        kind="relation",
    )
    assert [type(candidate) for candidate in relation_attribute_owners] == [generated.NetworkLink]
    assert [candidate.iid for candidate in relation_attribute_owners] == [network_iid]
    assert relation_attribute_owners[0].origin.iid == person.iid
    assert relation_attribute_owners[0].destination.iid == batch_people[0].iid

    network_session = generated.NetworkLink.query(clean_db)
    network_var = network_session.exact(generated.NetworkLink)
    queried_network = (
        network_session.query(network_var)
        .where(
            network_var.iid(network_iid),
            network_var.field(generated.NetworkLink.nickname).is_present(),
        )
        .one()
    )
    assert type(queried_network) is generated.NetworkLink
    assert queried_network.iid == network_iid
    participant_var = network_session.exact(generated.Person)
    participant = network_var.role(generated.NetworkLink.participant).connects(participant_var)
    network_rows = network_session.query(network_var).rows(
        limit=10,
        order_by=(network_var.field(generated.NetworkLink.identifier).asc(),),
    )
    assert [candidate.iid for candidate in network_rows] == [network.iid]

    reachable_source = network_session.exact(generated.Person)
    reachable_target = network_session.exact(generated.Person)
    reachable = network_session.reachable(
        reachable_source,
        reachable_target,
        generated.NetworkLink,
        generated.NetworkLink.origin,
        generated.NetworkLink.destination,
        min_depth=1,
        max_depth=1,
    )
    reachable_pair = (
        network_session.query(reachable_source, reachable_target)
        .where(
            reachable,
            reachable_source.field(generated.Person.identifier).eq(
                generated.Identifier("person-1")
            ),
            reachable_target.field(generated.Person.identifier).eq(
                generated.Identifier("person-2")
            ),
        )
        .one()
    )
    assert tuple(candidate.iid for candidate in reachable_pair) == (
        person.iid,
        batch_people[0].iid,
    )

    network_row = make_dataclass(
        "NetworkRow",
        [
            ("network", generated.NetworkLink),
            ("participants", tuple[generated.Person, ...]),
        ],
        frozen=True,
        slots=True,
    )
    network_page = (
        network_session.query_as(
            network_row,
            network=network_var,
            participants=participant_var.collect()
            .distinct()
            .order_by(participant_var.field(generated.Person.identifier).asc()),
        )
        .where(participant)
        .page_by(
            network_var,
            limit=10,
            order_by=(network_var.field(generated.NetworkLink.identifier).asc(),),
            include_total=True,
        )
    )
    assert network_page.total == 1
    assert len(network_page.items) == 1
    named_network = network_page.items[0]
    assert type(named_network) is network_row
    assert type(named_network.network) is generated.NetworkLink
    assert named_network.network.iid == network.iid
    assert len(named_network.participants) == 2
    assert all(type(candidate) is generated.Person for candidate in named_network.participants)
    assert tuple(candidate.iid for candidate in named_network.participants) == (
        person.iid,
        batch_people[0].iid,
    )

    event_manager = generated.Event.manager(clean_db)
    event = generated.Event(subject=person)
    assert event_manager.insert(event) is event
    assert event.iid
    assert type(event_manager.get_by_iid(event.iid)) is generated.Event

    container_manager = generated.Container.manager(clean_db)
    container = generated.Container(item=[generated.EventRef(event.iid)])
    assert container_manager.insert(container) is container
    assert container.iid

    stored_container = container_manager.get_by_iid(container.iid)
    assert type(stored_container) is generated.Container
    assert stored_container.iid == container.iid
    assert type(stored_container.item) is tuple
    assert len(stored_container.item) == 1
    stored_event = stored_container.item[0]
    assert type(stored_event) is generated.EventRef
    assert not isinstance(stored_event, generated.Event)
    assert stored_event.iid == event.iid

    container_session = generated.Container.query(clean_db)
    container_var = container_session.exact(generated.Container)
    queried_container = container_session.query(container_var).one()
    assert type(queried_container) is generated.Container
    assert queried_container.iid == container.iid
    assert type(queried_container.item) is tuple
    assert len(queried_container.item) == 1
    assert type(queried_container.item[0]) is generated.EventRef
    assert queried_container.item[0].iid == event.iid

    generated_file = generated.__file__
    assert generated_file is not None
    generated_path = Path(generated_file).resolve()
    authority = generated_path.parent.parent.joinpath("schema-authority.json").read_bytes()
    remote_port = _free_port()
    supplied_server = os.environ.get("TYPE_BRIDGE_V2_SMOKE_SERVER")
    if supplied_server is None:
        server_command = [
            "cargo",
            "run",
            "--quiet",
            "-p",
            "type-bridge-server",
            "--features",
            "v2-query",
            "--example",
            "v2_smoke_server",
        ]
        server_cwd = CORE
    else:
        server_path = Path(supplied_server).resolve()
        if not server_path.is_file() or server_path.is_symlink():
            raise AssertionError(f"supplied V2 smoke server is invalid: {server_path}")
        server_command = [str(server_path)]
        server_cwd = ROOT
    server = subprocess.Popen(
        server_command,
        cwd=server_cwd,
        env={
            **os.environ,
            "SMOKE_TYPEDB_ADDRESS": clean_db.address,
            "SMOKE_TYPEDB_USERNAME": clean_db.username or "admin",
            "SMOKE_TYPEDB_PASSWORD": clean_db.password or "password",
            "SMOKE_TYPEDB_HTTP_PORT": str(clean_db.http_port),
            "SMOKE_DATABASE": clean_db.database_name,
            "SMOKE_AUTHORITY_B64": base64.b64encode(authority).decode(),
            "SMOKE_PORT": str(remote_port),
        },
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        _wait_for_port(remote_port, server, timeout=300)
        with urllib_request.urlopen(
            f"http://127.0.0.1:{remote_port}/v2/capabilities",
            timeout=30,
        ) as response:
            advertisement = response.read()

        remote_requests: list[bytes] = []

        async def exchange(request: bytes) -> bytes:
            remote_requests.append(request)

            def post() -> bytes:
                http_request = urllib_request.Request(
                    f"http://127.0.0.1:{remote_port}/v2/query",
                    data=request,
                    headers={"content-type": "application/json"},
                    method="POST",
                )
                with urllib_request.urlopen(http_request, timeout=30) as response:
                    return response.read()

            return await asyncio.to_thread(post)

        remote_session = generated.RemoteQuerySession(
            advertisement,
            exchange,
            generated.RemoteQueryLimits(
                max_items=10,
                max_bytes=1 << 20,
                max_collection_members=100,
                max_graph_nodes=30,
                max_attribute_values=1_000,
                max_role_players=30,
                deadline_ms=30_000,
            ),
        )
        remote_person = remote_session.exact(generated.Person)
        remote_people = remote_session.query(remote_person).where(
            remote_person.field(generated.Person.identifier).eq(generated.Identifier("person-1"))
        )
        remote_person_one = asyncio.run(remote_people.one())
        assert type(remote_person_one) is generated.Person
        assert remote_person_one.iid == person.iid
        remote_person_first = asyncio.run(
            remote_people.first(order_by=(remote_person.field(generated.Person.identifier).asc(),))
        )
        assert type(remote_person_first) is generated.Person
        assert remote_person_first.iid == person.iid
        remote_present_aliases = asyncio.run(
            remote_session.query(remote_person)
            .where(remote_person.field(generated.Person.aliases).is_present())
            .rows(
                limit=10,
                order_by=(remote_person.field(generated.Person.identifier).asc(),),
            )
        )
        assert [candidate.iid for candidate in remote_present_aliases] == [person.iid]
        remote_missing_aliases = asyncio.run(
            remote_session.query(remote_person)
            .where(remote_person.field(generated.Person.aliases).is_missing())
            .rows(
                limit=10,
                order_by=(remote_person.field(generated.Person.identifier).asc(),),
            )
        )
        assert {candidate.iid for candidate in remote_missing_aliases} == {
            *batch_iids,
            transaction_person.iid,
        }
        assert (
            asyncio.run(
                remote_session.query(remote_person).where(remote_person.iid(person.iid)).one()
            ).iid
            == person.iid
        )
        remote_iid_set = asyncio.run(
            remote_session.query(remote_person)
            .where(remote_person.iid_in((person.iid, batch_people[0].iid)))
            .rows(
                limit=10,
                order_by=(remote_person.field(generated.Person.identifier).asc(),),
            )
        )
        assert [candidate.iid for candidate in remote_iid_set] == [
            person.iid,
            batch_people[0].iid,
        ]
        remote_network = remote_session.exact(generated.NetworkLink)
        remote_network_one = asyncio.run(
            remote_session.query(remote_network)
            .where(
                remote_network.iid(network_iid),
                remote_network.field(generated.NetworkLink.nickname).is_present(),
            )
            .one()
        )
        assert type(remote_network_one) is generated.NetworkLink
        assert remote_network_one.iid == network_iid
        remote_person_rows = asyncio.run(
            remote_people.rows(
                limit=10,
                order_by=(remote_person.field(generated.Person.identifier).asc(),),
            )
        )
        assert [candidate.iid for candidate in remote_person_rows] == [person.iid]
        remote_person_page = asyncio.run(
            remote_people.page_by(
                remote_person,
                limit=10,
                order_by=(remote_person.field(generated.Person.identifier).asc(),),
                include_total=True,
            )
        )
        assert [candidate.iid for candidate in remote_person_page.items] == [person.iid]
        assert remote_person_page.total == 1
        assert asyncio.run(remote_people.count_by(remote_person)) == 1
        assert asyncio.run(remote_people.exists_by(remote_person)) is True

        remote_party = remote_session.subtypes(generated.Party)
        remote_party_rows = asyncio.run(
            remote_session.query(remote_party).rows(
                limit=10,
                order_by=(remote_party.field(generated.Party.identifier).asc(),),
            )
        )
        assert [type(candidate) for candidate in remote_party_rows] == [
            generated.Employee,
            generated.Manager,
        ]
        assert [candidate.iid for candidate in remote_party_rows] == [employee.iid, manager.iid]

        remote_all_people = remote_session.query(remote_person)
        remote_score_field = remote_person.field(generated.Person.score)
        requests_before_reduction = len(remote_requests)
        remote_aggregate = asyncio.run(
            remote_all_people.aggregate(
                remote_person,
                generated.aggregate.count(),
                generated.aggregate.sum(remote_score_field),
                generated.aggregate.min(remote_score_field),
                generated.aggregate.max(remote_score_field),
                generated.aggregate.mean(remote_score_field),
                generated.aggregate.median(remote_score_field),
                generated.aggregate.std(remote_score_field),
            )
        )
        assert remote_aggregate[:6] == direct_aggregate[:6]
        assert isinstance(remote_aggregate[6], float)
        assert len(remote_requests) == requests_before_reduction + 1

        remote_field_grouped = asyncio.run(
            remote_all_people.group_by(
                remote_person,
                remote_person.field(generated.Person.val_bool),
            ).aggregate(
                generated.aggregate.count(),
                generated.aggregate.sum(remote_score_field),
            )
        )
        assert [(group.value, values) for group, values in remote_field_grouped] == [
            (group.value, values) for group, values in direct_field_grouped
        ]

        remote_tuple_field_grouped = asyncio.run(
            remote_all_people.group_by(
                remote_person,
                remote_person.field(generated.Person.val_bool),
                remote_score_field,
            ).aggregate(
                generated.aggregate.count(),
                generated.aggregate.sum(remote_score_field),
            )
        )
        assert [
            (bool_group.value, score_group.value, values)
            for (bool_group, score_group), values in remote_tuple_field_grouped
        ] == [
            (bool_group.value, score_group.value, values)
            for (bool_group, score_group), values in direct_tuple_field_grouped
        ]

        remote_employment = remote_session.exact(generated.Employment)
        remote_person_employment = remote_session.query(
            remote_person,
            remote_employment,
        ).where(remote_employment.role(generated.Employment.employee).connects(remote_person))
        remote_person_value, remote_employment_value = asyncio.run(remote_person_employment.one())
        assert type(remote_person_value) is generated.Person
        assert remote_person_value.iid == person.iid
        assert type(remote_employment_value) is generated.Employment
        assert remote_employment_value.iid == employment.iid
        requests_before_grouped_reduction = len(remote_requests)
        remote_grouped_aggregate = asyncio.run(
            remote_person_employment.group_by(remote_person, remote_employment).aggregate(
                generated.aggregate.count(),
                generated.aggregate.sum(remote_score_field),
            )
        )
        assert len(remote_grouped_aggregate) == 1
        remote_group, remote_group_values = remote_grouped_aggregate[0]
        assert type(remote_group) is generated.Employment
        assert remote_group.iid == direct_group.iid
        assert remote_group_values == direct_group_values
        assert len(remote_requests) == requests_before_grouped_reduction + 1

        remote_network = remote_session.exact(generated.NetworkLink)
        remote_participant = remote_session.exact(generated.Person)
        remote_network_page = asyncio.run(
            remote_session.query_as(
                network_row,
                network=remote_network,
                participants=remote_participant.collect()
                .distinct()
                .order_by(remote_participant.field(generated.Person.identifier).asc()),
            )
            .where(
                remote_network.role(generated.NetworkLink.participant).connects(remote_participant)
            )
            .page_by(
                remote_network,
                limit=10,
                order_by=(remote_network.field(generated.NetworkLink.identifier).asc(),),
                include_total=True,
            )
        )
        assert remote_network_page.total == network_page.total
        assert len(remote_network_page.items) == len(network_page.items)
        remote_named_network = remote_network_page.items[0]
        assert type(remote_named_network) is network_row
        assert type(remote_named_network.network) is generated.NetworkLink
        assert remote_named_network.network.iid == named_network.network.iid
        assert tuple(value.iid for value in remote_named_network.participants) == tuple(
            value.iid for value in named_network.participants
        )
        assert all(type(value) is generated.Person for value in remote_named_network.participants)

        remote_source = remote_session.exact(generated.Person)
        remote_target = remote_session.exact(generated.Person)
        remote_reachable = remote_session.reachable(
            remote_source,
            remote_target,
            generated.NetworkLink,
            generated.NetworkLink.origin,
            generated.NetworkLink.destination,
            min_depth=1,
            max_depth=1,
        )
        remote_reachable_pair = asyncio.run(
            remote_session.query(remote_source, remote_target)
            .where(
                remote_reachable,
                remote_source.field(generated.Person.identifier).eq(
                    generated.Identifier("person-1")
                ),
                remote_target.field(generated.Person.identifier).eq(
                    generated.Identifier("person-2")
                ),
            )
            .one()
        )
        assert tuple(candidate.iid for candidate in remote_reachable_pair) == tuple(
            candidate.iid for candidate in reachable_pair
        )
        remote_cross_left = remote_session.exact(generated.Person)
        remote_cross_right = remote_session.exact(generated.Person)
        remote_cross_pair = asyncio.run(
            remote_session.query(remote_cross_left, remote_cross_right)
            .allow_cross_join(remote_cross_left, remote_cross_right)
            .where(
                remote_cross_left.field(generated.Person.identifier).eq(
                    generated.Identifier("person-1")
                ),
                remote_cross_right.field(generated.Person.identifier).eq(
                    generated.Identifier("person-2")
                ),
            )
            .one()
        )
        assert tuple(candidate.iid for candidate in remote_cross_pair) == tuple(
            candidate.iid for candidate in cross_pair
        )

        if workforce_catalog is not None and workforce_journey is not None:
            workforce_results = _run_workforce_journey(
                generated,
                clean_db,
                person_manager,
                membership_manager,
                remote_session,
                remote_requests,
                workforce_catalog,
                workforce_journey,
            )
        if workforce_v2_catalog is not None and workforce_v2_journey is not None:
            assert workforce_v2_proof_observations is not None
            workforce_v2_results = _run_workforce_v2_journey(
                generated,
                clean_db,
                remote_session,
                remote_requests,
                advertisement,
                exchange,
                workforce_v2_catalog,
                workforce_v2_journey,
                workforce_v2_proof_observations,
            )
    finally:
        server.terminate()
        try:
            server.wait(timeout=30)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait(timeout=30)

    relation_batch = [
        generated.NetworkLink(
            identifier=generated.Identifier("network-2"),
            origin=batch_people[0],
            destination=batch_people[1],
            participant=[batch_people[0], batch_people[1]],
        ),
        generated.NetworkLink(
            identifier=generated.Identifier("network-3"),
            origin=batch_people[1],
            destination=person,
            participant=[batch_people[1], person],
        ),
    ]
    assert network_manager.insert_many(relation_batch) == relation_batch
    relation_batch_iids = [candidate.iid for candidate in relation_batch]
    assert all(relation_batch_iids)
    assert [candidate.iid for candidate in network_manager.put_many(relation_batch)] == (
        relation_batch_iids
    )
    for relation in relation_batch:
        network_manager.delete(relation)
        assert network_manager.get_by_iid(relation.iid) is None

    membership_manager.delete(membership)
    assert membership_manager.get_by_iid(membership.iid) is None
    network_manager.delete(network)
    assert network_manager.get_by_iid(network.iid) is None
    person_manager.delete(transaction_person)
    assert person_manager.get_by_iid(transaction_person.iid) is None

    if workforce_report_path is not None:
        assert catalog_raw is not None
        assert workforce_catalog is not None
        assert journey_raw is not None
        assert workforce_journey is not None
        assert workforce_results is not None
        _publish_workforce_report(
            workforce_report_path,
            _workforce_report(
                generated,
                catalog_raw,
                workforce_catalog,
                journey_raw,
                workforce_results,
            ),
        )
    if workforce_v2_report_path is not None:
        assert workforce_v2_catalog_raw is not None
        assert workforce_v2_catalog is not None
        assert workforce_v2_journey_raw is not None
        assert workforce_v2_results is not None
        _publish_workforce_report(
            workforce_v2_report_path,
            _workforce_v2_report(
                generated,
                workforce_v2_catalog_raw,
                workforce_v2_catalog,
                workforce_v2_journey_raw,
                workforce_v2_results,
            ),
        )
