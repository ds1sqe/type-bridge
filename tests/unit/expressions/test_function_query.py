"""Tests for FunctionQuery query generation."""

from __future__ import annotations

import pytest

from type_bridge.expressions import FunctionQuery, ReturnType


class TestReturnType:
    """Tests for ReturnType dataclass."""

    def test_single_non_stream(self) -> None:
        """Single value, non-stream return type."""
        rt = ReturnType(["integer"])
        assert rt.is_stream is False
        assert rt.is_composite is False
        assert rt.is_single_value is True
        assert rt.is_optional == [False]

    def test_single_stream(self) -> None:
        """Single value, stream return type."""
        rt = ReturnType(["id"], is_stream=True)
        assert rt.is_stream is True
        assert rt.is_composite is False
        assert rt.is_single_value is False

    def test_composite_stream(self) -> None:
        """Multiple values, stream return type (tuple)."""
        rt = ReturnType(["artifact", "integer"], is_stream=True)
        assert rt.is_stream is True
        assert rt.is_composite is True
        assert rt.is_single_value is False
        assert len(rt.types) == 2

    def test_optional_flags(self) -> None:
        """Return type with optional flags."""
        rt = ReturnType(["place", "name"], is_optional=[False, True])
        assert rt.is_optional == [False, True]


class TestFunctionQueryBasics:
    """Tests for basic FunctionQuery functionality."""

    def test_simple_call(self) -> None:
        """Generate simple function call."""
        fn = FunctionQuery(
            name="count-artifacts",
            return_type=ReturnType(["integer"]),
        )
        assert fn.to_call() == "count-artifacts()"

    def test_call_with_args(self) -> None:
        """Generate function call with arguments."""
        fn = FunctionQuery(
            name="get-neighbor-ids",
            args=[("$target_id", "abc-123")],
            return_type=ReturnType(["id"], is_stream=True),
        )
        assert fn.to_call() == 'get-neighbor-ids("abc-123")'

    def test_call_with_variable_arg(self) -> None:
        """Variable references are passed through unchanged."""
        fn = FunctionQuery(
            name="filter-by-type",
            args=[("$type", "$my_type")],
            return_type=ReturnType(["artifact"], is_stream=True),
        )
        assert fn.to_call() == "filter-by-type($my_type)"

    def test_call_with_quoted_string(self) -> None:
        """Already quoted strings are not double-quoted."""
        fn = FunctionQuery(
            name="search",
            args=[("$query", '"hello world"')],
            return_type=ReturnType(["result"], is_stream=True),
        )
        assert fn.to_call() == 'search("hello world")'

    def test_call_with_boolean(self) -> None:
        """Boolean arguments formatted correctly."""
        fn = FunctionQuery(
            name="set-flag",
            args=[("$enabled", True)],
            return_type=ReturnType(["bool"]),
        )
        assert fn.to_call() == "set-flag(true)"

    def test_call_with_integer(self) -> None:
        """Integer arguments formatted correctly."""
        fn = FunctionQuery(
            name="limit-results",
            args=[("$count", 10)],
            return_type=ReturnType(["result"], is_stream=True),
        )
        assert fn.to_call() == "limit-results(10)"


class TestFunctionQueryMatchLet:
    """Tests for match let clause generation."""

    def test_single_non_stream(self) -> None:
        """Match let for single non-stream result."""
        fn = FunctionQuery(
            name="count-all",
            return_type=ReturnType(["integer"]),
        )
        assert fn.to_match_let() == "match\nlet $integer = count-all();"

    def test_single_stream(self) -> None:
        """Match let for single stream result."""
        fn = FunctionQuery(
            name="get-ids",
            return_type=ReturnType(["id"], is_stream=True),
        )
        assert fn.to_match_let() == "match\nlet $id in get-ids();"

    def test_composite_stream(self) -> None:
        """Match let for composite stream result."""
        fn = FunctionQuery(
            name="get-pairs",
            return_type=ReturnType(["artifact", "count"], is_stream=True),
        )
        assert fn.to_match_let() == "match\nlet $artifact, $count in get-pairs();"

    def test_composite_non_stream(self) -> None:
        """Match let for composite non-stream result."""
        fn = FunctionQuery(
            name="divide",
            return_type=ReturnType(["quotient", "remainder"]),
        )
        assert fn.to_match_let() == "match\nlet $quotient, $remainder = divide();"

    def test_custom_result_vars(self) -> None:
        """Custom variable names for results."""
        fn = FunctionQuery(
            name="count-all",
            return_type=ReturnType(["integer"]),
        )
        assert fn.to_match_let(result_vars=["total"]) == "match\nlet $total = count-all();"

    def test_hyphenated_type_in_var(self) -> None:
        """Hyphenated types become underscored variables."""
        fn = FunctionQuery(
            name="get-display-ids",
            return_type=ReturnType(["display-id"], is_stream=True),
        )
        assert fn.to_match_let() == "match\nlet $display_id in get-display-ids();"


