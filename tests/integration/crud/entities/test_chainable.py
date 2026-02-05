"""Integration tests for chainable EntityQuery operations (delete and update_with)."""

import pytest

from type_bridge import Entity, Flag, Integer, Key, SchemaManager, String, TypeFlags


@pytest.mark.integration
@pytest.mark.order(160)
def test_chainable_delete_with_expression_filter(db_with_schema):
    """Test EntityQuery.delete() with expression-based filters."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age

    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    # Insert test data
    manager = Person.manager(db_with_schema)
    people = [
        Person(name=Name("Alice"), age=Age(25)),
        Person(name=Name("Bob"), age=Age(70)),
        Person(name=Name("Charlie"), age=Age(72)),
        Person(name=Name("Diana"), age=Age(30)),
    ]
    manager.insert_many(people)

    # Verify initial state
    all_persons = manager.all()
    expected_count = 4
    actual_count = len(all_persons)
    assert expected_count == actual_count

    # Act - Delete persons over 65 using chainable filter
    deleted_count = manager.filter(Age.gt(Age(65))).delete()

    # Assert
    expected_deleted = 2
    assert expected_deleted == deleted_count

    # Verify remaining entities
    remaining = manager.all()
    expected_remaining = 2
    actual_remaining = len(remaining)
    assert expected_remaining == actual_remaining

    # Verify only young persons remain
    for person in remaining:
        assert person.age.value <= 65


@pytest.mark.integration
@pytest.mark.order(161)
def test_chainable_delete_with_multiple_filters(db_with_schema):
    """Test EntityQuery.delete() with multiple expression filters."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Status(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age
        status: Status

    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    # Insert test data
    manager = Person.manager(db_with_schema)
    people = [
        Person(name=Name("Eve"), age=Age(20), status=Status("active")),
        Person(name=Name("Frank"), age=Age(16), status=Status("inactive")),
        Person(name=Name("Grace"), age=Age(17), status=Status("inactive")),
        Person(name=Name("Henry"), age=Age(19), status=Status("active")),
    ]
    manager.insert_many(people)

    # Act - Delete inactive minors (age < 18 AND status = inactive)
    deleted_count = manager.filter(Age.lt(Age(18)), Status.eq(Status("inactive"))).delete()

    # Assert
    expected_deleted = 2  # Frank and Grace
    assert expected_deleted == deleted_count

    # Verify remaining
    remaining = manager.all()
    expected_remaining = 2  # Eve and Henry
    assert expected_remaining == len(remaining)

    # Verify deleted entities are gone
    frank = manager.get(name="Frank")
    assert len(frank) == 0

    grace = manager.get(name="Grace")
    assert len(grace) == 0


@pytest.mark.integration
@pytest.mark.order(162)
def test_chainable_delete_returns_zero_for_no_matches(db_with_schema):
    """Test EntityQuery.delete() returns 0 when no entities match."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age

    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    # Insert test data
    manager = Person.manager(db_with_schema)
    manager.insert(Person(name=Name("Iris"), age=Age(25)))

    # Act - Delete persons over 100 (none exist)
    deleted_count = manager.filter(Age.gt(Age(100))).delete()

    # Assert
    expected = 0
    assert expected == deleted_count

    # Verify entity still exists
    remaining = manager.all()
    assert len(remaining) == 1
    assert remaining[0].name.value == "Iris"


@pytest.mark.integration
@pytest.mark.order(163)
def test_chainable_delete_with_range_filter(db_with_schema):
    """Test EntityQuery.delete() with range filters (gte and lt)."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age

    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    # Insert test data with ages: 17, 18, 20, 21, 22
    manager = Person.manager(db_with_schema)
    people = [
        Person(name=Name("Jack"), age=Age(17)),
        Person(name=Name("Kate"), age=Age(18)),
        Person(name=Name("Leo"), age=Age(20)),
        Person(name=Name("Mia"), age=Age(21)),
        Person(name=Name("Noah"), age=Age(22)),
    ]
    manager.insert_many(people)

    # Act - Delete persons in range [18, 21) - i.e., age >= 18 and age < 21
    deleted_count = manager.filter(Age.gte(Age(18)), Age.lt(Age(21))).delete()

    # Assert - Should delete Kate (18) and Leo (20)
    expected_deleted = 2
    assert expected_deleted == deleted_count

    # Verify remaining persons
    remaining = manager.all()
    expected_remaining = 3  # Jack (17), Mia (21), Noah (22)
    assert expected_remaining == len(remaining)

    # Verify correct persons remain
    remaining_names = {p.name.value for p in remaining}
    expected_names = {"Jack", "Mia", "Noah"}
    assert expected_names == remaining_names


