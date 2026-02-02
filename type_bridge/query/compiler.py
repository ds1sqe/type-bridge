"Compiler for converting AST nodes to TypeQL strings."

from __future__ import annotations

import logging

from type_bridge.query.ast import (
    AttributePattern,
    Clause,
    Constraint,
    DeleteClause,
    DeleteThingStatement,
    EntityPattern,
    FetchAttribute,
    FetchAttributeList,
    FetchClause,
    FetchFunction,
    FetchItem,
    FetchVariable,
    FetchWildcard,
    FunctionCallValue,
    HasConstraint,
    HasPattern,
    HasStatement,
    IidConstraint,
    IidPattern,
    InsertClause,
    IsaConstraint,
    IsaStatement,
    LetAssignment,
    LiteralValue,
    MatchClause,
    MatchLetClause,
    NotPattern,
    OrPattern,
    Pattern,
    QueryNode,
    RawPattern,
    RawStatement,
    ReduceAssignment,
    ReduceClause,
    RelationPattern,
    RelationStatement,
    Statement,
    SubTypePattern,
    UpdateClause,
    Value,
    ValueComparisonPattern,
)

logger = logging.getLogger(__name__)


class QueryCompiler:
    """Compiles AST nodes into TypeQL strings."""

    def compile(self, node: QueryNode) -> str:
        """Compile a single node to TypeQL."""
        if isinstance(node, Clause):
            return self._compile_clause(node)
        elif isinstance(node, Pattern):
            return self._compile_pattern(node)
        elif isinstance(node, Statement):
            return self._compile_statement(node)
        elif isinstance(node, Constraint):
            return self._compile_constraint(node)
        elif isinstance(node, Value):
            return self._compile_value(node)
        else:
            raise ValueError(f"Unknown node type: {type(node).__name__}")

    def compile_batch(self, nodes: list[QueryNode], operation: str = "") -> str:
        """Compile a list of nodes into a single query string.

        Args:
            nodes: List of clauses to compile
            operation: Optional operation keyword (e.g., "match", "insert") to prepend
                       if the nodes are just raw patterns/statements.
        """
        parts = [self.compile(node) for node in nodes]
        query = "\n".join(parts)
        if operation:
            return f"{operation}\n{query}"
        return query

    # --- Clauses ---

    def _compile_clause(self, clause: Clause) -> str:
        if isinstance(clause, MatchClause):
            patterns = [self._compile_pattern(p) for p in clause.patterns]
            return "match\n" + ";\n".join(patterns) + ";"

        elif isinstance(clause, MatchLetClause):
            return self._compile_match_let(clause)

        elif isinstance(clause, InsertClause):
            statements = [self._compile_statement(s) for s in clause.statements]
            return "insert\n" + ";\n".join(statements) + ";"

        elif isinstance(clause, DeleteClause):
            statements = [self._compile_statement(s) for s in clause.statements]
            return "delete\n" + ";\n".join(statements) + ";"

        elif isinstance(clause, UpdateClause):
            statements = [self._compile_statement(s) for s in clause.statements]
            # TypeDB 3.x typically uses 'delete' then 'insert' for updates,
            # but 'update' keyword exists as syntactic sugar for simple attribute replacement.
            # We assume the AST has already resolved this intent.
            return "update\n" + ";\n".join(statements) + ";"

        elif isinstance(clause, FetchClause):
            compiled_items = [self._compile_fetch_item(item) for item in clause.items]
            return "fetch {\n  " + ",\n  ".join(compiled_items) + "\n};"

        elif isinstance(clause, ReduceClause):
            assignments = [self._compile_reduce_assignment(a) for a in clause.assignments]
            reduce_str = "reduce " + ", ".join(assignments)
            if clause.group_by:
                reduce_str += f" groupby {clause.group_by}"
            return reduce_str + ";"

        raise ValueError(f"Unknown clause type: {type(clause).__name__}")

    def _compile_match_let(self, clause: MatchLetClause) -> str:
        assignments = [self._compile_let_assignment(a) for a in clause.assignments]
        return "match\n" + ";\n".join(assignments) + ";"

    def _compile_let_assignment(self, assignment: LetAssignment) -> str:
        vars_str = ", ".join(assignment.variables)
        op = "in" if assignment.is_stream else "="
        if len(assignment.variables) > 1:
            vars_str = f"({vars_str})"
        if isinstance(assignment.expression, Value):
            expr_str = self._compile_value(assignment.expression)
        else:
            expr_str = assignment.expression
        return f"let {vars_str} {op} {expr_str}"

    # --- Patterns (Match) ---

    def _compile_pattern(self, pattern: Pattern) -> str:
        if isinstance(pattern, EntityPattern):
            op = "isa!" if pattern.is_strict else "isa"
            parts = [f"{pattern.variable} {op} {pattern.type_name}"]
            for constraint in pattern.constraints:
                parts.append(self._compile_constraint(constraint))
            return ", ".join(parts)

        elif isinstance(pattern, RelationPattern):
            # TypeDB 3.x syntax: $r isa type (role: $player)
            # Order matters: isa comes before role players
            if pattern.role_players:
                role_parts = [f"{rp.role}: {rp.player_var}" for rp in pattern.role_players]
                roles_str = f"({', '.join(role_parts)})"
                parts = [f"{pattern.variable} isa {pattern.type_name} {roles_str}"]
            else:
                parts = [f"{pattern.variable} isa {pattern.type_name}"]
            for constraint in pattern.constraints:
                parts.append(self._compile_constraint(constraint))
            return ", ".join(parts)

        elif isinstance(pattern, SubTypePattern):
            return f"{pattern.variable} sub {pattern.parent_type}"

        elif isinstance(pattern, HasPattern):
            return f"{pattern.thing_var} has {pattern.attr_type} {pattern.attr_var}"

        elif isinstance(pattern, ValueComparisonPattern):
            if isinstance(pattern.value, Value):
                val_str = self._compile_value(pattern.value)
            else:
                val_str = pattern.value
            return f"{pattern.var} {pattern.operator} {val_str}"

        elif isinstance(pattern, NotPattern):
            sub_patterns = [self._compile_pattern(p) for p in pattern.patterns]
            # not { $p1; $p2; };
            inner = "; ".join(sub_patterns)
            return f"not {{ {inner}; }}"

        elif isinstance(pattern, OrPattern):
            # { p1; p2; } or { p3; };
            blocks = []
            for alt in pattern.alternatives:
                sub_patterns = [self._compile_pattern(p) for p in alt]
                block_content = "; ".join(sub_patterns)
                blocks.append(f"{{ {block_content}; }}")
            return " or ".join(blocks)

        elif isinstance(pattern, IidPattern):
            return f"{pattern.variable} iid {pattern.iid}"

        elif isinstance(pattern, AttributePattern):
            # $a isa age; or $a isa age; $a 30;
            parts = [f"{pattern.variable} isa {pattern.type_name}"]
            if pattern.value is not None:
                val_str = self._compile_value(pattern.value)
                parts.append(f"{pattern.variable} {val_str}")
            return "; ".join(parts)

        elif isinstance(pattern, RawPattern):
            return pattern.content

        raise ValueError(f"Unknown pattern type: {type(pattern).__name__}")

    # --- Statements (Insert/Delete) ---

    def _compile_statement(self, stmt: Statement) -> str:
        if isinstance(stmt, HasStatement):
            val_str = self._compile_value(stmt.value)
            return f"{stmt.subject_var} has {stmt.attr_name} {val_str}"

        elif isinstance(stmt, IsaStatement):
            return f"{stmt.variable} isa {stmt.type_name}"

        elif isinstance(stmt, RelationStatement):
            role_parts = [f"{rp.role}: {rp.player_var}" for rp in stmt.role_players]
            roles_str = f"({', '.join(role_parts)})"
            # TypeDB 3.x insert doesn't use variable prefix for relations
            if stmt.include_variable:
                base = f"{stmt.variable} {roles_str} isa {stmt.type_name}"
            else:
                base = f"{roles_str} isa {stmt.type_name}"

            # Add inline attributes (for insert statements without variable)
            if stmt.attributes:
                attr_parts = []
                for attr in stmt.attributes:
                    val_str = self._compile_value(attr.value)
                    attr_parts.append(f"has {attr.attr_name} {val_str}")
                return f"{base}, {', '.join(attr_parts)}"
            return base

        elif isinstance(stmt, DeleteThingStatement):
            return f"{stmt.variable}"

        elif isinstance(stmt, RawStatement):
            return stmt.content

        raise ValueError(f"Unknown statement type: {type(stmt).__name__}")

    # --- Constraints ---

    def _compile_constraint(self, constraint: Constraint) -> str:
        if isinstance(constraint, IidConstraint):
            return f"iid {constraint.iid}"

        elif isinstance(constraint, HasConstraint):
            if isinstance(constraint.value, Value):
                val_str = self._compile_value(constraint.value)
            else:
                # It's a variable string
                val_str = constraint.value
            return f"has {constraint.attr_name} {val_str}"

        elif isinstance(constraint, IsaConstraint):
            op = "isa!" if constraint.strict else "isa"
            return f"{op} {constraint.type_name}"

        raise ValueError(f"Unknown constraint type: {type(constraint).__name__}")

    # --- Values & Escaping ---

    def _compile_value(self, value_node: Value) -> str:
        if isinstance(value_node, FunctionCallValue):
            args_str = []
            for arg in value_node.args:
                if isinstance(arg, Value):
                    args_str.append(self._compile_value(arg))
                else:
                    args_str.append(arg)
            return f"{value_node.function}({', '.join(args_str)})"

        # Use existing formatting logic from type_bridge.crud.formatting if available,
        # otherwise implement robust escaping here.
        # For Phase 1, we import the existing robust formatter.
        from type_bridge.crud.formatting import format_value

        if isinstance(value_node, LiteralValue):
            return format_value(value_node.value)
        raise ValueError(f"Unknown value type: {type(value_node).__name__}")

    # --- Fetch Items ---

    def _compile_fetch_item(self, item: FetchItem | str) -> str:
        """Compile a fetch item to TypeQL string."""
        # Backwards compatibility: raw strings pass through
        if isinstance(item, str):
            return item

        if isinstance(item, FetchAttribute):
            return f'"{item.key}": {item.var}.{item.attr_name}'

        elif isinstance(item, FetchVariable):
            return f'"{item.key}": {item.var}'

        elif isinstance(item, FetchAttributeList):
            return f'"{item.key}": [{item.var}.{item.attr_name}]'

        elif isinstance(item, FetchFunction):
            return f'"{item.key}": {item.func_name}({item.var})'

        elif isinstance(item, FetchWildcard):
            return f'"{item.key}": {item.var}.*'

        raise ValueError(f"Unknown fetch item type: {type(item).__name__}")

    # --- Reduce/Aggregate ---

    def _compile_reduce_assignment(self, assignment: ReduceAssignment) -> str:
        """Compile a reduce assignment like $count = count($var)."""
        if isinstance(assignment.expression, Value):
            expr_str = self._compile_value(assignment.expression)
        else:
            expr_str = assignment.expression
        return f"{assignment.variable} = {expr_str}"
