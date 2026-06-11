"""Schema information container for TypeDB schema management."""

from type_bridge.attribute.base import Attribute
from type_bridge.migration.diff import SchemaDiff, from_rust_schema_diff
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
        self.validate()
        from type_bridge._rust_runtime import generate_define_block

        return generate_define_block(self.to_rust_schema_info())

    def compare(self, other: "SchemaInfo") -> SchemaDiff:
        """Compare this schema with another schema.

        Args:
            other: Another SchemaInfo to compare against

        Returns:
            SchemaDiff containing all differences between the schemas
        """
        from type_bridge._rust_runtime import compute_schema_diff

        rust_diff = compute_schema_diff(self.to_rust_schema_info(), other.to_rust_schema_info())
        return from_rust_schema_diff(rust_diff, current_schema=self, target_schema=other)

    def to_rust_schema_info(self) -> dict:
        """Serialize this Python model schema to the Rust ``SchemaInfo`` dict shape.

        Registers all non-base entity and relation descriptors into a fresh
        ``PyDescriptorRegistry`` and delegates to Rust ``SchemaInfo::from_descriptors``
        for the full projection: entity/relation entries, plays_cardinalities overlays,
        and foreign parent_type nulling all happen inside Rust.

        The attributes section is merged on the Python side because it requires
        attribute-class metadata (regex, range, allowed_values, etc.) that is not
        represented in the descriptor layer.
        """
        from type_bridge._rust_runtime import (
            attribute_schema_entry,
            descriptor_for_model,
            rust_core,
        )

        registry = rust_core().PyDescriptorRegistry()
        for entity in self.entities:
            if _is_base_model(entity):
                continue
            registry.register_entity(descriptor_for_model(entity))
        for relation in self.relations:
            if _is_base_model(relation):
                continue
            registry.register_relation(descriptor_for_model(relation))

        info = registry.schema_info()

        # Merge attribute-class metadata. Rust from_descriptors emits only the
        # attr_name/value_type pairs it derives from owned_attributes; full
        # attribute-class metadata (regex, range, allowed_values, abstract,
        # independent, parent_type) requires the Python Attribute class.
        for entity in self.entities:
            if _is_base_model(entity):
                continue
            for attr_info in entity.get_all_attributes().values():
                entry = attribute_schema_entry(attr_info.typ)
                info["attributes"][entry["attr_name"]] = entry
        for relation in self.relations:
            if _is_base_model(relation):
                continue
            for attr_info in relation.get_all_attributes().values():
                entry = attribute_schema_entry(attr_info.typ)
                info["attributes"][entry["attr_name"]] = entry
        for attr_cls in self.attribute_classes:
            entry = attribute_schema_entry(attr_cls)
            info["attributes"][entry["attr_name"]] = entry

        return info


def _is_base_model(model: type[Entity | Relation]) -> bool:
    return bool(getattr(model, "is_base", lambda: False)())
