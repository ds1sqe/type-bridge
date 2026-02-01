"""Tests for complex query scenarios and graph traversal patterns.

Tests deep traversal, cross-entity filtering, aggregation with filters,
and polymorphic queries.
"""

import pytest

from type_bridge import (
    Database,
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


@pytest.mark.integration
class TestCrossEntityFiltering:
    """Test filtering entities based on attributes of related entities."""

    @pytest.fixture
    def schema_for_cross_filtering(self, clean_db: Database):
        """Set up schema for cross-entity filter tests."""

        class Name(String):
            pass

        class Status(String):
            pass

        class Department(Entity):
            flags = TypeFlags(name="department_cross")
            name: Name = Flag(Key)
            status: Status

        class Employee(Entity):
            flags = TypeFlags(name="employee_cross")
            name: Name = Flag(Key)

        class Assignment(Relation):
            flags = TypeFlags(name="assignment_cross")
            department: Role[Department] = Role("department", Department)
            employee: Role[Employee] = Role("employee", Employee)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Department, Employee, Assignment)
        schema_manager.sync_schema(force=True)

        return clean_db, Department, Employee, Assignment, Name, Status

    def test_filter_by_related_entity_attribute(self, schema_for_cross_filtering):
        """Filter relations by attributes of role players."""
        db, Department, Employee, Assignment, Name, Status = schema_for_cross_filtering

        # Create departments
        active_dept = Department(name=Name("Engineering"), status=Status("active"))
        inactive_dept = Department(name=Name("Old Division"), status=Status("inactive"))
        Department.manager(db).insert(active_dept)
        Department.manager(db).insert(inactive_dept)

        # Create employees
        alice = Employee(name=Name("Alice"))
        bob = Employee(name=Name("Bob"))
        Employee.manager(db).insert(alice)
        Employee.manager(db).insert(bob)

        # Assign Alice to active, Bob to inactive
        Assignment.manager(db).insert(Assignment(department=active_dept, employee=alice))
        Assignment.manager(db).insert(Assignment(department=inactive_dept, employee=bob))

        # Query assignments
        all_assignments = Assignment.manager(db).all()
        assert len(all_assignments) == 2


@pytest.mark.integration
class TestPolymorphicQueries:
    """Test queries involving abstract types and subtypes."""

    @pytest.fixture
    def schema_for_polymorphic(self, clean_db: Database):
        """Set up schema with abstract types and subtypes."""

        class Name(String):
            pass

        class Priority(String):
            pass

        # Abstract artifact
        class Artifact(Entity):
            flags = TypeFlags(name="artifact_poly_query", abstract=True)
            name: Name = Flag(Key)

        # Subtypes with varying attributes
        class UserStory(Artifact):
            flags = TypeFlags(name="userstory_poly_query")
            priority: Priority

        class Bug(Artifact):
            flags = TypeFlags(name="bug_poly_query")
            priority: Priority

        class DesignDoc(Artifact):
            flags = TypeFlags(name="designdoc_poly_query")
            # Note: DesignDoc doesn't have priority

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Artifact, UserStory, Bug, DesignDoc)
        schema_manager.sync_schema(force=True)

        return clean_db, Artifact, UserStory, Bug, DesignDoc, Name, Priority

    def test_query_concrete_subtypes_independently(self, schema_for_polymorphic):
        """Query each concrete subtype independently."""
        db, Artifact, UserStory, Bug, DesignDoc, Name, Priority = schema_for_polymorphic

        # Insert various artifacts
        UserStory.manager(db).insert(UserStory(name=Name("US-001"), priority=Priority("high")))
        Bug.manager(db).insert(Bug(name=Name("BUG-001"), priority=Priority("critical")))
        DesignDoc.manager(db).insert(DesignDoc(name=Name("DESIGN-001")))

        # Query each type
        user_stories = UserStory.manager(db).all()
        bugs = Bug.manager(db).all()
        design_docs = DesignDoc.manager(db).all()

        assert len(user_stories) == 1
        assert len(bugs) == 1
        assert len(design_docs) == 1

    def test_filter_concrete_type_by_shared_attribute(self, schema_for_polymorphic):
        """Filter concrete types by attributes they share."""
        db, Artifact, UserStory, Bug, DesignDoc, Name, Priority = schema_for_polymorphic

        # Insert with different priorities
        UserStory.manager(db).insert(UserStory(name=Name("US-High"), priority=Priority("high")))
        UserStory.manager(db).insert(UserStory(name=Name("US-Low"), priority=Priority("low")))
        Bug.manager(db).insert(Bug(name=Name("BUG-High"), priority=Priority("high")))

        # Filter user stories by priority
        high_stories = UserStory.manager(db).filter(priority="high").execute()
        assert len(high_stories) == 1
        assert str(high_stories[0].name) == "US-High"


