"""Tests for function generation support."""

from __future__ import annotations

import tempfile
from pathlib import Path

from type_bridge.generator import generate_models, parse_tql_schema


class TestParseFunctions:
    """Tests for function parsing."""

    def test_parse_simple_function(self) -> None:
        """Parse a simple function definition."""
        schema_text = """
            define

            fun calculate-age($birth-date: date) -> { int }:
                match
                    $p isa person, has birth-date $birth-date;
                return { 30 };
        """
        schema = parse_tql_schema(schema_text)

        assert "calculate-age" in schema.functions
        func = schema.functions["calculate-age"]
        assert func.name == "calculate-age"
        assert func.return_type.is_stream is True
        assert [(t.name, t.optional) for t in func.return_type.types] == [("int", False)]
        assert len(func.parameters) == 1
        assert func.parameters[0].name == "birth-date"
        assert func.parameters[0].type == "date"

    def test_parse_multi_arg_function(self) -> None:
        """Parse a function with multiple arguments."""
        schema_text = """
            define

            fun risk-score($age: int, $income: double) -> { double }:
                match
                    ...
                return { 0.5 };
        """
        schema = parse_tql_schema(schema_text)

        assert "risk-score" in schema.functions
        func = schema.functions["risk-score"]
        assert len(func.parameters) == 2
        assert func.parameters[0].name == "age"
        assert func.parameters[0].type == "int"
        assert func.parameters[1].name == "income"
        assert func.parameters[1].type == "double"

    def test_parse_stream_tuple_return(self) -> None:
        """Parse function with stream tuple return type."""
        schema_text = """
            define
            fun all_users_and_phones() -> { user, phone, string }:
                match $u isa user, has phone $p;
                return { $u, $p, "x" };
        """
        schema = parse_tql_schema(schema_text)

        func = schema.functions["all_users_and_phones"]
        assert func.return_type.is_stream is True
        assert [(t.name, t.optional) for t in func.return_type.types] == [
            ("user", False),
            ("phone", False),
            ("string", False),
        ]
        assert len(func.parameters) == 0

    def test_parse_single_scalar_return(self) -> None:
        """Parse function with single scalar return (no braces)."""
        schema_text = """
            define
            fun add($x: integer, $y: integer) -> integer:
                match let $z = $x + $y;
                return first $z;
        """
        schema = parse_tql_schema(schema_text)

        func = schema.functions["add"]
        assert func.return_type.is_stream is False
        assert [(t.name, t.optional) for t in func.return_type.types] == [("integer", False)]
        assert len(func.parameters) == 2

    def test_parse_tuple_return(self) -> None:
        """Parse function with tuple return type."""
        schema_text = """
            define
            fun divide($a: integer, $b: integer) -> integer, integer:
                match let $q = $a / $b;
                return first $q, $r;
        """
        schema = parse_tql_schema(schema_text)

        func = schema.functions["divide"]
        assert func.return_type.is_stream is False
        assert [(t.name, t.optional) for t in func.return_type.types] == [
            ("integer", False),
            ("integer", False),
        ]

    def test_parse_bool_return(self) -> None:
        """Parse function with bool return type."""
        schema_text = """
            define
            fun is_reachable($from: node, $to: node) -> bool:
                match ($from, $to) isa edge;
                return first true;
        """
        schema = parse_tql_schema(schema_text)

        func = schema.functions["is_reachable"]
        assert func.return_type.is_stream is False
        assert [(t.name, t.optional) for t in func.return_type.types] == [("bool", False)]

    def test_parse_optional_return_type(self) -> None:
        """Parse function with optional return type."""
        schema_text = """
            define
            fun any_place_with_optional_name() -> place, name?:
                match $p isa place;
                return first $p, $n;
        """
        schema = parse_tql_schema(schema_text)

        func = schema.functions["any_place_with_optional_name"]
        assert func.return_type.is_stream is False
        assert [(t.name, t.optional) for t in func.return_type.types] == [
            ("place", False),
            ("name", True),
        ]

    def test_parse_no_parameters(self) -> None:
        """Parse function with no parameters."""
        schema_text = """
            define
            fun mean_karma() -> double:
                match $user isa user, has karma $karma;
                return mean($karma);
        """
        schema = parse_tql_schema(schema_text)

        func = schema.functions["mean_karma"]
        assert func.return_type.is_stream is False
        assert [(t.name, t.optional) for t in func.return_type.types] == [("double", False)]
        assert len(func.parameters) == 0

    def test_parse_aggregate_return(self) -> None:
        """Parse function with aggregate return."""
        schema_text = """
            define
            fun karma_sum_and_sum_squares() -> double, double:
                match $karma isa karma;
                return sum($karma), sum($karma);
        """
        schema = parse_tql_schema(schema_text)

        func = schema.functions["karma_sum_and_sum_squares"]
        assert func.return_type.is_stream is False
        assert [(t.name, t.optional) for t in func.return_type.types] == [
            ("double", False),
            ("double", False),
        ]


class TestGenerateFunctions:
    """Tests for full generation pipeline including functions."""

    def test_generates_functions_file(self) -> None:
        """Generate package with functions.py."""
        schema_text = """
            define
            fun calculate-age($birth-date: date) -> { int }:
                return { 1 };

            entity person, owns age @key;
            attribute age, value int;
        """

        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "models"
            generate_models(schema_text, output)

            # Check functions.py exists
            assert (output / "functions.py").exists()
            assert (output / "__init__.py").exists()

            # Check __init__.py exports functions
            init_content = (output / "__init__.py").read_text()
            assert (
                "from . import attributes, entities, functions, registry, relations" in init_content
            )
            assert '"functions",' in init_content

            # Check content compiles
            functions_content = (output / "functions.py").read_text()
            compile(functions_content, "functions.py", "exec")

    def test_skips_functions_file_if_none(self) -> None:
        """Do not generate functions.py if no functions in schema."""
        schema_text = """
            define
            entity person, owns age @key;
            attribute age, value int;
        """

        with tempfile.TemporaryDirectory() as tmpdir:
            output = Path(tmpdir) / "models"
            generate_models(schema_text, output)

            assert not (output / "functions.py").exists()
            init_content = (output / "__init__.py").read_text()
            assert "functions" not in init_content
