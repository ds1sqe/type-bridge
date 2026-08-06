from collections.abc import Mapping
from typing import TypedDict

from generated_v2 import (
    PLAYING_FACTS as BASE_PLAYING_FACTS,
)
from generated_v2 import (
    PROJECTION_FINGERPRINT_JSON as BASE_PROJECTION_FINGERPRINT,
)
from generated_v2 import (
    SEMANTIC_SCHEMA_FINGERPRINT_JSON as BASE_SEMANTIC_FINGERPRINT,
)
from generated_variant import (
    PLAYING_FACTS as VARIANT_PLAYING_FACTS,
)
from generated_variant import (
    PROJECTION_FINGERPRINT_JSON as VARIANT_PROJECTION_FINGERPRINT,
)
from generated_variant import (
    SEMANTIC_SCHEMA_FINGERPRINT_JSON as VARIANT_SEMANTIC_FINGERPRINT,
)

assert BASE_SEMANTIC_FINGERPRINT != VARIANT_SEMANTIC_FINGERPRINT
assert BASE_PROJECTION_FINGERPRINT != VARIANT_PROJECTION_FINGERPRINT


class LabelReference(TypedDict):
    label: str


class PlayingRole(TypedDict):
    declaring_relation: str
    label: str


class PlayingFactId(TypedDict):
    player: LabelReference


class PlayingCardinality(TypedDict):
    max: str | None


class PlayingMultiplicity(TypedDict):
    cardinality: PlayingCardinality


class PlayingFact(TypedDict):
    id: PlayingFactId
    role: PlayingRole
    multiplicity: PlayingMultiplicity


def playing_fact(
    facts: Mapping[str, PlayingFact],
    declaring_relation: str,
    role: str,
    player: str,
) -> PlayingFact:
    return next(
        fact
        for fact in facts.values()
        if fact["role"]["declaring_relation"] == declaring_relation
        and fact["role"]["label"] == role
        and fact["id"]["player"]["label"] == player
    )


assert len(BASE_PLAYING_FACTS) == 12
assert len(VARIANT_PLAYING_FACTS) == 12
base_membership = playing_fact(BASE_PLAYING_FACTS, "membership", "member", "person")
variant_membership = playing_fact(VARIANT_PLAYING_FACTS, "membership", "member", "person")
assert base_membership["multiplicity"]["cardinality"]["max"] == "2"
assert variant_membership["multiplicity"]["cardinality"]["max"] == "3"
assert playing_fact(BASE_PLAYING_FACTS, "event", "subject", "person") == playing_fact(
    VARIANT_PLAYING_FACTS,
    "event",
    "subject",
    "person",
)
