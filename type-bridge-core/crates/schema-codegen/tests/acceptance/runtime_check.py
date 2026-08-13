import asyncio
import hashlib
import json
import os
import struct
from dataclasses import dataclass
from datetime import UTC, date, datetime, timedelta
from decimal import Decimal
from pathlib import Path

import generated_v2._query as generated_query_module
from generated_v2 import (
    PLAYING_FACTS,
    PROJECTION_FINGERPRINT_JSON,
    RUNTIME_PROJECTION_JSON,
    SEMANTIC_SCHEMA_FINGERPRINT_JSON,
    Actor,
    Aliases,
    Container,
    Employment,
    Event,
    EventRef,
    FieldToken,
    FooBar,
    Identifier,
    Interaction,
    Membership,
    Nickname,
    Party,
    Person,
    PersonRef,
    QueryCancellation,
    QueryExecutionResourceLimits,
    RemoteQueryLimits,
    RemoteQuerySession,
    Robot,
    RobotId,
    RoleToken,
    Score,
    ValBool,
    ValConstrained,
    ValDate,
    ValDatetime,
    ValDatetimeTz,
    ValDecimal,
    ValDouble,
    ValDuration,
    aggregate,
    integer_input,
    qualifying_score,
)
from generated_variant import Person as VariantPerson
from type_bridge_core import MatchRequestError

from type_bridge.query import QueryBuilder
from type_bridge.session import Database


@dataclass(frozen=True, slots=True)
class EmploymentRow:
    person: Person
    employment: Employment


@dataclass
class MutableRow:
    person: Person


def make_person(identifier: str, **values: object) -> Person:
    fields: dict[str, object] = {
        "identifier": Identifier(identifier),
        "score": Score(3),
        "foo__bar": FooBar(7),
        "val_bool": ValBool(True),
        "val_constrained": ValConstrained(20),
        "val_date": ValDate(date(2026, 7, 29)),
        "val_datetime": ValDatetime(datetime(2026, 7, 29)),
        "val_datetime_tz": ValDatetimeTz(datetime(2026, 7, 29, tzinfo=UTC)),
        "val_decimal": ValDecimal(Decimal("3.5")),
        "val_double": ValDouble(3.5),
        "val_duration": ValDuration(timedelta(seconds=3)),
    }
    fields.update(values)
    return Person(**fields)


descriptor = Employment.__dict__["employee"]
assert descriptor.name == "employee"
assert isinstance(Employment.employee, RoleToken)
assert isinstance(Person.identifier, FieldToken)
assert Person.identifier.fact["key"] is True
assert Person.aliases.fact["unique"] is True

person = make_person(
    "person-1",
    nickname=Nickname("alice"),
    aliases=[Aliases("a"), Aliases("b")],
)
assert isinstance(person.identifier, Identifier)
assert isinstance(person.nickname, Nickname)
assert len(person.aliases) == 2
assert all(isinstance(alias, Aliases) for alias in person.aliases)
for invalid_owner_value in (
    lambda: make_person("person-range-low", val_constrained=ValConstrained(19)),
    lambda: make_person("person-range-high", val_constrained=ValConstrained(81)),
    lambda: Robot(robot_id=RobotId(1), val_constrained=ValConstrained(51)),
):
    try:
        invalid_owner_value()
    except ValueError as error:
        assert "range_violation" in str(error)
    else:
        raise AssertionError("generated ownership range was not enforced")

assert (
    make_person("person-range-valid", val_constrained=ValConstrained(51)).val_constrained.value
    == 51
)
try:
    person.val_constrained = ValConstrained(81)
except ValueError as error:
    assert "range_violation" in str(error)
else:
    raise AssertionError("generated assignment bypassed its ownership range")
assert person.val_constrained.value == 20
event = Event(subject=person)
assert event.subject is person
employment = Employment(employee=person)
assert employment.employee is person

container = Container(item=[EventRef("event-iid")])
assert len(container.item) == 1

