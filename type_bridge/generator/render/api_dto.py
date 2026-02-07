"""Render api_dto.py with schema-driven API Data Transfer Objects."""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import TYPE_CHECKING

from ..dto_config import DTOConfig
from ..naming import to_class_name, to_python_name
from .template_loader import get_template

if TYPE_CHECKING:
    from ..models import EntitySpec, ParsedSchema, RelationSpec

# Map TypeDB value types to Python types
TYPE_MAP = {
    "string": "str",
    "integer": "int",
    "long": "int",
    "double": "float",
    "boolean": "bool",
    "datetime": "datetime",
    "datetime-tz": "datetime",
    "date": "date",
    "decimal": "Decimal",
    "duration": "str",  # ISO 8601 duration string
}


@dataclass
class AttrContext:
    """Context for rendering an attribute field."""

    name: str  # TypeDB attribute name
    python_name: str  # Python field name (snake_case)
    python_type: str  # Python type annotation
    default: str  # Default value as string
    required: bool  # Whether the field is required
    create_required: bool | None = None  # Per-variant override for Create
    create_default: str | None = None  # Per-variant default for Create
    out_required: bool | None = None  # Per-variant override for Out
    out_default: str | None = None  # Per-variant default for Out


@dataclass
class RoleContext:
    """Context for rendering a role field."""

    name: str  # TypeDB role name
    python_name: str  # Python field name (snake_case)
    required: bool  # Whether the role is required (min cardinality >= 1)


@dataclass
class EntityContext:
    """Context for rendering an entity DTO."""

    name: str  # TypeDB entity name
    class_name: str  # Python class name (PascalCase)
    parent_class: str  # Parent DTO class name
    abstract: bool  # Whether this is an abstract entity
    own_attributes: list[AttrContext] = field(default_factory=list)


@dataclass
class RelationContext:
    """Context for rendering a relation DTO."""

    name: str  # TypeDB relation name
    class_name: str  # Python class name (PascalCase)
    parent_class: str  # Parent DTO class name
    abstract: bool  # Whether this is an abstract relation
    roles: list[RoleContext] = field(default_factory=list)
    own_attributes: list[AttrContext] = field(default_factory=list)


@dataclass
class BaseClassContext:
    """Context for rendering a custom base class."""

    base_name: str  # e.g., "BaseArtifact"
    source_entity: str  # e.g., "artifact"
    fields: list[AttrContext]  # Fields to render in the base class
    extra_fields: dict[str, str]  # Additional fields as name -> type annotation
    has_field_sync: bool  # Whether to generate field sync validator
    field_sync_a: str | None = None  # First field to sync
    field_sync_b: str | None = None  # Second field to sync


@dataclass
class ValidatorContext:
    """Context for rendering a custom validator."""

    name: str  # e.g., "DisplayId"
    func_name: str  # e.g., "_validate_display_id"
    pattern: str | None = None  # Regex pattern


@dataclass
class CompositeFieldContext:
    """Context for a field in a composite DTO."""

    name: str  # Python field name (snake_case)
    type_annotation: str  # Python type annotation
    default: str | None  # Default value as string, or None if required
    description: str | None = None  # Optional field description


@dataclass
class CompositeContext:
    """Context for rendering a composite entity DTO."""

    name: str  # e.g., "GraphNode"
    fields: list[CompositeFieldContext]  # All fields for the composite
    extra_fields: dict[str, str]  # Additional fields as name -> type annotation
    id_field_name: str  # Name of the ID field
    type_field_name: str  # Name of the type discriminator field
    type_enum_from_registry: bool  # Whether to generate dynamic enum
    included_entity_names: list[str]  # Entity names included in the composite
    has_field_sync: bool  # Whether to generate field sync validator
    field_sync_a: str | None = None  # First field to sync
    field_sync_b: str | None = None  # Second field to sync
    extra_fields_out: dict[str, str] | None = None  # Per-variant overrides for Out
    skip_variants: set[str] = field(default_factory=set)  # Variants to skip generating


def _is_required_attribute(attr_name: str, spec: EntitySpec | RelationSpec) -> bool:
    """Determine if an attribute is required based on schema annotations."""
    # @key implies required
    if attr_name in spec.keys:
        return True

    # @unique implies required
    if attr_name in spec.uniques:
        return True

    # Check cardinality - required if min >= 1
    if attr_name in spec.cardinalities:
        card = spec.cardinalities[attr_name]
        return card.min >= 1

    # Default: optional (card 0..1)
    return False


