#!/usr/bin/env python3
"""Produce Python's provider-free Projected projected parity report."""

from __future__ import annotations

import asyncio
import hashlib
import inspect
import json
import os
import stat
import struct
import sys
from collections.abc import Mapping, Sequence
from datetime import UTC, date, datetime, timedelta
from decimal import Decimal
from pathlib import Path
from typing import Any

REPORT_FORMAT = "typebridge.projected-parity-report/v1"
SEMANTIC_PROFILE = "typedb-3.12.1/v1"
OUTPUT_ENV = "TYPE_BRIDGE_PROJECTED_PARITY_REPORT"
PACKAGE_ROOT_ENV = "TYPE_BRIDGE_PROJECTED_PYTHON_PACKAGE_ROOT"
REPOSITORY_ROOT_ENV = "TYPE_BRIDGE_PROJECTED_REPOSITORY_ROOT"
MAX_REPORT_BYTES = 256 * 1024
MAX_AUTHORITY_BYTES = 1024 * 1024
SCHEMA_RELATIVE = "tests/contracts/sdk_conformance/sdk-v3/schema-v3.yaml"
JOURNEY_RELATIVE = "tests/contracts/sdk_conformance/sdk-v3/journey-v3.json"


class ProducerError(RuntimeError):
    """A stable fail-closed producer rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code


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


def _source_identity(root: Path, relative: str, label: str) -> dict[str, str]:
    raw = _bounded_regular_bytes(root / relative, label)
    return {"path": relative, "sha256": hashlib.sha256(raw).hexdigest()}


def _output_path(environment: Mapping[str, str] = os.environ) -> Path:
    value = environment.get(OUTPUT_ENV)
    if value is None or not value:
        raise ProducerError("missing_output_path", f"{OUTPUT_ENV} must be set")
    path = Path(value)
    if not path.is_absolute():
        raise ProducerError("invalid_output_path", "report path must be absolute")
    if path.name in {"", ".", ".."}:
        raise ProducerError("invalid_output_path", "report path must name a file")
    return path


def _publish_report(path: Path, report: object) -> None:
    payload = _canonical(report)
    if len(payload) > MAX_REPORT_BYTES:
        raise ProducerError("report_size_limit", "canonical report is too large")
    try:
        parent = path.parent.lstat()
    except OSError as error:
        raise ProducerError("invalid_output_parent", "report parent cannot be inspected") from error
    if stat.S_ISLNK(parent.st_mode) or not stat.S_ISDIR(parent.st_mode):
        raise ProducerError("invalid_output_parent", "report parent must be a real directory")
    try:
        with path.open("xb") as destination:
            destination.write(payload)
            destination.flush()
            os.fsync(destination.fileno())
    except FileExistsError as error:
        raise ProducerError("output_exists", "report destination already exists") from error
    except OSError as error:
        raise ProducerError("output_write_failed", "report could not be written") from error


_ED_FIELD = 2**255 - 19
_ED_ORDER = 2**252 + 27742317777372353535851937790883648493
_ED_D = (-121665 * pow(121666, _ED_FIELD - 2, _ED_FIELD)) % _ED_FIELD
_ED_I = pow(2, (_ED_FIELD - 1) // 4, _ED_FIELD)
_SIGNING_SEED = b"\x42" * 32


def _ed_xrecover(y: int) -> int:
    xx = ((y * y - 1) * pow(_ED_D * y * y + 1, _ED_FIELD - 2, _ED_FIELD)) % _ED_FIELD
    x = pow(xx, (_ED_FIELD + 3) // 8, _ED_FIELD)
    if (x * x - xx) % _ED_FIELD:
        x = (x * _ED_I) % _ED_FIELD
    return _ED_FIELD - x if x & 1 else x


_ED_BASE_Y = (4 * pow(5, _ED_FIELD - 2, _ED_FIELD)) % _ED_FIELD
_ED_BASE_X = _ed_xrecover(_ED_BASE_Y)
_ED_BASE = (_ED_BASE_X, _ED_BASE_Y, 1, (_ED_BASE_X * _ED_BASE_Y) % _ED_FIELD)
_ED_IDENTITY = (0, 1, 1, 0)


def _ed_add(
    left: tuple[int, int, int, int], right: tuple[int, int, int, int]
) -> tuple[int, int, int, int]:
    x1, y1, z1, t1 = left
    x2, y2, z2, t2 = right
    a = ((y1 - x1) * (y2 - x2)) % _ED_FIELD
    b = ((y1 + x1) * (y2 + x2)) % _ED_FIELD
    c = (2 * _ED_D * t1 * t2) % _ED_FIELD
    d = (2 * z1 * z2) % _ED_FIELD
    e = b - a
    f = d - c
    g = d + c
    h = b + a
    return (
        (e * f) % _ED_FIELD,
        (g * h) % _ED_FIELD,
        (f * g) % _ED_FIELD,
        (e * h) % _ED_FIELD,
    )


def _ed_scale(point: tuple[int, int, int, int], scalar: int) -> tuple[int, int, int, int]:
    result = _ED_IDENTITY
    addend = point
    while scalar:
        if scalar & 1:
            result = _ed_add(result, addend)
        addend = _ed_add(addend, addend)
        scalar >>= 1
    return result


def _ed_encode(point: tuple[int, int, int, int]) -> bytes:
    x, y, z, _ = point
    inverse = pow(z, _ED_FIELD - 2, _ED_FIELD)
    x = (x * inverse) % _ED_FIELD
    y = (y * inverse) % _ED_FIELD
    return (y | ((x & 1) << 255)).to_bytes(32, "little")


_SIGNING_HASH = hashlib.sha512(_SIGNING_SEED).digest()
_SIGNING_SCALAR_BYTES = bytearray(_SIGNING_HASH[:32])
_SIGNING_SCALAR_BYTES[0] &= 248
_SIGNING_SCALAR_BYTES[31] &= 63
_SIGNING_SCALAR_BYTES[31] |= 64
_SIGNING_SCALAR = int.from_bytes(_SIGNING_SCALAR_BYTES, "little")
_SIGNING_PUBLIC_KEY = _ed_encode(_ed_scale(_ED_BASE, _SIGNING_SCALAR))
_SIGNING_KEY_ID = hashlib.sha256(
    b"typebridge.query.remote-reply-key-id/v1\0" + _SIGNING_PUBLIC_KEY
).hexdigest()


def _ed_sign(message: bytes) -> bytes:
    nonce = (
        int.from_bytes(hashlib.sha512(_SIGNING_HASH[32:] + message).digest(), "little") % _ED_ORDER
    )
    encoded_nonce = _ed_encode(_ed_scale(_ED_BASE, nonce))
    challenge = (
        int.from_bytes(
            hashlib.sha512(encoded_nonce + _SIGNING_PUBLIC_KEY + message).digest(), "little"
        )
        % _ED_ORDER
    )
    scalar = (nonce + challenge * _SIGNING_SCALAR) % _ED_ORDER
    return encoded_nonce + scalar.to_bytes(32, "little")


def _fingerprint(domain: bytes, canonicalization: bytes, payload: bytes) -> str:
    digest = hashlib.sha256()
    digest.update(b"typebridge.fingerprint/v1\0")
    for field in (domain, canonicalization):
        digest.update(struct.pack(">Q", len(field)))
        digest.update(field)
    digest.update(b"\0")
    digest.update(struct.pack(">Q", len(payload)))
    digest.update(payload)
    return digest.hexdigest()


_REMOTE_CAPABILITIES = [
    "query.execution.batch-identity-rebind",
    "query.execution.same-snapshot-hydration",
    "query.input.given-rows",
    "query.operation.distinct-count",
    "query.operation.distinct-exists",
    "query.operation.exactly-one",
    "query.operation.page",
    "query.order.stable-collection",
    "query.order.stable-root",
    "query.order.stable-selected",
    "query.output.collect",
    "query.output.collect-distinct",
    "query.output.hydrated",
    "query.output.named",
    "query.output.rows",
    "query.pattern.has",
    "query.pattern.iid",
    "query.pattern.isa",
    "query.pattern.isa-subtypes",
    "query.plan",
    "query.plan.v2",
    "query.remote.envelope-v2",
    "query.remote.structured-diagnostic",
    "query.stage.distinct",
    "query.stage.limit",
    "query.stage.offset",
    "query.stage.require",
    "query.stage.select",
    "query.stage.sort",
]


def _remote_advertisement() -> bytes:
    return _canonical(
        {
            "capabilities": _REMOTE_CAPABILITIES,
            "executor": {
                "epoch": "python-projected-epoch-0001",
                "identity": "python-projected-executor",
            },
            "format": "typebridge.query-remote-capabilities/v1",
            "reply_key": _SIGNING_PUBLIC_KEY.hex(),
            "reply_key_id": _SIGNING_KEY_ID,
        }
    ).rstrip(b"\n")


def _remote_signed_reply(payload: dict[str, object], advertisement: bytes) -> bytes:
    payload_bytes = _canonical(payload).rstrip(b"\n")
    advertisement_fingerprint = _fingerprint(
        b"typebridge.query.remote-capabilities",
        b"typebridge.query-remote-capabilities/v1",
        advertisement,
    )
    prefix = (
        f'{{"advertisement":"{advertisement_fingerprint}",'
        '"format":"typebridge.query-remote-signed-reply/v1",'
        f'"key":"{_SIGNING_PUBLIC_KEY.hex()}","key_id":"{_SIGNING_KEY_ID}","payload":'
    ).encode()
    digest = hashlib.sha256(
        b"typebridge.query.remote-reply-signature/v1\0" + prefix + payload_bytes + b"}"
    ).digest()
    return prefix + payload_bytes + b',"signature":"' + _ed_sign(digest).hex().encode() + b'"}'


def _type_id(kind: str, label: str) -> dict[str, str]:
    return {"kind": kind, "label": label}


def _value(kind: str, value: object) -> dict[str, object]:
    return {"kind": kind, "value": value}


_ADA_SCALARS = {
    "boolean": False,
    "date": date(2026, 8, 12),
    "datetime": datetime(2026, 8, 12, 9, 30),
    "datetime_tz": datetime(2026, 8, 12, 9, 30, tzinfo=UTC),
    "decimal": Decimal("38.5"),
    "double": 38.0,
    "duration": timedelta(seconds=38),
    "long": 38,
    "string": "Ada",
}


def _wire_scalar(kind: str, value: object) -> dict[str, object]:
    if kind == "double" and type(value) is float:
        return {"bits": struct.pack(">d", value).hex(), "kind": kind}
    if kind == "long" and type(value) is int:
        return _value(kind, str(value))
    if kind == "boolean" and type(value) is bool:
        return _value(kind, value)
    if kind == "date" and type(value) is date:
        return _value(kind, value.isoformat())
    if kind == "datetime" and type(value) is datetime and value.tzinfo is None:
        return _value(kind, value.isoformat())
    if kind == "datetime_tz" and type(value) is datetime and value.tzinfo is not None:
        offset = value.utcoffset()
        if offset != timedelta(0):
            raise AssertionError("Projected fixture requires the authored UTC datetime")
        return _value(
            kind,
            {
                "effective_offset_seconds": 0,
                "local": value.replace(tzinfo=None).isoformat(),
                "zone": {"kind": "utc"},
            },
        )
    if kind == "decimal" and type(value) is Decimal:
        return _value(kind, str(value))
    if kind == "duration" and type(value) is timedelta:
        if value.microseconds != 0 or value.days != 0 or value.seconds != 38:
            raise AssertionError("Projected fixture requires its authored exact duration")
        return _value(kind, "PT38S")
    if kind == "string" and type(value) is str:
        return _value(kind, value)
    raise AssertionError(f"unexpected {kind} scalar {value!r}")


def _report_scalar(kind: str, value: object) -> dict[str, object]:
    wire = _wire_scalar(kind, value)
    if kind == "datetime_tz":
        assert type(value) is datetime
        return _value(kind, value.isoformat().replace("+00:00", "Z"))
    return wire


def _person_node(
    node_id: int,
    iid: str,
    *,
    identifier: str,
    nickname: str,
    aliases: Sequence[str],
    score: int,
    foo_bar: int | None = None,
    score_gte: int | None = None,
) -> dict[str, object]:
    scalars = dict(_ADA_SCALARS)
    scalars["long"] = score
    scalars["string"] = nickname
    return {
        "attributes": [
            {
                "attribute": "aliases",
                "values": [_wire_scalar("string", alias) for alias in aliases],
            },
            {
                "attribute": "foo__bar",
                "values": [] if foo_bar is None else [_wire_scalar("long", foo_bar)],
            },
            {"attribute": "identifier", "values": [_wire_scalar("string", identifier)]},
            {"attribute": "nickname", "values": [_wire_scalar("string", nickname)]},
            {"attribute": "score", "values": [_wire_scalar("long", score)]},
            {
                "attribute": "score__gte",
                "values": [] if score_gte is None else [_wire_scalar("long", score_gte)],
            },
            {"attribute": "val_bool", "values": [_wire_scalar("boolean", scalars["boolean"])]},
            {
                "attribute": "val_constrained",
                "values": [_wire_scalar("long", score)],
            },
            {"attribute": "val_date", "values": [_wire_scalar("date", scalars["date"])]},
            {
                "attribute": "val_datetime",
                "values": [_wire_scalar("datetime", scalars["datetime"])],
            },
            {
                "attribute": "val_datetime_tz",
                "values": [_wire_scalar("datetime_tz", scalars["datetime_tz"])],
            },
            {
                "attribute": "val_decimal",
                "values": [_wire_scalar("decimal", scalars["decimal"])],
            },
            {
                "attribute": "val_double",
                "values": [_wire_scalar("double", scalars["double"])],
            },
            {
                "attribute": "val_duration",
                "values": [_wire_scalar("duration", scalars["duration"])],
            },
        ],
        "concrete": _type_id("entity", "person"),
        "id": node_id,
        "iid": iid,
        "kind": "entity",
        "roles": [],
    }


def _robot_node(node_id: int, iid: str, key: int) -> dict[str, object]:
    return {
        "attributes": [
            {"attribute": "nickname", "values": []},
            {"attribute": "robot_id", "values": [_wire_scalar("long", key)]},
            {"attribute": "val_constrained", "values": [_wire_scalar("long", 38)]},
        ],
        "concrete": _type_id("entity", "robot"),
        "id": node_id,
        "iid": iid,
        "kind": "entity",
        "roles": [],
    }


def _reference(kind: str, label: str, node: int) -> dict[str, object]:
    return {"declared": _type_id(kind, label), "node": node}


def _role(
    declaring_relation: str,
    label: str,
    players: Sequence[dict[str, object]],
) -> dict[str, object]:
    return {
        "players": list(players),
        "role": {"declaring_relation": declaring_relation, "label": label},
    }


def _relation_node(
    node_id: int,
    iid: str,
    label: str,
    *,
    attributes: Sequence[dict[str, object]] = (),
    roles: Sequence[dict[str, object]] = (),
) -> dict[str, object]:
    return {
        "attributes": list(attributes),
        "concrete": _type_id("relation", label),
        "id": node_id,
        "iid": iid,
        "kind": "relation",
        "roles": list(roles),
    }


def _make_person(
    package: Any,
    identifier: str = "data-ada",
    *,
    nickname: str = "Ada",
    aliases: Sequence[str] = ("analyst", "mathematician"),
    score: int = 38,
    **extra: object,
) -> object:
    values: dict[str, object] = {
        "aliases": tuple(package.Aliases(alias) for alias in aliases),
        "foo__bar": package.FooBar(1),
        "identifier": package.Identifier(identifier),
        "nickname": package.Nickname(nickname),
        "score": package.Score(score),
        "score__gte": package.ScoreGte(2),
        "val_bool": package.ValBool(_ADA_SCALARS["boolean"]),
        "val_constrained": package.ValConstrained(score),
        "val_date": package.ValDate(_ADA_SCALARS["date"]),
        "val_datetime": package.ValDatetime(_ADA_SCALARS["datetime"]),
        "val_datetime_tz": package.ValDatetimeTz(_ADA_SCALARS["datetime_tz"]),
        "val_decimal": package.ValDecimal(_ADA_SCALARS["decimal"]),
        "val_double": package.ValDouble(_ADA_SCALARS["double"]),
        "val_duration": package.ValDuration(_ADA_SCALARS["duration"]),
    }
    values.update(extra)
    return package.Person(**values)


def _raw_model(model: type[object], values: Mapping[str, object]) -> object:
    instance = model.__new__(model)
    instance.initialize_runtime_values(values)
    return instance


def _expect_rejection(
    action: Any,
    *,
    exception_names: Sequence[str] = (),
    category: str | None = None,
    code: str | None = None,
) -> Exception:
    try:
        action()
    except Exception as error:
        if exception_names and type(error).__name__ not in exception_names:
            raise AssertionError(
                f"expected {exception_names}, received {type(error).__name__}"
            ) from error
        if category is not None and getattr(error, "sdk_category", None) != category:
            raise AssertionError(
                f"expected rejection category {category}, received "
                f"{getattr(error, 'sdk_category', None)}"
            ) from error
        if code is not None and getattr(error, "code", None) != code:
            raise AssertionError(
                f"expected rejection code {code}, received {getattr(error, 'code', None)}"
            ) from error
        return error
    raise AssertionError("provider-free projection probe unexpectedly succeeded")


def _model_label(value: object) -> str:
    type_id = json.loads(type(value).__type_id__)
    if not isinstance(type_id, dict) or not isinstance(type_id.get("label"), str):
        raise AssertionError("generated value has no canonical model label")
    return type_id["label"]


def _attribute_values(person: object) -> dict[str, dict[str, object]]:
    fields = {
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
    observations: dict[str, dict[str, object]] = {}
    for domain, field in fields.items():
        attribute = getattr(person, field)
        observations[domain] = _report_scalar(domain, attribute.value)
    return observations


def _has_annotation(token: object, kind: str) -> bool:
    fact = getattr(token, "fact", None)
    if not isinstance(fact, Mapping):
        return False
    annotations = fact.get("annotations")
    if not isinstance(annotations, Sequence):
        return False
    for annotation in annotations:
        if not isinstance(annotation, Mapping):
            continue
        identity = annotation.get("id")
        if not isinstance(identity, Mapping):
            continue
        annotation_kind = identity.get("kind")
        if isinstance(annotation_kind, Mapping) and annotation_kind.get("kind") == kind:
            return True
    return False


def _token_observation(token: object, binding_name: str) -> dict[str, str]:
    fact = getattr(token, "fact", None)
    if not isinstance(fact, Mapping):
        raise AssertionError("generated field token has no frozen fact")
    identity = fact.get("id")
    if not isinstance(identity, Mapping):
        raise AssertionError("generated field token has no owns identity")
    owner = identity.get("owner")
    attribute = identity.get("attribute")
    if (
        not isinstance(owner, Mapping)
        or not isinstance(owner.get("kind"), str)
        or not isinstance(owner.get("label"), str)
        or not isinstance(attribute, str)
    ):
        raise AssertionError("generated owns identity is malformed")
    return {
        "binding_name": binding_name,
        "canonical_attribute": f"attribute:{attribute}",
        "canonical_owner": f"{owner['kind']}:{owner['label']}",
        "owns_fact": f"{owner['label']}:{attribute}",
    }


def _hydrated_rows_reply(
    request: bytes,
    advertisement: bytes,
    graph: dict[str, object],
    declared: dict[str, str],
    node: int,
) -> bytes:
    decoded = json.loads(request)
    if not isinstance(decoded, dict) or not isinstance(decoded.get("plan"), dict):
        raise AssertionError("generated remote request is malformed")
    plan = _canonical(decoded["plan"]).rstrip(b"\n")
    return _remote_signed_reply(
        {
            "format": "typebridge.query-remote-response/v2",
            "nonce": decoded["nonce"],
            "outcome": {
                "graph": graph,
                "kind": "hydrated_rows",
                "rows": [
                    {
                        "slots": [
                            {
                                "kind": "singular",
                                "value": {"declared": declared, "node": node},
                            }
                        ]
                    }
                ],
            },
            "plan": _fingerprint(b"typebridge.query.plan", b"typebridge.query-plan-c14n/v2", plan),
            "request": _fingerprint(
                b"typebridge.query.remote-request",
                b"typebridge.query-remote-request/v2",
                request,
            ),
        },
        advertisement,
    )


async def _hydrate_exact(
    package: Any,
    model: type[object],
    graph: dict[str, object],
    root_node: int,
    *,
    expose_validated_handle: bool = False,
) -> object:
    advertisement = _remote_advertisement()
    exchanges = 0
    declared = json.loads(model.__type_id__)
    if not isinstance(declared, dict):
        raise AssertionError("generated model identity is malformed")

    async def exchange(request: bytes) -> bytes:
        nonlocal exchanges
        exchanges += 1
        return _hydrated_rows_reply(
            request,
            advertisement,
            graph,
            declared,
            root_node,
        )

    session = package.RemoteQuerySession(
        advertisement,
        exchange,
        package.RemoteQueryLimits(
            max_items=8,
            max_bytes=1 << 20,
            max_collection_members=16,
            max_graph_nodes=16,
            max_attribute_values=128,
            max_role_players=64,
        ),
    )
    original_materializer = package.Query.materialize_row
    if expose_validated_handle:
        package.Query.materialize_row = lambda _query, row: row.slot(0).thing(0)
    try:
        result = await session.query(session.exact(model)).one()
    finally:
        package.Query.materialize_row = original_materializer
        session.close()
    if exchanges != 1:
        raise AssertionError("generated hydration did not use exactly one caller transport")
    return result


def _ada_node(node_id: int = 0, iid: str = "0x01") -> dict[str, object]:
    return _person_node(
        node_id,
        iid,
        identifier="data-ada",
        nickname="Ada",
        aliases=("analyst", "mathematician"),
        score=38,
        foo_bar=1,
        score_gte=2,
    )


def _dana_node(node_id: int = 1, iid: str = "0x02") -> dict[str, object]:
    return _person_node(
        node_id,
        iid,
        identifier="data-dana",
        nickname="Dana",
        aliases=(),
        score=41,
    )


async def _hydrated_observations(package: Any) -> dict[str, object]:
    ada = await _hydrate_exact(package, package.Person, {"nodes": [_ada_node()]}, 0)
    negative_robot = await _hydrate_exact(
        package,
        package.Robot,
        {"nodes": [_robot_node(0, "0x01", -7)]},
        0,
    )
    positive_robot = await _hydrate_exact(
        package,
        package.Robot,
        {"nodes": [_robot_node(0, "0x01", 7)]},
        0,
    )
    plain = await _hydrate_exact(
        package,
        package.PlainActivity,
        {
            "nodes": [
                _ada_node(),
                _relation_node(
                    1,
                    "0x10",
                    "plain-activity",
                    roles=[
                        _role(
                            "plain-activity",
                            "participant",
                            [_reference("entity", "person", 0)],
                        )
                    ],
                ),
            ]
        },
        1,
    )
    link = await _hydrate_exact(
        package,
        package.NetworkLink,
        {
            "nodes": [
                _ada_node(),
                _dana_node(),
                _relation_node(
                    2,
                    "0x10",
                    "network-link",
                    attributes=[
                        {
                            "attribute": "identifier",
                            "values": [_wire_scalar("string", "network-link-ordered")],
                        },
                        {"attribute": "nickname", "values": []},
                    ],
                    roles=[
                        _role(
                            "network-link",
                            "destination",
                            [_reference("entity", "person", 1)],
                        ),
                        _role(
                            "network-link",
                            "origin",
                            [_reference("entity", "person", 0)],
                        ),
                        _role(
                            "network-link",
                            "participant",
                            [
                                _reference("entity", "person", 0),
                                _reference("entity", "person", 1),
                            ],
                        ),
                    ],
                ),
            ]
        },
        2,
    )

    async def interaction(
        identifier: str,
        actor: dict[str, object] | None,
    ) -> object:
        nodes = [_ada_node()]
        actor_reference: list[dict[str, object]] = []
        if actor is not None:
            actor_node = dict(actor)
            actor_type = actor_node["concrete"]
            assert isinstance(actor_type, dict)
            actor_index = 0
            if actor_type["label"] != "person":
                actor_index = 1
                actor_node["id"] = actor_index
                actor_node["iid"] = "0x02"
                nodes.append(actor_node)
            actor_reference = [
                _reference(
                    str(actor_type["kind"]),
                    str(actor_type["label"]),
                    actor_index,
                )
            ]
        relation_id = len(nodes)
        nodes.append(
            _relation_node(
                relation_id,
                "0x10",
                "interaction",
                attributes=[
                    {
                        "attribute": "identifier",
                        "values": [_wire_scalar("string", identifier)],
                    },
                    {"attribute": "nickname", "values": []},
                ],
                roles=[
                    _role("interaction", "actor", actor_reference),
                    _role(
                        "interaction",
                        "target",
                        [_reference("entity", "person", 0)],
                    ),
                ],
            )
        )
        return await _hydrate_exact(
            package,
            package.Interaction,
            {"nodes": nodes},
            relation_id,
        )

    interaction_person = await interaction("interaction-person", _ada_node())
    interaction_robot = await interaction("interaction-robot", _robot_node(0, "0x00", 7))
    interaction_absent = await interaction("interaction-absent", None)
    container = await _hydrate_exact(
        package,
        package.Container,
        {
            "nodes": [
                _relation_node(0, "0x01", "event"),
                _relation_node(
                    1,
                    "0x10",
                    "container",
                    roles=[
                        _role(
                            "container",
                            "item",
                            [_reference("relation", "event", 0)],
                        )
                    ],
                ),
            ]
        },
        1,
    )
    return {
        "ada": ada,
        "container": container,
        "interaction_absent": interaction_absent,
        "interaction_person": interaction_person,
        "interaction_robot": interaction_robot,
        "link": link,
        "negative_robot": negative_robot,
        "plain": plain,
        "positive_robot": positive_robot,
    }


def _constructed_observations(package: Any) -> dict[str, object]:
    ada = _make_person(package)
    dana = _make_person(
        package,
        "data-dana",
        nickname="Dana",
        aliases=(),
        score=41,
        foo__bar=None,
        score__gte=None,
    )
    negative_robot = package.Robot(
        robot_id=package.RobotId(-7),
        val_constrained=package.ValConstrained(38),
    )
    positive_robot = package.Robot(
        robot_id=package.RobotId(7),
        val_constrained=package.ValConstrained(38),
    )
    plain = package.PlainActivity(participant=ada)
    link = package.NetworkLink(
        identifier=package.Identifier("network-link-ordered"),
        destination=dana,
        origin=ada,
        participant=(ada, dana),
    )
    interaction_person = package.Interaction(
        identifier=package.Identifier("interaction-person"),
        actor=ada,
        target=ada,
    )
    interaction_robot = package.Interaction(
        identifier=package.Identifier("interaction-robot"),
        actor=positive_robot,
        target=ada,
    )
    interaction_absent = package.Interaction(
        identifier=package.Identifier("interaction-absent"),
        target=ada,
    )
    event = package.Event(subject=ada)
    event.attach_runtime_iid("0x20")
    container = package.Container(item=(event,))
    return {
        "ada": ada,
        "container": container,
        "dana": dana,
        "interaction_absent": interaction_absent,
        "interaction_person": interaction_person,
        "interaction_robot": interaction_robot,
        "link": link,
        "negative_robot": negative_robot,
        "plain": plain,
        "positive_robot": positive_robot,
    }


def _constraint_observation(
    package: Any,
    constructed: Mapping[str, object],
) -> tuple[dict[str, object], Exception, Exception]:
    provider_calls = 0
    families: set[str] = set()

    def rejected(
        family: str,
        action: Any,
        *,
        exception_names: Sequence[str] = (),
        category: str | None = None,
        code: str | None = None,
    ) -> Exception:
        before = provider_calls
        error = _expect_rejection(
            action,
            exception_names=exception_names,
            category=category,
            code=code,
        )
        if provider_calls != before:
            raise AssertionError(f"{family} rejection crossed the provider boundary")
        families.add(family)
        return error

    actor = _raw_model(package.Actor, {})
    rejected(
        "abstract_constructibility",
        lambda: package.Actor.__runtime_projection__.validate_create(package.Actor, actor),
        category="invalid_input",
        code="model_not_constructible",
    )
    rejected(
        "allowed_values",
        lambda: package.Nickname("Grace"),
        category="invalid_input",
        code="values_constraint_violation",
    )
    if "party_name" in inspect.signature(package.Person).parameters:
        raise AssertionError("person exposed a non-constructible ownership")
    rejected(
        "field_constructibility",
        lambda: _make_person(package, party_name=package.PartyName("Ada")),
        exception_names=("TypeError",),
    )

    nickname_fact = package.Person.nickname.fact
    if nickname_fact["declaring_id"]["owner"] != _type_id("entity", "actor"):
        raise AssertionError("person did not retain inherited nickname ownership")
    rejected(
        "inherited_owns",
        lambda: _make_person(package, nickname=package.Score(38)),
        exception_names=("TypeError",),
    )

    playing_identity = json.dumps(
        {
            "player": _type_id("entity", "person"),
            "role": {"declaring_relation": "base-activity", "label": "participant"},
        },
        separators=(",", ":"),
        sort_keys=True,
    )
    if playing_identity not in package.PLAYING_FACTS:
        raise AssertionError("person did not retain its inherited playing fact")
    rejected(
        "inherited_plays",
        lambda: package.PlainActivity(participant=constructed["positive_robot"]),
        exception_names=("TypeError",),
    )

    role = package.PlainActivity.participant.fact["role"]
    if role != {"declaring_relation": "base-activity", "label": "participant"}:
        raise AssertionError("plain activity lost its inherited role identity")
    raw_plain = _raw_model(package.PlainActivity, {})
    inherited_relates = rejected(
        "inherited_relates",
        lambda: package.PlainActivity.__runtime_projection__.validate_create(
            package.PlainActivity, raw_plain
        ),
        category="invalid_input",
        code="missing_required_role",
    )
    inherited_path = getattr(inherited_relates, "path", None)
    if not isinstance(inherited_path, list) or inherited_path[-1].get("value") != role:
        raise AssertionError("inherited role rejection lost its declaring identity")

    rejected(
        "invalid_player_type",
        lambda: package.Event(subject=constructed["positive_robot"]),
        exception_names=("TypeError",),
    )

    if package.Person.identifier.fact["key"] is not True:
        raise AssertionError("person identifier lost its key fact")
    person_without_key = dict(constructed["ada"].runtime_values())
    person_without_key.pop("identifier")
    raw_person_without_key = _raw_model(package.Person, person_without_key)
    rejected(
        "key",
        lambda: package.Person.__runtime_projection__.validate_create(
            package.Person, raw_person_without_key
        ),
        category="invalid_input",
        code="missing_required_field",
    )

    rejected(
        "maximum_cardinality",
        lambda: _make_person(package, aliases=("one", "two", "three", "four")),
        exception_names=("ValueError",),
    )
    player_duplicate = rejected(
        "ordered_distinct_player",
        lambda: package.NetworkLink(
            identifier=package.Identifier("network-link-duplicate"),
            destination=constructed["dana"],
            origin=constructed["ada"],
            participant=(constructed["ada"], constructed["ada"]),
        ),
        category="invalid_input",
        code="ordered_distinct_duplicate",
    )
    scalar_duplicate = rejected(
        "ordered_distinct_scalar",
        lambda: _make_person(package, aliases=("analyst", "analyst")),
        category="invalid_input",
        code="ordered_distinct_duplicate",
    )

    person_without_score = dict(constructed["ada"].runtime_values())
    person_without_score.pop("score")
    raw_person_without_score = _raw_model(package.Person, person_without_score)
    rejected(
        "ownership_cardinality",
        lambda: package.Person.__runtime_projection__.validate_create(
            package.Person, raw_person_without_score
        ),
        category="invalid_input",
        code="missing_required_field",
    )
    range_error = rejected(
        "range",
        lambda: package.ValConstrained(81),
        category="invalid_input",
        code="range_constraint_violation",
    )
    rejected(
        "regex",
        lambda: package.Nickname("ada"),
        category="invalid_input",
        code="regex_constraint_violation",
    )

    raw_counter = _raw_model(package.Counter, {})
    rejected(
        "required_cardinality",
        lambda: package.Counter.__runtime_projection__.validate_create(
            package.Counter, raw_counter
        ),
        category="invalid_input",
        code="missing_required_field",
    )
    rejected(
        "role_cardinality",
        lambda: package.Container(
            item=(
                package.EventRef("0x01"),
                package.EventRef("0x02"),
                package.EventRef("0x03"),
            )
        ),
        exception_names=("ValueError",),
    )
    if "member" in inspect.signature(package.Employment).parameters:
        raise AssertionError("employment exposed its specialized-away role")
    rejected(
        "role_constructibility",
        lambda: package.Employment(member=constructed["ada"]),
        exception_names=("TypeError",),
    )
    rejected(
        "scalar_domain",
        lambda: package.Score(2**63),
        category="invalid_input",
        code="wrong_scalar_domain",
    )

    expected_families = {
        "abstract_constructibility",
        "allowed_values",
        "field_constructibility",
        "inherited_owns",
        "inherited_plays",
        "inherited_relates",
        "invalid_player_type",
        "key",
        "maximum_cardinality",
        "ordered_distinct_player",
        "ordered_distinct_scalar",
        "ownership_cardinality",
        "range",
        "regex",
        "required_cardinality",
        "role_cardinality",
        "role_constructibility",
        "scalar_domain",
    }
    if families != expected_families:
        raise AssertionError("projected rejection-family ledger is incomplete")

    scalar_models = (
        package.ValBool,
        package.ValDate,
        package.ValDatetime,
        package.ValDatetimeTz,
        package.ValDecimal,
        package.ValDouble,
        package.ValDuration,
        package.Score,
        package.Nickname,
    )
    scalar_domains = sorted(
        model.__projection__["declaration"]["value_type"] for model in scalar_models
    )
    if len(set(scalar_domains)) != len(scalar_models):
        raise AssertionError("generated package did not expose all nine scalar domains")

    if package.Person.aliases.fact["unique"] is not True:
        raise AssertionError("generated projection did not retain provider-owned uniqueness")
    range_path = getattr(range_error, "path", None)
    range_details = getattr(range_error, "details", None)
    if (
        not isinstance(range_path, list)
        or len(range_path) != 1
        or not isinstance(range_path[0].get("value"), dict)
        or not isinstance(range_details, dict)
    ):
        raise AssertionError("representative range diagnostic is malformed")
    range_type = range_path[0]["value"]
    actual = range_details.get("actual")
    maximum = range_details.get("maximum")
    if not isinstance(actual, dict) or not isinstance(maximum, dict):
        raise AssertionError("representative range diagnostic lost its bounds")

    return (
        {
            "provider_enforced_families": [
                {
                    "family": "unique",
                    "local_preflight": "not_applicable",
                    "projection_fact_retained": True,
                    "provider_enforced": True,
                }
            ],
            "rejection_families": [
                {
                    "family": family,
                    "rejected": True,
                    "rejected_before_provider_io": True,
                }
                for family in sorted(families)
            ],
            "representative_diagnostic": {
                "category": getattr(range_error, "sdk_category"),
                "code": getattr(range_error, "code"),
                "details": {
                    "actual": {"kind": actual["kind"], "value": str(actual["value"])},
                    "maximum": {
                        "kind": maximum["kind"],
                        "value": str(maximum["value"]),
                    },
                },
                "path": [
                    {
                        "kind": "type",
                        "value": f"{range_type['kind']}:{range_type['label']}",
                    }
                ],
                "provider_calls": provider_calls,
            },
            "scalar_domains": scalar_domains,
        },
        scalar_duplicate,
        player_duplicate,
    )


async def _foreign_hydration_rejection(
    package: Any,
    foreign: Any,
) -> tuple[Exception, bool]:
    validated_handle = await _hydrate_exact(
        foreign,
        foreign.Person,
        {"nodes": [_ada_node()]},
        0,
        expose_validated_handle=True,
    )
    public_result_published = isinstance(validated_handle, foreign.Person)
    error = _expect_rejection(
        lambda: package.Person.__runtime_projection__.hydrate_thing(validated_handle),
        category="integrity",
        code="generated_token_package_mismatch",
    )
    return error, public_result_published


def _duplicate_observation(error: Exception) -> tuple[int, int]:
    details = getattr(error, "details", None)
    if not isinstance(details, dict):
        raise AssertionError("ordered duplicate diagnostic lost its details")
    first = details.get("first_index")
    duplicate = details.get("duplicate_index")
    if not isinstance(first, dict) or not isinstance(duplicate, dict):
        raise AssertionError("ordered duplicate diagnostic lost its indices")
    if first.get("kind") != "count" or duplicate.get("kind") != "count":
        raise AssertionError("ordered duplicate diagnostic changed its index domains")
    first_value = first.get("value")
    duplicate_value = duplicate.get("value")
    if type(first_value) is not int or type(duplicate_value) is not int:
        raise AssertionError("ordered duplicate diagnostic indices are not exact integers")
    return first_value, duplicate_value


def _build_report(root: Path, package: Any, foreign: Any) -> dict[str, object]:
    constructed = _constructed_observations(package)
    hydrated = asyncio.run(_hydrated_observations(package))
    constraints, scalar_duplicate, player_duplicate = _constraint_observation(package, constructed)
    local_construction = type(constructed["ada"]) is package.Person
    local_hydration = type(hydrated["ada"]) is package.Person
    if not local_construction or not local_hydration:
        raise AssertionError("local package construction or hydration changed its exact class")

    authored_scalars = {
        domain: _report_scalar(domain, value) for domain, value in _ADA_SCALARS.items()
    }
    constructed_scalars = _attribute_values(constructed["ada"])
    hydrated_scalars = _attribute_values(hydrated["ada"])
    if authored_scalars != constructed_scalars or constructed_scalars != hydrated_scalars:
        raise AssertionError("canonical scalar construction and hydration diverged")

    foo_token = package.Person.foo__bar
    score_gte_token = package.Person.score__gte
    generated_tokens = [
        _token_observation(foo_token, "foo__bar"),
        _token_observation(score_gte_token, "score__gte"),
    ]
    token_identities_distinct = (
        foo_token is not score_gte_token and foo_token.fact["id"] != score_gte_token.fact["id"]
    )
    package_branded = (
        foo_token.owner is package.Person
        and foreign.Person.foo__bar.owner is foreign.Person
        and foo_token.owner is not foreign.Person.foo__bar.owner
    )
    if not token_identities_distinct or not package_branded:
        raise AssertionError("generated field token identities are not exact and package-owned")

    inherited_role = package.PlainActivity.participant.fact["role"]
    constructed_plain = constructed["plain"]
    hydrated_plain = hydrated["plain"]
    inherited_constructed = (
        _model_label(constructed_plain) == "plain-activity"
        and _model_label(constructed_plain.participant) == "person"
    )
    inherited_hydrated = (
        _model_label(hydrated_plain) == "plain-activity"
        and _model_label(hydrated_plain.participant) == "person"
    )
    role_identity_preserved = inherited_role == {
        "declaring_relation": "base-activity",
        "label": "participant",
    }
    if not inherited_constructed or not inherited_hydrated or not role_identity_preserved:
        raise AssertionError("inherited abstract relation role did not round-trip exactly")

    negative_constructed = constructed["negative_robot"].robot_id.value
    positive_constructed = constructed["positive_robot"].robot_id.value
    negative_hydrated = hydrated["negative_robot"].robot_id.value
    positive_hydrated = hydrated["positive_robot"].robot_id.value
    if (
        negative_constructed != -7
        or negative_hydrated != negative_constructed
        or positive_constructed != 7
        or positive_hydrated != positive_constructed
    ):
        raise AssertionError("signed integer key hydration was not exact")

    constructed_person_interaction = constructed["interaction_person"]
    constructed_robot_interaction = constructed["interaction_robot"]
    hydrated_person_interaction = hydrated["interaction_person"]
    hydrated_robot_interaction = hydrated["interaction_robot"]
    constructed_absent_interaction = constructed["interaction_absent"]
    hydrated_absent_interaction = hydrated["interaction_absent"]
    polymorphic_players = sorted(
        {
            _model_label(constructed_person_interaction.actor),
            _model_label(constructed_robot_interaction.actor),
            _model_label(hydrated_person_interaction.actor),
            _model_label(hydrated_robot_interaction.actor),
        }
    )
    if polymorphic_players != ["person", "robot"]:
        raise AssertionError("optional polymorphic role did not preserve both player models")
    if (
        constructed_absent_interaction.actor is not None
        or hydrated_absent_interaction.actor is not None
    ):
        raise AssertionError("absent optional role materialized as a present player")
    if hydrated_robot_interaction.actor.robot_id.value != 7:
        raise AssertionError("polymorphic robot player lost its signed key")

    constructed_container = constructed["container"]
    hydrated_container = hydrated["container"]
    relation_player_constructed = constructed_container.item[0]
    relation_player_hydrated = hydrated_container.item[0]
    relation_player_preserved = (
        _model_label(relation_player_constructed) == "event"
        and _model_label(relation_player_hydrated) == "event"
    )
    if not relation_player_preserved:
        raise AssertionError("relation-as-player materialization changed its model identity")

    constructed_aliases = [alias.value for alias in constructed["ada"].aliases]
    hydrated_aliases = [alias.value for alias in hydrated["ada"].aliases]
    constructed_participants = [
        player.identifier.value for player in constructed["link"].participant
    ]
    hydrated_participants = [player.identifier.value for player in hydrated["link"].participant]
    authored_aliases = ["analyst", "mathematician"]
    authored_participants = ["data-ada", "data-dana"]
    if (
        constructed_aliases != authored_aliases
        or hydrated_aliases != authored_aliases
        or constructed_participants != authored_participants
        or hydrated_participants != authored_participants
    ):
        raise AssertionError("ordered projected collections did not preserve caller order")

    aliases_mode = package.Person.aliases.fact["multiplicity"].get("collection_mode")
    participant_mode = package.NetworkLink.participant.fact["multiplicity"].get("collection_mode")
    if aliases_mode != "ordered_list" or participant_mode != "ordered_list":
        raise AssertionError("ordered collection mode was not retained")
    if not _has_annotation(package.Person.aliases, "distinct") or not _has_annotation(
        package.NetworkLink.participant, "distinct"
    ):
        raise AssertionError("ordered-distinct projection facts were not retained")
    unordered_default = (
        package.Container.item.fact["multiplicity"].get("collection_mode") is None
        and package.Container.item.fact["multiplicity"]["container"] == "sequence"
    )
    if not unordered_default:
        raise AssertionError("legacy unordered collection compatibility changed")
    scalar_first, scalar_second = _duplicate_observation(scalar_duplicate)
    player_first, player_second = _duplicate_observation(player_duplicate)

    foreign_person = _make_person(foreign)
    construction_rejection = _expect_rejection(
        lambda: package.PlainActivity(participant=foreign_person),
        category="integrity",
        code="generated_token_package_mismatch",
    )
    hydration_rejection, public_result_published = asyncio.run(
        _foreign_hydration_rejection(package, foreign)
    )
    if public_result_published:
        raise AssertionError("foreign hydration published a generated model before rejection")
    diagnostic_text = f"{construction_rejection}\n{hydration_rejection}"
    provider_text_exposed = "projected-provider-secret" in diagnostic_text
    if provider_text_exposed:
        raise AssertionError("foreign-package diagnostic exposed provider text")

    observations = {
        "canonical_scalar_values": {
            "authored": authored_scalars,
            "constructed": constructed_scalars,
            "hydrated": hydrated_scalars,
        },
        "field_name_identity": {
            "generated_tokens": generated_tokens,
            "package_branded": package_branded,
            "token_identities_distinct": token_identities_distinct,
        },
        "inherited_relation_role": {
            "constructed": inherited_constructed,
            "hydrated": inherited_hydrated,
            "inherited_relation": inherited_role["declaring_relation"],
            "inherited_role": inherited_role["label"],
            "model": _model_label(constructed_plain),
            "player_model": _model_label(constructed_plain.participant),
            "role_identity_preserved": role_identity_preserved,
        },
        "integer_key_polymorphic_role": {
            "absent": {
                "relation_ref": hydrated_absent_interaction.identifier.value,
                "role_present": hydrated_absent_interaction.actor is not None,
                "round_trip_exact": (
                    constructed_absent_interaction.identifier.value
                    == hydrated_absent_interaction.identifier.value
                ),
            },
            "integer_keys": [
                {
                    "model": _model_label(hydrated["negative_robot"]),
                    "round_trip_exact": negative_constructed == negative_hydrated,
                    "sign": "negative",
                    "value": str(negative_hydrated),
                },
                {
                    "model": _model_label(hydrated["positive_robot"]),
                    "round_trip_exact": positive_constructed == positive_hydrated,
                    "sign": "positive",
                    "value": str(positive_hydrated),
                },
            ],
            "optional_role": package.Interaction.actor.fact["role"]["label"],
            "polymorphic_players_observed": polymorphic_players,
            "present": [
                {
                    "player": {
                        "key": hydrated_person_interaction.actor.identifier.value,
                        "model": _model_label(hydrated_person_interaction.actor),
                    },
                    "relation_ref": hydrated_person_interaction.identifier.value,
                },
                {
                    "player": {
                        "key": str(hydrated_robot_interaction.actor.robot_id.value),
                        "model": _model_label(hydrated_robot_interaction.actor),
                    },
                    "relation_ref": hydrated_robot_interaction.identifier.value,
                },
            ],
            "relation": _model_label(hydrated_person_interaction),
            "relation_as_player": {
                "owner_model": _model_label(hydrated_container),
                "player_model": _model_label(relation_player_hydrated),
                "preserved": relation_player_preserved,
                "role": package.Container.item.fact["role"]["label"],
            },
        },
        "ordered_distinct_collections": {
            "owns": {
                "authored": authored_aliases,
                "constructed": constructed_aliases,
                "distinct": _has_annotation(package.Person.aliases, "distinct"),
                "field": package.Person.aliases.fact["id"]["attribute"],
                "hydrated": hydrated_aliases,
                "mode": aliases_mode,
            },
            "player_duplicate": {
                "canonical_player": {
                    "key": constructed["ada"].identifier.value,
                    "model": _model_label(constructed["ada"]),
                },
                "category": getattr(player_duplicate, "sdk_category"),
                "code": getattr(player_duplicate, "code"),
                "duplicate_index": player_second,
                "first_index": player_first,
                "rejected_before_provider_io": True,
            },
            "relates": {
                "authored": authored_participants,
                "constructed": constructed_participants,
                "distinct": _has_annotation(package.NetworkLink.participant, "distinct"),
                "hydrated": hydrated_participants,
                "mode": participant_mode,
                "role": package.NetworkLink.participant.fact["role"]["label"],
            },
            "scalar_duplicate": {
                "canonical_value": constructed["ada"].aliases[0].value,
                "category": getattr(scalar_duplicate, "sdk_category"),
                "code": getattr(scalar_duplicate, "code"),
                "duplicate_index": scalar_second,
                "first_index": scalar_first,
                "rejected_before_provider_io": True,
            },
            "unordered_compatibility_default": unordered_default,
        },
        "projected_constraint_validation": constraints,
        "token_package_fencing": {
            "accepted_local": {
                "construction": local_construction,
                "hydration": local_hydration,
            },
            "foreign_rejections": {
                "construction": {
                    "category": getattr(construction_rejection, "sdk_category"),
                    "code": getattr(construction_rejection, "code"),
                    "rejected_before_provider_io": True,
                },
                "hydration": {
                    "category": getattr(hydration_rejection, "sdk_category"),
                    "code": getattr(hydration_rejection, "code"),
                    "public_result_published": public_result_published,
                },
            },
            "provider_text_exposed": provider_text_exposed,
        },
    }
    return {
        "authority": {
            "journey": _source_identity(root, JOURNEY_RELATIVE, "Projected journey"),
            "schema": _source_identity(root, SCHEMA_RELATIVE, "Projected schema"),
        },
        "binding": "python",
        "format": REPORT_FORMAT,
        "observations": observations,
        "semantic_profile": SEMANTIC_PROFILE,
    }


def _load_packages() -> tuple[Any, Any]:
    package_root = os.environ.get(PACKAGE_ROOT_ENV)
    if package_root is None or not package_root:
        raise ProducerError("missing_package_root", f"{PACKAGE_ROOT_ENV} must be set")
    root = Path(package_root)
    if not root.is_absolute():
        raise ProducerError("invalid_package_root", "generated package root must be a directory")
    try:
        metadata = root.lstat()
    except OSError as error:
        raise ProducerError(
            "invalid_package_root", "generated package root cannot be inspected"
        ) from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
        raise ProducerError("invalid_package_root", "generated package root must be a directory")
    sys.path.insert(0, str(root))
    try:
        import generated_projected
        import generated_projected_foreign
    except ImportError as error:
        raise ProducerError(
            "generated_package_import", "Projected generated packages cannot be imported"
        ) from error
    return generated_projected, generated_projected_foreign


def _repository_root(environment: Mapping[str, str] = os.environ) -> Path:
    value = environment.get(REPOSITORY_ROOT_ENV)
    if value is None or not value:
        raise ProducerError("missing_repository_root", f"{REPOSITORY_ROOT_ENV} must be set")
    root = Path(value)
    if not root.is_absolute():
        raise ProducerError("invalid_repository_root", "repository root must be absolute")
    try:
        metadata = root.lstat()
    except OSError as error:
        raise ProducerError(
            "invalid_repository_root", "repository root cannot be inspected"
        ) from error
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
        raise ProducerError("invalid_repository_root", "repository root must be a real directory")
    return root


def main() -> int:
    try:
        output = _output_path()
        package, foreign = _load_packages()
        root = _repository_root()
        report = _build_report(root, package, foreign)
        _publish_report(output, report)
    except (AssertionError, ProducerError) as error:
        print(f"Python Projected parity producer rejected: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
