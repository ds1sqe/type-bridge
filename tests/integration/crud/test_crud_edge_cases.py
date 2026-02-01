"""Tests for CRUD operation edge cases beyond happy path.

Tests update edge cases, delete edge cases, optional attribute lifecycle,
and multi-value attribute operations.
"""

import pytest

from type_bridge import (
    Card,
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
class TestUpdateEdgeCases:
    """Test update operation edge cases."""

    @pytest.fixture
    def schema_for_updates(self, clean_db: Database):
        """Set up schema for update tests."""

        class Name(String):
            pass

        class Age(Integer):
            pass

        class Bio(String):
            pass

        class Person(Entity):
            flags = TypeFlags(name="person_update_edge")
            name: Name = Flag(Key)
            age: Age | None = None
            bio: Bio | None = None

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Person)
        schema_manager.sync_schema(force=True)

        return clean_db, Person, Name, Age, Bio

    def test_update_optional_from_none_to_value(self, schema_for_updates):
        """Update optional attribute from None to a value."""
        db, Person, Name, Age, Bio = schema_for_updates

        # Insert person without age
        person = Person(name=Name("Alice"))
        manager = Person.manager(db)
        manager.insert(person)

        # Fetch, add age, update
        fetched = manager.get(name="Alice")[0]
        assert fetched.age is None

        fetched.age = Age(30)
        manager.update(fetched)

        # Verify update
        updated = manager.get(name="Alice")[0]
        assert updated.age is not None
        assert int(updated.age) == 30

    def test_update_optional_from_value_to_none(self, schema_for_updates):
        """Update optional attribute from value to None (remove)."""
        db, Person, Name, Age, Bio = schema_for_updates

        # Insert person with age
        person = Person(name=Name("Bob"), age=Age(25))
        manager = Person.manager(db)
        manager.insert(person)

        # Fetch, remove age, update
        fetched = manager.get(name="Bob")[0]
        assert fetched.age is not None

        fetched.age = None
        manager.update(fetched)

        # Verify update
        updated = manager.get(name="Bob")[0]
        assert updated.age is None

    def test_update_changes_value(self, schema_for_updates):
        """Update changes attribute value correctly."""
        db, Person, Name, Age, Bio = schema_for_updates

        # Insert person
        person = Person(name=Name("Carol"), age=Age(30))
        manager = Person.manager(db)
        manager.insert(person)

        # Fetch, change age, update
        fetched = manager.get(name="Carol")[0]
        fetched.age = Age(31)
        manager.update(fetched)

        # Verify
        updated = manager.get(name="Carol")[0]
        assert int(updated.age) == 31

    def test_update_multiple_optional_attrs(self, schema_for_updates):
        """Update multiple optional attributes at once."""
        db, Person, Name, Age, Bio = schema_for_updates

        # Insert person
        person = Person(name=Name("Dave"))
        manager = Person.manager(db)
        manager.insert(person)

        # Update both age and bio
        fetched = manager.get(name="Dave")[0]
        fetched.age = Age(40)
        fetched.bio = Bio("A software developer")
        manager.update(fetched)

        # Verify
        updated = manager.get(name="Dave")[0]
        assert int(updated.age) == 40
        assert str(updated.bio) == "A software developer"


@pytest.mark.integration
class TestDeleteEdgeCases:
    """Test delete operation edge cases."""

    @pytest.fixture
    def schema_for_deletes(self, clean_db: Database):
        """Set up schema for delete tests."""

        class Name(String):
            pass

        class Person(Entity):
            flags = TypeFlags(name="person_delete_edge")
            name: Name = Flag(Key)

        class Company(Entity):
            flags = TypeFlags(name="company_delete_edge")
            name: Name = Flag(Key)

        class Employment(Relation):
            flags = TypeFlags(name="employment_delete_edge")
            employee: Role[Person] = Role("employee", Person)
            employer: Role[Company] = Role("employer", Company)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Person, Company, Employment)
        schema_manager.sync_schema(force=True)

        return clean_db, Person, Company, Employment, Name

    def test_delete_entity_succeeds(self, schema_for_deletes):
        """Delete existing entity succeeds."""
        db, Person, Company, Employment, Name = schema_for_deletes

        person = Person(name=Name("Alice"))
        manager = Person.manager(db)
        manager.insert(person)

        # Verify exists
        assert len(manager.all()) == 1

        # Delete
        fetched = manager.get(name="Alice")[0]
        manager.delete(fetched)

        # Verify deleted
        assert len(manager.all()) == 0

    def test_delete_entity_with_relation_cascades(self, schema_for_deletes):
        """Delete entity that participates in relation also deletes the relation."""
        db, Person, Company, Employment, Name = schema_for_deletes

        # Create person and company
        person = Person(name=Name("Bob"))
        company = Company(name=Name("TechCorp"))
        Person.manager(db).insert(person)
        Company.manager(db).insert(company)

        # Create employment relation
        employment = Employment(employee=person, employer=company)
        Employment.manager(db).insert(employment)

        # Verify relation exists
        assert len(Employment.manager(db).all()) == 1

        # Delete person - TypeDB cascades and removes the relation too
        fetched_person = Person.manager(db).get(name="Bob")[0]
        Person.manager(db).delete(fetched_person)

        # Person is deleted
        assert len(Person.manager(db).get(name="Bob")) == 0

        # Relation is also deleted (cascade)
        assert len(Employment.manager(db).all()) == 0

    def test_delete_relation_then_entity_succeeds(self, schema_for_deletes):
        """Delete relation first, then entity succeeds."""
        db, Person, Company, Employment, Name = schema_for_deletes

        # Create person and company
        person = Person(name=Name("Carol"))
        company = Company(name=Name("DevCorp"))
        Person.manager(db).insert(person)
        Company.manager(db).insert(company)

        # Create employment relation
        employment = Employment(employee=person, employer=company)
        Employment.manager(db).insert(employment)

        # Delete relation first
        fetched_employment = Employment.manager(db).all()[0]
        Employment.manager(db).delete(fetched_employment)

        # Now delete person succeeds
        fetched_person = Person.manager(db).get(name="Carol")[0]
        Person.manager(db).delete(fetched_person)

        # Verify
        assert len(Person.manager(db).all()) == 0


