"""Unit tests for Query and QueryBuilder classes."""

from datetime import UTC

import pytest

from type_bridge import Entity, Flag, Integer, Key, Relation, Role, String, TypeFlags
from type_bridge.crud.utils import format_value
from type_bridge.query import Query, QueryBuilder


class TestQuery:
    """Tests for the Query builder class."""

    def test_empty_query_builds_empty_string(self):
        """An empty query should build to an empty string."""
        query = Query()
        assert query.build() == ""

    def test_single_match_clause(self):
        """A single match clause should be properly formatted."""
        query = Query().match("$p isa person")
        result = query.build()
        assert "match" in result
        assert "$p isa person" in result

    def test_multiple_match_clauses_joined(self):
        """Multiple match clauses should be joined with semicolons."""
        query = Query().match("$p isa person").match("$p has name $n")
        result = query.build()
        # Both patterns should be present, joined by semicolons
        assert "$p isa person" in result
        assert "$p has name $n" in result
        # Should have semicolons separating patterns
        assert ";" in result

    def test_fetch_single_variable(self):
        """Fetch with a single variable should use .* syntax."""
        query = Query().match("$p isa person").fetch("$p")
        result = query.build()
        assert "fetch {" in result
        assert "$p.*" in result

    def test_fetch_multiple_variables(self):
        """Fetch with multiple variables should include all."""
        query = Query().match("$p isa person").match("$c isa company").fetch("$p").fetch("$c")
        result = query.build()
        assert "$p.*" in result
        assert "$c.*" in result

    def test_delete_clause_generation(self):
        """Delete clause should be properly formatted."""
        query = Query().match("$p isa person").delete("$p isa person")
        result = query.build()
        assert "delete" in result
        assert "$p isa person" in result

    def test_insert_clause_generation(self):
        """Insert clause should be properly formatted."""
        query = Query().insert('$p isa person, has name "Alice"')
        result = query.build()
        assert "insert" in result
        assert '$p isa person, has name "Alice"' in result

    def test_combined_match_delete_query(self):
        """Match followed by delete should be in correct order."""
        query = Query().match("$p isa person, has name $n").delete("$p has name $n")
        result = query.build()
        lines = result.split("\n")
        match_idx = next(i for i, line in enumerate(lines) if "match" in line)
        delete_idx = next(i for i, line in enumerate(lines) if "delete" in line)
        assert match_idx < delete_idx

    def test_combined_match_insert_query(self):
        """Match followed by insert should be in correct order."""
        query = Query().match("$p isa person").insert("$p has age 30")
        result = query.build()
        lines = result.split("\n")
        match_idx = next(i for i, line in enumerate(lines) if "match" in line)
        insert_idx = next(i for i, line in enumerate(lines) if "insert" in line)
        assert match_idx < insert_idx

    def test_combined_match_delete_insert_query(self):
        """Match, delete, insert should be in correct order."""
        query = (
            Query()
            .match("$p isa person, has age $old")
            .delete("$p has age $old")
            .insert("$p has age 31")
        )
        result = query.build()
        # Find line indices
        lines = result.split("\n")
        match_idx = next(i for i, line in enumerate(lines) if "match" in line)
        delete_idx = next(i for i, line in enumerate(lines) if "delete" in line)
        insert_idx = next(i for i, line in enumerate(lines) if "insert" in line)
        assert match_idx < delete_idx < insert_idx

    def test_str_returns_build(self):
        """__str__ should return the same as build()."""
        query = Query().match("$p isa person")
        assert str(query) == query.build()


