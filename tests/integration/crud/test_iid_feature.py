"""Integration tests for IID (Internal ID) feature.

Tests for issue #62: Expose TypeDB IID on entity instances.
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


class PersonName(String):
    pass


class PersonAge(Integer):
    pass


class CompanyName(String):
    pass


class Position(String):
    pass


class IidPerson(Entity):
    flags = TypeFlags(name="iid_person")
    name: PersonName = Flag(Key)
    age: PersonAge | None = None


class IidCompany(Entity):
    flags = TypeFlags(name="iid_company")
    name: CompanyName = Flag(Key)


class IidEmployment(Relation):
    flags = TypeFlags(name="iid_employment")
    employee: Role[IidPerson] = Role("employee", IidPerson)
    employer: Role[IidCompany] = Role("employer", IidCompany)
    position: Position | None = None


@pytest.mark.integration
class TestEntityIidPopulation:
    """Tests for IID population on entities."""

    @pytest.fixture(autouse=True)
    def setup_schema(self, clean_db):
        """Setup schema for each test."""
        self.db = clean_db
        schema_manager = SchemaManager(clean_db)
        schema_manager.register(IidPerson)
        schema_manager.register(IidCompany)
        schema_manager.register(IidEmployment)
        schema_manager.sync_schema(force=True)

    def test_get_populates_iid(self):
        """Test that get() populates _iid on returned entities."""
        manager = IidPerson.manager(self.db)

        # Insert a person
        person = IidPerson(name=PersonName("Alice"), age=PersonAge(30))
        manager.insert(person)

        # Fetch the person
        fetched = manager.get(name="Alice")
        assert len(fetched) == 1

        # Verify IID is populated
        fetched_person = fetched[0]
        assert fetched_person._iid is not None
        assert fetched_person._iid.startswith("0x")

    def test_filter_execute_populates_iid(self):
        """Test that filter().execute() populates _iid on returned entities."""
        manager = IidPerson.manager(self.db)

        # Insert a person
        person = IidPerson(name=PersonName("Bob"), age=PersonAge(25))
        manager.insert(person)

        # Fetch using filter
        fetched = manager.filter(name=PersonName("Bob")).execute()
        assert len(fetched) == 1

        # Verify IID is populated
        fetched_person = fetched[0]
        assert fetched_person._iid is not None
        assert fetched_person._iid.startswith("0x")

    def test_iid_is_stable_across_queries(self):
        """Test that the same entity returns the same IID across queries."""
        manager = IidPerson.manager(self.db)

        # Insert a person
        person = IidPerson(name=PersonName("Charlie"), age=PersonAge(35))
        manager.insert(person)

        # Fetch twice
        first_fetch = manager.get(name="Charlie")[0]
        second_fetch = manager.get(name="Charlie")[0]

        # IIDs should match
        assert first_fetch._iid is not None
        assert second_fetch._iid is not None
        assert first_fetch._iid == second_fetch._iid


@pytest.mark.integration
class TestGetByIid:
    """Tests for get_by_iid method."""

    @pytest.fixture(autouse=True)
    def setup_schema(self, clean_db):
        """Setup schema for each test."""
        self.db = clean_db
        schema_manager = SchemaManager(clean_db)
        schema_manager.register(IidPerson)
        schema_manager.register(IidCompany)
        schema_manager.register(IidEmployment)
        schema_manager.sync_schema(force=True)

    def test_get_by_iid_returns_entity(self):
        """Test that get_by_iid returns the correct entity."""
        manager = IidPerson.manager(self.db)

        # Insert a person
        person = IidPerson(name=PersonName("David"), age=PersonAge(40))
        manager.insert(person)

        # Get the IID
        fetched = manager.get(name="David")[0]
        iid = fetched._iid
        assert iid is not None

        # Fetch by IID
        found = manager.get_by_iid(iid)
        assert found is not None
        assert found.name.value == "David"
        assert found.age is not None
        assert found.age.value == 40
        assert found._iid == iid

    def test_get_by_iid_returns_none_for_nonexistent(self):
        """Test that get_by_iid returns None for non-existent IID."""
        manager = IidPerson.manager(self.db)

        # Try to fetch with a fake IID
        result = manager.get_by_iid("0xdeadbeefdeadbeefdeadbeef")
        assert result is None

    def test_get_by_iid_returns_none_for_invalid_format(self):
        """Test that get_by_iid returns None for invalid IID format (graceful handling)."""
        manager = IidPerson.manager(self.db)

        # Invalid IIDs are treated as "not found" (graceful handling)
        result = manager.get_by_iid("not-a-valid-iid")
        assert result is None


@pytest.mark.integration
class TestRelationIidPopulation:
    """Tests for IID population on relations."""

    @pytest.fixture(autouse=True)
    def setup_schema(self, clean_db):
        """Setup schema for each test."""
        self.db = clean_db
        schema_manager = SchemaManager(clean_db)
        schema_manager.register(IidPerson)
        schema_manager.register(IidCompany)
        schema_manager.register(IidEmployment)
        schema_manager.sync_schema(force=True)

    def test_relation_get_populates_iid(self):
        """Test that get() populates _iid on returned relations."""
        # Insert entities
        person = IidPerson(name=PersonName("Eve"), age=PersonAge(28))
        IidPerson.manager(self.db).insert(person)

        company = IidCompany(name=CompanyName("TechCorp"))
        IidCompany.manager(self.db).insert(company)

        # Insert relation
        employment = IidEmployment(employee=person, employer=company, position=Position("Engineer"))
        IidEmployment.manager(self.db).insert(employment)

        # Fetch the relation
        fetched = IidEmployment.manager(self.db).get()
        assert len(fetched) == 1

        # Verify IID is populated
        fetched_emp = fetched[0]
        assert fetched_emp._iid is not None
        assert fetched_emp._iid.startswith("0x")

    def test_relation_get_by_iid(self):
        """Test that get_by_iid works for relations."""
        # Insert entities
        person = IidPerson(name=PersonName("Frank"), age=PersonAge(32))
        IidPerson.manager(self.db).insert(person)

        company = IidCompany(name=CompanyName("BigCorp"))
        IidCompany.manager(self.db).insert(company)

        # Insert relation
        employment = IidEmployment(employee=person, employer=company, position=Position("Manager"))
        IidEmployment.manager(self.db).insert(employment)

        # Get the IID
        fetched = IidEmployment.manager(self.db).get()[0]
        iid = fetched._iid
        assert iid is not None

        # Fetch by IID
        found = IidEmployment.manager(self.db).get_by_iid(iid)
        assert found is not None
        assert found.position is not None
        assert found.position.value == "Manager"
        assert found._iid == iid

    def test_relation_get_populates_role_player_iids(self):
        """Test that get() populates _iid on role player entities (issue #68)."""
        # Insert entities
        person = IidPerson(name=PersonName("Grace"), age=PersonAge(29))
        IidPerson.manager(self.db).insert(person)

        company = IidCompany(name=CompanyName("StartupCo"))
        IidCompany.manager(self.db).insert(company)

        # Get entity IIDs for comparison
        person_iid = IidPerson.manager(self.db).get(name="Grace")[0]._iid
        company_iid = IidCompany.manager(self.db).get(name="StartupCo")[0]._iid
        assert person_iid is not None
        assert company_iid is not None

        # Insert relation
        employment = IidEmployment(employee=person, employer=company, position=Position("Founder"))
        IidEmployment.manager(self.db).insert(employment)

        # Fetch the relation
        fetched = IidEmployment.manager(self.db).get()
        assert len(fetched) == 1

        # Verify relation IID is populated
        fetched_emp = fetched[0]
        assert fetched_emp._iid is not None
        assert fetched_emp._iid.startswith("0x")

        # Verify role player IIDs are populated (issue #68)
        assert fetched_emp.employee is not None
        assert fetched_emp.employee._iid is not None
        assert fetched_emp.employee._iid.startswith("0x")
        assert fetched_emp.employee._iid == person_iid

        assert fetched_emp.employer is not None
        assert fetched_emp.employer._iid is not None
        assert fetched_emp.employer._iid.startswith("0x")
        assert fetched_emp.employer._iid == company_iid

    def test_relation_filter_execute_populates_role_player_iids(self):
        """Test that filter().execute() populates _iid on role player entities (issue #68)."""
        # Insert entities
        person = IidPerson(name=PersonName("Henry"), age=PersonAge(45))
        IidPerson.manager(self.db).insert(person)

        company = IidCompany(name=CompanyName("MegaCorp"))
        IidCompany.manager(self.db).insert(company)

        # Get entity IIDs for comparison
        person_iid = IidPerson.manager(self.db).get(name="Henry")[0]._iid
        company_iid = IidCompany.manager(self.db).get(name="MegaCorp")[0]._iid
        assert person_iid is not None
        assert company_iid is not None

        # Insert relation
        employment = IidEmployment(employee=person, employer=company, position=Position("CEO"))
        IidEmployment.manager(self.db).insert(employment)

        # Fetch using filter
        fetched = IidEmployment.manager(self.db).filter(position=Position("CEO")).execute()
        assert len(fetched) == 1

        # Verify relation IID is populated
        fetched_emp = fetched[0]
        assert fetched_emp._iid is not None
        assert fetched_emp._iid.startswith("0x")

        # Verify role player IIDs are populated (issue #68)
        assert fetched_emp.employee is not None
        assert fetched_emp.employee._iid is not None
        assert fetched_emp.employee._iid == person_iid

        assert fetched_emp.employer is not None
        assert fetched_emp.employer._iid is not None
        assert fetched_emp.employer._iid == company_iid

    def test_relation_all_populates_role_player_iids(self):
        """Test that all() populates _iid on role player entities (issue #68)."""
        # Insert entities
        person = IidPerson(name=PersonName("Iris"), age=PersonAge(33))
        IidPerson.manager(self.db).insert(person)

        company = IidCompany(name=CompanyName("GiantCorp"))
        IidCompany.manager(self.db).insert(company)

        # Get entity IIDs for comparison
        person_iid = IidPerson.manager(self.db).get(name="Iris")[0]._iid
        company_iid = IidCompany.manager(self.db).get(name="GiantCorp")[0]._iid
        assert person_iid is not None
        assert company_iid is not None

        # Insert relation
        employment = IidEmployment(employee=person, employer=company, position=Position("CTO"))
        IidEmployment.manager(self.db).insert(employment)

        # Fetch all relations
        all_relations = IidEmployment.manager(self.db).all()

        # Find our relation
        fetched_emp = next(r for r in all_relations if r.position and r.position.value == "CTO")
        assert fetched_emp is not None

        # Verify relation IID is populated
        assert fetched_emp._iid is not None
        assert fetched_emp._iid.startswith("0x")

        # Verify role player IIDs are populated (issue #68)
        assert fetched_emp.employee is not None
        assert fetched_emp.employee._iid is not None
        assert fetched_emp.employee._iid == person_iid

        assert fetched_emp.employer is not None
        assert fetched_emp.employer._iid is not None
        assert fetched_emp.employer._iid == company_iid


@pytest.mark.integration
class TestRelationAllIidCorrectness:
    """Tests for issue #78: RelationManager.all() assigns incorrect IIDs to role players.

    When using RelationManager.all() to fetch multiple relations, each relation
    should have unique, correct IIDs for its role players - not the same IIDs
    from the first matched result.
    """

    @pytest.fixture(autouse=True)
    def setup_schema(self, clean_db):
        """Setup schema for each test."""
        self.db = clean_db
        schema_manager = SchemaManager(clean_db)
        schema_manager.register(IidPerson)
        schema_manager.register(IidCompany)
        schema_manager.register(IidEmployment)
        schema_manager.sync_schema(force=True)

    def test_all_returns_unique_role_player_iids_issue_78(self):
        """Test that all() returns unique IIDs for each relation's role players.

        Regression test for issue #78: Previously, all relations returned by all()
        would have the same IIDs for their role players (from the first result).
        """
        # Create two different people
        person_a = IidPerson(name=PersonName("PersonA"), age=PersonAge(25))
        person_b = IidPerson(name=PersonName("PersonB"), age=PersonAge(30))
        IidPerson.manager(self.db).insert(person_a)
        IidPerson.manager(self.db).insert(person_b)

        # Create two different companies
        company_x = IidCompany(name=CompanyName("CompanyX"))
        company_y = IidCompany(name=CompanyName("CompanyY"))
        IidCompany.manager(self.db).insert(company_x)
        IidCompany.manager(self.db).insert(company_y)

        # Get the actual IIDs for each entity from the database
        person_a_iid = IidPerson.manager(self.db).get(name="PersonA")[0]._iid
        person_b_iid = IidPerson.manager(self.db).get(name="PersonB")[0]._iid
        company_x_iid = IidCompany.manager(self.db).get(name="CompanyX")[0]._iid
        company_y_iid = IidCompany.manager(self.db).get(name="CompanyY")[0]._iid

        # Verify all IIDs are unique
        all_entity_iids = {person_a_iid, person_b_iid, company_x_iid, company_y_iid}
        assert len(all_entity_iids) == 4, "All entities should have unique IIDs"

        # Create two employments with different role players
        emp1 = IidEmployment(employee=person_a, employer=company_x, position=Position("Role1"))
        emp2 = IidEmployment(employee=person_b, employer=company_y, position=Position("Role2"))
        IidEmployment.manager(self.db).insert(emp1)
        IidEmployment.manager(self.db).insert(emp2)

        # Fetch all relations using all()
        all_relations = list(IidEmployment.manager(self.db).all())
        assert len(all_relations) == 2

        # Find each relation by position
        rel1 = next(r for r in all_relations if r.position and r.position.value == "Role1")
        rel2 = next(r for r in all_relations if r.position and r.position.value == "Role2")

        # Verify each relation has the correct IIDs for its role players
        # This is the key assertion for issue #78 - previously both relations
        # would have the same IIDs (from the first matched result)

        # Relation 1 should have PersonA and CompanyX
        assert rel1.employee is not None
        assert rel1.employee._iid == person_a_iid, (
            f"Expected PersonA IID {person_a_iid}, got {rel1.employee._iid}"
        )
        assert rel1.employer is not None
        assert rel1.employer._iid == company_x_iid, (
            f"Expected CompanyX IID {company_x_iid}, got {rel1.employer._iid}"
        )

        # Relation 2 should have PersonB and CompanyY
        assert rel2.employee is not None
        assert rel2.employee._iid == person_b_iid, (
            f"Expected PersonB IID {person_b_iid}, got {rel2.employee._iid}"
        )
        assert rel2.employer is not None
        assert rel2.employer._iid == company_y_iid, (
            f"Expected CompanyY IID {company_y_iid}, got {rel2.employer._iid}"
        )

        # Also verify that the relation IIDs themselves are unique
        assert rel1._iid is not None
        assert rel2._iid is not None
        assert rel1._iid != rel2._iid, "Each relation should have a unique IID"

    def test_get_returns_unique_role_player_iids_issue_78(self):
        """Test that get() also returns unique IIDs for each relation's role players.

        Similar to test_all but using get() without filters.
        """
        # Create people and companies
        person_c = IidPerson(name=PersonName("PersonC"), age=PersonAge(35))
        person_d = IidPerson(name=PersonName("PersonD"), age=PersonAge(40))
        IidPerson.manager(self.db).insert(person_c)
        IidPerson.manager(self.db).insert(person_d)

        company_z = IidCompany(name=CompanyName("CompanyZ"))
        company_w = IidCompany(name=CompanyName("CompanyW"))
        IidCompany.manager(self.db).insert(company_z)
        IidCompany.manager(self.db).insert(company_w)

        # Get entity IIDs
        person_c_iid = IidPerson.manager(self.db).get(name="PersonC")[0]._iid
        person_d_iid = IidPerson.manager(self.db).get(name="PersonD")[0]._iid
        company_z_iid = IidCompany.manager(self.db).get(name="CompanyZ")[0]._iid
        company_w_iid = IidCompany.manager(self.db).get(name="CompanyW")[0]._iid

        # Create employments
        emp3 = IidEmployment(employee=person_c, employer=company_z, position=Position("Role3"))
        emp4 = IidEmployment(employee=person_d, employer=company_w, position=Position("Role4"))
        IidEmployment.manager(self.db).insert(emp3)
        IidEmployment.manager(self.db).insert(emp4)

        # Fetch using get() with no filters
        all_relations = IidEmployment.manager(self.db).get()
        assert len(all_relations) == 2

        # Find each relation
        rel3 = next(r for r in all_relations if r.position and r.position.value == "Role3")
        rel4 = next(r for r in all_relations if r.position and r.position.value == "Role4")

        # Verify correct IIDs
        assert rel3.employee._iid == person_c_iid
        assert rel3.employer._iid == company_z_iid
        assert rel4.employee._iid == person_d_iid
        assert rel4.employer._iid == company_w_iid


@pytest.mark.integration
class TestIidInLookup:
    """Tests for iid__in lookup filter (issue #80)."""

    @pytest.fixture(autouse=True)
    def setup_schema(self, clean_db):
        """Setup schema for each test."""
        self.db = clean_db
        schema_manager = SchemaManager(clean_db)
        schema_manager.register(IidPerson)
        schema_manager.register(IidCompany)
        schema_manager.register(IidEmployment)
        schema_manager.sync_schema(force=True)

    def test_entity_filter_iid_in_multiple(self):
        """Test filter(iid__in=[...]) with multiple IIDs returns correct entities."""
        manager = IidPerson.manager(self.db)

        # Insert three persons
        alice = IidPerson(name=PersonName("Alice_iid_in"), age=PersonAge(30))
        bob = IidPerson(name=PersonName("Bob_iid_in"), age=PersonAge(25))
        charlie = IidPerson(name=PersonName("Charlie_iid_in"), age=PersonAge(35))
        manager.insert(alice)
        manager.insert(bob)
        manager.insert(charlie)

        # Get IIDs
        all_persons = manager.all()
        alice_iid = next(p._iid for p in all_persons if p.name.value == "Alice_iid_in")
        bob_iid = next(p._iid for p in all_persons if p.name.value == "Bob_iid_in")

        # Filter by two IIDs
        result = manager.filter(iid__in=[alice_iid, bob_iid]).execute()

        # Should return exactly 2 persons
        assert len(result) == 2
        names = {p.name.value for p in result}
        assert names == {"Alice_iid_in", "Bob_iid_in"}

    def test_entity_filter_iid_in_single(self):
        """Test filter(iid__in=[single]) with one IID returns correct entity."""
        manager = IidPerson.manager(self.db)

        # Insert person
        person = IidPerson(name=PersonName("Dave_iid_in"), age=PersonAge(40))
        manager.insert(person)

        # Get IID
        dave_iid = manager.get(name="Dave_iid_in")[0]._iid

        # Filter by single IID
        result = manager.filter(iid__in=[dave_iid]).execute()

        assert len(result) == 1
        assert result[0].name.value == "Dave_iid_in"
        assert result[0]._iid == dave_iid

    def test_entity_filter_iid_in_with_other_filters(self):
        """Test iid__in combined with other filters."""
        manager = IidPerson.manager(self.db)

        # Insert persons
        eve = IidPerson(name=PersonName("Eve_iid_in"), age=PersonAge(28))
        frank = IidPerson(name=PersonName("Frank_iid_in"), age=PersonAge(32))
        grace = IidPerson(name=PersonName("Grace_iid_in"), age=PersonAge(25))
        manager.insert(eve)
        manager.insert(frank)
        manager.insert(grace)

        # Get all IIDs
        all_persons = manager.all()
        iids = [p._iid for p in all_persons if "_iid_in" in p.name.value]

        # Filter by IIDs AND age > 27
        result = manager.filter(iid__in=iids, age__gt=27).execute()

        # Only Eve (28) and Frank (32) match
        assert len(result) == 2
        names = {p.name.value for p in result}
        assert names == {"Eve_iid_in", "Frank_iid_in"}

    def test_relation_filter_iid_in(self):
        """Test filter(iid__in=[...]) on relations."""
        person_mgr = IidPerson.manager(self.db)
        company_mgr = IidCompany.manager(self.db)
        emp_mgr = IidEmployment.manager(self.db)

        # Insert entities
        person = IidPerson(name=PersonName("Helen_iid_in"), age=PersonAge(35))
        person_mgr.insert(person)

        company1 = IidCompany(name=CompanyName("Corp1_iid_in"))
        company2 = IidCompany(name=CompanyName("Corp2_iid_in"))
        company_mgr.insert(company1)
        company_mgr.insert(company2)

        # Insert two relations
        emp1 = IidEmployment(employee=person, employer=company1, position=Position("Eng1"))
        emp2 = IidEmployment(employee=person, employer=company2, position=Position("Eng2"))
        emp_mgr.insert(emp1)
        emp_mgr.insert(emp2)

        # Get IIDs
        all_emps = emp_mgr.all()
        emp1_iid = next(e._iid for e in all_emps if e.position and e.position.value == "Eng1")

        # Filter by single relation IID
        result = emp_mgr.filter(iid__in=[emp1_iid]).execute()

        assert len(result) == 1
        assert result[0].position is not None
        assert result[0].position.value == "Eng1"

    def test_relation_filter_role_iid_in(self):
        """Test filter(role__iid__in=[...]) to filter by role player IIDs."""
        person_mgr = IidPerson.manager(self.db)
        company_mgr = IidCompany.manager(self.db)
        emp_mgr = IidEmployment.manager(self.db)

        # Insert entities
        p1 = IidPerson(name=PersonName("Ivan_iid_in"), age=PersonAge(30))
        p2 = IidPerson(name=PersonName("Jane_iid_in"), age=PersonAge(28))
        person_mgr.insert(p1)
        person_mgr.insert(p2)

        company = IidCompany(name=CompanyName("BigTech_iid_in"))
        company_mgr.insert(company)

        # Insert relations for both persons
        emp1 = IidEmployment(employee=p1, employer=company, position=Position("Dev"))
        emp2 = IidEmployment(employee=p2, employer=company, position=Position("QA"))
        emp_mgr.insert(emp1)
        emp_mgr.insert(emp2)

        # Get Ivan's IID
        ivan_iid = person_mgr.get(name="Ivan_iid_in")[0]._iid

        # Filter relations where employee IID matches Ivan
        result = emp_mgr.filter(employee__iid__in=[ivan_iid]).execute()

        assert len(result) == 1
        assert result[0].position is not None
        assert result[0].position.value == "Dev"
        assert result[0].employee.name.value == "Ivan_iid_in"

    def test_relation_filter_role_iid_in_multiple(self):
        """Test filter(role__iid__in=[...]) with multiple role player IIDs."""
        person_mgr = IidPerson.manager(self.db)
        company_mgr = IidCompany.manager(self.db)
        emp_mgr = IidEmployment.manager(self.db)

        # Insert entities
        p1 = IidPerson(name=PersonName("Kate_iid_in"), age=PersonAge(31))
        p2 = IidPerson(name=PersonName("Leo_iid_in"), age=PersonAge(27))
        p3 = IidPerson(name=PersonName("Mike_iid_in"), age=PersonAge(40))
        person_mgr.insert(p1)
        person_mgr.insert(p2)
        person_mgr.insert(p3)

        company = IidCompany(name=CompanyName("SmallTech_iid_in"))
        company_mgr.insert(company)

        # Insert relations for all three
        emp_mgr.insert(IidEmployment(employee=p1, employer=company, position=Position("Lead")))
        emp_mgr.insert(IidEmployment(employee=p2, employer=company, position=Position("Junior")))
        emp_mgr.insert(IidEmployment(employee=p3, employer=company, position=Position("Senior")))

        # Get Kate and Leo's IIDs
        kate_iid = person_mgr.get(name="Kate_iid_in")[0]._iid
        leo_iid = person_mgr.get(name="Leo_iid_in")[0]._iid

        # Filter relations where employee IID is Kate or Leo
        result = emp_mgr.filter(employee__iid__in=[kate_iid, leo_iid]).execute()

        assert len(result) == 2
        assert all(r.position is not None for r in result)
        positions = {r.position.value for r in result if r.position is not None}
        assert positions == {"Lead", "Junior"}
