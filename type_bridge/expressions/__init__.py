"""
Type-safe query expression system for TypeBridge.

This module provides expression classes for building type-safe TypeQL queries.
"""

from type_bridge.expressions.aggregate import AggregateExpr
from type_bridge.expressions.base import Expression
from type_bridge.expressions.boolean import BooleanExpr
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
from type_bridge.expressions.comparison import AttributeExistsExpr, ComparisonExpr
from type_bridge.expressions.functions import (
    FunctionCall,
    FunctionCallExpr,
    FunctionQuery,
    ReturnType,
)
from type_bridge.expressions.iid import IidExpr
from type_bridge.expressions.role_player import RolePlayerExpr
from type_bridge.expressions.string import StringExpr

__all__ = [
    "Expression",
    "ComparisonExpr",
    "AttributeExistsExpr",
    "StringExpr",
    "BooleanExpr",
    "AggregateExpr",
    "FunctionCallExpr",
    "FunctionCall",
    "FunctionQuery",
    "ReturnType",
    "IidExpr",
    "RolePlayerExpr",
    # Built-in functions (TypeQL 3.8.0+)
    "BuiltinFunctionExpr",
    "iid",
    "label",
    "abs_",
    "ceil",
    "floor",
    "round_",
    "len_",
    "max_",
    "min_",
]