class TestQuerySorting:
    """Tests for Query sorting functionality."""

    def test_sort_ascending(self):
        """Sort ascending should use 'asc' keyword."""
        query = Query().match("$p isa person").sort("$p", "asc")
        result = query.build()
        assert "sort $p asc" in result

    def test_sort_descending(self):
        """Sort descending should use 'desc' keyword."""
        query = Query().match("$p isa person").sort("$p", "desc")
        result = query.build()
        assert "sort $p desc" in result

    def test_sort_default_direction(self):
        """Sort without direction should default to ascending."""
        query = Query().match("$p isa person").sort("$p")
        result = query.build()
        assert "sort $p asc" in result

    def test_sort_invalid_direction_raises_value_error(self):
        """Sort with invalid direction should raise ValueError."""
        query = Query()
        with pytest.raises(ValueError, match="Invalid sort direction"):
            query.sort("$p", "invalid")

    def test_multiple_sort_clauses(self):
        """Multiple sort clauses should be comma-separated in a single sort statement."""
        query = Query().match("$p isa person").sort("$p", "asc").sort("$n", "desc")
        result = query.build()
        # TypeQL uses comma-separated sort variables: sort $p asc, $n desc;
        assert "sort $p asc, $n desc;" in result


class TestQueryPagination:
    """Tests for Query limit and offset functionality."""

    def test_limit_only(self):
        """Limit without offset should be properly formatted."""
        query = Query().match("$p isa person").limit(10)
        result = query.build()
        assert "limit 10" in result
        assert "offset" not in result

    def test_offset_only(self):
        """Offset without limit should be properly formatted."""
        query = Query().match("$p isa person").offset(5)
        result = query.build()
        assert "offset 5" in result
        assert "limit" not in result

    def test_limit_and_offset_correct_order(self):
        """Offset should come before limit in output."""
        query = Query().match("$p isa person").limit(10).offset(5)
        result = query.build()
        offset_pos = result.find("offset")
        limit_pos = result.find("limit")
        assert offset_pos < limit_pos

    def test_pagination_before_fetch(self):
        """Sort, offset, and limit should come before fetch."""
        query = Query().match("$p isa person").sort("$p").offset(5).limit(10).fetch("$p")
        result = query.build()
        fetch_pos = result.find("fetch")
        offset_pos = result.find("offset")
        limit_pos = result.find("limit")
        sort_pos = result.find("sort")
        assert sort_pos < offset_pos < limit_pos < fetch_pos


class TestQueryBuilderHelpers:
    """Tests for QueryBuilder helper methods."""

    def test_match_entity_basic(self):
        """match_entity should create basic entity match pattern."""

        class Name(String):
            pass

        class Person(Entity):
            flags = TypeFlags(name="person")
            name: Name = Flag(Key)

        query = QueryBuilder.match_entity(Person, "$e")
        result = query.build()
        assert "$e isa person" in result

    def test_match_entity_with_filters(self):
        """match_entity with filters should include attribute conditions."""

        class Name(String):
            pass

        class Person(Entity):
            flags = TypeFlags(name="person")
            name: Name = Flag(Key)

        query = QueryBuilder.match_entity(Person, "$e", name="Alice")
        result = query.build()
        assert "$e isa person" in result
        # Attribute name comes from the class name (Name -> Name)
        assert 'has Name "Alice"' in result

    def test_match_entity_with_attribute_instance(self):
        """match_entity should extract value from Attribute instances."""

        class Name(String):
            pass

        class Person(Entity):
            flags = TypeFlags(name="person")
            name: Name = Flag(Key)

        query = QueryBuilder.match_entity(Person, "$e", name=Name("Bob"))
        result = query.build()
        # Attribute name comes from the class name (Name -> Name)
        assert 'has Name "Bob"' in result

    def test_match_relation_basic(self):
        """match_relation should create basic relation match pattern."""

        class Doc(String):
            pass

        class User(Entity):
            flags = TypeFlags(name="user")
            doc: Doc = Flag(Key)

        class Friendship(Relation):
            flags = TypeFlags(name="friendship")
            friend: Role[User] = Role("friend", User)

        query = QueryBuilder.match_relation(Friendship, "$r")
        result = query.build()
        assert "$r isa friendship" in result

    def test_match_relation_with_role_players(self):
        """match_relation with role players should include role patterns."""

        class Doc(String):
            pass

        class User(Entity):
            flags = TypeFlags(name="user")
            doc: Doc = Flag(Key)

        class Friendship(Relation):
            flags = TypeFlags(name="friendship")
            friend: Role[User] = Role("friend", User)

        query = QueryBuilder.match_relation(Friendship, "$r", role_players={"friend": "$u"})
        result = query.build()
        # TypeDB 3.x syntax: isa type comes before role players
        assert "$r isa friendship (friend: $u)" in result

    def test_insert_entity_generates_correct_pattern(self):
        """insert_entity should generate insert query from instance."""

        class Name(String):
            pass

        class Age(Integer):
            pass

        class Person(Entity):
            flags = TypeFlags(name="person")
            name: Name = Flag(Key)
            age: Age | None = None

        person = Person(name=Name("Charlie"), age=Age(25))
        query = QueryBuilder.insert_entity(person, "$e")
        result = query.build()
        assert "insert" in result