try:
    Container(
        item=[
            EventRef("event-1"),
            EventRef("event-2"),
            EventRef("event-3"),
        ]
    )
except ValueError:
    pass
else:
    raise AssertionError("role maximum cardinality was not enforced")

try:
    Employment(employee=event)
except TypeError:
    pass
else:
    raise AssertionError("wrong role-player type was not rejected")

try:
    Employment(
        employee=make_person("person-1"),
        member=make_person("person-2"),
    )
except TypeError:
    pass
else:
    raise AssertionError("specialized-away keyword was accepted")

reference = PersonRef("person-iid", identifier=Identifier("person-1"))
assert reference.iid == "person-iid"
assert reference.__model_form__ == "reference"
assert person.iid is None
assert person.identifier.value == "person-1"

entity_match = QueryBuilder.match_entity(
    Person,
    "$person",
    identifier=Identifier("person-1"),
).build()
assert "$person isa person" in entity_match
assert 'has identifier "person-1"' in entity_match
entity_insert = QueryBuilder.insert_entity(person, "$person").build()
assert "$person isa person" in entity_insert
assert 'has identifier "person-1"' in entity_insert
relation_match = QueryBuilder.match_relation(
    Membership,
    "$membership",
    role_players={"member": "$person"},
).build()
assert "$membership isa membership" in relation_match
assert "member: $person" in relation_match
try:
    QueryBuilder.match_entity(type("ForgedEntity", (), {}))
except TypeError:
    pass
else:
    raise AssertionError("raw QueryBuilder accepted an uninstalled model class")

try:
    make_person("person-1", aliases="not-a-sequence-value")
except TypeError:
    pass
else:
    raise AssertionError("string was accepted as a multi-cardinality owns value")

assert len(PLAYING_FACTS) == 12
membership_facts = [
    fact
    for fact in PLAYING_FACTS.values()
    if fact["role"]["declaring_relation"] == "membership" and fact["role"]["label"] == "member"
]
assert len(membership_facts) == 2
membership_by_player = {fact["id"]["player"]["label"]: fact for fact in membership_facts}
assert set(membership_by_player) == {"person", "robot"}
assert membership_by_player["person"]["multiplicity"]["cardinality"]["max"] == "2"
assert membership_by_player["robot"]["multiplicity"]["cardinality"]["max"] == "2"
assert "membership player" in json.dumps(membership_by_player["person"])
assert "robot membership player" in json.dumps(membership_by_player["robot"])

event_subject_facts = [
    fact
    for fact in PLAYING_FACTS.values()
    if fact["role"]["declaring_relation"] == "event" and fact["role"]["label"] == "subject"
]
assert len(event_subject_facts) == 1
event_subject_fact = event_subject_facts[0]
assert event_subject_fact["id"]["player"]["label"] == "person"
assert event_subject_fact["multiplicity"]["cardinality"]["max"] == "1"
assert "event subject player" in json.dumps(event_subject_fact)
assert event_subject_fact["id"] != membership_by_player["person"]["id"]

projection = json.loads(RUNTIME_PROJECTION_JSON)
assert json.loads(SEMANTIC_SCHEMA_FINGERPRINT_JSON) == projection["semantic_fingerprint"]
assert json.loads(PROJECTION_FINGERPRINT_JSON) == projection["projection_fingerprint"]

match_session = Person.__runtime_projection__.match_session()
assert match_session.exact("person")
assert match_session.subtypes("party")
try:
    match_session.exact("unprojected-model")
except Exception:
    pass
else:
    raise AssertionError("projection match session accepted an unprojected type")

