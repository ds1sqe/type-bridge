"""Entity class for TypeDB entities."""

from __future__ import annotations

import logging
from typing import (
    TYPE_CHECKING,
    Any,
    Self,
    TypeVar,
    dataclass_transform,
)

from pydantic import ConfigDict

from type_bridge.attribute import Attribute, AttributeFlags, TypeFlags
from type_bridge.crud.utils import unwrap_attribute
from type_bridge.models.base import TypeDBType
from type_bridge.models.utils import (
    MatchClauseInfo,
    ModelAttrInfo,
)

if TYPE_CHECKING:
    from type_bridge.query.ast import EntityPattern, InsertClause

logger = logging.getLogger(__name__)

# Type variable for self type
E = TypeVar("E", bound="Entity")


@dataclass_transform(kw_only_default=True, field_specifiers=(AttributeFlags,))
class Entity(TypeDBType):
    """Base class for TypeDB entities with Pydantic validation.

    Entities own attributes defined as Attribute subclasses.
    Use TypeFlags to configure type name and abstract status.
    Supertype is determined automatically from Python inheritance.

    This class inherits from TypeDBType and Pydantic's BaseModel, providing:
    - Automatic validation of attribute values
    - JSON serialization/deserialization
    - Type checking and coercion
    - Field metadata via Pydantic's Field()

    Example:
        class Name(String):
            pass

        class Age(Integer):
            pass

        class Person(Entity):
            flags = TypeFlags(name="person")
            name: Name = Flag(Key)
            age: Age

        # Abstract entity
        class AbstractPerson(Entity):
            flags = TypeFlags(abstract=True)
            name: Name

        # Inheritance (Person sub abstract-person)
        class ConcretePerson(AbstractPerson):
            age: Age
    """

    # Pydantic configuration (extends TypeDBType config)
    model_config = ConfigDict(
        arbitrary_types_allowed=True,
        validate_assignment=True,
        extra="allow",
        ignored_types=(TypeFlags,),
        revalidate_instances="always",
    )

    _type_context = "entity"

    def __init_subclass__(cls) -> None:
        """Called when Entity subclass is created."""
        super().__init_subclass__()
        logger.debug(f"Initializing Entity subclass: {cls.__name__}")

        from type_bridge.models.schema_scanner import SchemaScanner

        scanner = SchemaScanner(cls)
        cls._owned_attrs = scanner.scan_attributes(is_relation=False)

        from type_bridge.models.registry import ModelRegistry

        ModelRegistry.register_attribute_owners(cls)

    @classmethod
    def _get_base_type_class(cls) -> type[Entity]:
        """Return Entity as the base type class for supertype resolution."""
        return Entity

    @classmethod
    def _get_manager_class(cls) -> type:
        from type_bridge.crud import TypeDBManager

        return TypeDBManager

    @classmethod
    def to_schema_definition(cls) -> str | None:
        """Generate TypeQL schema definition for this entity.

        Returns:
            TypeQL schema definition string, or None if this is a base class
        """
        from type_bridge.typeql.annotations import format_type_annotations

        # Base classes don't appear in TypeDB schema
        if cls.is_base():
            return None

        type_name = cls.get_type_name()
        lines = []

        # Define entity type with supertype from Python inheritance
        # TypeDB 3.x syntax: entity name @abstract, sub parent,
        supertype = cls.get_supertype()
        type_annotations = format_type_annotations(abstract=cls.is_abstract())

        entity_def = f"entity {type_name}"
        if type_annotations:
            entity_def += " " + " ".join(type_annotations)
        if supertype:
            entity_def += f", sub {supertype}"

        lines.append(entity_def)

        # Add attribute ownerships using shared helper
        lines.extend(cls._build_owns_lines())

        # Join with commas, but end with semicolon (no comma before semicolon)
        return ",\n".join(lines) + ";"

    def to_ast(self, var: str = "$e") -> InsertClause:
        """Generate AST InsertClause for this instance.

        Args:
            var: Variable name to use

        Returns:
            InsertClause containing statements
        """
        from type_bridge.query.ast import InsertClause, IsaStatement, Statement

        type_name = self.get_type_name()
        statements: list[Statement] = [IsaStatement(variable=var, type_name=type_name)]

        # Add attribute statements using shared helper from TypeDBType
        statements.extend(self._build_attribute_statements(var))

        return InsertClause(statements=statements)

    def get_match_clause_info(self, var_name: str = "$e") -> MatchClauseInfo:
        """Get match clause info for this entity instance.

        Prefers IID-based matching when available (most precise).
        Falls back to @key attribute matching.

        Args:
            var_name: Variable name to use in the match clause

        Returns:
            MatchClauseInfo with the match clause

        Raises:
            ValueError: If entity has neither _iid nor key attributes
        """
        type_name = self.get_type_name()

        # Prefer IID-based matching when available
        entity_iid = getattr(self, "_iid", None)
        if entity_iid:
            main_clause = f"{var_name} isa {type_name}, iid {entity_iid}"
            return MatchClauseInfo(main_clause=main_clause, extra_clauses=[], var_name=var_name)

        # Fall back to key attribute matching
        key_attrs = {
            field_name: attr_info
            for field_name, attr_info in self.get_all_attributes().items()
            if attr_info.flags.is_key
        }

        if key_attrs:
            parts = [f"{var_name} isa {type_name}"]
            for field_name, attr_info in key_attrs.items():
                value = getattr(self, field_name, None)
                if value is None:
                    from type_bridge.crud.exceptions import KeyAttributeError

                    raise KeyAttributeError(
                        entity_type=self.__class__.__name__,
                        operation="identify",
                        field_name=field_name,
                    )
                attr_name = attr_info.typ.get_attribute_name()
                parts.append(f"has {attr_name} {self._format_value(value)}")
            main_clause = ", ".join(parts)
            return MatchClauseInfo(main_clause=main_clause, extra_clauses=[], var_name=var_name)

        # Neither IID nor key attributes available
        raise ValueError(
            f"Entity '{self.__class__.__name__}' cannot be identified: "
            f"no _iid set and no @key attributes defined. Either fetch the entity from the "
            f"database first (to populate _iid) or add Flag(Key) to an attribute."
        )

    def _build_identification_constraints(self) -> list[Any]:
        """Build AST constraints to identify this entity instance.

        Returns constraints for either IID-based or key-based identification.
        This is a shared helper used by get_match_pattern() and EntityStrategy.identify().

        Returns:
            List of Constraint AST nodes (IidConstraint or HasConstraint)

        Raises:
            ValueError: If entity has neither _iid nor key attributes
        """
        from type_bridge.crud.patterns import _get_literal_type
        from type_bridge.query.ast import HasConstraint, IidConstraint, LiteralValue

        # Prefer IID-based matching when available
        entity_iid = getattr(self, "_iid", None)
        if entity_iid:
            return [IidConstraint(iid=entity_iid)]

        # Fall back to key attribute matching
        key_attrs = {
            field_name: attr_info
            for field_name, attr_info in self.get_all_attributes().items()
            if attr_info.flags.is_key
        }

        if not key_attrs:
            raise ValueError(
                f"Entity '{self.__class__.__name__}' cannot be identified: "
                f"no _iid set and no @key attributes defined."
            )

        constraints: list[HasConstraint] = []
        for field_name, attr_info in key_attrs.items():
            value = getattr(self, field_name, None)
            if value is None:
                from type_bridge.crud.exceptions import KeyAttributeError

                raise KeyAttributeError(
                    entity_type=self.__class__.__name__,
                    operation="identify",
                    field_name=field_name,
                )
            attr_name = attr_info.typ.get_attribute_name()
            # Unwrap Attribute wrapper if needed
            raw_value = value.value if hasattr(value, "value") else value
            literal_type = _get_literal_type(raw_value)
            constraints.append(
                HasConstraint(
                    attr_name=attr_name,
                    value=LiteralValue(value=raw_value, value_type=literal_type),
                )
            )

        return constraints

    def get_match_pattern(self, var_name: str = "$e") -> EntityPattern:
        """Get an AST EntityPattern for matching this entity instance.

        Prefers IID-based matching when available (most precise).
        Falls back to @key attribute matching.

        Args:
            var_name: Variable name to use in the pattern

        Returns:
            EntityPattern AST node

        Raises:
            ValueError: If entity has neither _iid nor key attributes
        """
        from type_bridge.query.ast import EntityPattern

        type_name = self.get_type_name()
        constraints = self._build_identification_constraints()
        return EntityPattern(variable=var_name, type_name=type_name, constraints=constraints)

    def to_dict(
        self,
        *,
        include: set[str] | None = None,
        exclude: set[str] | None = None,
        by_alias: bool = False,
        exclude_unset: bool = False,
    ) -> dict[str, Any]:
        """Serialize the entity to a primitive dict.

        Args:
            include: Optional set of field names to include.
            exclude: Optional set of field names to exclude.
            by_alias: When True, use attribute TypeQL names instead of Python field names.
            exclude_unset: When True, omit fields that were never explicitly set.
        """
        # Let Pydantic handle include/exclude/exclude_unset, then unwrap Attribute values.
        dumped = self.model_dump(
            include=include,
            exclude=exclude,
            by_alias=False,
            exclude_unset=exclude_unset,
        )

        attrs = self.get_all_attributes()
        result: dict[str, Any] = {}

        for field_name, raw_value in dumped.items():
            attr_info = attrs[field_name]
            key = attr_info.typ.get_attribute_name() if by_alias else field_name
            if by_alias and key in result and key != field_name:
                # Avoid collisions when multiple fields share the same attribute type
                key = field_name
            result[key] = self._unwrap_value(raw_value)

        return result

    @staticmethod
    def _unwrap_value(value: Any) -> Any:
        """Convert Attribute instances (or lists of them) to primitive values."""
        if isinstance(value, list):
            return [Entity._unwrap_value(item) for item in value]
        if isinstance(value, Attribute):
            return value.value
        return value

    @classmethod
    def from_dict(
        cls,
        data: dict[str, Any],
        *,
        field_mapping: dict[str, str] | None = None,
        strict: bool = True,
    ) -> Self:
        """Construct an Entity from a plain dictionary.

        Args:
            data: External data to hydrate the Entity.
            field_mapping: Optional mapping of external keys to internal field names.
            strict: When True, raise on unknown fields; otherwise ignore them.
        """
        mapping = field_mapping or {}
        attrs = cls.get_all_attributes()
        alias_to_field = {info.typ.get_attribute_name(): name for name, info in attrs.items()}
        normalized: dict[str, Any] = {}

        for raw_key, raw_value in data.items():
            internal_key = mapping.get(raw_key, raw_key)
            if internal_key not in attrs and raw_key in alias_to_field:
                internal_key = alias_to_field[raw_key]

            if internal_key not in attrs:
                if strict:
                    raise ValueError(f"Unknown field '{raw_key}' for {cls.__name__}")
                continue

            if raw_value is None:
                continue

            attr_info = attrs[internal_key]
            wrapped_value = cls._wrap_attribute_value(raw_value, attr_info)

            if wrapped_value is None:
                continue

            normalized[internal_key] = wrapped_value

        return cls(**normalized)

    @staticmethod
    def _wrap_attribute_value(value: Any, attr_info: ModelAttrInfo) -> Any:
        """Wrap raw values using the attribute class, handling multi-value fields.

        Uses the unified wrap_attribute_value() helper for consistent behavior
        across all hydration paths.
        """
        from type_bridge.crud.types import wrap_attribute_value

        return wrap_attribute_value(value, attr_info, use_pydantic_validate=True)

    def __repr__(self) -> str:
        """Developer-friendly string representation of entity."""
        field_strs = []
        for field_name in self._owned_attrs:
            value = getattr(self, field_name, None)
            if value is not None:
                field_strs.append(f"{field_name}={value!r}")
        return f"{self.__class__.__name__}({', '.join(field_strs)})"

    def __str__(self) -> str:
        """User-friendly string representation of entity."""
        # Extract key attributes first
        key_parts = []
        other_parts = []

        for field_name, attr_info in self._owned_attrs.items():
            value = getattr(self, field_name, None)
            if value is None:
                continue

            # Extract actual value from Attribute instance
            display_value = unwrap_attribute(value)

            # Format the field
            field_str = f"{field_name}={display_value}"

            # Separate key attributes
            if attr_info.flags.is_key:
                key_parts.append(field_str)
            else:
                other_parts.append(field_str)

        # Show key attributes first, then others
        all_parts = key_parts + other_parts

        if all_parts:
            return f"{self.get_type_name()}({', '.join(all_parts)})"
        else:
            return f"{self.get_type_name()}()"
