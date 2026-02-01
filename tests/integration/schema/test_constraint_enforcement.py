"""Tests that verify TypeDB constraints are enforced during CRUD operations.

These tests ensure constraints (@key, @unique, @card, @range, @regex, @values)
are not just synced to the database but actually enforced at runtime.
"""

import pytest
from typedb.driver import TypeDBDriverException

from type_bridge import (
    Card,
    Database,
    Entity,
    Flag,
    Integer,
    Key,
    SchemaManager,
    String,
    TypeFlags,
    Unique,
)


@pytest.mark.integration
class TestKeyConstraintEnforcement:
    """Test @key constraint enforcement."""

    @pytest.fixture
    def schema_with_key(self, clean_db: Database):
        """Set up schema with @key constraint."""

        class Name(String):
            pass

        class Person(Entity):
            flags = TypeFlags(name="person_key_test")
            name: Name = Flag(Key)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Person)
        schema_manager.sync_schema(force=True)

        return clean_db, Person, Name

    def test_insert_with_key_succeeds(self, schema_with_key):
        """Insert entity with key attribute succeeds."""
        db, Person, Name = schema_with_key

        person = Person(name=Name("Alice"))
        manager = Person.manager(db)
        manager.insert(person)

        results = manager.all()
        assert len(results) == 1
        assert str(results[0].name) == "Alice"

    def test_insert_duplicate_key_fails(self, schema_with_key):
        """Insert second entity with same key fails."""
        db, Person, Name = schema_with_key

        manager = Person.manager(db)

        # Insert first
        person1 = Person(name=Name("Alice"))
        manager.insert(person1)

        # Insert duplicate should fail
        person2 = Person(name=Name("Alice"))
        with pytest.raises(TypeDBDriverException):
            manager.insert(person2)


@pytest.mark.integration
class TestUniqueConstraintEnforcement:
    """Test @unique constraint enforcement."""

    @pytest.fixture
    def schema_with_unique(self, clean_db: Database):
        """Set up schema with @unique constraint."""

        class Name(String):
            pass

        class Email(String):
            pass

        class Person(Entity):
            flags = TypeFlags(name="person_unique_test")
            name: Name = Flag(Key)
            email: Email = Flag(Unique)

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Person)
        schema_manager.sync_schema(force=True)

        return clean_db, Person, Name, Email

    def test_insert_with_unique_succeeds(self, schema_with_unique):
        """Insert entity with unique attribute succeeds."""
        db, Person, Name, Email = schema_with_unique

        person = Person(name=Name("Alice"), email=Email("alice@example.com"))
        manager = Person.manager(db)
        manager.insert(person)

        results = manager.all()
        assert len(results) == 1

    def test_insert_duplicate_unique_fails(self, schema_with_unique):
        """Insert second entity with same unique value fails."""
        db, Person, Name, Email = schema_with_unique

        manager = Person.manager(db)

        # Insert first
        person1 = Person(name=Name("Alice"), email=Email("shared@example.com"))
        manager.insert(person1)

        # Insert with same email should fail
        person2 = Person(name=Name("Bob"), email=Email("shared@example.com"))
        with pytest.raises(TypeDBDriverException):
            manager.insert(person2)


@pytest.mark.integration
class TestCardinalityConstraintEnforcement:
    """Test @card constraint enforcement on attributes."""

    @pytest.fixture
    def schema_with_card(self, clean_db: Database):
        """Set up schema with @card constraints."""

        class Name(String):
            pass

        class Tag(String):
            pass

        class Item(Entity):
            flags = TypeFlags(name="item_card_test")
            name: Name = Flag(Key)
            # Cardinality 1..3: at least 1, at most 3 tags
            tags: list[Tag] = Flag(Card(1, 3))

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Item)
        schema_manager.sync_schema(force=True)

        return clean_db, Item, Name, Tag

    def test_insert_within_card_bounds_succeeds(self, schema_with_card):
        """Insert with value count within cardinality bounds succeeds."""
        db, Item, Name, Tag = schema_with_card

        # 2 tags is within 1..3 bounds
        item = Item(name=Name("Test Item"), tags=[Tag("a"), Tag("b")])
        manager = Item.manager(db)
        manager.insert(item)

        results = manager.all()
        assert len(results) == 1
        assert len(results[0].tags) == 2

    def test_insert_at_min_card_succeeds(self, schema_with_card):
        """Insert with minimum cardinality value succeeds."""
        db, Item, Name, Tag = schema_with_card

        # 1 tag is at minimum
        item = Item(name=Name("Min Item"), tags=[Tag("single")])
        manager = Item.manager(db)
        manager.insert(item)

        results = manager.all()
        assert len(results) == 1

    def test_insert_at_max_card_succeeds(self, schema_with_card):
        """Insert with maximum cardinality value succeeds."""
        db, Item, Name, Tag = schema_with_card

        # 3 tags is at maximum
        item = Item(name=Name("Max Item"), tags=[Tag("a"), Tag("b"), Tag("c")])
        manager = Item.manager(db)
        manager.insert(item)

        results = manager.all()
        assert len(results) == 1
        assert len(results[0].tags) == 3

    def test_insert_exceeds_max_card_fails(self, schema_with_card):
        """Insert with value count exceeding max cardinality fails."""
        db, Item, Name, Tag = schema_with_card

        # 4 tags exceeds max of 3
        item = Item(
            name=Name("Over Max Item"),
            tags=[Tag("a"), Tag("b"), Tag("c"), Tag("d")],
        )
        manager = Item.manager(db)

        with pytest.raises(TypeDBDriverException):
            manager.insert(item)


