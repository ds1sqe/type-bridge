import asyncio
import hashlib
import json
import os
import struct
import weakref
from dataclasses import dataclass
from datetime import UTC, date, datetime, timedelta, timezone
from decimal import Decimal
from pathlib import Path

import generated_identical as identical
import generated_v2._query as generated_query_module
import generated_variant as variant
from generated_ordered import CrudHook as OrderedCrudHook
from generated_ordered import DirectConnectionPolicy as OrderedDirectConnectionPolicy
from generated_ordered import Membership as OrderedMembership
from generated_ordered import MembershipRef as OrderedMembershipRef
from generated_ordered import Person as OrderedPerson
from generated_ordered import PersonRef as OrderedPersonRef
from generated_ordered import ProjectedModelManager as OrderedProjectedModelManager
from generated_ordered import Tag as OrderedTag
from generated_ordered import connect as ordered_connect
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
    ProjectedManagerComparison,
    ProjectedModelFilter,
    ProjectedModelManager,
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

direct_policy = OrderedDirectConnectionPolicy(
    "hostile-endpoint:1729",
    "hostile-database",
    username="hostile-user",
    password="hostile-password",
    tls_root_ca="/hostile/root.pem",
)
assert repr(direct_policy) == "DirectConnectionPolicy([REDACTED])"
for secret in (
    "hostile-endpoint",
    "hostile-database",
    "hostile-user",
    "hostile-password",
    "/hostile/root.pem",
):
    assert secret not in repr(direct_policy)
try:
    ordered_connect(object())
except TypeError as error:
    assert str(error) == "connect requires this generated package's DirectConnectionPolicy"
else:
    raise AssertionError("connect accepted a foreign direct connection policy")


class FakeCanonicalFilterNative:
    def __init__(
        self,
        calls: tuple[tuple[object, str, str, str, object], ...] = (),
        *,
        rejected: bool = False,
    ) -> None:
        self.calls = calls
        self.rejected = rejected

    def where_field(
        self,
        owner: object,
        field_name: str,
        metadata_json: str,
        comparison: str,
        value: object,
    ) -> "FakeCanonicalFilterNative":
        return FakeCanonicalFilterNative(
            (*self.calls, (owner, field_name, metadata_json, comparison, value))
        )

    def reject_unissued_field(self) -> "FakeCanonicalFilterNative":
        return FakeCanonicalFilterNative(self.calls, rejected=True)

    def all(self) -> list[object]:
        return []

    def first(self) -> object | None:
        return None

    def count(self) -> int:
        return 0

    def exists(self) -> bool:
        return False


class FakeCanonicalManagerNative:
    def canonical_filter(self) -> FakeCanonicalFilterNative:
        return FakeCanonicalFilterNative()


canonical_manager = ProjectedModelManager.__new__(ProjectedModelManager)
object.__setattr__(canonical_manager, "_model", Person)
object.__setattr__(canonical_manager, "_native", FakeCanonicalManagerNative())
object.__setattr__(canonical_manager, "_hooks", [])
object.__setattr__(canonical_manager, "_filtered", False)
issued_foo = Person.foo__bar
object.__setattr__(issued_foo, "owner", Employment)
object.__setattr__(issued_foo, "fact", {})
canonical_filter = canonical_manager.where(
    issued_foo,
    ProjectedManagerComparison.GTE,
    FooBar(7),
)
assert isinstance(canonical_filter, ProjectedModelFilter)
assert canonical_filter._native.calls[0][0:2] == (Person, "foo__bar")
assert canonical_filter._native.calls[0][3] == "gte"
assert (
    canonical_filter.where(
        Person.identifier,
        ProjectedManagerComparison.EQ,
        Identifier("person-1"),
    )._native.calls[:1]
    == canonical_filter._native.calls
)


class SpoofManagerComparison:
    value = "eq"


prior_canonical_calls = canonical_filter._native.calls
try:
    canonical_filter.where(
        Person.identifier,
        SpoofManagerComparison(),
        Identifier("person-spoof"),
    )
except TypeError as error:
    assert str(error) == "canonical manager comparison must use ProjectedManagerComparison"
else:
    raise AssertionError("a spoof canonical manager comparison was accepted")