query_session = Person.query(Database(address="localhost:1729", database="generated-query"))
person_var = query_session.exact(Person)
other_person_var = query_session.exact(Person)
employment_var = query_session.exact(Employment)
party_var = query_session.subtypes(Party)
actor_var = query_session.subtypes(Actor)
other_actor_var = query_session.subtypes(Actor)
adult = person_var.field(Person.score).gte(Score(18))
query = query_session.query(person_var).where(adult).match(party_var)
assert query is not None
employee = employment_var.role(Employment.employee).connects(person_var)
named = query_session.query_as(
    EmploymentRow,
    person=person_var,
    employment=employment_var,
).where(employee)
assert type(named).__module__ == "generated_v2._query"
reachable = query_session.reachable(
    person_var,
    other_person_var,
    Event,
    Event.subject,
    Event.subject,
    min_depth=0,
    max_depth=3,
)
assert query_session.query(person_var).match(other_person_var).where(reachable) is not None
actor_reachable = query_session.reachable(
    actor_var,
    other_actor_var,
    Interaction,
    Interaction.actor,
    Interaction.actor,
    min_depth=0,
    max_depth=3,
)
assert query_session.query(actor_var).match(other_actor_var).where(actor_reachable) is not None
assert query_session.query(person_var, employment_var.collect()).where(employee).page_by is not None

lifecycle_session = Person.query(Database(address="localhost:1729", database="lifecycle-only"))
lifecycle_person = lifecycle_session.exact(Person)
lifecycle_predicate = lifecycle_person.field(Person.score).gte(Score(1))
lifecycle_source = lifecycle_session.query(lifecycle_person)
lifecycle_clone = lifecycle_source.clone()
lifecycle_derived = lifecycle_source.where(lifecycle_predicate)
lifecycle_source.close()
lifecycle_source.close()
assert lifecycle_source.is_closed
assert not lifecycle_clone.is_closed
assert not lifecycle_derived.is_closed
try:
    lifecycle_source.where(lifecycle_predicate)
except MatchRequestError as error:
    assert error.code == "query_resource_closed"
else:
    raise AssertionError("closed Python query remained composable")
assert lifecycle_clone.clone() is not None
lifecycle_derived.close()
assert not lifecycle_clone.is_closed
lifecycle_session.close()
lifecycle_session.close()
assert lifecycle_session.is_closed
assert lifecycle_clone.is_closed
try:
    lifecycle_clone.where(lifecycle_predicate)
except MatchRequestError as error:
    assert error.code == "query_resource_closed"
else:
    raise AssertionError("query remained composable after its session closed")
for immutable in (query_session, person_var, employee, query, named):
    try:
        immutable.projection = object()
    except AttributeError:
        pass
    else:
        raise AssertionError("generated query wrapper was mutable")
for rejected_same_package_query in (
    lambda: person_var.field(Employment.employee),
    lambda: person_var.field(Person.score).gte(Identifier("wrong-wrapper")),
    lambda: (
        query_session.exact(Employment)
        .role(Employment.employee)
        .connects(query_session.exact(Event))
    ),
    lambda: query_session.query(*(query_session.exact(Person) for _ in range(17))),
    lambda: query_session.query_as(MutableRow, person=person_var),
    lambda: query_session.query_as(
        EmploymentRow,
        employment=employment_var,
        person=person_var,
    ),
    lambda: query_session.reachable(
        person_var,
        other_person_var,
        Event,
        Event.subject,
        Event.subject,
        min_depth=True,
        max_depth=3,
    ),
):
    try:
        rejected_same_package_query()
    except (TypeError, ValueError):
        pass
    else:
        raise AssertionError("generated query accepted an invalid package-local token or value")
try:
    query_session.exact(VariantPerson)
except TypeError:
    pass
else:
    raise AssertionError("generated query accepted a foreign-package model")
try:
    person_var.field(VariantPerson.score)
except TypeError:
    pass
else:
    raise AssertionError("generated query accepted a foreign-package field token")


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


def _remote_fingerprint(domain: bytes, canonicalization: bytes, payload: bytes) -> str:
    digest = hashlib.sha256()
    digest.update(b"typebridge.fingerprint/v1\0")
    for field in (domain, canonicalization):
        digest.update(struct.pack(">Q", len(field)))
        digest.update(field)
    digest.update(b"\0")
    digest.update(struct.pack(">Q", len(payload)))
    digest.update(payload)
    return digest.hexdigest()


