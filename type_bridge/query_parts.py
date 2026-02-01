"""Query parts/clauses for the robust QueryBuilder."""

from abc import ABC, abstractmethod


class QueryPart(ABC):
    """Abstract base class for query parts."""

    @abstractmethod
    def build(self) -> str:
        """Build the TypeQL string for this part."""
        ...


class MatchBlock(QueryPart):
    """Represents a 'match' block in TypeQL."""

    def __init__(self) -> None:
        self.patterns: list[str] = []

    def add(self, pattern: str) -> None:
        """Add a pattern to the match block."""
        if pattern:
            self.patterns.append(pattern.strip().rstrip(";"))

    def is_empty(self) -> bool:
        return not self.patterns

    def build(self) -> str:
        if not self.patterns:
            return ""
        # Join patterns with semicolons and newlines
        body = ";\n".join(self.patterns)
        return f"match\n{body};"


class DeleteBlock(QueryPart):
    """Represents a 'delete' block in TypeQL."""

    def __init__(self) -> None:
        self.patterns: list[str] = []

    def add(self, pattern: str) -> None:
        if pattern:
            self.patterns.append(pattern.strip().rstrip(";"))

    def is_empty(self) -> bool:
        return not self.patterns

    def build(self) -> str:
        if not self.patterns:
            return ""
        body = ";\n".join(self.patterns)
        return f"delete\n{body};"


class InsertBlock(QueryPart):
    """Represents an 'insert' block in TypeQL."""

    def __init__(self) -> None:
        self.patterns: list[str] = []

    def add(self, pattern: str) -> None:
        if pattern:
            self.patterns.append(pattern.strip().rstrip(";"))

    def is_empty(self) -> bool:
        return not self.patterns

    def build(self) -> str:
        if not self.patterns:
            return ""
        body = ";\n".join(self.patterns)
        return f"insert\n{body};"


class FetchBlock(QueryPart):
    """Represents a 'fetch' block in TypeQL."""

    def __init__(self) -> None:
        # Maps variable name to list of attributes/projections
        self.projections: dict[str, list[str]] = {}

    def add(self, variable: str, attributes: list[str] | None = None) -> None:
        """Add a fetch projection.
        
        Args:
            variable: The variable to fetch (e.g. "$x")
            attributes: Optional specific attributes (not used in generic fetch { $x.* })
        """
        if variable not in self.projections:
            self.projections[variable] = []
        if attributes:
            self.projections[variable].extend(attributes)

    def is_empty(self) -> bool:
        return not self.projections

    def build(self) -> str:
        if not self.projections:
            return ""
        
        lines = []
        for var, _attrs in self.projections.items():
            # For now, default to fetching all attributes ($var.*)
            # TODO: Support specific attribute fetching when _attrs is non-empty
            lines.append(f"  {var}.*")
            
        body = ",\n".join(lines)
        return f"fetch {{\n{body}\n}};"


class Modifiers(QueryPart):
    """Represents query modifiers (sort, offset, limit)."""

    def __init__(self) -> None:
        self.sort_clauses: list[tuple[str, str]] = []
        self.offset_val: int | None = None
        self.limit_val: int | None = None

    def sort(self, variable: str, direction: str = "asc") -> None:
        if direction not in ("asc", "desc"):
            raise ValueError(f"Invalid sort direction: {direction}")
        self.sort_clauses.append((variable, direction))

    def offset(self, offset: int) -> None:
        self.offset_val = offset

    def limit(self, limit: int) -> None:
        self.limit_val = limit

    def is_empty(self) -> bool:
        return not self.sort_clauses and self.offset_val is None and self.limit_val is None

    def build(self) -> str:
        parts = []
        if self.sort_clauses:
            sort_items = [f"{var} {dir}" for var, dir in self.sort_clauses]
            parts.append(f"sort {', '.join(sort_items)};")
        
        # Order matters: offset then limit
        if self.offset_val is not None:
            parts.append(f"offset {self.offset_val};")
        if self.limit_val is not None:
            parts.append(f"limit {self.limit_val};")
            
        return "\n".join(parts)