assert canonical_filter._native.calls == prior_canonical_calls
assert (
    canonical_filter.where(
        Person.identifier,
        ProjectedManagerComparison.EQ,
        Identifier("person-sibling"),
    )._native.calls[:1]
    == canonical_filter._native.calls
)
forged_field = FieldToken.__new__(FieldToken)
forged_field.owner = Person
forged_field.fact = Person.identifier.fact
equality_target = Person.identifier


class EqualityForgedField(FieldToken):
    def __hash__(self) -> int:
        return hash(equality_target)

    def __eq__(self, other: object) -> bool:
        return other is equality_target


equality_forged_field = EqualityForgedField(Person, equality_target.fact)
for unissued in (
    forged_field,
    equality_forged_field,
    identical.Person.identifier,
    variant.Person.identifier,
):
    rejected = canonical_manager.where(
        unissued,
        ProjectedManagerComparison.EQ,
        Identifier("person-1"),
    )
    assert rejected._native.rejected is True

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
    except MatchRequestError as error:
        assert error.category == "invalid_input"
        assert error.sdk_category == "invalid_input"
        assert error.code == "range_constraint_violation"
    else:
        raise AssertionError("generated ownership range was not enforced")

assert (
    make_person("person-range-valid", val_constrained=ValConstrained(51)).val_constrained.value
    == 51
)
try:
    person.val_constrained = ValConstrained(81)
except MatchRequestError as error:
    assert error.category == "invalid_input"
    assert error.code == "range_constraint_violation"
else:
    raise AssertionError("generated assignment bypassed its ownership range")
assert person.val_constrained.value == 20
event = Event(subject=person)
assert event.subject is person
employment = Employment(employee=person)
assert employment.employee is person
interaction = Interaction(
    identifier=Identifier("interaction-1"),
    actor=person,
    target=person,
)
assert interaction.actor is person
assert interaction.target is person

fractional_datetime = datetime(2026, 7, 29, 1, 2, 3, 120000)
fractional_utc = datetime(2026, 7, 29, 1, 2, 3, 120000, tzinfo=UTC)
fractional_fixed = datetime(
    2026,
    7,
    29,
    1,
    2,
    3,
    120000,
    tzinfo=timezone(timedelta(hours=5, minutes=30)),
)
utc_person = make_person(
    "person-temporal-utc",
    val_datetime=ValDatetime(fractional_datetime),
    val_datetime_tz=ValDatetimeTz(fractional_utc),
)
fixed_person = make_person(
    "person-temporal-fixed",
    val_datetime_tz=ValDatetimeTz(fractional_fixed),
)
assert utc_person.val_datetime.value == fractional_datetime
assert utc_person.val_datetime_tz.value == fractional_utc
assert fixed_person.val_datetime_tz.value == fractional_fixed

for invalid_scalar_value in (
    lambda: Score(2**63),
    lambda: ValDouble(float("inf")),
    lambda: ValDouble(float("nan")),
):
    try:
        invalid_scalar_value()
    except MatchRequestError as error:
        assert error.category == "invalid_input"
        assert error.sdk_category == "invalid_input"
        assert error.code == "wrong_scalar_domain"
    else:
        raise AssertionError("unconstrained scalar bypassed canonical conversion")

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
try:
    weakref.ref(person)
except TypeError:
    pass
else:
    raise AssertionError("legacy unordered facade unexpectedly changed weakref layout")

ordered_person = OrderedPerson(tag=[OrderedTag("first"), OrderedTag("second")])
assert [tag.value for tag in ordered_person.tag] == ["first", "second"]
ordered_person_weak = weakref.ref(ordered_person)
assert ordered_person_weak() is ordered_person
ordered_person_ref = OrderedPersonRef("0xa1")
assert weakref.ref(ordered_person_ref)() is ordered_person_ref
ordered_membership_ref = OrderedMembershipRef("0xb1")
assert weakref.ref(ordered_membership_ref)() is ordered_membership_ref
try:
    OrderedPerson(tag=[OrderedTag("duplicate"), OrderedTag("duplicate")])
except MatchRequestError as error:
    assert error.category == "invalid_input"
    assert error.sdk_category == "invalid_input"
    assert error.code == "ordered_distinct_duplicate"
else:
    raise AssertionError("ordered create ownership accepted a duplicate scalar")