def _canonical(payload: dict[str, object]) -> bytes:
    return json.dumps(payload, separators=(",", ":"), sort_keys=True).encode()


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
                "epoch": "python-generated-epoch-0001",
                "identity": "python-generated-executor",
            },
            "format": "typebridge.query-remote-capabilities/v1",
            "reply_key": _SIGNING_PUBLIC_KEY.hex(),
            "reply_key_id": _SIGNING_KEY_ID,
        }
    )


def _remote_signed_reply(payload: dict[str, object], advertisement: bytes) -> bytes:
    payload_bytes = _canonical(payload)
    advertisement_fingerprint = _remote_fingerprint(
        b"typebridge.query.remote-capabilities",
        b"typebridge.query-remote-capabilities/v1",
        advertisement,
    )
    key = _SIGNING_PUBLIC_KEY.hex()
    prefix = (
        f'{{"advertisement":"{advertisement_fingerprint}",'
        '"format":"typebridge.query-remote-signed-reply/v1",'
        f'"key":"{key}","key_id":"{_SIGNING_KEY_ID}","payload":'
    ).encode()
    digest = hashlib.sha256(
        b"typebridge.query.remote-reply-signature/v1\0" + prefix + payload_bytes + b"}"
    ).digest()
    return prefix + payload_bytes + b',"signature":"' + _ed_sign(digest).hex().encode() + b'"}'


generated_advertisement = _remote_advertisement()
generated_remote_exchanges = 0
generated_failure_reply: bytes | None = None


async def generated_failure_exchange(request: bytes) -> bytes:
    global generated_failure_reply, generated_remote_exchanges
    generated_remote_exchanges += 1
    decoded = json.loads(request)
    assert _canonical(decoded) == request
    generated_failure_reply = _remote_signed_reply(
        {
            "category": "integrity",
            "code": "remote_application_failure",
            "details": {
                "attempt": {"kind": "long", "value": "-7"},
                "expected": {"kind": "text_list", "value": ["person", "employee"]},
                "retryable": {"kind": "boolean", "value": False},
                "subject": {"kind": "text", "value": "person"},
            },
            "format": "typebridge.query-remote-failure/v2",
            "message": "the remote application rejected this query",
            "nonce": decoded["nonce"],
            "path": [
                {"kind": "field", "value": "plan"},
                {"kind": "index", "value": 2},
                {"kind": "identifier", "value": "person"},
            ],
            "request": _remote_fingerprint(
                b"typebridge.query.remote-request",
                b"typebridge.query-remote-request/v2",
                request,
            ),
        },
        generated_advertisement,
    )
    return generated_failure_reply


generated_remote_session = RemoteQuerySession(
    generated_advertisement,
    generated_failure_exchange,
    RemoteQueryLimits(
        max_items=11,
        max_bytes=1 << 20,
        max_collection_members=12,
        max_graph_nodes=13,
        max_attribute_values=14,
        max_role_players=15,
    ),
)
generated_remote_person = generated_remote_session.exact(Person)
captured_failure_pending: list[object] = []
original_prepare_remote_rows = getattr(
    generated_query_module,
    "query_v2_prepare_remote_model_rows",
)


def capture_failure_pending(*args: object, **kwargs: object) -> object:
    pending = original_prepare_remote_rows(*args, **kwargs)
    captured_failure_pending.append(pending)
    return pending


setattr(
    generated_query_module,
    "query_v2_prepare_remote_model_rows",
    capture_failure_pending,
)
remote_diagnostic_observation: dict[str, object]
try:
    asyncio.run(generated_remote_session.query(generated_remote_person).one())