@pytest.mark.integration
class TestOptionalAttributeLifecycle:
    """Test the full lifecycle of optional attributes."""

    @pytest.fixture
    def schema_for_optional(self, clean_db: Database):
        """Set up schema for optional attribute tests."""

        class Name(String):
            pass

        class Nickname(String):
            pass

        class Person(Entity):
            flags = TypeFlags(name="person_optional_lifecycle")
            name: Name = Flag(Key)
            nickname: Nickname | None = None

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Person)
        schema_manager.sync_schema(force=True)

        return clean_db, Person, Name, Nickname

    def test_optional_attribute_lifecycle(self, schema_for_optional):
        """Test full lifecycle: None → value → None."""
        db, Person, Name, Nickname = schema_for_optional

        manager = Person.manager(db)

        # 1. Insert without optional attr
        person = Person(name=Name("Alice"))
        manager.insert(person)

        fetched = manager.get(name="Alice")[0]
        assert fetched.nickname is None

        # 2. Add optional attr
        fetched.nickname = Nickname("Ali")
        manager.update(fetched)

        fetched2 = manager.get(name="Alice")[0]
        assert str(fetched2.nickname) == "Ali"

        # 3. Remove optional attr
        fetched2.nickname = None
        manager.update(fetched2)

        fetched3 = manager.get(name="Alice")[0]
        assert fetched3.nickname is None


@pytest.mark.integration
class TestMultiValueAttributeOperations:
    """Test multi-value attribute operations."""

    @pytest.fixture
    def schema_for_multivalue(self, clean_db: Database):
        """Set up schema for multi-value attribute tests."""

        class Name(String):
            pass

        class Tag(String):
            pass

        class Item(Entity):
            flags = TypeFlags(name="item_multivalue_ops")
            name: Name = Flag(Key)
            # Card(0) means min=0, max=unlimited
            tags: list[Tag] = Flag(Card(0))

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Item)
        schema_manager.sync_schema(force=True)

        return clean_db, Item, Name, Tag

    def test_insert_with_empty_list(self, schema_for_multivalue):
        """Insert entity with empty list for multi-value attribute."""
        db, Item, Name, Tag = schema_for_multivalue

        item = Item(name=Name("Empty Item"), tags=[])
        manager = Item.manager(db)
        manager.insert(item)

        results = manager.all()
        assert len(results) == 1
        # Tags should be empty list or equivalent
        assert len(results[0].tags) == 0

    def test_insert_with_multiple_values(self, schema_for_multivalue):
        """Insert entity with multiple values for multi-value attribute."""
        db, Item, Name, Tag = schema_for_multivalue

        item = Item(name=Name("Tagged Item"), tags=[Tag("a"), Tag("b"), Tag("c")])
        manager = Item.manager(db)
        manager.insert(item)

        results = manager.all()
        assert len(results) == 1
        assert len(results[0].tags) == 3

    def test_update_add_to_list(self, schema_for_multivalue):
        """Update to add items to multi-value attribute."""
        db, Item, Name, Tag = schema_for_multivalue

        # Insert with 2 tags
        item = Item(name=Name("Growing Item"), tags=[Tag("a"), Tag("b")])
        manager = Item.manager(db)
        manager.insert(item)

        # Update to add more tags
        fetched = manager.get(name="Growing Item")[0]
        fetched.tags = [Tag("a"), Tag("b"), Tag("c"), Tag("d")]
        manager.update(fetched)

        # Verify
        updated = manager.get(name="Growing Item")[0]
        assert len(updated.tags) == 4

    def test_update_remove_from_list(self, schema_for_multivalue):
        """Update to remove items from multi-value attribute."""
        db, Item, Name, Tag = schema_for_multivalue

        # Insert with 3 tags
        item = Item(name=Name("Shrinking Item"), tags=[Tag("a"), Tag("b"), Tag("c")])
        manager = Item.manager(db)
        manager.insert(item)

        # Update to fewer tags
        fetched = manager.get(name="Shrinking Item")[0]
        fetched.tags = [Tag("a")]
        manager.update(fetched)

        # Verify
        updated = manager.get(name="Shrinking Item")[0]
        assert len(updated.tags) == 1

    def test_update_replace_entire_list(self, schema_for_multivalue):
        """Update to replace entire multi-value attribute list."""
        db, Item, Name, Tag = schema_for_multivalue

        # Insert with initial tags
        item = Item(name=Name("Replace Item"), tags=[Tag("x"), Tag("y")])
        manager = Item.manager(db)
        manager.insert(item)

        # Replace with completely different tags
        fetched = manager.get(name="Replace Item")[0]
        fetched.tags = [Tag("p"), Tag("q"), Tag("r")]
        manager.update(fetched)

        # Verify
        updated = manager.get(name="Replace Item")[0]
        tag_values = {str(t) for t in updated.tags}
        assert tag_values == {"p", "q", "r"}
