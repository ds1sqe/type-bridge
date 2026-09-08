import asyncio
import hashlib
import json
import struct
from dataclasses import dataclass
from datetime import UTC, date, datetime, timedelta
from decimal import Decimal

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
)
from generated_variant import Person as VariantPerson
from type_bridge_core import QueryV2Error

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


async def generated_failure_exchange(request: bytes) -> bytes:
    global generated_remote_exchanges
    generated_remote_exchanges += 1
    decoded = json.loads(request)
    assert _canonical(decoded) == request
    return _remote_signed_reply(
        {
            "category": "invalid_contract",
            "code": "remote_application_failure",
            "details": {
                "attempt": {"kind": "long", "value": "7"},
                "expected": {"kind": "text_list", "value": ["person", "employee"]},
                "retryable": {"kind": "boolean", "value": False},
                "subject": {"kind": "text", "value": "person"},
            },
            "format": "typebridge.query-remote-failure/v2",
            "message": "the remote application rejected this query",
            "nonce": decoded["nonce"],
            "path": [
                {"kind": "field", "value": "plan"},
                {"kind": "index", "value": 0},
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
try:
    asyncio.run(generated_remote_session.query(generated_remote_person).one())
except QueryV2Error as error:
    assert error.category == "invalid_contract"
    assert error.code == "remote_application_failure"
    assert error.message == "the remote application rejected this query"
    assert error.path == [
        {"kind": "field", "value": "plan"},
        {"kind": "index", "value": 0},
        {"kind": "identifier", "value": "person"},
    ]
    assert error.details == {
        "attempt": {"kind": "long", "value": "7"},
        "expected": {"kind": "text_list", "value": ["person", "employee"]},
        "retryable": {"kind": "boolean", "value": False},
        "subject": {"kind": "text", "value": "person"},
    }
else:
    raise AssertionError("generated remote query accepted an authenticated application failure")
assert generated_remote_exchanges == 1


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
