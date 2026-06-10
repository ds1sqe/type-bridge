"""Breaking change analysis for TypeDB schema migrations.

This module provides classification of schema changes to help determine
which changes are safe, require warnings, or are breaking changes.
"""

from dataclasses import dataclass
from enum import Enum

from type_bridge.migration.diff import SchemaDiff


class ChangeCategory(Enum):
    """Classification of schema changes by severity."""

    SAFE = "safe"
    """Backwards compatible change - no data loss or errors."""

    WARNING = "warning"
    """May cause issues - review required."""

    BREAKING = "breaking"
    """Will cause data loss or errors - requires migration plan."""


@dataclass
class ClassifiedChange:
    """A schema change with its classification and recommendation."""

    description: str
    category: ChangeCategory
    recommendation: str


class BreakingChangeAnalyzer:
    """Analyzes schema diffs to classify changes by severity.

    Classification rules:
    - SAFE: Adding new types, widening role player types
    - WARNING: Adding required attributes to existing types
    - BREAKING: Removing types, narrowing role player types, removing roles

    Example:
        analyzer = BreakingChangeAnalyzer()
        diff = old_schema.compare(new_schema)
        changes = analyzer.analyze(diff)

        for change in changes:
            print(f"[{change.category.value}] {change.description}")
            print(f"  Recommendation: {change.recommendation}")
    """

    def analyze(self, diff: SchemaDiff) -> list[ClassifiedChange]:
        """Classify all changes in the schema diff.

        Args:
            diff: SchemaDiff from SchemaInfo.compare()

        Returns:
            List of classified changes with recommendations
        """
        from type_bridge._rust_runtime import classify_schema_diff

        rust_diff = diff._rust_diff if diff._rust_diff is not None else diff.to_rust_dict()
        return [
            ClassifiedChange(
                description=change["description"],
                category=ChangeCategory(change["category"]),
                recommendation=change["recommendation"],
            )
            for change in classify_schema_diff(rust_diff)
        ]

    def has_breaking_changes(self, diff: SchemaDiff) -> bool:
        """Quick check for any breaking changes.

        Args:
            diff: SchemaDiff from SchemaInfo.compare()

        Returns:
            True if any breaking changes exist
        """
        from type_bridge._rust_runtime import schema_diff_is_breaking

        rust_diff = diff._rust_diff if diff._rust_diff is not None else diff.to_rust_dict()
        return schema_diff_is_breaking(rust_diff)

    def has_warnings(self, diff: SchemaDiff) -> bool:
        """Quick check for any warning-level changes.

        Args:
            diff: SchemaDiff from SchemaInfo.compare()

        Returns:
            True if any warnings exist
        """
        classified = self.analyze(diff)
        return any(c.category == ChangeCategory.WARNING for c in classified)

    def get_breaking_changes(self, diff: SchemaDiff) -> list[ClassifiedChange]:
        """Get only breaking changes from the diff.

        Args:
            diff: SchemaDiff from SchemaInfo.compare()

        Returns:
            List of breaking changes only
        """
        return [c for c in self.analyze(diff) if c.category == ChangeCategory.BREAKING]

    def summary(self, diff: SchemaDiff) -> str:
        """Generate a human-readable summary of classified changes.

        Args:
            diff: SchemaDiff from SchemaInfo.compare()

        Returns:
            Formatted summary string
        """
        classified = self.analyze(diff)

        if not classified:
            return "No schema changes detected."

        lines = ["Schema Change Analysis", "=" * 50]

        # Group by category
        breaking = [c for c in classified if c.category == ChangeCategory.BREAKING]
        warnings = [c for c in classified if c.category == ChangeCategory.WARNING]
        safe = [c for c in classified if c.category == ChangeCategory.SAFE]

        if breaking:
            lines.append(f"\n[BREAKING] ({len(breaking)} changes)")
            for change in breaking:
                lines.append(f"  - {change.description}")
                lines.append(f"    Recommendation: {change.recommendation}")

        if warnings:
            lines.append(f"\n[WARNING] ({len(warnings)} changes)")
            for change in warnings:
                lines.append(f"  - {change.description}")
                lines.append(f"    Recommendation: {change.recommendation}")

        if safe:
            lines.append(f"\n[SAFE] ({len(safe)} changes)")
            for change in safe:
                lines.append(f"  - {change.description}")

        return "\n".join(lines)