except MatchRequestError as error:
    assert error.category == "result_decode"
    assert error.sdk_category == "integrity"
    assert error.query_category == "result_decode"
    assert error.code == "remote_application_failure"
    assert error.message == "Typed query evidence does not match the validated request invocation"
    assert error.path == [
        {"kind": "contract_field", "value": "plan"},
        {"kind": "index", "value": 2},
        {"kind": "contract_identity", "value": "person"},
    ]
    assert error.details == {
        "attempt": {"kind": "signed", "value": -7},
        "expected": {"kind": "query_identity_list", "value": ["person", "employee"]},
        "retryable": {"kind": "boolean", "value": False},
        "subject": {"kind": "query_identity", "value": "person"},
    }
    attempt_detail = error.details["attempt"]
    assert isinstance(attempt_detail, dict)
    attempt_value = attempt_detail["value"]
    assert isinstance(attempt_value, int)
    diagnostic_serialized = json.dumps(
        {"message": error.message, "path": error.path, "details": error.details},
        sort_keys=True,
    )
    remote_diagnostic_observation = {
        "category": error.sdk_category,
        "query_category": error.query_category,
        "code": error.code,
        "message": error.message,
        "path": error.path,
        "details": {
            "attempt": {"kind": "signed", "value": str(attempt_value)},
            "expected": error.details["expected"],
            "retryable": error.details["retryable"],
            "subject": error.details["subject"],
        },
        "redacted": all(
            secret not in diagnostic_serialized
            for secret in (
                "the remote application rejected this query",
                "localhost",
                "password",
            )
        ),
    }
else:
    raise AssertionError("generated remote query accepted an authenticated application failure")
finally:
    setattr(
        generated_query_module,
        "query_v2_prepare_remote_model_rows",
        original_prepare_remote_rows,
    )
assert generated_remote_exchanges == 1
assert len(captured_failure_pending) == 1
assert generated_failure_reply is not None
try:
    getattr(captured_failure_pending[0], "decode_reply")(generated_failure_reply)
except MatchRequestError as error:
    assert error.code == "query_remote_v2_reply_replayed", error.code
else:
    raise AssertionError("authenticated remote application failure did not consume its claim")
remote_diagnostic_observation["claim_consumed"] = True


async def _cancelled_before_remote_exchange() -> tuple[int, str, str]:
    cancellation = QueryCancellation()
    cancellation.cancel()
    exchanges = 0

    async def unexpected_exchange(_request: bytes) -> bytes:
        nonlocal exchanges
        exchanges += 1
        raise AssertionError("pre-cancelled remote query reached caller transport")

    session = RemoteQuerySession(
        generated_advertisement,
        unexpected_exchange,
        QueryExecutionResourceLimits(),
        cancellation=cancellation,
    )
    person = session.exact(Person)
    try:
        await session.query(person).one()
    except MatchRequestError as error:
        session.close()
        return exchanges, error.sdk_category, error.code
    raise AssertionError("pre-cancelled Python remote query returned a result")


async def _cancelled_after_remote_exchange() -> tuple[int, bool, str, str]:
    cancellation = QueryCancellation()
    exchanges = 0
    exchange_cancelled_after_send = False

    async def completed_exchange(request: bytes) -> bytes:
        nonlocal exchanges
        exchanges += 1
        decoded = json.loads(request)
        reply = _remote_signed_reply(
            {
                "category": "integrity",
                "code": "remote_application_failure",
                "details": {},
                "format": "typebridge.query-remote-failure/v2",
                "message": "provider text must be redacted",
                "nonce": decoded["nonce"],
                "path": [],
                "request": _remote_fingerprint(
                    b"typebridge.query.remote-request",
                    b"typebridge.query-remote-request/v2",
                    request,
                ),
            },
            generated_advertisement,
        )
        asyncio.get_running_loop().call_soon(cancellation.cancel)
        return reply

    session = RemoteQuerySession(
        generated_advertisement,
        completed_exchange,
        QueryExecutionResourceLimits(),
        cancellation=cancellation,
    )
    person = session.exact(Person)
    try:
        await session.query(person).one()
    except MatchRequestError as error:
        session.close()
        return exchanges, exchange_cancelled_after_send, error.sdk_category, error.code
    raise AssertionError("cancelled Python remote decode returned a result")


