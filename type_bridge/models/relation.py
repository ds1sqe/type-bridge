"""Relation class for TypeDB relations."""

from __future__ import annotations

import logging
from typing import (
    TYPE_CHECKING,
    Any,
    ClassVar,
    TypeVar,
    dataclass_transform,
    get_origin,
    get_type_hints,
)

from pydantic import ConfigDict
from pydantic._internal._model_construction import ModelMetaclass

from type_bridge.attribute import AttributeFlags, TypeFlags
from type_bridge.models.base import TypeDBType
from type_bridge.models.role import Role
from type_bridge.models.utils import ModelAttrInfo, extract_metadata

if TYPE_CHECKING:
    from type_bridge.crud import RelationManager
    from type_bridge.session import Connection

logger = logging.getLogger(__name__)

# Type variable for self type
R = TypeVar("R", bound="Relation")


class RelationMeta(ModelMetaclass):
    """
    Metaclass for Relation that enables class-level field access for query building.

    Intercepts class-level attribute access to return FieldRef instances
    for defined fields, enabling syntax like Employment.position.eq(Position("Engineer")).
    """

    def __new__(
        mcs, name: str, bases: tuple[type, ...], namespace: dict[str, Any], **kwargs: Any
    ) -> type:
        """
        Create a new Relation class.

        Removes any non-default field values from namespace before Pydantic processes it.
        This prevents Pydantic from capturing spurious defaults.
        """
        import warnings

        # Now call parent __new__ (ModelMetaclass)
        # The smart __getattribute__ below will prevent FieldRef from being
        # captured as defaults during Pydantic's field collection
        # Suppress Pydantic's field shadowing warnings - field shadowing is intentional
        # in TypeBridge when child relations override parent attributes
        with warnings.catch_warnings():
            warnings.filterwarnings(
                "ignore", message=".*shadows an attribute.*", category=UserWarning
            )
            return super().__new__(mcs, name, bases, namespace, **kwargs)

    def __getattribute__(cls, name: str) -> Any:
        """
        Intercept class-level attribute access.

        For owned attributes AFTER initialization is complete, return FieldRef instances.
        For roles AFTER initialization is complete, return RoleRef instances.
        During Pydantic initialization, return the actual descriptor.
        """
        # Check if this is a field/role and if we should return FieldRef/RoleRef
        try:
            pydantic_complete = super().__getattribute__("__pydantic_complete__")

            if pydantic_complete:
                # Check if it's an owned attribute -> return FieldRef
                owned_attrs = super().__getattribute__("_owned_attrs")
                if name in owned_attrs:
                    from type_bridge.fields import FieldDescriptor

                    attr_info = owned_attrs[name]
                    descriptor = FieldDescriptor(field_name=name, attr_type=attr_info.typ)
                    return descriptor.__get__(None, cls)

                # Check if it's a role -> return RoleRef
                roles = super().__getattribute__("_roles")
                if name in roles:
                    from type_bridge.fields.role import RoleRef

                    role = roles[name]
                    return RoleRef(
                        role_name=role.role_name,
                        player_types=role.player_entity_types,
                    )
        except AttributeError:
            # _owned_attrs, _roles, or __pydantic_complete__ not defined yet
            pass

        # For all other cases, use normal access
        return super().__getattribute__(name)


