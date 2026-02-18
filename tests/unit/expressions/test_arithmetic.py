"""Test arithmetic expressions for TypeQL queries."""

import pytest

from type_bridge.expressions.arithmetic import (
    ArithmeticExpr,
    add,
    div,
    mod,
    mul,
    pow_,
    sub,
)
from type_bridge.query.ast import ArithmeticValue
from type_bridge.query.compiler import QueryCompiler


class TestArithmeticExpr:
    """Test ArithmeticExpr dataclass."""

    def test_basic_addition(self):
        expr = ArithmeticExpr("$x", "+", "$y")
        assert expr.to_typeql("$unused") == "($x + $y)"

    def test_basic_subtraction(self):
        expr = ArithmeticExpr("$a", "-", "$b")
        assert expr.to_typeql("") == "($a - $b)"

    def test_basic_multiplication(self):
        expr = ArithmeticExpr("$price", "*", "$qty")
        assert expr.to_typeql("") == "($price * $qty)"

    def test_basic_division(self):
        expr = ArithmeticExpr("$total", "/", "$count")
        assert expr.to_typeql("") == "($total / $count)"

    def test_basic_modulo(self):
        expr = ArithmeticExpr("$x", "%", "$y")
        assert expr.to_typeql("") == "($x % $y)"

    def test_basic_power(self):
        expr = ArithmeticExpr("$base", "^", "$exp")
        assert expr.to_typeql("") == "($base ^ $exp)"

    def test_literal_operand(self):
        expr = ArithmeticExpr("$x", "+", "5")
        assert expr.to_typeql("") == "($x + 5)"

    def test_invalid_operator(self):
        with pytest.raises(ValueError, match="Invalid arithmetic operator"):
            ArithmeticExpr("$x", "&&", "$y")

    def test_to_ast_returns_empty(self):
        expr = ArithmeticExpr("$x", "+", "$y")
        assert expr.to_ast("$v") == []

    def test_to_value_ast(self):
        expr = ArithmeticExpr("$x", "+", "$y")
        value = expr.to_value_ast()
        assert isinstance(value, ArithmeticValue)
        assert value.left == "$x"
        assert value.operator == "+"
        assert value.right == "$y"

    def test_get_attribute_types_empty(self):
        expr = ArithmeticExpr("$x", "+", "$y")
        assert expr.get_attribute_types() == set()

    def test_equality(self):
        e1 = ArithmeticExpr("$x", "+", "$y")
        e2 = ArithmeticExpr("$x", "+", "$y")
        e3 = ArithmeticExpr("$x", "-", "$y")
        assert e1 == e2
        assert e1 != e3

    def test_equality_not_implemented(self):
        expr = ArithmeticExpr("$x", "+", "$y")
        assert expr.__eq__("not an expr") is NotImplemented

    def test_hash(self):
        e1 = ArithmeticExpr("$x", "+", "$y")
        e2 = ArithmeticExpr("$x", "+", "$y")
        assert hash(e1) == hash(e2)
        # Can be used in sets
        s = {e1, e2}
        assert len(s) == 1

    def test_repr(self):
        expr = ArithmeticExpr("$x", "+", "$y")
        assert repr(expr) == "ArithmeticExpr('$x', '+', '$y')"


class TestHelperFunctions:
    """Test the helper factory functions."""

    def test_add_variables(self):
        expr = add("x", "y")
        assert expr.to_typeql("") == "($x + $y)"

    def test_add_with_dollar_prefix(self):
        expr = add("$x", "$y")
        assert expr.to_typeql("") == "($x + $y)"

    def test_add_with_literal(self):
        expr = add("price", 10)
        assert expr.to_typeql("") == "($price + 10)"

    def test_sub_helper(self):
        expr = sub("total", "discount")
        assert expr.to_typeql("") == "($total - $discount)"

    def test_mul_helper(self):
        expr = mul("price", "quantity")
        assert expr.to_typeql("") == "($price * $quantity)"

    def test_div_helper(self):
        expr = div("total", "count")
        assert expr.to_typeql("") == "($total / $count)"

    def test_mod_helper(self):
        expr = mod("x", 2)
        assert expr.to_typeql("") == "($x % 2)"

    def test_pow_helper(self):
        expr = pow_("base", 3)
        assert expr.to_typeql("") == "($base ^ 3)"

    def test_float_literal(self):
        expr = mul("price", 1.1)
        assert expr.to_typeql("") == "($price * 1.1)"


class TestCompilerIntegration:
    """Test that ArithmeticValue compiles correctly via QueryCompiler."""

    def test_compile_simple(self):
        compiler = QueryCompiler()
        node = ArithmeticValue(left="$x", operator="+", right="$y")
        result = compiler.compile(node)
        assert result == "($x + $y)"

    def test_compile_nested(self):
        compiler = QueryCompiler()
        inner = ArithmeticValue(left="$x", operator="+", right="$y")
        outer = ArithmeticValue(left=inner, operator="*", right="$z")
        result = compiler.compile(outer)
        assert result == "(($x + $y) * $z)"

    def test_compile_from_expr(self):
        expr = add("x", "y")
        value = expr.to_value_ast()
        compiler = QueryCompiler()
        result = compiler.compile(value)
        assert result == "($x + $y)"


class TestNestedExpressions:
    """Test composing arithmetic expressions."""

    def test_nested_typeql_via_compiler(self):
        """Test nested expression through AST and compiler."""
        inner = ArithmeticValue(left="$a", operator="+", right="$b")
        outer = ArithmeticValue(left=inner, operator="*", right="$c")
        compiler = QueryCompiler()
        result = compiler.compile(outer)
        assert result == "(($a + $b) * $c)"

    def test_deeply_nested(self):
        """Test three levels of nesting."""
        compiler = QueryCompiler()
        level1 = ArithmeticValue(left="$x", operator="+", right="$y")
        level2 = ArithmeticValue(left=level1, operator="*", right="$z")
        level3 = ArithmeticValue(left=level2, operator="-", right="$w")
        result = compiler.compile(level3)
        assert result == "((($x + $y) * $z) - $w)"