def _get_attribute_context(
    attr_name: str,
    schema: ParsedSchema,
    spec: EntitySpec | RelationSpec,
) -> AttrContext:
    """Create attribute context from schema."""
    attr_spec = schema.attributes.get(attr_name)
    if not attr_spec:
        # Fallback for unknown attributes
        return AttrContext(
            name=attr_name,
            python_name=to_python_name(attr_name),
            python_type="str",
            default="None",
            required=False,
        )

    python_type = TYPE_MAP.get(attr_spec.value_type, "str")
    required = _is_required_attribute(attr_name, spec)

    return AttrContext(
        name=attr_name,
        python_name=to_python_name(attr_name),
        python_type=python_type,
        default="None",
        required=required,
    )


def _get_parent_class(
    entity_name: str,
    spec_parent: str | None,
    class_names: dict[str, str],
    base_class: str,
    config: DTOConfig | None,
    schema_entities: dict,
) -> str:
    """Determine the parent DTO class based on schema inheritance and config."""
    # Check if config specifies a custom base class for this entity's hierarchy
    if config:
        base_config = config.get_base_class_for_entity(entity_name, schema_entities)
        if base_config:
            # If this entity IS the source entity, use the default base
            # (the custom base class itself inherits from default)
            if entity_name == base_config.source_entity:
                return base_class
            # If parent is the source entity, use the custom base
            if spec_parent == base_config.source_entity:
                return base_config.base_name
            # Otherwise, check if parent has a class name and is not abstract
            if spec_parent and spec_parent in class_names:
                parent_spec = schema_entities.get(spec_parent)
                if parent_spec and not parent_spec.abstract:
                    return class_names[spec_parent]
            # Fallback to custom base
            return base_config.base_name

    # Default: use schema inheritance (but skip abstract parents since they don't generate DTOs)
    if spec_parent and spec_parent in class_names:
        parent_spec = schema_entities.get(spec_parent)
        if parent_spec and not parent_spec.abstract:
            return class_names[spec_parent]
    return base_class


def _build_base_class_contexts(
    config: DTOConfig,
    schema: ParsedSchema,
) -> list[BaseClassContext]:
    """Build contexts for custom base classes from config."""
    contexts = []

    for bc in config.base_classes:
        # Get the source entity spec to find its attributes
        entity_spec = schema.entities.get(bc.source_entity)
        if not entity_spec:
            continue

        # Build attribute contexts for inherited attrs
        fields = []
        for attr_name in bc.inherited_attrs:
            if attr_name in entity_spec.owns:
                attr_ctx = _get_attribute_context(attr_name, schema, entity_spec)
                # Apply create_field_overrides from base class config
                override = bc.create_field_overrides.get(attr_name)
                if override:
                    if override.required is not None:
                        attr_ctx.create_required = override.required
                    if override.default is not None:
                        attr_ctx.create_default = override.default
                fields.append(attr_ctx)

        # Check for field sync
        has_field_sync = len(bc.field_syncs) > 0
        field_sync_a = bc.field_syncs[0].field_a if has_field_sync else None
        field_sync_b = bc.field_syncs[0].field_b if has_field_sync else None

        contexts.append(
            BaseClassContext(
                base_name=bc.base_name,
                source_entity=bc.source_entity,
                fields=fields,
                extra_fields=bc.extra_fields,
                has_field_sync=has_field_sync,
                field_sync_a=field_sync_a,
                field_sync_b=field_sync_b,
            )
        )

    return contexts


def _build_validator_contexts(config: DTOConfig) -> list[ValidatorContext]:
    """Build contexts for custom validators from config."""
    contexts = []

    for v in config.validators:
        func_name = f"_validate_{to_python_name(v.name.lower())}"
        contexts.append(
            ValidatorContext(
                name=v.name,
                func_name=func_name,
                pattern=v.pattern,
            )
        )

    return contexts


