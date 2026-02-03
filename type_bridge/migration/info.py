"""Schema information container for TypeDB schema management."""

from type_bridge.attribute.base import Attribute
from type_bridge.migration.diff import (
    AttributeFlagChange,
    EntityChanges,
    RelationChanges,
    RolePlayerChange,
    SchemaDiff,
)
from type_bridge.models import Entity, Relation


class SchemaInfo:
    """Container for organized schema information."""

    def __init__(self):
        """Initialize SchemaInfo with empty collections."""
        self.entities: list[type[Entity]] = []
        self.relations: list[type[Relation]] = []
        self.attribute_classes: set[type[Attribute]] = set()

    def get_entity_by_name(self, name: str) -> type[Entity] | None:
        """Get entity by type name.

        Args:
            name: Entity type name

        Returns:
            Entity class or None if not found
        """
        for entity in self.entities:
            if entity.get_type_name() == name:
                return entity
        return None

    def get_relation_by_name(self, name: str) -> type[Relation] | None:
        """Get relation by type name.

        Args:
            name: Relation type name

        Returns:
            Relation class or None if not found
        """
        for relation in self.relations:
            if relation.get_type_name() == name:
                return relation
        return None

    def validate(self) -> None:
        """Validate schema definitions for TypeDB constraints.

        Raises:
            SchemaValidationError: If schema violates TypeDB constraints
        """
        # Validate entities
        for entity_model in self.entities:
            self._validate_no_duplicate_attribute_types(entity_model, entity_model.get_type_name())

        # Validate relations
        for relation_model in self.relations:
            self._validate_no_duplicate_attribute_types(
                relation_model, relation_model.get_type_name()
            )

    def _validate_no_duplicate_attribute_types(
        self, model: type[Entity | Relation], type_name: str
    ) -> None:
        """Validate that the same attribute type is not used for multiple fields.

        TypeDB does not store field names - only attribute types. When the same
        attribute type is used for multiple fields, TypeDB sees a single ownership
        with incorrect cardinality.

        Args:
            model: Entity or Relation class to validate
            type_name: Type name for error messages

        Raises:
            SchemaValidationError: If duplicate attribute types are detected
        """
        from type_bridge.migration.exceptions import SchemaValidationError

        owned_attrs = model.get_owned_attributes()

        # Track attribute types and their field names
        attr_type_to_fields: dict[type[Attribute], list[str]] = {}

        for field_name, attr_info in owned_attrs.items():
            attr_type = attr_info.typ
            if attr_type not in attr_type_to_fields:
                attr_type_to_fields[attr_type] = []
            attr_type_to_fields[attr_type].append(field_name)

        # Check for duplicates
        duplicates = {
            attr_type: fields
            for attr_type, fields in attr_type_to_fields.items()
            if len(fields) > 1
        }

        if duplicates:
            lines = []
            lines.append(
                f"Schema validation failed for '{type_name}': "
                "The same attribute type is used for multiple fields."
            )
            lines.append("")
            lines.append(
                "TypeDB best practice: Use distinct attribute types for each semantic field, "
                "even when they share the same underlying value type (string, datetime, etc.). "
                "This makes schemas more expressive and avoids ownership conflicts."
            )
            lines.append("")
            lines.append("Why this happens:")
            lines.append(
                "  TypeDB does not store field names - it only stores attribute types and their values."
            )
            lines.append(
                "  When you use the same attribute type for multiple fields (e.g., 'created' and 'modified' "
                "both using 'TimeStamp'),"
            )
            lines.append(
                "  TypeDB sees a single ownership: 'Issue owns TimeStamp', not 'Issue owns created' and 'Issue owns modified'."
            )
            lines.append("")
            lines.append("Duplicate attribute types found:")
            for attr_type, fields in duplicates.items():
                attr_name = attr_type.get_attribute_name()
                fields_str = ", ".join(f"'{f}'" for f in fields)
                lines.append(f"  - {attr_name} used in fields: {fields_str}")
            lines.append("")
            lines.append("Solution:")
            lines.append(
                "  Create separate attribute classes for each field, even if they use the same value type:"
            )
            lines.append("")

            # Show example solution for the first duplicate
            first_attr_type, first_fields = next(iter(duplicates.items()))
            first_attr_name = first_attr_type.get_attribute_name()
            value_type = first_attr_type.__bases__[0].__name__  # e.g., DateTime, String

            lines.append("  Example:")
            lines.append("    # Instead of:")
            lines.append(f"    class {first_attr_name}({value_type}):")
            lines.append("        pass")
            lines.append("")
            lines.append(f"    class {type_name}(Entity):")
            for field in first_fields:
                lines.append(f"        {field}: {first_attr_name}  # ❌ Reusing same type")
            lines.append("")
            lines.append("    # Use:")
            for field in first_fields:
                field_class_name = (
                    field.capitalize() + "Stamp" if "time" in field.lower() else field.capitalize()
                )
                lines.append(f"    class {field_class_name}({value_type}):")
                lines.append("        pass")
                lines.append("")
            lines.append(f"    class {type_name}(Entity):")
            for field in first_fields:
                field_class_name = (
                    field.capitalize() + "Stamp" if "time" in field.lower() else field.capitalize()
                )
                lines.append(f"        {field}: {field_class_name}  # ✓ Distinct types")

            raise SchemaValidationError("\n".join(lines))

    def to_typeql(self) -> str:
        """Generate TypeQL schema definition from collected schema information.

        Base classes (with base=True) are skipped as they don't appear in TypeDB schema.

        Validates the schema before generation.

        Returns:
            TypeQL schema definition string

        Raises:
            SchemaValidationError: If schema validation fails
        """
        # Validate schema before generation
        self.validate()

        lines = []

        # Define attributes first
        lines.append("define")
        lines.append("")

        # Sort attributes by name for consistent output
        sorted_attrs = sorted(self.attribute_classes, key=lambda x: x.get_attribute_name())
        for attr_class in sorted_attrs:
            lines.append(attr_class.to_schema_definition())

        lines.append("")

        # Define entities (skip base classes)
        for entity_model in self.entities:
            schema_def = entity_model.to_schema_definition()
            if schema_def is not None:  # Skip base classes
                lines.append(schema_def)
                lines.append("")

        # Define relations (skip base classes)
        for relation_model in self.relations:
            schema_def = relation_model.to_schema_definition()
            if schema_def is not None:  # Skip base classes
                lines.append(schema_def)

                # Add role player definitions
                for role_name, role in relation_model._roles.items():
                    for player_type in role.player_types:
                        lines.append(
                            f"{player_type} plays {relation_model.get_type_name()}:{role.role_name};"
                        )
                lines.append("")

        return "\n".join(lines)

    def compare(self, other: "SchemaInfo") -> SchemaDiff:
        """Compare this schema with another schema.

        Args:
            other: Another SchemaInfo to compare against

        Returns:
            SchemaDiff containing all differences between the schemas
        """
        diff = SchemaDiff()

        # Compare entities by type name (not Python object identity)
        self_entity_by_name = {e.get_type_name(): e for e in self.entities}
        other_entity_by_name = {e.get_type_name(): e for e in other.entities}

        self_ent_names = set(self_entity_by_name.keys())
        other_ent_names = set(other_entity_by_name.keys())

        diff.added_entities = {other_entity_by_name[n] for n in other_ent_names - self_ent_names}
        diff.removed_entities = {self_entity_by_name[n] for n in self_ent_names - other_ent_names}

        # Compare entities that exist in both (by type name)
        for type_name in self_ent_names & other_ent_names:
            self_ent = self_entity_by_name[type_name]
            other_ent = other_entity_by_name[type_name]
            entity_changes = self._compare_entity(self_ent, other_ent)
            if entity_changes:
                diff.modified_entities[other_ent] = entity_changes

        # Compare relations by type name (not Python object identity)
        self_relation_by_name = {r.get_type_name(): r for r in self.relations}
        other_relation_by_name = {r.get_type_name(): r for r in other.relations}

        self_rel_names = set(self_relation_by_name.keys())
        other_rel_names = set(other_relation_by_name.keys())

        diff.added_relations = {other_relation_by_name[n] for n in other_rel_names - self_rel_names}
        diff.removed_relations = {
            self_relation_by_name[n] for n in self_rel_names - other_rel_names
        }

        # Compare relations that exist in both (by type name)
        for type_name in self_rel_names & other_rel_names:
            self_rel = self_relation_by_name[type_name]
            other_rel = other_relation_by_name[type_name]
            relation_changes = self._compare_relation(self_rel, other_rel)
            if relation_changes:
                diff.modified_relations[other_rel] = relation_changes

        # Compare attributes
        diff.added_attributes = other.attribute_classes - self.attribute_classes
        diff.removed_attributes = self.attribute_classes - other.attribute_classes

        return diff

    def _compare_entity(
        self, self_entity: type[Entity], other_entity: type[Entity]
    ) -> EntityChanges | None:
        """Compare two entity types for differences.

        Args:
            self_entity: Entity from this schema
            other_entity: Entity from other schema

        Returns:
            EntityChanges with differences, or None if no changes
        """
        # Compare owned attributes
        self_attrs = self_entity.get_owned_attributes()
        other_attrs = other_entity.get_owned_attributes()

        added_attrs = list(set(other_attrs.keys()) - set(self_attrs.keys()))
        removed_attrs = list(set(self_attrs.keys()) - set(other_attrs.keys()))

        # Compare attribute flags for common attributes
        common_attrs = set(self_attrs.keys()) & set(other_attrs.keys())
        modified_attrs = []
        for attr_name in common_attrs:
            self_info = self_attrs[attr_name]
            other_info = other_attrs[attr_name]

            # Compare flags
            if self_info.flags != other_info.flags:
                modified_attrs.append(
                    AttributeFlagChange(
                        name=attr_name,
                        old_flags=str(self_info.flags.to_typeql_annotations()),
                        new_flags=str(other_info.flags.to_typeql_annotations()),
                    )
                )

        changes = EntityChanges(
            added_attributes=added_attrs,
            removed_attributes=removed_attrs,
            modified_attributes=modified_attrs,
        )

        return changes if changes.has_changes() else None

    def _compare_relation(
        self, self_relation: type[Relation], other_relation: type[Relation]
    ) -> RelationChanges | None:
        """Compare two relation types for differences.

        Detects:
        - Added/removed roles
        - Role player type changes (which entities can play each role)
        - Added/removed/modified attributes

        Args:
            self_relation: Relation from this schema
            other_relation: Relation from other schema

        Returns:
            RelationChanges with differences, or None if no changes
        """
        # Compare roles
        self_roles = set(self_relation._roles.keys())
        other_roles = set(other_relation._roles.keys())

        added_roles = list(other_roles - self_roles)
        removed_roles = list(self_roles - other_roles)

        # Compare role player types for common roles
        common_roles = self_roles & other_roles
        modified_role_players: list[RolePlayerChange] = []

        for role_name in common_roles:
            self_role = self_relation._roles[role_name]
            other_role = other_relation._roles[role_name]

            # Get player types as sets for comparison
            self_player_types = set(self_role.player_types)
            other_player_types = set(other_role.player_types)

            if self_player_types != other_player_types:
                added_player_types = list(other_player_types - self_player_types)
                removed_player_types = list(self_player_types - other_player_types)

                modified_role_players.append(
                    RolePlayerChange(
                        role_name=role_name,
                        added_player_types=added_player_types,
                        removed_player_types=removed_player_types,
                    )
                )

        # Compare owned attributes
        self_attrs = self_relation.get_owned_attributes()
        other_attrs = other_relation.get_owned_attributes()

        added_attrs = list(set(other_attrs.keys()) - set(self_attrs.keys()))
        removed_attrs = list(set(self_attrs.keys()) - set(other_attrs.keys()))

        # Compare attribute flags for common attributes
        common_attrs = set(self_attrs.keys()) & set(other_attrs.keys())
        modified_attrs: list[AttributeFlagChange] = []

        for attr_name in common_attrs:
            self_info = self_attrs[attr_name]
            other_info = other_attrs[attr_name]

            # Compare flags
            if self_info.flags != other_info.flags:
                modified_attrs.append(
                    AttributeFlagChange(
                        name=attr_name,
                        old_flags=str(self_info.flags.to_typeql_annotations()),
                        new_flags=str(other_info.flags.to_typeql_annotations()),
                    )
                )

        changes = RelationChanges(
            added_roles=added_roles,
            removed_roles=removed_roles,
            modified_role_players=modified_role_players,
            added_attributes=added_attrs,
            removed_attributes=removed_attrs,
            modified_attributes=modified_attrs,
        )

        return changes if changes.has_changes() else None
