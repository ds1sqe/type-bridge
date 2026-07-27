import json

from generated_v2 import (
    PLAYING_FACTS,
    PROJECTION_FINGERPRINT_JSON,
    RUNTIME_PROJECTION_JSON,
    SEMANTIC_SCHEMA_FINGERPRINT_JSON,
    Aliases,
    Container,
    Employment,
    Event,
    EventRef,
    FieldToken,
    Identifier,
    Nickname,
    Person,
    PersonRef,
    RoleToken,
)

descriptor = Employment.__dict__["employee"]
assert descriptor.name == "employee"
assert isinstance(Employment.employee, RoleToken)
assert isinstance(Person.identifier, FieldToken)
assert Person.identifier.fact["key"] is True
assert Person.aliases.fact["unique"] is True

person = Person(
    identifier=Identifier("person-1"),
    nickname=Nickname("alice"),
    aliases=[Aliases("a"), Aliases("b")],
)
assert isinstance(person.identifier, Identifier)
assert isinstance(person.nickname, Nickname)
assert len(person.aliases) == 2
assert all(isinstance(alias, Aliases) for alias in person.aliases)
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
        employee=Person(identifier=Identifier("person-1")),
        member=Person(identifier=Identifier("person-2")),
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
    Person(identifier=Identifier("person-1"), aliases="not-a-sequence-value")
except TypeError:
    pass
else:
    raise AssertionError("string was accepted as a multi-cardinality owns value")

assert len(PLAYING_FACTS) == 5
membership_facts = [
    fact
    for fact in PLAYING_FACTS.values()
    if fact["role"]["declaring_relation"] == "membership" and fact["role"]["label"] == "member"
]
assert len(membership_facts) == 2
membership_by_player = {fact["id"]["player"]["label"]: fact for fact in membership_facts}
assert set(membership_by_player) == {"person", "robot"}
assert membership_by_player["person"]["multiplicity"]["cardinality"]["max"] == "2"
assert membership_by_player["robot"]["multiplicity"]["cardinality"]["max"] == "1"
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