async def _cancelled_remote_transport() -> tuple[bool, str, str]:
    cancellation = QueryCancellation()
    exchange_started = asyncio.Event()
    exchange_cancelled = False

    async def blocked_exchange(_request: bytes) -> bytes:
        nonlocal exchange_cancelled
        exchange_started.set()
        try:
            await asyncio.Future()
        except asyncio.CancelledError:
            exchange_cancelled = True
            raise

    session = RemoteQuerySession(
        generated_advertisement,
        blocked_exchange,
        QueryExecutionResourceLimits(),
        cancellation=cancellation,
    )
    person = session.exact(Person)
    execution = asyncio.create_task(session.query(person).one())
    await exchange_started.wait()
    cancellation.cancel()
    try:
        await execution
    except MatchRequestError as error:
        return exchange_cancelled, error.category, error.code
    raise AssertionError("cancelled Python remote transport returned a result")


transport_cancelled, cancellation_category, cancellation_code = asyncio.run(
    _cancelled_remote_transport()
)
assert transport_cancelled
assert cancellation_category == "cancelled"
assert cancellation_code == "provider_cancelled"
before_exchange_count, before_exchange_category, before_exchange_code = asyncio.run(
    _cancelled_before_remote_exchange()
)
assert before_exchange_count == 0
assert before_exchange_category == "cancelled"
assert before_exchange_code == "provider_cancelled"
(
    after_exchange_count,
    exchange_cancelled_after_send,
    after_exchange_category,
    after_exchange_code,
) = asyncio.run(_cancelled_after_remote_exchange())
assert after_exchange_count == 1
assert not exchange_cancelled_after_send
assert after_exchange_category == "cancelled"
assert after_exchange_code == "provider_cancelled"
remote_cancellation_observation = {
    "before_exchange": {
        "category": before_exchange_category,
        "code": before_exchange_code,
        "exchange_count": before_exchange_count,
        "partial_result": False,
    },
    "during_decode": {
        "category": after_exchange_category,
        "code": after_exchange_code,
        "exchange_count": after_exchange_count,
        "partial_result": False,
    },
    "caller_transport_abort_supported": transport_cancelled,
    "server_exchange_cancelled_after_send": exchange_cancelled_after_send,
}


def _proof_source_identity(root: Path, relative: str) -> dict[str, str]:
    source = root / relative
    if source.is_symlink() or not source.is_file():
        raise AssertionError(f"workforce-v2 proof source is not a regular file: {relative}")
    with source.open("rb") as source_file:
        digest = hashlib.sha256(source_file.read()).hexdigest()
    return {"path": relative, "sha256": digest}