def _build_composite_contexts(
    config: DTOConfig,
    schema: ParsedSchema,
) -> list[CompositeContext]:
    """Build contexts for composite entity DTOs from config."""
    contexts = []

    for comp in config.composite_entities:
        # Get list of entities included in this composite
        included_entities = comp.get_included_entities(schema.entities)

        # Collect all attributes from included entities
        all_attrs: dict[str, tuple[str, bool]] = {}  # name -> (python_type, required)
        for entity_name in included_entities:
            entity_spec = schema.entities.get(entity_name)
            if not entity_spec:
                continue
            for attr_name in entity_spec.owns:
                attr_spec = schema.attributes.get(attr_name)
                if attr_spec:
                    python_type = TYPE_MAP.get(attr_spec.value_type, "str")
                    required = _is_required_attribute(attr_name, entity_spec)
                    python_name = to_python_name(attr_name)
                    # If attr already exists, it's only required if ALL usages require it
                    if python_name in all_attrs:
                        existing_type, existing_required = all_attrs[python_name]
                        all_attrs[python_name] = (existing_type, existing_required and required)
                    else:
                        all_attrs[python_name] = (python_type, required)

        # Build fields from common_fields config (these override schema-derived fields)
        fields: list[CompositeFieldContext] = []
        configured_field_names = set()

        for field_cfg in comp.common_fields:
            fields.append(
                CompositeFieldContext(
                    name=field_cfg.name,
                    type_annotation=field_cfg.type_annotation,
                    default=field_cfg.default,
                    description=field_cfg.description,
                )
            )
            configured_field_names.add(field_cfg.name)

        # Add remaining schema-derived attributes not in common_fields
        for python_name, (python_type, required) in sorted(all_attrs.items()):
            if python_name in configured_field_names:
                continue
            fields.append(
                CompositeFieldContext(
                    name=python_name,
                    type_annotation=python_type,
                    default=None if required else "None",
                    description=None,
                )
            )

        # Check for field sync
        has_field_sync = len(comp.field_syncs) > 0
        field_sync_a = comp.field_syncs[0].field_a if has_field_sync else None
        field_sync_b = comp.field_syncs[0].field_b if has_field_sync else None

        contexts.append(
            CompositeContext(
                name=comp.name,
                fields=fields,
                extra_fields=comp.extra_fields,
                id_field_name=comp.id_field_name,
                type_field_name=comp.type_field_name,
                type_enum_from_registry=comp.type_enum_from_registry,
                included_entity_names=included_entities,
                has_field_sync=has_field_sync,
                field_sync_a=field_sync_a,
                field_sync_b=field_sync_b,
                extra_fields_out=comp.extra_fields_out or None,
                skip_variants=comp.skip_variants,
            )
        )

    return contexts


