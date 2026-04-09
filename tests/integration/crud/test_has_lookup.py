"""Integration tests for cross-type attribute lookup (Entity.has / Relation.has).

Tests the TypeQL patterns:
  match entity $e; $x isa $e, has Name "Alice"; ...
  match relation $r; $x isa $r, has Name $n; ...
"""

import pytest

from type_bridge import (
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    SchemaManager,
    String,
    TypeFlags,
)

# ── Test models ─────────────────────────────────────────────────────


class HLName(String):
    """Shared attribute across entities and relation."""

    pass


class HLRegion(String):
    pass


class HLSalary(Integer):
    pass


class HLPerson(Entity):
    flags = TypeFlags(name="hl_person")
    name: HLName = Flag(Key)


class HLCompany(Entity):
    flags = TypeFlags(name="hl_company")
    name: HLName = Flag(Key)
    region: HLRegion | None = None


class HLEmployment(Relation):
    flags = TypeFlags(name="hl_employment")
    employee: Role[HLPerson] = Role("employee", HLPerson)
    employer: Role[HLCompany] = Role("employer", HLCompany)
    name: HLName | None = None
    region: HLRegion | None = None
    salary: HLSalary | None = None


# ── Tests ───────────────────────────────────────────────────────────


@pytest.mark.integration
class TestEntityHas:
    @pytest.fixture(autouse=True)
    def setup(self, clean_db):
        self.db = clean_db
        sm = SchemaManager(clean_db)
        sm.register(HLPerson)
        sm.register(HLCompany)
        sm.register(HLEmployment)
        sm.sync_schema(force=True)

        # Insert test data
        HLPerson.manager(clean_db).insert(
            HLPerson(name=HLName("Alice")),
        )
        HLPerson.manager(clean_db).insert(
            HLPerson(name=HLName("Bob")),
        )
        HLCompany.manager(clean_db).insert(
            HLCompany(name=HLName("Acme"), region=HLRegion("US")),
        )
        HLCompany.manager(clean_db).insert(
            HLCompany(name=HLName("Globex"), region=HLRegion("EU")),
        )

        # Insert relation
        alice = HLPerson.manager(clean_db).get(name="Alice")[0]
        acme = HLCompany.manager(clean_db).get(name="Acme")[0]
        HLEmployment.manager(clean_db).insert(
            HLEmployment(
                employee=alice,
                employer=acme,
                name=HLName("Engineer"),
                region=HLRegion("US"),
                salary=HLSalary(120000),
            ),
        )

        bob = HLPerson.manager(clean_db).get(name="Bob")[0]
        globex = HLCompany.manager(clean_db).get(name="Globex")[0]
        HLEmployment.manager(clean_db).insert(
            HLEmployment(
                employee=bob,
                employer=globex,
                name=HLName("Manager"),
                region=HLRegion("EU"),
                salary=HLSalary(150000),
            ),
        )

    def test_entity_has_exact_match(self):
        """Entity.has with exact value returns only matching entities."""
        results = Entity.has(self.db, HLName, "Alice")
        assert len(results) == 1
        person = results[0]
        assert isinstance(person, HLPerson)
        assert person.name.value == "Alice"

    def test_entity_has_no_value_returns_all_with_attr(self):
        """Entity.has with no value returns all entities owning that attribute."""
        results = Entity.has(self.db, HLName)
        assert len(results) == 4  # 2 persons + 2 companies
        types = {type(r).__name__ for r in results}
        assert types == {"HLPerson", "HLCompany"}

    def test_entity_has_shared_attr_returns_only_entities(self):
        """Entity.has with Region returns only entities, not relations."""
        results = Entity.has(self.db, HLRegion)
        assert len(results) == 2  # 2 companies only
        for r in results:
            assert isinstance(r, HLCompany)

    def test_entity_has_comparison(self):
        """Entity.has with comparison expression filters correctly."""
        results = Entity.has(self.db, HLName, HLName.gt(HLName("B")))
        names = {r.name.value for r in results if isinstance(r, (HLPerson, HLCompany))}
        # "Bob" > "B" and "Globex" > "B" (string comparison)
        assert "Bob" in names
        assert "Globex" in names
        assert "Alice" not in names
        assert "Acme" not in names

    def test_entity_has_with_attribute_instance(self):
        """Entity.has with Attribute instance does exact match."""
        results = Entity.has(self.db, HLName, HLName("Bob"))
        assert len(results) == 1
        person = results[0]
        assert isinstance(person, HLPerson)
        assert person.name.value == "Bob"

    def test_entity_has_returns_iid(self):
        """Returned instances have _iid set."""
        results = Entity.has(self.db, HLName, "Alice")
        assert len(results) == 1
        assert results[0]._iid is not None

    def test_entity_has_mixed_types_correct_classes(self):
        """Results contain correct concrete classes, not base Entity."""
        results = Entity.has(self.db, HLName)
        for r in results:
            assert type(r) in (HLPerson, HLCompany)
            assert type(r) is not Entity

    def test_entity_has_no_match_returns_empty(self):
        """Entity.has with a value that matches nothing returns empty list."""
        results = Entity.has(self.db, HLName, "ZZZ_NoSuchName")
        assert results == []

    def test_entity_has_contains_expression(self):
        """Entity.has with contains expression filters by substring."""
        results = Entity.has(self.db, HLName, HLName.contains(HLName("li")))
        names = {r.name.value for r in results if isinstance(r, (HLPerson, HLCompany))}
        assert "Alice" in names
        assert "Bob" not in names

    # ── Concrete-class narrowing ────────────────────────────────────

    def test_concrete_class_narrows_to_subclass(self):
        """HLPerson.has(HLName) must return only HLPerson, not HLCompany."""
        results = HLPerson.has(self.db, HLName)
        assert len(results) == 2  # Alice + Bob, NOT Acme/Globex
        names: set[str] = set()
        for r in results:
            assert isinstance(r, HLPerson)
            names.add(r.name.value)
        assert names == {"Alice", "Bob"}

    def test_concrete_class_excludes_other_entity_types(self):
        """HLPerson.has(HLName, "Acme") must return [] — Acme is HLCompany."""
        results = HLPerson.has(self.db, HLName, "Acme")
        assert results == []

    def test_concrete_class_narrows_with_attribute_unique_to_one_type(self):
        """HLCompany.has(HLRegion) returns all companies with a region."""
        results = HLCompany.has(self.db, HLRegion)
        assert len(results) == 2
        for r in results:
            assert isinstance(r, HLCompany)

    def test_base_entity_has_remains_cross_type(self):
        """Regression guard: Entity.has stays cross-type after narrowing lands."""
        results = Entity.has(self.db, HLName)
        assert len(results) == 4  # 2 HLPerson + 2 HLCompany
        types = {type(r).__name__ for r in results}
        assert types == {"HLPerson", "HLCompany"}