ordered_person.tag = [OrderedTag("third"), OrderedTag("fourth")]
assert [tag.value for tag in ordered_person.tag] == ["third", "fourth"]
previous_ordered_tags = ordered_person.tag
try:
    duplicate_tag = OrderedTag("duplicate")
    ordered_person.tag = [duplicate_tag, duplicate_tag]
except MatchRequestError as error:
    assert error.category == "invalid_input"
    assert error.sdk_category == "invalid_input"
    assert error.code == "ordered_distinct_duplicate"
    assert error.details == {
        "duplicate_index": {"kind": "count", "value": 1},
        "first_index": {"kind": "count", "value": 0},
    }
else:
    raise AssertionError("ordered ownership reassignment accepted a duplicate scalar")
assert ordered_person.tag is previous_ordered_tags

ordered_person.attach_runtime_iid("0xa")
try:
    OrderedMembership(member=[ordered_person, ordered_person])
except MatchRequestError as error:
    assert error.category == "invalid_input"
    assert error.code == "ordered_distinct_duplicate"
else:
    raise AssertionError("ordered create role accepted a duplicate identity")
other_ordered_person = OrderedPerson(tag=[OrderedTag("other")])
other_ordered_person.attach_runtime_iid("0xc")
ordered_membership = OrderedMembership(member=[ordered_person, other_ordered_person])
previous_ordered_members = ordered_membership.member
try:
    ordered_membership.member = [ordered_person, ordered_person]
except MatchRequestError as error:
    assert error.category == "invalid_input"
    assert error.code == "ordered_distinct_duplicate"
else:
    raise AssertionError("ordered role reassignment accepted a duplicate identity")
assert ordered_membership.member is previous_ordered_members

foreign_person = variant.Person(
    identifier=variant.Identifier("foreign-person"),
    score=variant.Score(3),
    foo__bar=variant.FooBar(7),
    val_bool=variant.ValBool(True),
    val_constrained=variant.ValConstrained(20),
    val_date=variant.ValDate(date(2026, 7, 29)),
    val_datetime=variant.ValDatetime(datetime(2026, 7, 29)),
    val_datetime_tz=variant.ValDatetimeTz(datetime(2026, 7, 29, tzinfo=UTC)),
    val_decimal=variant.ValDecimal(Decimal("3.5")),
    val_double=variant.ValDouble(3.5),
    val_duration=variant.ValDuration(timedelta(seconds=3)),
)
foreign_person.attach_runtime_iid("0xb")
try:
    OrderedMembership(member=[foreign_person])
except MatchRequestError as error:
    assert error.category == "integrity"
    assert error.sdk_category == "integrity"
    assert error.code == "generated_token_package_mismatch"
    assert [segment["kind"] for segment in error.path] == ["type", "role", "index"]
else:
    raise AssertionError("ordered create accepted a foreign-package role player")
try:
    ordered_membership.member = [foreign_person]
except MatchRequestError as error:
    assert error.category == "integrity"
    assert error.sdk_category == "integrity"
    assert error.code == "generated_token_package_mismatch"
    assert [segment["kind"] for segment in error.path] == ["type", "role", "index"]
    assert error.path[-1] == {"kind": "index", "value": 0}
else:
    raise AssertionError("ordered role reassignment accepted a foreign-package role player")
assert ordered_membership.member is previous_ordered_members


class MutatingOrderedUpdateHook(OrderedCrudHook[OrderedPerson]):
    def __init__(self) -> None:
        self.pre_update_calls = 0
        self.post_update_calls = 0

    def pre_update(
        self,
        sender: type[OrderedPerson],
        instance: OrderedPerson,
    ) -> None:
        assert sender is OrderedPerson
        self.pre_update_calls += 1
        duplicate = OrderedTag("hook-duplicate")
        instance.runtime_values()["tag"] = (duplicate, duplicate)

    def post_update(
        self,
        sender: type[OrderedPerson],
        instance: OrderedPerson,
    ) -> None:
        assert sender is OrderedPerson
        assert isinstance(instance, OrderedPerson)
        self.post_update_calls += 1


class FakeOrderedUpdateNative:
    def __init__(self) -> None:
        self.resolve_iid_calls = 0
        self.update_calls = 0

    def resolve_iid(self, instance: OrderedPerson) -> str | None:
        assert isinstance(instance, OrderedPerson)
        self.resolve_iid_calls += 1
        return "0xd"

    def update(self, instance: OrderedPerson) -> OrderedPerson:
        self.update_calls += 1
        return instance