@dataclass_transform(kw_only_default=True, field_specifiers=(AttributeFlags,))
class Relation(TypeDBType, metaclass=RelationMeta):
    """Base class for TypeDB relations with Pydantic validation.

    Relations can own attributes and have role players.
    Use TypeFlags to configure type name and abstract status.
    Supertype is determined automatically from Python inheritance.

    This class inherits from TypeDBType and Pydantic's BaseModel, providing:
    - Automatic validation of attribute values
    - JSON serialization/deserialization
    - Type checking and coercion
    - Field metadata via Pydantic's Field()

    Example:
        class Position(String):
            pass

        class Salary(Integer):
            pass

        class Employment(Relation):
            flags = TypeFlags(name="employment")

            employee: Role[Person] = Role("employee", Person)
            employer: Role[Company] = Role("employer", Company)

            position: Position
            salary: Salary | None
    """

    # Pydantic configuration (extends TypeDBType config)
    model_config = ConfigDict(
        arbitrary_types_allowed=True,
        validate_assignment=True,
        extra="allow",
        ignored_types=(TypeFlags, Role),
        revalidate_instances="always",
    )

    # Relation-specific metadata
    _roles: ClassVar[dict[str, Role]] = {}

    def __init_subclass__(cls) -> None:
        """Initialize relation subclass."""
        super().__init_subclass__()
        logger.debug(f"Initializing Relation subclass: {cls.__name__}")

        # Collect roles from type hints
        roles = {}

        # Check annotations for Role[T] fields
        annotations = getattr(cls, "__annotations__", {})
        for key, hint in annotations.items():
            if not key.startswith("_") and key != "flags":
                # Check if it's a Role[T] type
                origin = get_origin(hint)
                if origin is Role:
                    # It's Role[T] - get value directly from __dict__ to avoid
                    # triggering Role.__get__ descriptor (which returns RoleRef)
                    value = cls.__dict__.get(key)
                    if isinstance(value, Role):
                        roles[key] = value

        cls._roles = roles

        # Extract owned attributes from type hints
        owned_attrs = {}

        # Get direct annotations from this class
        direct_annotations = set(getattr(cls, "__annotations__", {}).keys())

        # Also include annotations from base=True parent classes
        # (they don't appear in TypeDB schema, so child must own their attributes)
        # Stop when we hit a non-base Relation class (it already handles its base=True parents)
        for base in cls.__mro__[1:]:  # Skip cls itself
            if base is Relation or not issubclass(base, Relation):
                continue
            if hasattr(base, "_flags") and base._flags.base:
                base_annotations = getattr(base, "__annotations__", {})
                direct_annotations.update(base_annotations.keys())
            else:
                # Stop at first non-base Relation class
                break

        try:
            # Use include_extras=True to preserve Annotated metadata
            all_hints = get_type_hints(cls, include_extras=True)
            # Filter to only include direct annotations and base=True parent annotations
            hints = {k: v for k, v in all_hints.items() if k in direct_annotations}
        except Exception:
            hints = {
                k: v
                for k, v in getattr(cls, "__annotations__", {}).items()
                if k in direct_annotations
            }

        # Rewrite annotations to add base types for type checker support
        new_annotations = {}

        for field_name, field_type in hints.items():
            if field_name.startswith("_"):
                new_annotations[field_name] = field_type
                continue
            if field_name == "flags":  # Skip the flags field itself
                new_annotations[field_name] = field_type
                continue
            if field_name in roles:  # Skip role fields
                new_annotations[field_name] = field_type
                continue

            # Get the default value (should be AttributeFlags from Flag())
            default_value = getattr(cls, field_name, None)

            # Extract attribute type and cardinality/key/unique metadata
            field_info = extract_metadata(field_type)

            # Check if field type is a list annotation
            field_origin = get_origin(field_type)
            is_list_type = field_origin is list

            # If we found an Attribute type, add it to owned attributes
            if field_info.attr_type is not None:
                # Validate: list[Type] must have Flag(Card(...))
                if is_list_type and not isinstance(default_value, AttributeFlags):
                    raise TypeError(
                        f"Field '{field_name}' in {cls.__name__}: "
                        f"list[Type] annotations must use Flag(Card(...)) to specify cardinality. "
                        f"Example: {field_name}: list[{field_info.attr_type.__name__}] = Flag(Card(min=1))"
                    )

                # Get flags from default value or create new flags
                if isinstance(default_value, AttributeFlags):
                    flags = default_value

                    # Validate: Flag(Card(...)) should only be used with list[Type]
                    if flags.has_explicit_card and not is_list_type:
                        raise TypeError(
                            f"Field '{field_name}' in {cls.__name__}: "
                            f"Flag(Card(...)) can only be used with list[Type] annotations. "
                            f"For optional single values, use Optional[{field_info.attr_type.__name__}] instead."
                        )

                    # Validate: list[Type] must have Flag(Card(...))
                    if is_list_type and not flags.has_explicit_card:
                        raise TypeError(
                            f"Field '{field_name}' in {cls.__name__}: "
                            f"list[Type] annotations must use Flag(Card(...)) to specify cardinality. "
                            f"Example: {field_name}: list[{field_info.attr_type.__name__}] = Flag(Card(min=1))"
                        )

                    # Merge with cardinality from type annotation if not already set
                    if flags.card_min is None and flags.card_max is None:
                        flags.card_min = field_info.card_min
                        flags.card_max = field_info.card_max
                    # Set is_key and is_unique from type annotation if found
                    if field_info.is_key:
                        flags.is_key = True
                    if field_info.is_unique:
                        flags.is_unique = True
                else:
                    # Create flags from type annotation metadata
                    flags = AttributeFlags(
                        is_key=field_info.is_key,
                        is_unique=field_info.is_unique,
                        card_min=field_info.card_min,
                        card_max=field_info.card_max,
                    )

                owned_attrs[field_name] = ModelAttrInfo(typ=field_info.attr_type, flags=flags)

                # Keep annotation as-is - no need for unions since validators always return Attribute instances
                # - position: Position → stays as Position
                # - salary: Salary | None → stays as Salary | None
                # - tags: list[Tag] → stays as list[Tag]
                new_annotations[field_name] = field_type
            else:
                new_annotations[field_name] = field_type

        # Update class annotations for Pydantic's benefit
        cls.__annotations__ = new_annotations
        cls._owned_attrs = owned_attrs

    @classmethod
    def get_supertype(cls) -> str | None:
        """Get the supertype from Python inheritance, skipping base classes.

        Base classes (with base=True) are Python-only and don't appear in TypeDB schema.
        This method skips them when determining the TypeDB supertype.

        Returns:
            Type name of the parent Relation class, or None if direct Relation subclass
        """
        for base in cls.__bases__:
            if base is not Relation and issubclass(base, Relation):
                # Skip base classes - they don't appear in TypeDB schema
                if base.is_base():
                    # Recursively find the first non-base parent
                    return base.get_supertype()
                return base.get_type_name()
        return None

    @classmethod
    def get_roles(cls) -> dict[str, Role]:
        """Get all roles defined on this relation.

        Returns:
            Dictionary mapping role names to Role instances
        """
        return cls._roles

    @classmethod
    def manager(cls: type[R], connection: Connection) -> RelationManager[R]:
        """Create a RelationManager for this relation type.

        Args:
            connection: Database, Transaction, or TransactionContext

        Returns:
            RelationManager instance for this relation type with proper type information

        Example:
            from type_bridge import Database

            db = Database()
            db.connect()

            # Create typed relation instance
            employment = Employment(
                employee=person,
                employer=company,
                position=Position("Engineer")
            )

            # Insert using manager - with full type safety!
            Employment.manager(db).insert(employment)
            # employment is inferred as Employment type by type checkers
        """
        from type_bridge.crud import RelationManager

        return RelationManager(connection, cls)

    def to_insert_query(self, var: str = "$r") -> str:
        """Generate TypeQL insert query for this relation instance.

        Args:
            var: Variable name to use

        Returns:
            TypeQL insert pattern for the relation

        Example:
            >>> employment = Employment(employee=alice, employer=tech_corp, position="Engineer")
            >>> employment.to_insert_query()
            '$r (employee: $alice, employer: $tech_corp) isa employment, has position "Engineer"'
        """
        type_name = self.get_type_name()

        # Build role players
        role_parts = []
        for role_name, role in self.__class__._roles.items():
            # Get the entity from the instance
            entity = self.__dict__.get(role_name)
            if entity is not None:
                # Use the entity's variable or IID
                if hasattr(entity, "_iid") and entity._iid:
                    # Use existing entity's IID
                    role_parts.append(f"{role.role_name}: ${role_name}")
                else:
                    # New entity - use a variable
                    role_parts.append(f"{role.role_name}: ${role_name}")

        # Start with relation pattern
        relation_pattern = f"{var} ({', '.join(role_parts)}) isa {type_name}"
        parts = [relation_pattern]

        # Add attribute ownerships
        for field_name, attr_info in self._owned_attrs.items():
            value = getattr(self, field_name, None)
            if value is not None:
                attr_class = attr_info.typ
                attr_name = attr_class.get_attribute_name()

                # Handle lists (multi-value attributes)
                if isinstance(value, list):
                    for item in value:
                        parts.append(f"has {attr_name} {self._format_value(item)}")
                else:
                    parts.append(f"has {attr_name} {self._format_value(value)}")

        return ", ".join(parts)

    def insert(self: R, connection: Connection) -> R:
        """Insert this relation instance into the database.

        Args:
            connection: Database, Transaction, or TransactionContext

        Returns:
            Self for chaining

        Example:
            employment = Employment(
                position=Position("Engineer"),
                salary=Salary(100000),
                employee=person,
                employer=company
            )
            employment.insert(db)
        """
        manager = self.__class__.manager(connection)
        manager.insert(self)
        return self

    def delete(self: R, connection: Connection) -> R:
        """Delete this relation instance from the database.

        Args:
            connection: Database, Transaction, or TransactionContext

        Returns:
            Self for chaining

        Example:
            employment = Employment(
                position=Position("Engineer"),
                salary=Salary(100000),
                employee=person,
                employer=company
            )
            employment.insert(db)
            employment.delete(db)
        """
        manager = self.__class__.manager(connection)
        manager.delete(self)
        return self

    @classmethod
    def to_schema_definition(cls) -> str | None:
        """Generate TypeQL schema definition for this relation.

        Returns:
            TypeQL schema definition string, or None if this is a base class
        """
        # Base classes don't appear in TypeDB schema
        if cls.is_base():
            return None

        type_name = cls.get_type_name()
        lines = []

        # Define relation type with supertype from Python inheritance
        # TypeDB 3.x syntax: relation name @abstract, sub parent,
        supertype = cls.get_supertype()
        is_abstract = cls.is_abstract()

        relation_def = f"relation {type_name}"
        if is_abstract:
            relation_def += " @abstract"
        if supertype:
            relation_def += f", sub {supertype}"

        lines.append(relation_def)

        # Add roles with optional cardinality constraints
        for _role_name, role in cls._roles.items():
            role_def = f"    relates {role.role_name}"
            # Add cardinality annotation if not default (1..1)
            if role.cardinality is not None:
                card = role.cardinality
                if card.max is None:
                    role_def += f" @card({card.min}..)"
                else:
                    role_def += f" @card({card.min}..{card.max})"
            lines.append(role_def)

        # Add attribute ownerships
        for _field_name, attr_info in cls._owned_attrs.items():
            attr_class = attr_info.typ
            flags = attr_info.flags
            attr_name = attr_class.get_attribute_name()

            ownership = f"    owns {attr_name}"
            annotations = flags.to_typeql_annotations()
            if annotations:
                ownership += " " + " ".join(annotations)
            lines.append(ownership)

        # Join with commas, but end with semicolon (no comma before semicolon)
        return ",\n".join(lines) + ";"

    def __repr__(self) -> str:
        """Developer-friendly string representation of relation."""
        parts = []
        # Show role players
        for role_name in self._roles:
            player = getattr(self, role_name, None)
            if player is not None:
                parts.append(f"{role_name}={player!r}")
        # Show attributes
        for field_name in self._owned_attrs:
            value = getattr(self, field_name, None)
            if value is not None:
                parts.append(f"{field_name}={value!r}")
        return f"{self.__class__.__name__}({', '.join(parts)})"

    def __str__(self) -> str:
        """User-friendly string representation of relation."""
        parts = []

        # Show role players first (more important)
        role_parts = []
        for role_name, role in self._roles.items():
            player = getattr(self, role_name, None)
            # Only show role players that are actual entity instances (have _owned_attrs)
            if player is not None and hasattr(player, "_owned_attrs"):
                # Get a simple representation of the player (their key attribute)
                player_str = None
                for field_name, attr_info in player._owned_attrs.items():
                    if attr_info.flags.is_key:
                        key_value = getattr(player, field_name, None)
                        if key_value is not None:
                            if hasattr(key_value, "value"):
                                player_str = str(key_value.value)
                            else:
                                player_str = str(key_value)
                            break

                if player_str:
                    role_parts.append(f"{role_name}={player_str}")

        if role_parts:
            parts.append("(" + ", ".join(role_parts) + ")")

        # Show attributes
        attr_parts = []
        for field_name, attr_info in self._owned_attrs.items():
            value = getattr(self, field_name, None)
            if value is None:
                continue

            # Extract actual value from Attribute instance
            if hasattr(value, "value"):
                display_value = value.value
            else:
                display_value = value

            attr_parts.append(f"{field_name}={display_value}")

        if attr_parts:
            parts.append("[" + ", ".join(attr_parts) + "]")

        if parts:
            return f"{self.get_type_name()}{' '.join(parts)}"
        else:
            return f"{self.get_type_name()}()"