class TestFormatValue:
    """Tests for format_value helper function."""

    def test_format_string_simple(self):
        """Simple strings should be quoted."""
        assert format_value("hello") == '"hello"'

    def test_format_string_with_double_quotes(self):
        """Strings with quotes should be escaped."""
        assert format_value('say "hello"') == '"say \\"hello\\""'

    def test_format_string_with_backslash(self):
        """Strings with backslashes should be escaped."""
        assert format_value("path\\to\\file") == '"path\\\\to\\\\file"'

    def test_format_string_empty(self):
        """Empty strings should be quoted."""
        assert format_value("") == '""'

    def test_format_string_with_newline(self):
        """Strings with newlines should escape them for TypeQL."""
        result = format_value("line1\nline2")
        assert result == r'"line1\nline2"'

    def test_format_boolean_true_returns_lowercase(self):
        """Boolean True should be 'true' (lowercase)."""
        assert format_value(True) == "true"

    def test_format_boolean_false_returns_lowercase(self):
        """Boolean False should be 'false' (lowercase)."""
        assert format_value(False) == "false"

    def test_format_integer_positive(self):
        """Positive integers should be formatted as strings."""
        assert format_value(42) == "42"

    def test_format_integer_negative(self):
        """Negative integers should include the sign."""
        assert format_value(-5) == "-5"

    def test_format_integer_zero(self):
        """Zero should be formatted as '0'."""
        assert format_value(0) == "0"

    def test_format_float_positive(self):
        """Positive floats should be formatted as strings."""
        assert format_value(3.14) == "3.14"

    def test_format_float_negative(self):
        """Negative floats should include the sign."""
        assert format_value(-2.5) == "-2.5"

    def test_format_decimal_adds_dec_suffix(self):
        """Decimal values should have 'dec' suffix."""
        from decimal import Decimal

        result = format_value(Decimal("123.45"))
        assert result == "123.45dec"

    def test_format_datetime_naive(self):
        """Naive datetime should be ISO formatted."""
        from datetime import datetime

        dt = datetime(2024, 1, 15, 10, 30, 0)
        result = format_value(dt)
        assert result == "2024-01-15T10:30:00"

    def test_format_datetime_with_timezone(self):
        """Timezone-aware datetime should include timezone."""
        from datetime import datetime

        dt = datetime(2024, 1, 15, 10, 30, 0, tzinfo=UTC)
        result = format_value(dt)
        assert "2024-01-15T10:30:00" in result
        assert "+00:00" in result or "Z" in result or "UTC" in result

    def test_format_date(self):
        """Date should be ISO formatted."""
        from datetime import date

        d = date(2024, 1, 15)
        result = format_value(d)
        assert result == "2024-01-15"

    def test_format_timedelta(self):
        """Timedelta should be ISO duration formatted."""
        from datetime import timedelta

        td = timedelta(days=1, hours=2, minutes=30)
        result = format_value(td)
        # ISO duration format
        assert "P" in result

    def test_format_duration_isodate(self):
        """Isodate Duration should be ISO formatted."""
        import isodate

        duration = isodate.parse_duration("P1DT2H30M")
        result = format_value(duration)
        assert "P" in result

    def test_format_attribute_instance_extracts_value(self):
        """Attribute instances should have their value extracted."""

        class Name(String):
            pass

        name = Name("Alice")
        result = format_value(name)
        assert result == '"Alice"'

    def test_format_custom_object_stringifies(self):
        """Unknown objects should be converted to quoted strings."""

        class CustomObj:
            def __str__(self):
                return "custom"

        result = format_value(CustomObj())
        assert result == '"custom"'