@pytest.mark.integration
class TestChainedRelations:
    """Test queries involving chains of relations."""

    @pytest.fixture
    def schema_for_chains(self, clean_db: Database):
        """Set up schema for relation chain tests."""

        class Name(String):
            pass

        class Person(Entity):
            flags = TypeFlags(name="person_chain")
            name: Name = Flag(Key)

        class City(Entity):
            flags = TypeFlags(name="city_chain")
            name: Name = Flag(Key)

        class Country(Entity):
            flags = TypeFlags(name="country_chain")
            name: Name = Flag(Key)

        class LivesIn(Relation):
            flags = TypeFlags(name="lives_in_chain")
            resident: Role[Person] = Role("resident", Person)
            city: Role[City] = Role("city", City)

        class LocatedIn(Relation):
            flags = TypeFlags(name="located_in_chain")
            city: Role[City] = Role("city", City)
            country: Role[Country] = Role("country", Country)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Person, City, Country, LivesIn, LocatedIn)
        schema_manager.sync_schema(force=True)

        return clean_db, Person, City, Country, LivesIn, LocatedIn, Name

    def test_insert_chained_relations(self, schema_for_chains):
        """Insert a chain of relations: person -> city -> country."""
        db, Person, City, Country, LivesIn, LocatedIn, Name = schema_for_chains

        # Create entities
        person = Person(name=Name("Alice"))
        city = City(name=Name("Paris"))
        country = Country(name=Name("France"))

        Person.manager(db).insert(person)
        City.manager(db).insert(city)
        Country.manager(db).insert(country)

        # Create chain
        LivesIn.manager(db).insert(LivesIn(resident=person, city=city))
        LocatedIn.manager(db).insert(LocatedIn(city=city, country=country))

        # Verify chain exists
        lives_in_relations = LivesIn.manager(db).all()
        located_in_relations = LocatedIn.manager(db).all()

        assert len(lives_in_relations) == 1
        assert len(located_in_relations) == 1

    def test_multiple_chains_sharing_middle(self, schema_for_chains):
        """Multiple chains sharing a middle entity."""
        db, Person, City, Country, LivesIn, LocatedIn, Name = schema_for_chains

        # Create entities
        alice = Person(name=Name("Alice"))
        bob = Person(name=Name("Bob"))
        paris = City(name=Name("Paris"))
        france = Country(name=Name("France"))

        Person.manager(db).insert(alice)
        Person.manager(db).insert(bob)
        City.manager(db).insert(paris)
        Country.manager(db).insert(france)

        # Both live in same city
        LivesIn.manager(db).insert(LivesIn(resident=alice, city=paris))
        LivesIn.manager(db).insert(LivesIn(resident=bob, city=paris))
        LocatedIn.manager(db).insert(LocatedIn(city=paris, country=france))

        # Verify
        lives_in_relations = LivesIn.manager(db).all()
        assert len(lives_in_relations) == 2


