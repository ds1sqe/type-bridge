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
        changes: list[ClassifiedChange] = []

        # Analyze entity changes
        changes.extend(self._analyze_entity_changes(diff))

        # Analyze relation changes
        changes.extend(self._analyze_relation_changes(diff))

        # Analyze attribute changes
        changes.extend(self._analyze_attribute_changes(diff))

        return changes

    def has_breaking_changes(self, diff: SchemaDiff) -> bool:
        """Quick check for any breaking changes.

        Args:
            diff: SchemaDiff from SchemaInfo.compare()

        Returns:
            True if any breaking changes exist
        """
        classified = self.analyze(diff)
        return any(c.category == ChangeCategory.BREAKING for c in classified)

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

    def _analyze_entity_changes(self, diff: SchemaDiff) -> list[ClassifiedChange]:
        """Analyze entity additions, removals, and modifications."""
        changes: list[ClassifiedChange] = []

        # Added entities - SAFE
        for entity in diff.added_entities:
            type_name = entity.get_type_name()
            changes.append(
                ClassifiedChange(
                    description=f"Add entity: {type_name}",
                    category=ChangeCategory.SAFE,
                    recommendation="No action needed",
                )
            )

        # Removed entities - BREAKING
        for entity in diff.removed_entities:
            type_name = entity.get_type_name()
            changes.append(
                ClassifiedChange(
                    description=f"Remove entity: {type_name}",
                    category=ChangeCategory.BREAKING,
                    recommendation=f"Delete all '{type_name}' instances before removing the type",
                )
            )

        # Modified entities
        for entity, entity_changes in diff.modified_entities.items():
            type_name = entity.get_type_name()

            # Added attributes - WARNING (may require data backfill)
            for attr_name in entity_changes.added_attributes:
                changes.append(
                    ClassifiedChange(
                        description=f"Add attribute '{attr_name}' to entity '{type_name}'",
                        category=ChangeCategory.WARNING,
                        recommendation="Existing instances will need values for this attribute",
                    )
                )

            # Removed attributes - BREAKING
            for attr_name in entity_changes.removed_attributes:
                changes.append(
                    ClassifiedChange(
                        description=f"Remove attribute '{attr_name}' from entity '{type_name}'",
                        category=ChangeCategory.BREAKING,
                        recommendation=f"Remove '{attr_name}' values from all '{type_name}' instances first",
                    )
                )

            # Modified attribute flags - WARNING
            for attr_change in entity_changes.modified_attributes:
                changes.append(
                    ClassifiedChange(
                        description=(
                            f"Modify attribute flags for '{attr_change.name}' "
                            f"on entity '{type_name}': {attr_change.old_flags} -> {attr_change.new_flags}"
                        ),
                        category=ChangeCategory.WARNING,
                        recommendation="Review cardinality/constraint changes for compatibility",
                    )
                )

        return changes

    def _analyze_relation_changes(self, diff: SchemaDiff) -> list[ClassifiedChange]:
        """Analyze relation additions, removals, and modifications."""
        changes: list[ClassifiedChange] = []

        # Added relations - SAFE
        for relation in diff.added_relations:
            type_name = relation.get_type_name()
            changes.append(
                ClassifiedChange(
                    description=f"Add relation: {type_name}",
                    category=ChangeCategory.SAFE,
                    recommendation="No action needed",
                )
            )

        # Removed relations - BREAKING
        for relation in diff.removed_relations:
            type_name = relation.get_type_name()
            changes.append(
                ClassifiedChange(
                    description=f"Remove relation: {type_name}",
                    category=ChangeCategory.BREAKING,
                    recommendation=f"Delete all '{type_name}' instances before removing the type",
                )
            )

        # Modified relations
        for relation, rel_changes in diff.modified_relations.items():
            type_name = relation.get_type_name()

            # Added roles - SAFE (widening)
            for role_name in rel_changes.added_roles:
                changes.append(
                    ClassifiedChange(
                        description=f"Add role '{role_name}' to relation '{type_name}'",
                        category=ChangeCategory.SAFE,
                        recommendation="No action needed",
                    )
                )

            # Removed roles - BREAKING
            for role_name in rel_changes.removed_roles:
                changes.append(
                    ClassifiedChange(
                        description=f"Remove role '{role_name}' from relation '{type_name}'",
                        category=ChangeCategory.BREAKING,
                        recommendation=(
                            f"Update all '{type_name}' relations to remove "
                            f"'{role_name}' role players before removing the role"
                        ),
                    )
                )

            # Modified role player types
            for rpc in rel_changes.modified_role_players:
                # Added player types - SAFE (widening)
                for player_type in rpc.added_player_types:
                    changes.append(
                        ClassifiedChange(
                            description=(
                                f"Add player type '{player_type}' to role "
                                f"'{rpc.role_name}' in relation '{type_name}'"
                            ),
                            category=ChangeCategory.SAFE,
                            recommendation="No action needed - role can now accept more entity types",
                        )
                    )

                # Removed player types - BREAKING (narrowing)
                for player_type in rpc.removed_player_types:
                    changes.append(
                        ClassifiedChange(
                            description=(
                                f"Remove player type '{player_type}' from role "
                                f"'{rpc.role_name}' in relation '{type_name}'"
                            ),
                            category=ChangeCategory.BREAKING,
                            recommendation=(
                                f"Update all '{type_name}' relations where '{player_type}' "
                                f"plays '{rpc.role_name}' before removing the player type"
                            ),
                        )
                    )

            # Modified role cardinality
            for rcc in rel_changes.modified_role_cardinality:
                old_min, old_max = rcc.old_cardinality
                new_min, new_max = rcc.new_cardinality

                # Increasing minimum - BREAKING
                if new_min is not None and (old_min is None or new_min > old_min):
                    changes.append(
                        ClassifiedChange(
                            description=(
                                f"Increase minimum cardinality for role '{rcc.role_name}' "
                                f"in relation '{type_name}': {old_min} -> {new_min}"
                            ),
                            category=ChangeCategory.BREAKING,
                            recommendation="Existing relations may violate new minimum constraint",
                        )
                    )

                # Decreasing maximum - BREAKING
                if new_max is not None and (old_max is None or new_max < old_max):
                    changes.append(
                        ClassifiedChange(
                            description=(
                                f"Decrease maximum cardinality for role '{rcc.role_name}' "
                                f"in relation '{type_name}': {old_max} -> {new_max}"
                            ),
                            category=ChangeCategory.BREAKING,
                            recommendation="Existing relations may violate new maximum constraint",
                        )
                    )

                # Decreasing minimum or increasing maximum - SAFE
                if (new_min is not None and old_min is not None and new_min < old_min) or (
                    new_max is not None and old_max is not None and new_max > old_max
                ):
                    changes.append(
                        ClassifiedChange(
                            description=(
                                f"Relax cardinality for role '{rcc.role_name}' "
                                f"in relation '{type_name}'"
                            ),
                            category=ChangeCategory.SAFE,
                            recommendation="No action needed - constraint is relaxed",
                        )
                    )

            # Added relation attributes - WARNING
            for attr_name in rel_changes.added_attributes:
                changes.append(
                    ClassifiedChange(
                        description=f"Add attribute '{attr_name}' to relation '{type_name}'",
                        category=ChangeCategory.WARNING,
                        recommendation="Existing relations may need values for this attribute",
                    )
                )

            # Removed relation attributes - BREAKING
            for attr_name in rel_changes.removed_attributes:
                changes.append(
                    ClassifiedChange(
                        description=f"Remove attribute '{attr_name}' from relation '{type_name}'",
                        category=ChangeCategory.BREAKING,
                        recommendation=(
                            f"Remove '{attr_name}' values from all '{type_name}' relations first"
                        ),
                    )
                )

            # Modified relation attribute flags - WARNING
            for attr_change in rel_changes.modified_attributes:
                changes.append(
                    ClassifiedChange(
                        description=(
                            f"Modify attribute flags for '{attr_change.name}' "
                            f"on relation '{type_name}': {attr_change.old_flags} -> {attr_change.new_flags}"
                        ),
                        category=ChangeCategory.WARNING,
                        recommendation="Review cardinality/constraint changes for compatibility",
                    )
                )

        return changes

    def _analyze_attribute_changes(self, diff: SchemaDiff) -> list[ClassifiedChange]:
        """Analyze attribute type additions and removals."""
        changes: list[ClassifiedChange] = []

        # Added attributes - SAFE
        for attr in diff.added_attributes:
            attr_name = attr.get_attribute_name()
            changes.append(
                ClassifiedChange(
                    description=f"Add attribute type: {attr_name}",
                    category=ChangeCategory.SAFE,
                    recommendation="No action needed",
                )
            )

        # Removed attributes - BREAKING
        for attr in diff.removed_attributes:
            attr_name = attr.get_attribute_name()
            changes.append(
                ClassifiedChange(
                    description=f"Remove attribute type: {attr_name}",
                    category=ChangeCategory.BREAKING,
                    recommendation=(
                        f"Remove all ownership and instances of '{attr_name}' "
                        "before removing the attribute type"
                    ),
                )
            )

        return changes

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
