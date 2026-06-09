"""Live-TypeDB evidence for #130: plays-side cardinality enforces at-most-one.

A plays-side ``@card(0..1)`` on a role is the only TypeDB form that enforces "a given
player plays this role in at most one relation". The relates-side ``@card(1..1)`` constrains
players-per-relation and does NOT prevent a player from appearing across relations. These
tests reproduce that contrast against a live database — the closure evidence for #130.
"""

import pytest
from typedb.driver import TypeDBDriverException

from type_bridge import (
    Card,
    Database,
    Entity,
    Flag,
    Key,
    Relation,
    Role,
    SchemaManager,
    String,
    TypeFlags,
)


@pytest.mark.integration
class TestPlaysCardinalityEnforcement:
    """#130: plays-side @card(0..1) enforces at-most-one relation per player."""

    def test_plays_card_rejects_second_relation_for_same_player(self, clean_db: Database):
        """A player bound to a plays @card(0..1) role cannot join a second relation."""

        class PcName(String):
            pass

        class PcPerson(Entity):
            flags = TypeFlags(name="amo-person")
            name: PcName = Flag(Key)

        class PcCompany(Entity):
            flags = TypeFlags(name="amo-company")
            name: PcName = Flag(Key)

        class PcEmployment(Relation):
            flags = TypeFlags(name="amo-employment")
            # Plays-side @card(0..1): a person is an employee in at most one employment.
            employee: Role[PcPerson] = Role("employee", PcPerson, plays_cardinality=Card(0, 1))
            employer: Role[PcCompany] = Role("employer", PcCompany)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(PcPerson, PcCompany, PcEmployment)
        schema_manager.sync_schema(force=True)

        person_mgr = PcPerson.manager(clean_db)
        company_mgr = PcCompany.manager(clean_db)
        person_mgr.insert(PcPerson(name=PcName("Alice")))
        company_mgr.insert(PcCompany(name=PcName("Acme")))
        company_mgr.insert(PcCompany(name=PcName("Globex")))

        alice = person_mgr.get(name="Alice")[0]
        acme = company_mgr.get(name="Acme")[0]
        globex = company_mgr.get(name="Globex")[0]

        employment_mgr = PcEmployment.manager(clean_db)
        # First employment for Alice is fine.
        employment_mgr.insert(PcEmployment(employee=alice, employer=acme))

        # A second employment reuses Alice as employee, exceeding plays @card(0..1).
        with pytest.raises(TypeDBDriverException):
            employment_mgr.insert(PcEmployment(employee=alice, employer=globex))

    def test_relates_card_one_one_allows_player_in_multiple_relations(self, clean_db: Database):
        """The relates-side @card(1..1) contrast does NOT enforce at-most-one-per-player.

        This is exactly why #130 needs the plays-side form: relates-side cardinality fixes the
        number of players in each relation, not the number of relations a player appears in.
        """

        class RcName(String):
            pass

        class RcPerson(Entity):
            flags = TypeFlags(name="rc-person")
            name: RcName = Flag(Key)

        class RcCompany(Entity):
            flags = TypeFlags(name="rc-company")
            name: RcName = Flag(Key)

        class RcEmployment(Relation):
            flags = TypeFlags(name="rc-employment")
            # Relates-side @card(1..1): exactly one employee per employment — no per-player limit.
            employee: Role[RcPerson] = Role("employee", RcPerson, cardinality=Card(1, 1))
            employer: Role[RcCompany] = Role("employer", RcCompany)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(RcPerson, RcCompany, RcEmployment)
        schema_manager.sync_schema(force=True)

        person_mgr = RcPerson.manager(clean_db)
        company_mgr = RcCompany.manager(clean_db)
        person_mgr.insert(RcPerson(name=RcName("Bob")))
        company_mgr.insert(RcCompany(name=RcName("Acme")))
        company_mgr.insert(RcCompany(name=RcName("Globex")))

        bob = person_mgr.get(name="Bob")[0]
        acme = company_mgr.get(name="Acme")[0]
        globex = company_mgr.get(name="Globex")[0]

        employment_mgr = RcEmployment.manager(clean_db)
        employment_mgr.insert(RcEmployment(employee=bob, employer=acme))
        # Bob in a second relation is accepted — relates-side card imposes no per-player limit.
        employment_mgr.insert(RcEmployment(employee=bob, employer=globex))

        assert len(employment_mgr.all()) == 2