def _emit_workforce_v2_remote_proof_fragment() -> None:
    raw_destination = os.environ.get("TYPE_BRIDGE_WORKFORCE_V2_PROOF_FRAGMENT")
    if raw_destination is None:
        return
    run_nonce = os.environ.get("TYPE_BRIDGE_WORKFORCE_V2_PROOF_RUN_NONCE")
    if (
        run_nonce is None
        or len(run_nonce) != 64
        or any(character not in "0123456789abcdef" for character in run_nonce)
    ):
        raise AssertionError("workforce-v2 proof run nonce must be 64 lowercase hex characters")
    destination = Path(raw_destination)
    if not destination.is_absolute():
        raise AssertionError("workforce-v2 proof fragment path must be absolute")
    if destination.parent.is_symlink() or not destination.parent.is_dir():
        raise AssertionError("workforce-v2 proof fragment parent must be a regular directory")
    root = Path.cwd()
    if not (root / "type-bridge-core").is_dir():
        raise AssertionError("workforce-v2 proof fragment emitter requires the repository root")
    contract_paths = {
        "proof_schema": (
            "tests/contracts/sdk_conformance/workforce-v2/proof-fragment-schema-v1.json"
        ),
        "allowlist": (
            "tests/contracts/sdk_conformance/workforce-v2/proof-fragment-allowlist-v1.json"
        ),
        "journey": "tests/contracts/sdk_conformance/workforce-v2/journey-v2.json",
    }
    producer_paths = sorted(
        (
            "type-bridge-core/crates/python/src/match_runtime.rs",
            "type-bridge-core/crates/python/src/query_v2_model_remote_runtime.rs",
            "type-bridge-core/crates/schema-codegen/src/python/query.py",
            "type-bridge-core/crates/schema-codegen/tests/acceptance/runtime_check.py",
        )
    )
    fragment = {
        "format": "typebridge.workforce-v2-proof-fragment/v1",
        "binding": "python",
        "semantic_profile": "typedb-3.12.1/v1",
        "run_nonce": run_nonce,
        "contract": {
            name: _proof_source_identity(root, relative)
            for name, relative in contract_paths.items()
        },
        "producer": {
            "id": "python.generated_remote_acceptance",
            "sources": [_proof_source_identity(root, relative) for relative in producer_paths],
        },
        "results": [
            {
                "observation_ref": "cancellation_remote",
                "proof_kind": "remote_runtime",
                "test_id": "python.generated_remote_cancellation",
                "outcome": "passed",
                "observation": remote_cancellation_observation,
            },
            {
                "observation_ref": "remote_structured_diagnostic",
                "proof_kind": "diagnostic",
                "test_id": "python.generated_remote_structured_diagnostic",
                "outcome": "passed",
                "observation": remote_diagnostic_observation,
            },
        ],
    }
    payload = (
        json.dumps(fragment, ensure_ascii=False, separators=(",", ":"), sort_keys=True) + "\n"
    ).encode()
    if len(payload) > 64 * 1024:
        raise AssertionError("workforce-v2 proof fragment exceeds 64 KiB")
    with destination.open("xb") as output:
        output.write(payload)
        output.flush()
        os.fsync(output.fileno())


_emit_workforce_v2_remote_proof_fragment()

remote_lifecycle = generated_remote_session.query(generated_remote_person)
remote_lifecycle_clone = remote_lifecycle.clone()
remote_lifecycle_derived = remote_lifecycle.where(
    generated_remote_person.field(Person.score).gte(Score(1))
)
remote_lifecycle.close()
remote_lifecycle.close()
assert remote_lifecycle.is_closed
assert not remote_lifecycle_clone.is_closed
assert not remote_lifecycle_derived.is_closed
remote_lifecycle_exchange_count = generated_remote_exchanges
try:
    asyncio.run(remote_lifecycle.one())
except MatchRequestError as error:
    assert error.code == "query_resource_closed"
else:
    raise AssertionError("closed Python remote query reached its terminal")
assert generated_remote_exchanges == remote_lifecycle_exchange_count
remote_lifecycle_derived.close()
assert not remote_lifecycle_clone.is_closed
generated_remote_session.close()
generated_remote_session.close()
assert generated_remote_session.is_closed
assert remote_lifecycle_clone.is_closed
try:
    asyncio.run(remote_lifecycle_clone.one())
except MatchRequestError as error:
    assert error.code == "query_resource_closed"
else:
    raise AssertionError("Python remote query remained usable after session close")
assert generated_remote_exchanges == remote_lifecycle_exchange_count


class FakeRow:
    pass


class FakeRemoteResult:
    def row_count(self) -> int:
        return 1

    def row(self, index: int) -> FakeRow:
        assert index == 0
        return FakeRow()

    def page_entry_count(self) -> int:
        return 1

    def page_entry(self, index: int) -> FakeRow:
        return self.row(index)

    def page_offset(self) -> int:
        return 0

    def page_limit(self) -> int:
        return 1

    def page_total(self) -> int:
        return 1

    def count_value(self) -> int:
        return 1

    def exists_value(self) -> bool:
        return True


class FakePending:
    def request_bytes(self) -> bytes:
        return b"generated-remote-request"

    def decode_reply(self, response: bytes) -> FakeRemoteResult:
        assert response == b"generated-remote-response"
        return FakeRemoteResult()


async def fake_exchange(request: bytes) -> bytes:
    assert request == b"generated-remote-request"
    return b"generated-remote-response"


