"""Lang→IR marshalling of plays-side cardinality (#130).

A relation-side ``Role(..., plays_cardinality=Card(N, M))`` resolves into the entity-side
``plays_cardinalities["{relation}:{role}"]`` IR overlay on each player's entry. These tests
pin the overlay routing (entity vs relation player, single vs multi) and the byte-identical
no-card path that the Rust core emitter consumes.
"""

from __future__ import annotations

import pytest

from tests.utils.handwritten import Card, Entity, Relation, Role, TypeFlags
from type_bridge._rust_runtime import generate_define_block
from type_bridge.migration._lower import _schema_info_for_models


class PcCompany(Entity):
    flags = TypeFlags(name="pc-company")


class PcPerson(Entity):
    flags = TypeFlags(name="pc-person")


class PcContractor(Entity):
    flags = TypeFlags(name="pc-contractor")


class PcEmployment(Relation):
    flags = TypeFlags(name="pc-employment")

    employer: Role[PcCompany] = Role("employer", PcCompany, plays_cardinality=Card(0, 1))


class PcGig(Relation):
    flags = TypeFlags(name="pc-gig")

    worker: Role[PcPerson] = Role.multi(
        "worker", PcPerson, PcContractor, plays_cardinality=Card(0, 1)
    )


class PcContract(Relation):
    flags = TypeFlags(name="pc-contract")

    party: Role[PcPerson] = Role("party", PcPerson)


class PcDispute(Relation):
    flags = TypeFlags(name="pc-dispute")

    subject: Role[PcContract] = Role("subject", PcContract, plays_cardinality=Card(0, 1))


def _schema(*, entities: list, relations: list) -> dict:
    return _schema_info_for_models([*entities, *relations])


def test_plays_card_overlay_lands_on_entity_player() -> None:
    info = _schema(entities=[PcCompany], relations=[PcEmployment])

    # Tuple, not list: the overlay is built by the Rust lowering and pythonizes
    # its (min, max) pair as a tuple; only Rust consumers read it back.
    assert info["entities"]["pc-company"]["plays_cardinalities"] == {
        "pc-employment:employer": (0, 1)
    }
    # The relation declaring the role carries no overlay; plays-card is per-player.
    assert info["relations"]["pc-employment"]["plays_cardinalities"] == {}


def test_plays_card_overlay_written_per_player_for_multi_player_role() -> None:
    info = _schema(entities=[PcPerson, PcContractor], relations=[PcGig])

    assert info["entities"]["pc-person"]["plays_cardinalities"] == {"pc-gig:worker": (0, 1)}
    assert info["entities"]["pc-contractor"]["plays_cardinalities"] == {"pc-gig:worker": (0, 1)}


def test_plays_card_overlay_lands_on_relation_player() -> None:
    # PcContract is a relation that plays "subject" in PcDispute; the overlay must land on
    # the relations dict, not entities.
    info = _schema(entities=[PcPerson], relations=[PcContract, PcDispute])

    assert info["relations"]["pc-contract"]["plays_cardinalities"] == {"pc-dispute:subject": (0, 1)}
    assert info["entities"]["pc-person"]["plays_cardinalities"] == {}


def test_no_plays_card_leaves_overlay_empty() -> None:
    info = _schema(entities=[PcPerson], relations=[PcContract])

    assert info["entities"]["pc-person"]["plays_cardinalities"] == {}
    assert info["relations"]["pc-contract"]["plays_cardinalities"] == {}


def test_plays_card_emits_card_annotation_on_plays_line() -> None:
    pytest.importorskip("type_bridge_core")
    typeql = generate_define_block(_schema(entities=[PcCompany], relations=[PcEmployment]))

    assert "pc-company plays pc-employment:employer @card(0..1);" in typeql


def test_no_plays_card_emits_bare_plays_line_byte_identical() -> None:
    pytest.importorskip("type_bridge_core")
    typeql = generate_define_block(_schema(entities=[PcPerson], relations=[PcContract]))

    # Invariant 4 regression lock: no plays_cardinality anywhere ⇒ no @card emitted, and the
    # plays line is the bare form unchanged from today.
    assert "pc-person plays pc-contract:party;" in typeql
    assert "@card" not in typeql
