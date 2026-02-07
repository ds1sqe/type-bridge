"""Configuration for API DTO generation.

This module provides dataclasses for configuring how API DTOs are generated,
allowing customization of base classes, validators, field syncs, and more.

Example usage:

    from type_bridge.generator import DTOConfig, BaseClassConfig

    config = DTOConfig(
        base_classes=[
            BaseClassConfig(
                source_entity="artifact",
                base_name="BaseArtifact",
                inherited_attrs=["display_id", "name", "description"],
            ),
        ],
    )

    generate_models("schema.tql", "./models/", generate_dto=True, dto_config=config)
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any


@dataclass
class FieldOverride:
    """Per-variant override for a single field's requiredness or default.

    Use this inside ``BaseClassConfig.create_field_overrides`` to make a
    normally-required field optional (or vice-versa) on a specific DTO variant.

    Attributes:
        required: Override requiredness. ``None`` keeps the schema default.
        default: Override default value as a Python literal string
            (e.g., ``"None"``). ``None`` keeps the schema default.
    """

    required: bool | None = None
    default: str | None = None


@dataclass
class EntityFieldOverride:
    """Per-entity, per-variant field override.

    Allows targeting a specific entity + field + variant combination.

    Attributes:
        entity: TypeDB entity name (e.g., ``"task"``)
        field: TypeDB attribute name (e.g., ``"display_id"``)
        variant: DTO variant to override (``"create"``, ``"out"``, ``"patch"``)
        required: Override requiredness. ``None`` keeps the schema default.
        default: Override default value as a Python literal string.
            ``None`` keeps the schema default.
    """

    entity: str
    field: str
    variant: str  # "create", "out", or "patch"
    required: bool | None = None
    default: str | None = None


@dataclass
class ValidatorConfig:
    """Configuration for a custom Pydantic validator type.

    Validators are rendered as Annotated types with AfterValidator.

    Attributes:
        name: The validator type name (e.g., "DisplayId")
        pattern: Regex pattern for validation (optional).
            For complex validators that need more than a regex pattern,
            use `preamble` in DTOConfig to define custom functions.

    Example:
        ValidatorConfig(name="DisplayId", pattern=r"^[A-Z]{1,10}-\\d+$")

        # Generates:
        # def _validate_display_id(value: str) -> str:
        #     if not re.match(r"^[A-Z]{1,10}-\\d+$", value):
        #         raise ValueError(...)
        #     return value
        # DisplayId = Annotated[str, AfterValidator(_validate_display_id)]
    """

    name: str
    pattern: str | None = None


@dataclass
class FieldSyncConfig:
    """Configuration for syncing two fields in a model validator.

    When one field is set but not the other, the value is copied.

    Attributes:
        field_a: First field name
        field_b: Second field name

    Example:
        FieldSyncConfig(field_a="description", field_b="content")

        # Generates a model_validator that syncs description <-> content
    """

    field_a: str
    field_b: str


@dataclass
class CompositeFieldConfig:
    """Configuration for a field in a composite entity DTO.

    Attributes:
        name: Field name in Python (snake_case)
        type_annotation: Python type annotation as string (e.g., "str", "int | None")
        default: Default value as string (e.g., "None", "'proposed'", "''")
            If None, field is required in Create DTOs.
        description: Optional field description for documentation

    Example:
        CompositeFieldConfig(
            name="status",
            type_annotation="str",
            default="'proposed'",
            description="Lifecycle status"
        )
    """

    name: str
    type_annotation: str
    default: str | None = None
    description: str | None = None


@dataclass
class CompositeEntityConfig:
    """Configuration for a composite/flat DTO that merges multiple entity types.

    Composite DTOs provide a polymorphic API where a single DTO class handles
    multiple entity types. This is useful for graph APIs where you want one
    endpoint to handle all node types.

    Attributes:
        name: Name prefix for the composite (e.g., "GraphNode" generates
            GraphNodeOut, GraphNodeCreate, GraphNodePatch)
        base_entity: Schema entity name - all entities inheriting from this
            will be included in the composite (e.g., "artifact")
        include_entities: Explicit list of entity names to include.
            Use this OR base_entity, not both.
        exclude_entities: Entity names to exclude (useful with base_entity)
        common_fields: Fields that appear on all variants with specified types.
            These are required (or have defaults) in Create, present in Out/Patch.
        field_syncs: Field sync configurations for the composite
        extra_fields: Additional fields not in schema (e.g., computed fields)
        id_field_name: Name of the ID field (default: "id")
        type_field_name: Name of the type discriminator field (default: "type")
        type_enum_from_registry: If True, generate dynamic enum from registry

    Example:
        CompositeEntityConfig(
            name="GraphNode",
            base_entity="artifact",
            exclude_entities=["design_aspect"],
            common_fields=[
                CompositeFieldConfig("name", "str"),
                CompositeFieldConfig("content", "str", "''"),
                CompositeFieldConfig("status", "str", "'proposed'"),
            ],
            field_syncs=[FieldSyncConfig("description", "content")],
            extra_fields={"version": "int | None = None"},
        )
    """

    name: str
    base_entity: str | None = None
    include_entities: list[str] = field(default_factory=list)
    exclude_entities: list[str] = field(default_factory=list)
    common_fields: list[CompositeFieldConfig] = field(default_factory=list)
    field_syncs: list[FieldSyncConfig] = field(default_factory=list)
    extra_fields: dict[str, str] = field(default_factory=dict)
    id_field_name: str = "id"
    type_field_name: str = "type"
    type_enum_from_registry: bool = True

    def get_included_entities(self, schema_entities: dict[str, Any]) -> list[str]:
        """Get list of entity names included in this composite.

        Args:
            schema_entities: Dict of entity name -> EntitySpec from parsed schema

        Returns:
            List of entity names to include in the composite
        """
        if self.include_entities:
            # Explicit list provided
            return [e for e in self.include_entities if e not in self.exclude_entities]

        if self.base_entity:
            # Find all entities that inherit from base_entity
            included = []
            for name, spec in schema_entities.items():
                if name in self.exclude_entities:
                    continue
                if spec.abstract:
                    continue
                # Check if this entity inherits from base_entity
                current = name
                while current:
                    if current == self.base_entity:
                        included.append(name)
                        break
                    parent_spec = schema_entities.get(current)
                    if parent_spec and parent_spec.parent:
                        current = parent_spec.parent
                    else:
                        break
            return included

        return []


@dataclass
class BaseClassConfig:
    """Configuration for a custom base class in the DTO hierarchy.

    When an entity inherits from `source_entity` in the schema, its DTOs
    will inherit from the custom base class instead of the default.

    Attributes:
        source_entity: The schema entity name that triggers this base class
            (e.g., "artifact"). Entities that inherit from this entity
            will use the custom base.
        base_name: The base class name prefix (e.g., "BaseArtifact").
            Will generate BaseArtifactOut, BaseArtifactCreate, BaseArtifactPatch.
        inherited_attrs: Attribute names that are defined in the base class.
            These will be skipped when rendering child DTOs to avoid duplication.
        extra_fields: Additional fields to add to the base class.
            Keys are field names, values are type annotations as strings.
        field_syncs: Field sync configurations for this base class.
        validators: List of validator names to apply to fields in this base.

    Example:
        BaseClassConfig(
            source_entity="artifact",
            base_name="BaseArtifact",
            inherited_attrs=["display_id", "name", "description", "status"],
            extra_fields={"version": "int | None = None"},
            field_syncs=[FieldSyncConfig("description", "content")],
        )
    """

    source_entity: str
    base_name: str
    inherited_attrs: list[str] = field(default_factory=list)
    extra_fields: dict[str, str] = field(default_factory=dict)
    field_syncs: list[FieldSyncConfig] = field(default_factory=list)
    validators: list[str] = field(default_factory=list)
    create_field_overrides: dict[str, FieldOverride] = field(default_factory=dict)


@dataclass
class DTOConfig:
    """Configuration for API DTO generation.

        This configuration controls how Pydantic DTOs are generated from a
        TypeDB schema. It allows customization of base classes, validators,
        field syncs, and naming conventions.

        Attributes:
            base_classes: Custom base class configurations for entity hierarchies.
            validators: Custom validator type configurations.
            preamble: Custom Python code to inject at the top of the file
                (after imports). Use for complex validators or helper functions.
            entity_union_name: Name for the entity union type (default: "Entity").
            relation_union_name: Name for the relation union type (default: "Relation").
            exclude_entities: Entity names to exclude from generation (e.g., internal
                utility entities like "display_id_counter").
            iid_field_name: Field name for TypeDB IID in output DTOs (default: "iid").
                Set to "id" for more conventional REST API naming.
            skip_relation_output: If True, don't generate XxxOut classes for relations.
                Use with relation_preamble to define custom output classes.
            relation_create_base_class: Name of custom base class for relation Create
                DTOs (must be defined in preamble). If set, relation Create classes
                won't generate role-specific fields, only owned attributes.
            relation_preamble: Custom Python code to inject in the relation section.
                Use for custom relation output classes like GraphEdgeOut.
            composite_entities: Composite entity configurations for creating flat/merged
                DTOs that handle multiple entity types in one class.
            strict_out_models: If True, fields with @key or min>=1 cardinality will be
                required (not Optional) in Out DTOs. Default False for safety.

        Example:
            config = DTOConfig(
                base_classes=[
                    BaseClassConfig(
                        source_entity="artifact",
                        base_name="BaseArtifact",
                        inherited_attrs=["display_id", "name"],
                    ),
                ],
                validators=[
                    ValidatorConfig(name="DisplayId", pattern=r"^[A-Z]{1,10}-\\d+$"),
                ],
                exclude_entities=["display_id_counter", "schema_status"],
                iid_field_name="id",
                preamble='''
    # Custom node ID validator
    def _validate_node_id(value: str) -> str:
        # Complex validation logic here
        return value

    NodeId = Annotated[str, AfterValidator(_validate_node_id)]
    ''',
            )
    """

    base_classes: list[BaseClassConfig] = field(default_factory=list)
    validators: list[ValidatorConfig] = field(default_factory=list)
    preamble: str | None = None
    entity_union_name: str = "Entity"
    relation_union_name: str = "Relation"
    exclude_entities: list[str] = field(default_factory=list)
    iid_field_name: str = "iid"
    skip_relation_output: bool = False
    relation_create_base_class: str | None = None
    relation_preamble: str | None = None
    composite_entities: list[CompositeEntityConfig] = field(default_factory=list)
    strict_out_models: bool = False
    entity_field_overrides: list[EntityFieldOverride] = field(default_factory=list)

    def get_base_class_for_entity(
        self, entity_name: str, schema_entities: dict[str, Any]
    ) -> BaseClassConfig | None:
        """Find the base class config for an entity based on inheritance.

        Walks up the inheritance chain to find if this entity or any ancestor
        matches a configured base class.

        Args:
            entity_name: The entity name to check
            schema_entities: Dict of entity name -> EntitySpec from parsed schema

        Returns:
            The matching BaseClassConfig, or None if no match
        """
        if not self.base_classes:
            return None

        # Build a set of source entities for quick lookup
        source_entities = {bc.source_entity: bc for bc in self.base_classes}

        # Walk up the inheritance chain
        current = entity_name
        while current:
            if current in source_entities:
                return source_entities[current]
            # Get parent
            entity_spec = schema_entities.get(current)
            if entity_spec and entity_spec.parent:
                current = entity_spec.parent
            else:
                break

        return None