@pytest.mark.integration
class TestRegexConstraintEnforcement:
    """Test @regex constraint enforcement."""

    @pytest.fixture
    def schema_with_regex(self, clean_db: Database):
        """Set up schema with @regex constraint."""

        class Name(String):
            pass

        class Code(String):
            # Pattern: exactly 3 uppercase letters
            regex = r"^[A-Z]{3}$"  # type: ignore[assignment]

        class Airport(Entity):
            flags = TypeFlags(name="airport_regex_test")
            name: Name = Flag(Key)
            code: Code

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Airport)
        schema_manager.sync_schema(force=True)

        return clean_db, Airport, Name, Code

    def test_insert_matching_regex_succeeds(self, schema_with_regex):
        """Insert with value matching regex succeeds."""
        db, Airport, Name, Code = schema_with_regex

        airport = Airport(name=Name("San Francisco"), code=Code("SFO"))
        manager = Airport.manager(db)
        manager.insert(airport)

        results = manager.all()
        assert len(results) == 1
        assert str(results[0].code) == "SFO"

    def test_insert_not_matching_regex_fails(self, schema_with_regex):
        """Insert with value not matching regex fails."""
        db, Airport, Name, Code = schema_with_regex

        # Lowercase doesn't match pattern
        airport = Airport(name=Name("Test Airport"), code=Code("abc"))
        manager = Airport.manager(db)

        with pytest.raises(TypeDBDriverException):
            manager.insert(airport)

    def test_insert_wrong_length_regex_fails(self, schema_with_regex):
        """Insert with wrong length for regex fails."""
        db, Airport, Name, Code = schema_with_regex

        # 4 letters doesn't match 3-letter pattern
        airport = Airport(name=Name("Test Airport"), code=Code("ABCD"))
        manager = Airport.manager(db)

        with pytest.raises(TypeDBDriverException):
            manager.insert(airport)


@pytest.mark.integration
class TestValuesConstraintEnforcement:
    """Test @values constraint enforcement."""

    @pytest.fixture
    def schema_with_values(self, clean_db: Database):
        """Set up schema with @values constraint."""

        class Name(String):
            pass

        class Status(String):
            # Only allow specific values
            allowed_values = ("active", "inactive", "pending")

        class Account(Entity):
            flags = TypeFlags(name="account_values_test")
            name: Name = Flag(Key)
            status: Status

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Account)
        schema_manager.sync_schema(force=True)

        return clean_db, Account, Name, Status

    def test_insert_allowed_value_succeeds(self, schema_with_values):
        """Insert with allowed value succeeds."""
        db, Account, Name, Status = schema_with_values

        account = Account(name=Name("Test Account"), status=Status("active"))
        manager = Account.manager(db)
        manager.insert(account)

        results = manager.all()
        assert len(results) == 1
        assert str(results[0].status) == "active"

    def test_insert_all_allowed_values_succeed(self, schema_with_values):
        """Insert with each allowed value succeeds."""
        db, Account, Name, Status = schema_with_values

        manager = Account.manager(db)

        for i, status_val in enumerate(["active", "inactive", "pending"]):
            account = Account(name=Name(f"Account {i}"), status=Status(status_val))
            manager.insert(account)

        results = manager.all()
        assert len(results) == 3

    def test_insert_disallowed_value_fails(self, schema_with_values):
        """Insert with disallowed value fails."""
        db, Account, Name, Status = schema_with_values

        # "deleted" is not in allowed values
        account = Account(name=Name("Bad Account"), status=Status("deleted"))
        manager = Account.manager(db)

        with pytest.raises(TypeDBDriverException):
            manager.insert(account)


@pytest.mark.integration
class TestRangeConstraintEnforcement:
    """Test @range constraint enforcement."""

    @pytest.fixture
    def schema_with_range(self, clean_db: Database):
        """Set up schema with @range constraint."""

        class Name(String):
            pass

        class Score(Integer):
            # Score must be 0-100
            range_constraint = (0, 100)

        class Student(Entity):
            flags = TypeFlags(name="student_range_test")
            name: Name = Flag(Key)
            score: Score

        schema_manager = SchemaManager(clean_db)
        schema_manager.register(Student)
        schema_manager.sync_schema(force=True)

        return clean_db, Student, Name, Score

    def test_insert_within_range_succeeds(self, schema_with_range):
        """Insert with value within range succeeds."""
        db, Student, Name, Score = schema_with_range

        student = Student(name=Name("Alice"), score=Score(85))
        manager = Student.manager(db)
        manager.insert(student)

        results = manager.all()
        assert len(results) == 1
        assert int(results[0].score) == 85

    def test_insert_at_range_boundaries_succeeds(self, schema_with_range):
        """Insert at range boundaries succeeds."""
        db, Student, Name, Score = schema_with_range

        manager = Student.manager(db)

        # At minimum
        student_min = Student(name=Name("Min Student"), score=Score(0))
        manager.insert(student_min)

        # At maximum
        student_max = Student(name=Name("Max Student"), score=Score(100))
        manager.insert(student_max)

        results = manager.all()
        assert len(results) == 2

    def test_insert_below_range_fails(self, schema_with_range):
        """Insert with value below range fails at Python validation level."""
        db, Student, Name, Score = schema_with_range

        # Range constraint is enforced when creating the attribute instance
        with pytest.raises(ValueError, match="below minimum"):
            Score(-1)

    def test_insert_above_range_fails(self, schema_with_range):
        """Insert with value above range fails at Python validation level."""
        db, Student, Name, Score = schema_with_range

        # Range constraint is enforced when creating the attribute instance
        with pytest.raises(ValueError, match="above maximum"):
            Score(101)
