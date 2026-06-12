"""Python public model declarations for the cross-language parity fixture."""

from type_bridge import (
    AttributeFlags,
    Boolean,
    Card,
    Date,
    DateTime,
    DateTimeTZ,
    Decimal,
    Double,
    Duration,
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    String,
    TypeFlags,
    Unique,
)


class ParityId(String):
    flags = AttributeFlags(name="parity-id")


class ParityName(String):
    flags = AttributeFlags(name="parity-name")


class ParityEmail(String):
    flags = AttributeFlags(name="parity-email")


class ParityAge(Integer):
    flags = AttributeFlags(name="parity-age")


class ParityScore(Double):
    flags = AttributeFlags(name="parity-score")


class ParityActive(Boolean):
    flags = AttributeFlags(name="parity-active")


class ParityBirthDate(Date):
    flags = AttributeFlags(name="parity-birth-date")


class ParityLoginAt(DateTime):
    flags = AttributeFlags(name="parity-login-at")


class ParitySeenAt(DateTimeTZ):
    flags = AttributeFlags(name="parity-seen-at")


class ParityBalance(Decimal):
    flags = AttributeFlags(name="parity-balance")


class ParitySessionLength(Duration):
    flags = AttributeFlags(name="parity-session-length")


class ParityTag(String):
    flags = AttributeFlags(name="parity-tag")


class ParityNote(String):
    flags = AttributeFlags(name="parity-note")


class ParitySince(Date):
    flags = AttributeFlags(name="parity-since")


class ParityConfidence(Integer):
    flags = AttributeFlags(name="parity-confidence")


class ParityKind(String):
    flags = AttributeFlags(name="parity-kind")


class ParityParty(Entity):
    flags = TypeFlags(name="parity-party", abstract=True)

    id: ParityId = Flag(Key)
    name: ParityName | None = None


class ParityPerson(ParityParty):
    flags = TypeFlags(name="parity-person")

    email: ParityEmail = Flag(Unique)
    age: ParityAge | None = None
    score: ParityScore | None = None
    active: ParityActive | None = None
    birth_date: ParityBirthDate | None = None
    login_at: ParityLoginAt | None = None
    seen_at: ParitySeenAt | None = None
    balance: ParityBalance | None = None
    session_length: ParitySessionLength | None = None
    tags: list[ParityTag] = Flag(Card(0, 5))


class ParityCompany(Entity):
    flags = TypeFlags(name="parity-company")

    id: ParityId = Flag(Key)
    name: ParityName


class ParityEmailMessage(Entity):
    flags = TypeFlags(name="parity-email-message")

    id: ParityId = Flag(Key)
    note: ParityNote


class ParityMembership(Relation):
    flags = TypeFlags(name="parity-membership")

    member: Role[ParityPerson] = Role("member", ParityPerson, cardinality=Card(1, 1))
    organization: Role[ParityCompany] = Role(
        "organization",
        ParityCompany,
        cardinality=Card(1, 1),
    )
    evidence: Role[ParityPerson | ParityEmailMessage] = Role.multi(
        "evidence",
        ParityPerson,
        ParityEmailMessage,
        cardinality=Card(0, 5),
    )
    since: ParitySince
    confidence: ParityConfidence | None = None


class ParityTokenOrigin(Relation):
    flags = TypeFlags(name="parity-token-origin")

    token: Role[ParityParty | ParityPerson] = Role.multi(
        "token",
        ParityParty,
        ParityPerson,
        cardinality=Card(1, 1),
    )
    issue: Role[ParityCompany] = Role("issue", ParityCompany, cardinality=Card(1, 1))
    kind: ParityKind


class ParityContribution(Relation):
    flags = TypeFlags(name="parity-contribution")

    contributor: Role[ParityPerson] = Role("contributor", ParityPerson, abstract=True)
    work: Role[ParityEmailMessage] = Role("work", ParityEmailMessage)


class ParityAuthoring(ParityContribution):
    flags = TypeFlags(name="parity-authoring")

    author: Role[ParityPerson] = Role("author", ParityPerson, overrides="contributor")


PARITY_ENTITIES = [
    ParityParty,
    ParityPerson,
    ParityCompany,
    ParityEmailMessage,
]

PARITY_RELATIONS = [
    ParityMembership,
    ParityTokenOrigin,
    ParityContribution,
    ParityAuthoring,
]
