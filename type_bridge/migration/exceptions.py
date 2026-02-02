"""Schema-related exceptions for TypeDB schema management."""

from typing import cast

from type_bridge.migration.diff import EntityChanges, RelationChanges, SchemaDiff


class SchemaValidationError(Exception):
    """Raised when schema validation fails during schema generation.

    This exception is raised when the Python model definitions violate
    TypeDB constraints or best practices.
    """

    pass


class SchemaConflictError(Exception):
    """Raised when there are conflicting schema changes during sync.

    This exception is raised when attempting to sync a schema that has
    breaking changes (removed or modified types/attributes) compared to
    the existing database schema.
    """

    def __init__(self, diff: SchemaDiff, message: str | None = None):
        """Initialize SchemaConflictError.

        Args:
            diff: SchemaDiff containing the conflicting changes
            message: Optional custom error message
        """
        self.diff = diff

        if message is None:
            message = self._build_default_message()

        super().__init__(message)

    def _build_default_message(self) -> str:
        """Build a comprehensive error message from the diff.

        Returns:
            Formatted error message with conflict details
        """
        lines = []
        lines.append("Schema conflict detected! Cannot safely sync schema.")
        lines.append("")
        lines.append("The following conflicts were found:")
        lines.append("")

        # Show removed items (breaking changes)
        if self.diff.removed_entities:
            lines.append(f"❌ Removed Entities ({len(self.diff.removed_entities)}):")
            for entity in sorted(self.diff.removed_entities, key=lambda e: e.__name__):
                lines.append(f"   - {entity.__name__}")
            lines.append("")

        if self.diff.removed_relations:
            lines.append(f"❌ Removed Relations ({len(self.diff.removed_relations)}):")
            for relation in sorted(self.diff.removed_relations, key=lambda r: r.__name__):
                lines.append(f"   - {relation.__name__}")
            lines.append("")

        if self.diff.removed_attributes:
            lines.append(f"❌ Removed Attributes ({len(self.diff.removed_attributes)}):")
            for attr in sorted(self.diff.removed_attributes, key=lambda a: a.get_attribute_name()):
                lines.append(f"   - {attr.get_attribute_name()}")
            lines.append("")

        # Show modified items (potentially breaking changes)
        if self.diff.modified_entities:
            lines.append(f"⚠️  Modified Entities ({len(self.diff.modified_entities)}):")
            for entity, changes in self.diff.modified_entities.items():
                changes = cast(EntityChanges, changes)
                lines.append(f"   ~ {entity.__name__}")
                if changes.added_attributes:
                    lines.append(f"     + added: {changes.added_attributes}")
                if changes.removed_attributes:
                    lines.append(f"     - removed: {changes.removed_attributes}")
                if changes.modified_attributes:
                    lines.append("     ~ modified:")
                    for attr_change in changes.modified_attributes:
                        lines.append(f"       - {attr_change.name}:")
                        lines.append(f"           old: {attr_change.old_flags}")
                        lines.append(f"           new: {attr_change.new_flags}")
            lines.append("")

        if self.diff.modified_relations:
            lines.append(f"⚠️  Modified Relations ({len(self.diff.modified_relations)}):")
            for relation, relation_changes in self.diff.modified_relations.items():
                rel_changes: RelationChanges = relation_changes
                lines.append(f"   ~ {relation.__name__}")
                if rel_changes.added_roles:
                    lines.append(f"     + added roles: {rel_changes.added_roles}")
                if rel_changes.removed_roles:
                    lines.append(f"     - removed roles: {rel_changes.removed_roles}")
                if rel_changes.added_attributes:
                    lines.append(f"     + added attributes: {rel_changes.added_attributes}")
                if rel_changes.removed_attributes:
                    lines.append(f"     - removed attributes: {rel_changes.removed_attributes}")
            lines.append("")

        # Provide resolution suggestions
        lines.append("Resolution options:")
        lines.append("1. Use sync_schema(force=True) to recreate the database from scratch")
        lines.append("2. Manually migrate the data before applying schema changes")
        lines.append("3. Use MigrationManager to apply incremental migrations")

        return "\n".join(lines)

    def has_breaking_changes(self) -> bool:
        """Check if the diff contains breaking changes.

        Breaking changes include removed or modified types/attributes.

        Returns:
            True if there are breaking changes
        """
        return bool(
            self.diff.removed_entities
            or self.diff.removed_relations
            or self.diff.removed_attributes
            or self.diff.modified_entities
            or self.diff.modified_relations
        )