class TestFunctionQueryFetch:
    """Tests for fetch clause generation."""

    def test_single_fetch(self) -> None:
        """Fetch single result."""
        fn = FunctionQuery(
            name="count-all",
            return_type=ReturnType(["integer"]),
        )
        assert fn.to_fetch() == 'fetch {\n  "integer": $integer\n};'

    def test_composite_fetch(self) -> None:
        """Fetch composite result."""
        fn = FunctionQuery(
            name="get-pairs",
            return_type=ReturnType(["artifact", "count"], is_stream=True),
        )
        assert fn.to_fetch() == 'fetch {\n  "artifact": $artifact,\n  "count": $count\n};'

    def test_custom_fetch_keys(self) -> None:
        """Custom keys for fetch object."""
        fn = FunctionQuery(
            name="count-all",
            return_type=ReturnType(["integer"]),
        )
        assert fn.to_fetch(fetch_keys=["total"]) == 'fetch {\n  "total": $integer\n};'

    def test_custom_vars_and_keys(self) -> None:
        """Both custom vars and keys."""
        fn = FunctionQuery(
            name="count-all",
            return_type=ReturnType(["integer"]),
        )
        assert (
            fn.to_fetch(result_vars=["my_count"], fetch_keys=["count"])
            == 'fetch {\n  "count": $my_count\n};'
        )


class TestFunctionQueryFullQuery:
    """Tests for complete query generation."""

    def test_simple_query(self) -> None:
        """Generate complete query for simple function."""
        fn = FunctionQuery(
            name="count-artifacts",
            return_type=ReturnType(["integer"]),
        )
        query = fn.to_query()
        assert "match\nlet $integer = count-artifacts();" in query
        assert 'fetch {\n  "integer": $integer\n};' in query

    def test_stream_query(self) -> None:
        """Generate complete query for stream function."""
        fn = FunctionQuery(
            name="get-ids",
            return_type=ReturnType(["id"], is_stream=True),
        )
        query = fn.to_query()
        assert "match\nlet $id in get-ids();" in query
        assert 'fetch {\n  "id": $id\n};' in query

    def test_query_with_limit(self) -> None:
        """Query with limit clause."""
        fn = FunctionQuery(
            name="get-ids",
            return_type=ReturnType(["id"], is_stream=True),
        )
        query = fn.to_query(limit=10)
        assert "limit 10;" in query

    def test_query_with_offset(self) -> None:
        """Query with offset clause."""
        fn = FunctionQuery(
            name="get-ids",
            return_type=ReturnType(["id"], is_stream=True),
        )
        query = fn.to_query(offset=5)
        assert "offset 5;" in query

    def test_query_with_pagination(self) -> None:
        """Query with both offset and limit."""
        fn = FunctionQuery(
            name="get-ids",
            return_type=ReturnType(["id"], is_stream=True),
        )
        query = fn.to_query(limit=10, offset=20)
        # Offset must come before limit in TypeQL
        assert query.index("offset 20;") < query.index("limit 10;")

    def test_query_with_sort(self) -> None:
        """Query with sort clause."""
        fn = FunctionQuery(
            name="get-scores",
            return_type=ReturnType(["id", "score"], is_stream=True),
        )
        query = fn.to_query(sort_var="score", sort_order="desc")
        assert "sort $score desc;" in query

    def test_query_with_parameters(self) -> None:
        """Query with function parameters."""
        fn = FunctionQuery(
            name="get-neighbors",
            args=[("$target_id", "xyz-123")],
            return_type=ReturnType(["neighbor"], is_stream=True),
        )
        query = fn.to_query()
        assert 'get-neighbors("xyz-123")' in query