@pytest.mark.integration
class TestRelationHas:
    @pytest.fixture(autouse=True)
    def setup(self, clean_db):
        self.db = clean_db
        sm = SchemaManager(clean_db)
        sm.register(HLPerson)
        sm.register(HLCompany)
        sm.register(HLEmployment)
        sm.sync_schema(force=True)

        # Insert entities
        HLPerson.manager(clean_db).insert(HLPerson(name=HLName("Alice")))
        HLCompany.manager(clean_db).insert(HLCompany(name=HLName("Acme"), region=HLRegion("US")))

        # Insert relation
        alice = HLPerson.manager(clean_db).get(name="Alice")[0]
        acme = HLCompany.manager(clean_db).get(name="Acme")[0]
        HLEmployment.manager(clean_db).insert(
            HLEmployment(
                employee=alice,
                employer=acme,
                name=HLName("Engineer"),
                region=HLRegion("US"),
                salary=HLSalary(120000),
            ),
        )

    def test_relation_has_exact_match(self):
        """Relation.has with exact value returns only matching relations."""
        results = Relation.has(self.db, HLName, "Engineer")
        assert len(results) == 1
        emp = results[0]
        assert isinstance(emp, HLEmployment)
        assert emp.name is not None
        assert emp.name.value == "Engineer"

    def test_relation_has_no_value_returns_all(self):
        """Relation.has with no value returns all relations owning that attr."""
        results = Relation.has(self.db, HLName)
        assert len(results) == 1
        assert isinstance(results[0], HLEmployment)

    def test_relation_has_shared_attr_only_relations(self):
        """Relation.has with Region returns only relations, not entities."""
        results = Relation.has(self.db, HLRegion)
        assert len(results) == 1
        assert isinstance(results[0], HLEmployment)

    def test_relation_has_returns_iid(self):
        """Returned relation instances have _iid set."""
        results = Relation.has(self.db, HLName, "Engineer")
        assert len(results) == 1
        assert results[0]._iid is not None

    def test_relation_has_integer_attr(self):
        """Relation.has with integer attribute works."""
        results = Relation.has(self.db, HLSalary, 120000)
        assert len(results) == 1
        assert isinstance(results[0], HLEmployment)

    def test_relation_has_comparison_expression(self):
        """Relation.has with comparison expression on integer attr."""
        results = Relation.has(self.db, HLSalary, HLSalary.gt(HLSalary(100000)))
        assert len(results) == 1
        emp = results[0]
        assert isinstance(emp, HLEmployment)
        assert emp.salary is not None
        assert emp.salary.value == 120000

    def test_relation_has_no_match_returns_empty(self):
        """Relation.has with unmatched value returns empty list."""
        results = Relation.has(self.db, HLName, "NoSuchRelation")
        assert results == []

    # ── Concrete-class narrowing ────────────────────────────────────

    def test_concrete_relation_narrows_to_subclass(self):
        """HLEmployment.has narrows the match to its concrete type."""
        results = HLEmployment.has(self.db, HLName)
        assert len(results) == 1
        assert isinstance(results[0], HLEmployment)

    def test_base_relation_has_remains_cross_type(self):
        """Regression guard: Relation.has stays cross-type after narrowing lands."""
        results = Relation.has(self.db, HLName)
        assert len(results) == 1
        assert isinstance(results[0], HLEmployment)

    # ── Role-player hydration (Option B) ────────────────────────────

    def test_concrete_relation_hydrates_role_players(self):
        """HLEmployment.has must return relations with role players populated."""
        results = HLEmployment.has(self.db, HLName, "Engineer")
        assert len(results) == 1
        emp = results[0]
        assert isinstance(emp, HLEmployment)
        assert emp.employee is not None
        assert isinstance(emp.employee, HLPerson)
        assert emp.employee.name.value == "Alice"
        assert emp.employer is not None
        assert isinstance(emp.employer, HLCompany)
        assert emp.employer.name.value == "Acme"

    def test_cross_type_relation_hydrates_role_players(self):
        """Relation.has (cross-type) must also hydrate role players (Option B)."""
        results = Relation.has(self.db, HLName)
        assert len(results) == 1
        emp = results[0]
        assert isinstance(emp, HLEmployment)
        assert emp.employee is not None
        assert emp.employee.name.value == "Alice"
        assert emp.employer is not None
        assert emp.employer.name.value == "Acme"

    def test_relation_has_returns_iid_after_hydration(self):
        """Regression guard: switching to manager.get must still set _iid."""
        results = Relation.has(self.db, HLName)
        assert len(results) == 1
        assert results[0]._iid is not None