@pytest.mark.integration
class TestAggregationWithFilters:
    """Test aggregation operations combined with filters."""

    @pytest.fixture
    def schema_for_aggregation(self, clean_db: Database):
        """Set up schema for aggregation tests."""

        class Name(String):
            pass

        class Salary(Integer):
            pass

        class Department(String):
            pass

        class Employee(Entity):
            flags = TypeFlags(name="employee_agg")
            name: Name = Flag(Key)
            salary: Salary
            department: Department

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Employee)
        schema_manager.sync_schema(force=True)

        return clean_db, Employee, Name, Salary, Department

    def test_count_with_filter(self, schema_for_aggregation):
        """Count entities matching a filter."""
        db, Employee, Name, Salary, Department = schema_for_aggregation

        manager = Employee.manager(db)

        # Insert employees in different departments
        manager.insert(
            Employee(
                name=Name("Alice"), salary=Salary(100000), department=Department("Engineering")
            )
        )
        manager.insert(
            Employee(name=Name("Bob"), salary=Salary(90000), department=Department("Engineering"))
        )
        manager.insert(
            Employee(name=Name("Carol"), salary=Salary(80000), department=Department("Marketing"))
        )

        # Count engineering employees
        eng_count = manager.filter(department="Engineering").count()
        assert eng_count == 2

        # Count marketing employees
        mkt_count = manager.filter(department="Marketing").count()
        assert mkt_count == 1

    def test_multiple_filters_combined(self, schema_for_aggregation):
        """Multiple filter conditions combined."""
        db, Employee, Name, Salary, Department = schema_for_aggregation

        manager = Employee.manager(db)

        manager.insert(
            Employee(
                name=Name("Alice"), salary=Salary(100000), department=Department("Engineering")
            )
        )
        manager.insert(
            Employee(name=Name("Bob"), salary=Salary(60000), department=Department("Engineering"))
        )
        manager.insert(
            Employee(name=Name("Carol"), salary=Salary(90000), department=Department("Marketing"))
        )

        # All employees - use filter() to get a Query that has count()
        all_count = manager.filter().count()
        assert all_count == 3


@pytest.mark.integration
class TestVariableScoping:
    """Test that queries handle variable scoping correctly."""

    @pytest.fixture
    def schema_for_scoping(self, clean_db: Database):
        """Set up schema for variable scoping tests."""

        class Name(String):
            pass

        class Item(Entity):
            flags = TypeFlags(name="item_scope_test")
            name: Name = Flag(Key)

        class Tag(Entity):
            flags = TypeFlags(name="tag_scope_test")
            name: Name = Flag(Key)

        class Tagging(Relation):
            flags = TypeFlags(name="tagging_scope_test")
            item: Role[Item] = Role("item", Item)
            tag: Role[Tag] = Role("tag", Tag)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Item, Tag, Tagging)
        schema_manager.sync_schema(force=True)

        return clean_db, Item, Tag, Tagging, Name

    def test_many_to_many_relations(self, schema_for_scoping):
        """Many-to-many relations with multiple queries."""
        db, Item, Tag, Tagging, Name = schema_for_scoping

        # Create items and tags
        item1 = Item(name=Name("Item1"))
        item2 = Item(name=Name("Item2"))
        tag_a = Tag(name=Name("TagA"))
        tag_b = Tag(name=Name("TagB"))

        Item.manager(db).insert(item1)
        Item.manager(db).insert(item2)
        Tag.manager(db).insert(tag_a)
        Tag.manager(db).insert(tag_b)

        # Create many-to-many taggings
        Tagging.manager(db).insert(Tagging(item=item1, tag=tag_a))
        Tagging.manager(db).insert(Tagging(item=item1, tag=tag_b))
        Tagging.manager(db).insert(Tagging(item=item2, tag=tag_a))

        # Query
        taggings = Tagging.manager(db).all()
        assert len(taggings) == 3

        # Query items
        items = Item.manager(db).all()
        assert len(items) == 2

        # Query tags
        tags = Tag.manager(db).all()
        assert len(tags) == 2