class TestFunctionQueryReduceQuery:
    """Tests for reduce query generation."""

    def test_reduce_query(self) -> None:
        """Generate reduce query for single-value function."""
        fn = FunctionQuery(
            name="count-all",
            return_type=ReturnType(["integer"]),
        )
        assert fn.to_reduce_query() == "match\nlet $integer = count-all();"

    def test_reduce_query_error_on_stream(self) -> None:
        """Reduce query raises error for stream functions."""
        fn = FunctionQuery(
            name="get-ids",
            return_type=ReturnType(["id"], is_stream=True),
        )
        with pytest.raises(ValueError, match="Cannot use to_reduce_query with stream functions"):
            fn.to_reduce_query()


class TestFunctionQueryWithArgs:
    """Tests for with_args method."""

    def test_with_args_basic(self) -> None:
        """Create function query with bound arguments."""
        fn = FunctionQuery(
            name="get-neighbors",
            args=[("$target_id", None)],
            return_type=ReturnType(["neighbor"], is_stream=True),
        )
        bound = fn.with_args(target_id="abc-123")
        assert bound.args == [("$target_id", "abc-123")]

    def test_with_args_preserves_metadata(self) -> None:
        """with_args preserves name and return_type."""
        fn = FunctionQuery(
            name="get-neighbors",
            args=[("$target_id", None)],
            return_type=ReturnType(["neighbor"], is_stream=True),
            docstring="Get all neighbors",
        )
        bound = fn.with_args(target_id="xyz")
        assert bound.name == "get-neighbors"
        assert bound.return_type.types == ["neighbor"]
        assert bound.docstring == "Get all neighbors"

    def test_with_args_hyphenated_param(self) -> None:
        """Hyphenated parameter names are converted to underscore."""
        fn = FunctionQuery(
            name="search",
            args=[("$search-term", None)],
            return_type=ReturnType(["result"], is_stream=True),
        )
        bound = fn.with_args(search_term="hello")
        assert bound.args == [("$search-term", "hello")]


class TestFunctionQueryRealWorldExamples:
    """Tests based on real Auto_K schema patterns."""

    def test_count_artifacts(self) -> None:
        """Simple count function."""
        fn = FunctionQuery(
            name="count-artifacts",
            return_type=ReturnType(["integer"]),
        )
        query = fn.to_query()
        expected = 'match\nlet $integer = count-artifacts();\nfetch {\n  "integer": $integer\n};'
        assert query == expected

    def test_list_user_artifact_ids(self) -> None:
        """Stream function returning single values."""
        fn = FunctionQuery(
            name="list-user-artifact-ids",
            return_type=ReturnType(["display-id"], is_stream=True),
        )
        query = fn.to_query(limit=100)
        assert "match\nlet $display_id in list-user-artifact-ids();" in query
        assert "limit 100;" in query

    def test_get_neighbor_ids(self) -> None:
        """Parameterized stream function."""
        fn = FunctionQuery(
            name="get-neighbor-ids",
            args=[("$target_id", "art-001")],
            return_type=ReturnType(["id"], is_stream=True),
        )
        query = fn.to_query()
        assert 'get-neighbor-ids("art-001")' in query
        assert "match\nlet $id in" in query

    def test_count_artifacts_by_type(self) -> None:
        """Stream function with composite return (tuples)."""
        fn = FunctionQuery(
            name="count-artifacts-by-type",
            return_type=ReturnType(["artifact", "integer"], is_stream=True),
        )
        query = fn.to_query()
        assert "match\nlet $artifact, $integer in count-artifacts-by-type();" in query
        assert '"artifact": $artifact' in query
        assert '"integer": $integer' in query
