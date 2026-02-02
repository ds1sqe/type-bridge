"""Function call expression support for TypeDB schema-defined functions."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any

from .base import Expression

if TYPE_CHECKING:
    from type_bridge.query.ast import FunctionCallValue, Pattern, Value


@dataclass
class FunctionCallExpr[T](Expression):
    """Represents a call to a TypeDB function."""

    name: str
    args: list[Any]

    def to_ast(self, var: str) -> list[Pattern]:
        raise NotImplementedError("Function calls are values, not patterns. Use to_value_ast().")

    def to_value_ast(self) -> Value:
        """Convert to AST FunctionCallValue."""
        from type_bridge.query.ast import FunctionCallValue, LiteralValue

        ast_args: list[Value | str] = []
        for arg in self.args:
            if isinstance(arg, Expression):
                ast_args.append(arg.to_value_ast())
            elif isinstance(arg, bool):
                ast_args.append(LiteralValue(arg, "boolean"))
            elif isinstance(arg, int):
                ast_args.append(LiteralValue(arg, "long"))
            elif isinstance(arg, float):
                ast_args.append(LiteralValue(arg, "double"))
            elif isinstance(arg, str):
                # Variables pass through as-is
                if arg.startswith("$"):
                    ast_args.append(arg)
                else:
                    ast_args.append(LiteralValue(arg, "string"))
            else:
                # Unknown types default to string
                ast_args.append(LiteralValue(arg, "string"))

        return FunctionCallValue(function=self.name, args=ast_args)

    def to_typeql(self, var: str) -> str:
        """Deprecated: generate TypeQL function call syntax."""
        arg_strs = []
        for arg in self.args:
            if isinstance(arg, Expression):
                arg_strs.append(arg.to_typeql(var))
            else:
                arg_strs.append(str(arg))

        return f"{self.name}({', '.join(arg_strs)})"


@dataclass
class ReturnType:
    """Describes the return type of a TypeDB function.

    Attributes:
        types: List of type names (e.g., ["integer"] or ["artifact", "integer"])
        is_stream: True if function returns multiple rows (uses { } syntax)
        is_optional: List of booleans indicating if each type is optional
    """

    types: list[str]
    is_stream: bool = False
    is_optional: list[bool] = field(default_factory=list)

    def __post_init__(self) -> None:
        if not self.is_optional:
            self.is_optional = [False] * len(self.types)

    @property
    def is_composite(self) -> bool:
        """True if function returns multiple values per row (tuple)."""
        return len(self.types) > 1

    @property
    def is_single_value(self) -> bool:
        """True if function returns exactly one value (not a stream)."""
        return not self.is_stream and len(self.types) == 1


@dataclass
class FunctionQuery[T]:
    """A TypeDB function call with query generation capabilities.

    This class wraps a TypeDB schema function and provides methods to generate
    complete TypeQL queries for calling the function.

    Attributes:
        name: The TypeDB function name (e.g., "count-artifacts")
        args: Ordered list of (param_name, value) tuples
        return_type: Description of what the function returns
        docstring: Optional documentation for the function

    Example:
        >>> # Simple count function
        >>> fn = FunctionQuery(
        ...     name="count-artifacts",
        ...     return_type=ReturnType(["integer"], is_stream=False),
        ... )
        >>> fn.to_query()
        'match let $result = count-artifacts(); fetch { "result": $result };'

        >>> # Stream function with parameter
        >>> fn = FunctionQuery(
        ...     name="get-neighbor-ids",
        ...     args=[("$target_id", '"abc-123"')],
        ...     return_type=ReturnType(["id"], is_stream=True),
        ... )
        >>> fn.to_query(limit=10)
        'match let $id in get-neighbor-ids("abc-123"); limit 10; fetch { "id": $id };'

        >>> # Composite return type
        >>> fn = FunctionQuery(
        ...     name="count-artifacts-by-type",
        ...     return_type=ReturnType(["artifact", "integer"], is_stream=True),
        ... )
        >>> fn.to_query()
        'match let ($artifact, $integer) in count-artifacts-by-type(); fetch { "artifact": $artifact, "integer": $integer };'
    """

    name: str
    return_type: ReturnType
    args: list[tuple[str, Any]] = field(default_factory=list)
    docstring: str | None = None

    def _format_arg_ast(self, value: Any) -> Value | str:
        """Format an argument value for AST."""
        from type_bridge.query.ast import LiteralValue

        if isinstance(value, str):
            if value.startswith("$"):
                return value
            # Already-quoted strings pass through as raw values
            if value.startswith('"') and value.endswith('"'):
                return value
            return LiteralValue(value, "string")

        if isinstance(value, Expression):
            return value.to_value_ast()

        if isinstance(value, bool):
            return LiteralValue(value, "boolean")
        if isinstance(value, int):
            return LiteralValue(value, "long")
        if isinstance(value, float):
            return LiteralValue(value, "double")

        # fallback for other types (datetime etc not handled in this basic map yet)
        return LiteralValue(value, "string")

    def _get_result_vars(self, custom_vars: list[str] | None = None) -> list[str]:
        """Get variable names for the function results."""
        if custom_vars:
            return [v if v.startswith("$") else f"${v}" for v in custom_vars]

        # Generate default variable names from return types
        return [f"${t.replace('-', '_')}" for t in self.return_type.types]

    def to_call_ast(self) -> FunctionCallValue:
        """Generate AST for function call."""
        from type_bridge.query.ast import FunctionCallValue

        ast_args = [self._format_arg_ast(v) for _, v in self.args]
        return FunctionCallValue(function=self.name, args=ast_args)

    def to_call(self) -> str:
        """Generate just the function call expression string."""
        from type_bridge.query.compiler import QueryCompiler

        return QueryCompiler()._compile_value(self.to_call_ast())

    def to_match_let(
        self,
        result_vars: list[str] | None = None,
    ) -> str:
        """Generate the match let clause string for calling this function."""
        vars_list = self._get_result_vars(result_vars)
        call_ast = self.to_call_ast()
        is_stream = self.return_type.is_stream

        from type_bridge.query.ast import LetAssignment, MatchLetClause
        from type_bridge.query.compiler import QueryCompiler

        assignment = LetAssignment(variables=vars_list, expression=call_ast, is_stream=is_stream)
        clause = MatchLetClause(assignments=[assignment])
        compiler = QueryCompiler()

        return compiler.compile(clause)

    def to_fetch(
        self,
        result_vars: list[str] | None = None,
        fetch_keys: list[str] | None = None,
    ) -> str:
        """Generate the fetch clause string for the function results."""
        from type_bridge.query.ast import FetchClause, FetchVariable
        from type_bridge.query.compiler import QueryCompiler

        vars_list = self._get_result_vars(result_vars)

        if fetch_keys:
            keys = fetch_keys
        else:
            # Use type names as keys (without $)
            keys = [v.lstrip("$") for v in vars_list]

        items = [FetchVariable(key=k, var=v) for k, v in zip(keys, vars_list, strict=True)]
        clause = FetchClause(items=items)
        compiler = QueryCompiler()

        return compiler.compile(clause)

    def to_query(
        self,
        result_vars: list[str] | None = None,
        fetch_keys: list[str] | None = None,
        limit: int | None = None,
        offset: int | None = None,
        sort_var: str | None = None,
        sort_order: str = "asc",
    ) -> str:
        """Generate a complete TypeQL query for calling this function.

        Args:
            result_vars: Optional custom variable names for results
            fetch_keys: Optional custom keys for the fetch object
            limit: Optional limit on number of results
            offset: Optional offset for pagination
            sort_var: Optional variable to sort by
            sort_order: Sort order ("asc" or "desc")

        Returns:
            Complete TypeQL query string

        Example:
            >>> fn.to_query(limit=10, offset=5)
            'match let $id in get-ids(); offset 5; limit 10; fetch { "id": $id };'
        """
        parts = [self.to_match_let(result_vars)]

        # Add sort if specified (must come before offset/limit)
        if sort_var:
            var = sort_var if sort_var.startswith("$") else f"${sort_var}"
            parts.append(f"sort {var} {sort_order};")

        # Add offset before limit (TypeQL order)
        if offset is not None:
            parts.append(f"offset {offset};")

        if limit is not None:
            parts.append(f"limit {limit};")

        parts.append(self.to_fetch(result_vars, fetch_keys))

        return "\n".join(parts)

    def to_reduce_query(
        self,
        result_vars: list[str] | None = None,
    ) -> str:
        """Generate a query that returns the raw reduce result.

        Useful for single-value functions like count() where you just
        want the number without fetch overhead.

        Returns:
            Query using reduce syntax
        """
        if self.return_type.is_stream:
            raise ValueError("Cannot use to_reduce_query with stream functions")

        return self.to_match_let(result_vars)

    def with_args(self, **kwargs: Any) -> FunctionQuery[T]:
        """Create a new FunctionQuery with the given arguments.

        This is useful for parameterized functions where you want to
        create a bound version with specific argument values.

        Args:
            **kwargs: Argument values keyed by parameter name (without $)

        Returns:
            New FunctionQuery with the arguments set

        Example:
            >>> fn = get_neighbor_ids.with_args(target_id="abc-123")
            >>> fn.to_query()
        """
        # Map kwargs to our args format
        new_args = []
        for param_name, _ in self.args:
            clean_name = param_name.lstrip("$").replace("-", "_")
            if clean_name in kwargs:
                new_args.append((param_name, kwargs[clean_name]))
            else:
                new_args.append((param_name, None))

        return FunctionQuery(
            name=self.name,
            return_type=self.return_type,
            args=new_args,
            docstring=self.docstring,
        )