detached_ordered_person = OrderedPerson(tag=[OrderedTag("detached")])
fake_ordered_native = FakeOrderedUpdateNative()
ordered_update_hook = MutatingOrderedUpdateHook()
ordered_manager = OrderedProjectedModelManager.__new__(OrderedProjectedModelManager)
object.__setattr__(ordered_manager, "_model", OrderedPerson)
object.__setattr__(ordered_manager, "_native", fake_ordered_native)
object.__setattr__(ordered_manager, "_hooks", [ordered_update_hook])
object.__setattr__(ordered_manager, "_filtered", False)
try:
    ordered_manager.update(detached_ordered_person)
except MatchRequestError as error:
    assert error.category == "invalid_input"
    assert error.code == "ordered_distinct_duplicate"
else:
    raise AssertionError("ordered detached update resolved before whole-create validation")
assert ordered_update_hook.pre_update_calls == 1
assert ordered_update_hook.post_update_calls == 0
assert fake_ordered_native.resolve_iid_calls == 0
assert fake_ordered_native.update_calls == 0

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
        raise AssertionError(f"sdk-v2 proof source is not a regular file: {relative}")
    with source.open("rb") as source_file:
        digest = hashlib.sha256(source_file.read()).hexdigest()
    return {"path": relative, "sha256": digest}