@pytest.mark.integration
@pytest.mark.order(164)
def test_update_with_lambda_increments_age(db_with_schema):
    """Test EntityQuery.update_with() using lambda to increment age."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age

    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    # Insert test data
    manager = Person.manager(db_with_schema)
    people = [
        Person(name=Name("Oliver"), age=Age(28)),
        Person(name=Name("Penny"), age=Age(35)),
        Person(name=Name("Quinn"), age=Age(40)),
    ]
    manager.insert_many(people)

    # Act - Increment age for persons over 30
    updated = manager.filter(Age.gt(Age(30))).update_with(
        lambda person: setattr(person, "age", Age(person.age.value + 1))
    )

    # Assert - Should update Penny and Quinn
    expected_count = 2
    actual_count = len(updated)
    assert expected_count == actual_count

    # Verify updates persisted
    penny = manager.get(name="Penny")[0]
    quinn = manager.get(name="Quinn")[0]
    oliver = manager.get(name="Oliver")[0]

    assert penny.age.value == 36  # 35 + 1
    assert quinn.age.value == 41  # 40 + 1
    assert oliver.age.value == 28  # Unchanged


@pytest.mark.integration
@pytest.mark.order(165)
def test_update_with_function_modifies_status(db_with_schema):
    """Test EntityQuery.update_with() using named function."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Status(String):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age
        status: Status

    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    # Insert test data
    manager = Person.manager(db_with_schema)
    people = [
        Person(name=Name("Rose"), age=Age(28), status=Status("junior")),
        Person(name=Name("Sam"), age=Age(35), status=Status("senior")),
        Person(name=Name("Tina"), age=Age(40), status=Status("senior")),
    ]
    manager.insert_many(people)

    # Define update function
    def promote_to_lead(person):
        """Promote person to lead status."""
        person.status = Status("lead")

    # Act - Promote all senior persons
    updated = manager.filter(Status.eq(Status("senior"))).update_with(promote_to_lead)

    # Assert
    expected_count = 2  # Sam and Tina
    assert expected_count == len(updated)

    # Verify updates persisted
    sam = manager.get(name="Sam")[0]
    tina = manager.get(name="Tina")[0]
    rose = manager.get(name="Rose")[0]

    assert sam.status.value == "lead"
    assert tina.status.value == "lead"
    assert rose.status.value == "junior"  # Unchanged


@pytest.mark.integration
@pytest.mark.order(166)
def test_update_with_returns_empty_list_for_no_matches(db_with_schema):
    """Test EntityQuery.update_with() returns empty list when no entities match."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age

    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    # Insert test data
    manager = Person.manager(db_with_schema)
    manager.insert(Person(name=Name("Uma"), age=Age(25)))

    # Act - Update persons over 100 (none exist)
    updated = manager.filter(Age.gt(Age(100))).update_with(lambda p: setattr(p, "age", Age(99)))

    # Assert
    expected = []
    assert expected == updated
    assert len(updated) == 0

    # Verify entity unchanged
    uma = manager.get(name="Uma")[0]
    assert uma.age.value == 25


@pytest.mark.integration
@pytest.mark.order(167)
def test_update_with_complex_function_multiple_attributes(db_with_schema):
    """Test EntityQuery.update_with() modifying multiple attributes."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Status(String):
        pass

    class Salary(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age
        status: Status
        salary: Salary | None = None

    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    # Insert test data
    manager = Person.manager(db_with_schema)
    people = [
        Person(name=Name("Victor"), age=Age(32), status=Status("regular"), salary=Salary(50000)),
        Person(name=Name("Wendy"), age=Age(35), status=Status("regular"), salary=Salary(60000)),
        Person(name=Name("Xavier"), age=Age(25), status=Status("regular"), salary=Salary(45000)),
    ]
    manager.insert_many(people)

    # Define complex update function
    def promote_and_raise(person):
        """Promote to senior and give 10% raise."""
        person.status = Status("senior")
        if person.salary:
            person.salary = Salary(int(person.salary.value * 1.1))

    # Act - Promote persons age >= 30
    updated = manager.filter(Age.gte(Age(30))).update_with(promote_and_raise)

    # Assert
    expected_count = 2  # Victor and Wendy
    assert expected_count == len(updated)

    # Verify updates persisted
    victor = manager.get(name="Victor")[0]
    wendy = manager.get(name="Wendy")[0]
    xavier = manager.get(name="Xavier")[0]

    # Victor: status changed, salary increased by 10%
    assert victor.status.value == "senior"
    assert victor.salary is not None
    assert victor.salary.value == 55000  # 50000 * 1.1

    # Wendy: status changed, salary increased by 10%
    assert wendy.status.value == "senior"
    assert wendy.salary is not None
    assert wendy.salary.value == 66000  # 60000 * 1.1

    # Xavier: unchanged
    assert xavier.status.value == "regular"
    assert xavier.salary is not None
    assert xavier.salary.value == 45000


@pytest.mark.integration
@pytest.mark.order(168)
def test_update_with_atomic_transaction(db_with_schema):
    """Test EntityQuery.update_with() uses atomic transaction (all or nothing)."""

    # Arrange
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age

    schema_manager = SchemaManager(db_with_schema)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    # Insert test data
    manager = Person.manager(db_with_schema)
    people = [
        Person(name=Name("Yara"), age=Age(28)),
        Person(name=Name("Zane"), age=Age(35)),
    ]
    manager.insert_many(people)

    # Define function that will fail on second entity
    call_count = [0]

    def failing_update(person):
        """Update function that fails on second call."""
        call_count[0] += 1
        if call_count[0] == 2:
            raise ValueError("Intentional failure on second entity")
        person.age = Age(person.age.value + 1)

    # Act & Assert - Should raise error and not update any entities
    with pytest.raises(ValueError) as exc_info:
        manager.filter(Age.gt(Age(0))).update_with(failing_update)

    assert "Intentional failure" in str(exc_info.value)

    # Verify no updates persisted (atomic transaction rolled back)
    yara = manager.get(name="Yara")[0]
    zane = manager.get(name="Zane")[0]

    assert yara.age.value == 28  # Unchanged
    assert zane.age.value == 35  # Unchanged