def render_api_dto(
    schema: ParsedSchema,
    config: DTOConfig | None = None,
) -> str:
    """Render the api_dto.py file from the parsed schema.

    Args:
        schema: The parsed TypeDB schema
        config: Optional DTO configuration for customization

    Returns:
        Generated Python code as a string
    """
    effective_config = config or DTOConfig()

    # Build class name mappings
    entity_class_names = {name: to_class_name(name) for name in schema.entities}
    relation_class_names = {name: to_class_name(name) for name in schema.relations}

    # Check for name clashes between entity/relation names and union type names
    entity_union = effective_config.entity_union_name
    relation_union = effective_config.relation_union_name

    for name, class_name in entity_class_names.items():
        if class_name == entity_union:
            raise ValueError(
                f"Entity '{name}' generates class name '{class_name}' which clashes "
                f"with entity_union_name='{entity_union}'. Use a different "
                f"entity_union_name in DTOConfig to avoid this conflict."
            )
        if class_name == relation_union:
            raise ValueError(
                f"Entity '{name}' generates class name '{class_name}' which clashes "
                f"with relation_union_name='{relation_union}'. Use a different "
                f"relation_union_name in DTOConfig to avoid this conflict."
            )

    for name, class_name in relation_class_names.items():
        if class_name == entity_union:
            raise ValueError(
                f"Relation '{name}' generates class name '{class_name}' which clashes "
                f"with entity_union_name='{entity_union}'. Use a different "
                f"entity_union_name in DTOConfig to avoid this conflict."
            )
        if class_name == relation_union:
            raise ValueError(
                f"Relation '{name}' generates class name '{class_name}' which clashes "
                f"with relation_union_name='{relation_union}'. Use a different "
                f"relation_union_name in DTOConfig to avoid this conflict."
            )

    # Build custom base class contexts
    base_class_contexts = _build_base_class_contexts(effective_config, schema)

    # Build validator contexts
    validator_contexts = _build_validator_contexts(effective_config)

    # Build composite entity contexts
    composite_contexts = _build_composite_contexts(effective_config, schema)

    # Build set of inherited attrs to skip (from all base classes)
    inherited_attrs_to_skip: dict[str, set[str]] = {}
    for bc in effective_config.base_classes:
        inherited_attrs_to_skip[bc.source_entity] = set(bc.inherited_attrs)

    # Build set of excluded entities
    excluded_entities = set(effective_config.exclude_entities)

    # Build entity contexts
    entities = []
    for name, spec in schema.entities.items():
        # Skip excluded entities
        if name in excluded_entities:
            continue

        parent_class = _get_parent_class(
            name,
            spec.parent,
            entity_class_names,
            "BaseDTO",
            effective_config,
            schema.entities,
        )

        # Find which attrs to skip (inherited from custom base)
        skip_attrs: set[str] = set()
        base_config = effective_config.get_base_class_for_entity(name, schema.entities)
        if base_config and name != base_config.source_entity:
            skip_attrs = inherited_attrs_to_skip.get(base_config.source_entity, set())

        # Get attributes owned by this entity (excluding inherited ones)
        own_attrs = []
        for attr_name in sorted(spec.owns):
            if attr_name in skip_attrs:
                continue
            attr_ctx = _get_attribute_context(attr_name, schema, spec)
            # Apply entity_field_overrides
            for efo in effective_config.entity_field_overrides:
                if efo.entity == name and efo.field == attr_name:
                    if efo.variant == "create":
                        if efo.required is not None:
                            attr_ctx.create_required = efo.required
                        if efo.default is not None:
                            attr_ctx.create_default = efo.default
                    elif efo.variant == "out":
                        if efo.required is not None:
                            attr_ctx.out_required = efo.required
                        if efo.default is not None:
                            attr_ctx.out_default = efo.default
            own_attrs.append(attr_ctx)

        entities.append(
            EntityContext(
                name=name,
                class_name=entity_class_names[name],
                parent_class=parent_class,
                abstract=spec.abstract,
                own_attributes=own_attrs,
            )
        )

    # Sort entities by inheritance (parents before children)
    entities = _sort_by_inheritance(entities, schema.entities)

    # Build relation contexts
    relations = []
    for name, spec in schema.relations.items():
        parent_class = _get_parent_class(
            name,
            spec.parent,
            relation_class_names,
            "BaseRelation",
            effective_config,
            {},  # Relations don't have custom base classes yet
        )

        # Get roles for this relation
        roles = []
        for role_spec in spec.roles:
            # Check cardinality to determine if required
            required = False
            if role_spec.cardinality:
                required = role_spec.cardinality.min >= 1

            roles.append(
                RoleContext(
                    name=role_spec.name,
                    python_name=to_python_name(role_spec.name),
                    required=required,
                )
            )

        # Get attributes owned by this relation
        own_attrs = []
        for attr_name in sorted(spec.owns):
            attr_ctx = _get_attribute_context(attr_name, schema, spec)
            own_attrs.append(attr_ctx)

        relations.append(
            RelationContext(
                name=name,
                class_name=relation_class_names[name],
                parent_class=parent_class,
                abstract=spec.abstract,
                roles=roles,
                own_attributes=own_attrs,
            )
        )

    # Sort relations by inheritance
    relations = _sort_by_inheritance(relations, schema.relations)

    # Filter concrete (non-abstract) types for union types
    concrete_entities = [e for e in entities if not e.abstract]
    concrete_relations = [r for r in relations if not r.abstract]

    template = get_template("api_dto.py.jinja")
    return template.render(
        entities=entities,
        relations=relations,
        concrete_entities=concrete_entities,
        concrete_relations=concrete_relations,
        base_classes=base_class_contexts,
        validators=validator_contexts,
        composites=composite_contexts,
        preamble=effective_config.preamble,
        entity_union_name=effective_config.entity_union_name,
        relation_union_name=effective_config.relation_union_name,
        iid_field_name=effective_config.iid_field_name,
        skip_relation_output=effective_config.skip_relation_output,
        relation_create_base_class=effective_config.relation_create_base_class,
        relation_preamble=effective_config.relation_preamble,
        strict_out_models=effective_config.strict_out_models,
    )


def _sort_by_inheritance(
    items: list[EntityContext] | list[RelationContext],
    specs: dict,
) -> list:
    """Sort items so that parent types come before child types."""
    # Build dependency graph
    name_to_item = {item.name: item for item in items}
    sorted_items: list = []
    visited: set[str] = set()

    def visit(name: str) -> None:
        if name in visited:
            return
        visited.add(name)

        # Visit parent first
        spec = specs.get(name)
        if spec and spec.parent and spec.parent in name_to_item:
            visit(spec.parent)

        if name in name_to_item:
            sorted_items.append(name_to_item[name])

    for item in items:
        visit(item.name)

    return sorted_items