def _emit_sdk_v2_remote_proof_fragment() -> None:
    raw_destination = os.environ.get("TYPE_BRIDGE_SDK_V2_PROOF_FRAGMENT")
    if raw_destination is None:
        return
    run_nonce = os.environ.get("TYPE_BRIDGE_SDK_V2_PROOF_RUN_NONCE")
    if (
        run_nonce is None
        or len(run_nonce) != 64
        or any(character not in "0123456789abcdef" for character in run_nonce)
    ):
        raise AssertionError("sdk-v2 proof run nonce must be 64 lowercase hex characters")
    destination = Path(raw_destination)
    if not destination.is_absolute():
        raise AssertionError("sdk-v2 proof fragment path must be absolute")
    if destination.parent.is_symlink() or not destination.parent.is_dir():
        raise AssertionError("sdk-v2 proof fragment parent must be a regular directory")
    root = Path.cwd()
    if not (root / "type-bridge-core").is_dir():
        raise AssertionError("sdk-v2 proof fragment emitter requires the repository root")
    contract_paths = {
        "proof_schema": ("tests/contracts/sdk_conformance/sdk-v2/proof-fragment-schema-v1.json"),
        "allowlist": ("tests/contracts/sdk_conformance/sdk-v2/proof-fragment-allowlist-v1.json"),
        "journey": "tests/contracts/sdk_conformance/sdk-v2/journey-v2.json",
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
        "format": "typebridge.sdk-v2-proof-fragment/v1",
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
        raise AssertionError("sdk-v2 proof fragment exceeds 64 KiB")
    with destination.open("xb") as output:
        output.write(payload)
        output.flush()
        os.fsync(output.fileno())


_emit_sdk_v2_remote_proof_fragment()


def _emit_sdk_v3_package_proof_fragment() -> None:
    raw_destination = os.environ.get("TYPE_BRIDGE_SDK_V3_PROOF_FRAGMENT")
    if raw_destination is None:
        return
    run_nonce = os.environ.get("TYPE_BRIDGE_SDK_V3_PROOF_RUN_NONCE")
    if (
        run_nonce is None
        or len(run_nonce) != 64
        or any(character not in "0123456789abcdef" for character in run_nonce)
    ):
        raise AssertionError("sdk-v3 proof run nonce must be 64 lowercase hex characters")
    destination = Path(raw_destination)
    if not destination.is_absolute() or destination.exists():
        raise AssertionError("sdk-v3 proof destination must be absent and absolute")
    root = Path.cwd()
    contract_paths = {
        "proof_schema": "tests/contracts/sdk_conformance/sdk-v3/proof-fragment-schema-v1.json",
        "allowlist": "tests/contracts/sdk_conformance/sdk-v3/proof-fragment-allowlist-v1.json",
        "journey": "tests/contracts/sdk_conformance/sdk-v3/journey-v3.json",
    }
    producer_paths = sorted(
        (
            "type-bridge-core/crates/python/src/runtime_projection.rs",
            "type-bridge-core/crates/schema-codegen/src/python/runtime.py",
            "type-bridge-core/crates/schema-codegen/tests/acceptance/runtime_check.py",
        )
    )
    rejection_families = [
        {
            "family": family,
            "rejected": True,
            "rejected_before_provider_io": True,
        }
        for family in (
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
        )
    ]
    constraint = {
        "scalar_domains": [
            "boolean",
            "date",
            "datetime",
            "datetime_tz",
            "decimal",
            "double",
            "duration",
            "long",
            "string",
        ],
        "rejection_families": rejection_families,
        "provider_enforced_families": [
            {
                "family": "unique",
                "projection_fact_retained": True,
                "local_preflight": "not_applicable",
                "provider_enforced": True,
            }
        ],
        "representative_diagnostic": {
            "category": "invalid_input",
            "code": "range_constraint_violation",
            "path": [{"kind": "type", "value": "attribute:val_constrained"}],
            "details": {
                "actual": {"kind": "signed", "value": "81"},
                "maximum": {"kind": "signed", "value": "80"},
            },
            "provider_calls": 0,
        },
    }
    evidence = {
        "rejected_mutations": [
            "duplicated",
            "extra",
            "foreign",
            "forged",
            "missing",
            "reordered",
            "stale",
        ],
        "representative_mutation": {"evidence": "semantic_schema_fingerprint", "kind": "missing"},
        "diagnostic": {
            "category": "integrity",
            "code": "projection_evidence_mismatch",
            "path": [
                {"kind": "argument", "value": "projection_evidence"},
                {"kind": "index", "value": 0},
                {"kind": "contract_identity", "value": "semantic_schema_fingerprint"},
            ],
            "details": {
                "expected_occurrence_count": {"kind": "count", "value": "1"},
                "actual_occurrence_count": {"kind": "count", "value": "0"},
                "foreign_package": {"kind": "boolean", "value": False},
            },
        },
        "rejected_before_provider_io": True,
    }
    fencing = {
        "accepted_local": {"construction": True, "batch": True, "filter": True, "hydration": True},
        "rejections": {
            "construction": {
                "category": "integrity",
                "code": "generated_token_package_mismatch",
                "rejected_before_provider_io": True,
            },
            "batch": {
                "category": "integrity",
                "code": "generated_token_package_mismatch",
                "rejected_before_provider_io": True,
            },
            "filter": {
                "category": "integrity",
                "code": "generated_token_package_mismatch",
                "rejected_before_provider_io": True,
            },
            "hydration": {
                "category": "integrity",
                "code": "generated_token_package_mismatch",
                "public_result_published": False,
            },
        },
        "rejected_token_states": ["foreign", "forged", "reordered", "stale"],
        "provider_text_exposed": False,
    }
    test_id = "python.generated_package_v3_integrity"
    fragment = {
        "format": "typebridge.sdk-v3-proof-fragment/v1",
        "binding": "python",
        "semantic_profile": "typedb-3.12.1/v1",
        "run_nonce": run_nonce,
        "contract": {
            name: _proof_source_identity(root, path) for name, path in contract_paths.items()
        },
        "producer": {
            "id": "python.generated-package-v3-proof",
            "sources": [_proof_source_identity(root, path) for path in producer_paths],
        },
        "results": [
            {
                "observation_ref": "projected_constraint_validation",
                "proof_kind": "diagnostic",
                "test_id": test_id,
                "outcome": "passed",
                "observation": constraint,
            },
            {
                "observation_ref": "projection_evidence_integrity",
                "proof_kind": "diagnostic",
                "test_id": test_id,
                "outcome": "passed",
                "observation": evidence,
            },
            {
                "observation_ref": "token_package_fencing",
                "proof_kind": "diagnostic",
                "test_id": test_id,
                "outcome": "passed",
                "observation": fencing,
            },
        ],
    }
    payload = (
        json.dumps(fragment, ensure_ascii=False, separators=(",", ":"), sort_keys=True) + "\n"
    ).encode()
    with destination.open("xb") as output:
        output.write(payload)
        output.flush()
        os.fsync(output.fileno())


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

_emit_sdk_v3_package_proof_fragment()
