"""Tests for TypeQL 3.8.0 built-in function expressions."""

import pytest

from type_bridge.expressions.builtin import (
    BuiltinFunctionExpr,
    abs_,
    ceil,
    floor,
    iid,
    label,
    len_,
    max_,
    min_,
    round_,
)


class TestBuiltinFunctionExpr:
    """Tests for the BuiltinFunctionExpr class."""

    def test_basic_construction(self):
        """Test basic construction of BuiltinFunctionExpr."""
        expr = BuiltinFunctionExpr("iid", "$e")
        assert expr.name == "iid"
        assert expr.args == ("$e",)

    def test_to_typeql_single_arg(self):
        """Test TypeQL generation with single argument."""
        expr = BuiltinFunctionExpr("iid", "$e")
        assert expr.to_typeql("$ignored") == "iid($e)"

    def test_to_typeql_multiple_args(self):
        """Test TypeQL generation with multiple arguments."""
        expr = BuiltinFunctionExpr("max", "$a", "$b", "$c")
        assert expr.to_typeql("$ignored") == "max($a, $b, $c)"

    def test_get_attribute_types(self):
        """Test that built-in functions don't reference attribute types."""
        expr = BuiltinFunctionExpr("iid", "$e")
        assert expr.get_attribute_types() == set()

    def test_repr(self):
        """Test string representation."""
        expr = BuiltinFunctionExpr("iid", "$e")
        assert repr(expr) == "BuiltinFunctionExpr('iid', '$e')"

    def test_equality(self):
        """Test equality comparison."""
        expr1 = BuiltinFunctionExpr("iid", "$e")
        expr2 = BuiltinFunctionExpr("iid", "$e")
        expr3 = BuiltinFunctionExpr("label", "$e")
        expr4 = BuiltinFunctionExpr("iid", "$x")

        assert expr1 == expr2
        assert expr1 != expr3
        assert expr1 != expr4

    def test_hash(self):
        """Test that expressions can be used in sets/dicts."""
        expr1 = BuiltinFunctionExpr("iid", "$e")
        expr2 = BuiltinFunctionExpr("iid", "$e")
        expr3 = BuiltinFunctionExpr("label", "$e")

        # Same expressions should have same hash
        assert hash(expr1) == hash(expr2)

        # Can be used in sets
        expr_set = {expr1, expr2, expr3}
        assert len(expr_set) == 2  # expr1 and expr2 are equal


class TestIidFunction:
    """Tests for the iid() helper function."""

    def test_iid_with_dollar_prefix(self):
        """Test iid() with variable that has $ prefix."""
        expr = iid("$e")
        assert expr.to_typeql("") == "iid($e)"

    def test_iid_without_dollar_prefix(self):
        """Test iid() normalizes variables without $ prefix."""
        expr = iid("e")
        assert expr.to_typeql("") == "iid($e)"

    def test_iid_returns_builtin_expr(self):
        """Test that iid() returns BuiltinFunctionExpr."""
        expr = iid("$e")
        assert isinstance(expr, BuiltinFunctionExpr)
        assert expr.name == "iid"


class TestLabelFunction:
    """Tests for the label() helper function."""

    def test_label_with_dollar_prefix(self):
        """Test label() with variable that has $ prefix."""
        expr = label("$t")
        assert expr.to_typeql("") == "label($t)"

    def test_label_without_dollar_prefix(self):
        """Test label() normalizes variables without $ prefix."""
        expr = label("t")
        assert expr.to_typeql("") == "label($t)"

    def test_label_returns_builtin_expr(self):
        """Test that label() returns BuiltinFunctionExpr."""
        expr = label("$t")
        assert isinstance(expr, BuiltinFunctionExpr)
        assert expr.name == "label"


class TestMathFunctions:
    """Tests for math helper functions."""

    def test_abs(self):
        """Test abs_() function."""
        expr = abs_("x")
        assert expr.to_typeql("") == "abs($x)"
        assert expr.name == "abs"

    def test_ceil(self):
        """Test ceil() function."""
        expr = ceil("$x")
        assert expr.to_typeql("") == "ceil($x)"
        assert expr.name == "ceil"

    def test_floor(self):
        """Test floor() function."""
        expr = floor("x")
        assert expr.to_typeql("") == "floor($x)"
        assert expr.name == "floor"

    def test_round(self):
        """Test round_() function."""
        expr = round_("$x")
        assert expr.to_typeql("") == "round($x)"
        assert expr.name == "round"


class TestCollectionFunctions:
    """Tests for collection helper functions."""

    def test_len(self):
        """Test len_() function."""
        expr = len_("items")
        assert expr.to_typeql("") == "len($items)"
        assert expr.name == "len"

    def test_max_two_args(self):
        """Test max_() with two arguments."""
        expr = max_("a", "b")
        assert expr.to_typeql("") == "max($a, $b)"
        assert expr.name == "max"

    def test_max_multiple_args(self):
        """Test max_() with multiple arguments."""
        expr = max_("$a", "$b", "$c", "$d")
        assert expr.to_typeql("") == "max($a, $b, $c, $d)"

    def test_min_two_args(self):
        """Test min_() with two arguments."""
        expr = min_("a", "b")
        assert expr.to_typeql("") == "min($a, $b)"
        assert expr.name == "min"

    def test_min_multiple_args(self):
        """Test min_() with multiple arguments."""
        expr = min_("$x", "$y", "$z")
        assert expr.to_typeql("") == "min($x, $y, $z)"


class TestVariableNormalization:
    """Tests for variable name normalization across all functions."""

    @pytest.mark.parametrize(
        "func,input_var,expected",
        [
            (iid, "e", "iid($e)"),
            (iid, "$e", "iid($e)"),
            (label, "t", "label($t)"),
            (label, "$t", "label($t)"),
            (abs_, "num", "abs($num)"),
            (abs_, "$num", "abs($num)"),
            (ceil, "val", "ceil($val)"),
            (floor, "val", "floor($val)"),
            (round_, "val", "round($val)"),
            (len_, "list", "len($list)"),
        ],
    )
    def test_normalization(self, func, input_var, expected):
        """Test that all functions normalize variable names."""
        expr = func(input_var)
        assert expr.to_typeql("") == expected