remote_names = (
    "query_v2_remote_model_context",
    "query_v2_prepare_remote_model_rows",
    "query_v2_prepare_remote_model_page",
    "query_v2_prepare_remote_model_count",
    "query_v2_prepare_remote_model_exists",
    "query_v2_prepare_remote_model_reduce",
)
saved_remote = {name: getattr(generated_query_module, name) for name in remote_names}
saved_row_materializer = generated_query_module.Query.materialize_row
saved_page_materializer = generated_query_module.Query.materialize_page
saved_reduction_materializer = generated_query_module.Query.materialize_reduction
reduce_calls: list[tuple[object, ...]] = []
try:
    generated_query_module.query_v2_remote_model_context = lambda *arguments: object()
    generated_query_module.query_v2_prepare_remote_model_rows = lambda *arguments: FakePending()
    generated_query_module.query_v2_prepare_remote_model_page = lambda *arguments: FakePending()
    generated_query_module.query_v2_prepare_remote_model_count = lambda *arguments: FakePending()
    generated_query_module.query_v2_prepare_remote_model_exists = lambda *arguments: FakePending()
    generated_query_module.query_v2_prepare_remote_model_reduce = lambda *arguments: (
        reduce_calls.append(arguments),
        FakePending(),
    )[1]
    generated_query_module.Query.materialize_row = lambda self, row: person
    generated_query_module.Query.materialize_page = lambda self, result: (
        generated_query_module.Page(
            (person,),
            offset=0,
            limit=1,
            total=1,
        )
    )
    generated_query_module.Query.materialize_reduction = lambda self, result, term_count, *, group: (
        ((person, tuple(range(1, term_count + 1))),)
        if group is not None
        else tuple(range(1, term_count + 1))
    )
    remote_session = RemoteQuerySession(
        b"generated-advertisement",
        fake_exchange,
        RemoteQueryLimits(
            max_items=10,
            max_bytes=1 << 20,
            max_collection_members=10,
            max_graph_nodes=10,
            max_attribute_values=10,
            max_role_players=10,
        ),
    )
    remote_person = remote_session.exact(Person)
    remote_minimum = integer_input(remote_session, Score(18))
    remote_call = qualifying_score(remote_session, remote_person, remote_minimum)
    remote_nested_call = qualifying_score(remote_session, remote_person, remote_call)
    assert remote_call.gte_call(remote_nested_call) is not None
    remote_query = remote_session.query(remote_person)
    assert asyncio.run(remote_query.one()) is person
    assert asyncio.run(remote_query.first()) is person
    assert asyncio.run(remote_query.rows(limit=1)) == [person]
    assert asyncio.run(remote_query.count_by(remote_person)) == 1
    assert asyncio.run(remote_query.exists_by(remote_person)) is True
    remote_page = asyncio.run(remote_query.page_by(remote_person, limit=1, include_total=True))
    assert remote_page.items == (person,)
    assert remote_page.total == 1
    remote_score = remote_person.field(Person.score)
    assert asyncio.run(
        remote_query.aggregate(
            remote_person,
            aggregate.count(),
            aggregate.sum(remote_score),
        )
    ) == (1, 2)
    assert asyncio.run(
        remote_query.group_by(remote_person, remote_person).aggregate(aggregate.count())
    ) == ((person, (1,)),)
    assert reduce_calls[0][3] is None
    assert reduce_calls[0][4] == ["count", "sum"]
    assert reduce_calls[1][3] is not None
    for invalid_terms in ((), tuple(aggregate.count() for _ in range(17))):
        try:
            asyncio.run(remote_query.aggregate(remote_person, *invalid_terms))
        except ValueError:
            pass
        else:
            raise AssertionError("generated remote aggregate accepted invalid term cardinality")
finally:
    for name, value in saved_remote.items():
        setattr(generated_query_module, name, value)
    generated_query_module.Query.materialize_row = saved_row_materializer
    generated_query_module.Query.materialize_page = saved_page_materializer
    generated_query_module.Query.materialize_reduction = saved_reduction_materializer
