"""Integration tests for TypeQL 3.8.0 built-in function expressions.

These tests verify that the generated TypeQL syntax works correctly with TypeDB.
"""

import pytest

from tests.integration.conftest import TEST_DB_ADDRESS
from type_bridge import Database, Entity, Flag, Integer, Key, SchemaManager, String, TypeFlags
from type_bridge.expressions.builtin import abs_, iid, label


# Test schema
class PersonId(String):
    pass


class PersonName(String):
    pass


class PersonAge(Integer):
    pass


class Person(Entity):
    flags = TypeFlags(name="person")
    person_id: PersonId = Flag(Key)
    name: PersonName
    age: PersonAge | None = None


@pytest.fixture(scope="module")
def db(docker_typedb):
    """Create a test database for builtin function tests."""
    database = Database(address=TEST_DB_ADDRESS, database="test_builtin_functions")
    database.connect()

    # Clean up if exists
    if database.database_exists():
        database.delete_database()
    database.create_database()

    # Create schema
    schema_manager = SchemaManager(database)
    schema_manager.register(Person)
    schema_manager.sync_schema(force=True)

    # Insert test data
    manager = Person.manager(database)
    manager.insert(Person(person_id=PersonId("p1"), name=PersonName("Alice"), age=PersonAge(25)))
    manager.insert(Person(person_id=PersonId("p2"), name=PersonName("Bob"), age=PersonAge(35)))

    yield database

    # Cleanup
    database.delete_database()
    database.close()


@pytest.mark.integration
class TestIidFunction:
    """Integration tests for the iid() function."""

    def test_iid_returns_valid_id(self, db):
        """Test that iid() returns a valid internal ID."""
        # Use raw query with iid() function
        query = """
        match $p isa person, has PersonId "p1";
        fetch { "id": iid($p) };
        """
        results = db.execute_query(query, "read")

        assert len(results) == 1
        assert "id" in results[0]
        # IID should be a hex string
        iid_value = results[0]["id"]
        assert isinstance(iid_value, str)
        assert len(iid_value) > 0

    def test_iid_unique_per_entity(self, db):
        """Test that each entity has a unique IID."""
        query = """
        match $p isa person;
        fetch { "id": iid($p), "name": $p.PersonName };
        """
        results = db.execute_query(query, "read")

        assert len(results) == 2
        iids = [r["id"] for r in results]
        assert len(set(iids)) == 2  # All IIDs should be unique


@pytest.mark.integration
class TestLabelFunction:
    """Integration tests for the label() function."""

    def test_label_returns_type_name(self, db):
        """Test that label() returns the type name."""
        # Note: label() works on TYPE variables, not instance variables
        query = """
        match $p isa! $t, has PersonId "p1"; $t sub person;
        fetch { "type": label($t) };
        """
        results = db.execute_query(query, "read")

        assert len(results) == 1
        assert results[0]["type"] == "person"


@pytest.mark.integration
class TestMathFunctions:
    """Integration tests for math built-in functions.

    Note: ceil, floor, round only work on double/decimal types, not integers.
    abs works on both integers and doubles.
    """

    def test_abs_function(self, db):
        """Test abs() function with integer value."""
        query = """
        match $p isa person, has PersonAge $age;
        fetch { "abs_val": abs($age) };
        """
        results = db.execute_query(query, "read")

        assert len(results) == 2
        for r in results:
            assert r["abs_val"] >= 0


@pytest.mark.integration
class TestBuiltinExpressionGeneration:
    """Test that BuiltinFunctionExpr generates valid TypeQL."""

    def test_iid_expression_in_query(self, db):
        """Test that iid() expression generates valid TypeQL."""
        expr = iid("$p")
        typeql = expr.to_typeql("$p")

        # Use the generated TypeQL in a query
        query = f"""
        match $p isa person, has PersonId "p1";
        fetch {{ "id": {typeql} }};
        """
        results = db.execute_query(query, "read")

        assert len(results) == 1
        assert "id" in results[0]

    def test_label_expression_in_query(self, db):
        """Test that label() expression generates valid TypeQL."""
        expr = label("$t")
        typeql = expr.to_typeql("$t")

        # Use the generated TypeQL in a query
        query = f"""
        match $p isa! $t, has PersonId "p1"; $t sub person;
        fetch {{ "type": {typeql} }};
        """
        results = db.execute_query(query, "read")

        assert len(results) == 1
        assert results[0]["type"] == "person"

    def test_abs_expression_in_query(self, db):
        """Test that abs() expression generates valid TypeQL."""
        expr = abs_("$age")
        typeql = expr.to_typeql("$age")

        query = f"""
        match $p isa person, has PersonAge $age;
        fetch {{ "result": {typeql} }};
        """
        results = db.execute_query(query, "read")

        assert len(results) == 2
        for r in results:
            assert r["result"] >= 0
