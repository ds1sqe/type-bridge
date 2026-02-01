"""Role descriptor for TypeDB relation role players."""

from __future__ import annotations

from typing import TYPE_CHECKING, Any, overload

from pydantic import GetCoreSchemaHandler
from pydantic_core import CoreSchema

from type_bridge.attribute.flags import Card
from type_bridge.validation import validate_type_name as validate_reserved_word

if TYPE_CHECKING:
    from type_bridge.fields.role import RoleRef
    from type_bridge.models.base import TypeDBType


class Role[T: "TypeDBType"]:
    """Descriptor for relation role players with type safety.

    Generic type T represents the type (Entity or Relation) that can play this role.
    TypeDB supports both entities and relations as role players.

    Example:
        # Entity as role player
        class Employment(Relation):
            employee: Role[Person] = Role("employee", Person)
            employer: Role[Company] = Role("employer", Company)

        # Relation as role player
        class Permission(Relation):
            permitted_subject: Role[Subject] = Role("permitted_subject", Subject)
            permitted_access: Role[Access] = Role("permitted_access", Access)  # Access is a Relation
    """

    def __init__(
        self,
        role_name: str,
        player_type: type[T],
        *additional_player_types: type[T],
        cardinality: Card | None = None,
    ):
        """Initialize a role.

        Args:
            role_name: The name of the role in TypeDB
            player_type: The type (Entity or Relation) that can play this role
            additional_player_types: Optional additional types allowed to play this role
            cardinality: Optional cardinality constraint for the role (e.g., Card(2, 2) for exactly 2)

        Raises:
            ReservedWordError: If role_name is a TypeQL reserved word
            TypeError: If player type is a library base class (Entity, Relation, TypeDBType)
        """
        # Validate role name doesn't conflict with TypeQL reserved words
        validate_reserved_word(role_name, "role")

        self.role_name = role_name
        self.cardinality = cardinality
        unique_types: list[type[T]] = []
        for typ in (player_type, *additional_player_types):
            # Validate that we're not using library base classes directly
            self._validate_player_type(typ)
            if typ not in unique_types:
                unique_types.append(typ)

        if not unique_types:
            # Should be impossible because player_type is required, but keeps type checkers happy
            raise ValueError("Role requires at least one player type")

        self.player_entity_types: tuple[type[T], ...] = tuple(unique_types)
        first_entity_type = unique_types[0]
        self.player_entity_type = first_entity_type
        # Get type name from the entity class(es)
        self.player_types = tuple(pt.get_type_name() for pt in self.player_entity_types)
        self.player_type = first_entity_type.get_type_name()
        self.attr_name: str | None = None

    def _validate_player_type(self, typ: type[T]) -> None:
        """Validate that player type is not a library base class.

        TypeDB doesn't have built-in Entity/Relation types - these are Python
        abstractions. Users must define their own abstract types to use as
        polymorphic role player types.

        Args:
            typ: The type to validate

        Raises:
            TypeError: If typ is Entity, Relation, or TypeDBType base class
        """
        # Check if this is a library base class (not a user-defined subclass)
        # We check module to distinguish library classes from user classes
        library_modules = (
            "type_bridge.models",
            "type_bridge.models.base",
            "type_bridge.models.entity",
            "type_bridge.models.relation",
        )

        if typ.__module__ in library_modules and typ.__name__ in (
            "Entity",
            "Relation",
            "TypeDBType",
        ):
            raise TypeError(
                f"Cannot use library base class '{typ.__name__}' as role player type. "
                f"TypeDB doesn't have a built-in '{typ.__name__}' type. "
                f"Define your own abstract type instead:\n\n"
                f"  class Subject(Entity):\n"
                f"      flags = TypeFlags(abstract=True)\n\n"
                f'Then use: Role("{self.role_name}", Subject)'
            )

    @property
    def is_multi_player(self) -> bool:
        """Check if this role allows multiple players.

        Returns True if cardinality allows more than one player (max > 1 or unbounded).
        """
        if self.cardinality is None:
            return False  # Default is single player
        return self.cardinality.max is None or self.cardinality.max > 1

    def __set_name__(self, owner: type, name: str) -> None:
        """Called when role is assigned to a class."""
        self.attr_name = name

    @overload
    def __get__(self, obj: None, objtype: type) -> RoleRef[T]:
        """Get RoleRef when accessed from class (for query building)."""
        ...

    @overload
    def __get__(self, obj: Any, objtype: type) -> T:
        """Get role player entity when accessed from instance."""
        ...

    def __get__(self, obj: Any, objtype: type) -> T | RoleRef[T]:
        """Get role player from instance or RoleRef from class.

        When accessed from the class (obj is None), returns RoleRef for
        type-safe query building (e.g., Employment.employee.age.gt(Age(30))).
        When accessed from an instance, returns the entity playing the role.
        """
        if obj is None:
            from type_bridge.fields.role import RoleRef

            return RoleRef(
                role_name=self.role_name,
                player_types=self.player_entity_types,
            )
        return obj.__dict__.get(self.attr_name)

    def __set__(self, obj: Any, value: T | list[T]) -> None:
        """Set role player(s) on instance.

        For roles with cardinality > 1, accepts a list of entities.
        For single-player roles, accepts a single entity.
        """
        if isinstance(value, list):
            # Multi-player role - validate each item in the list
            if not self.is_multi_player:
                raise TypeError(
                    f"Role '{self.role_name}' does not allow multiple players. "
                    f"Use cardinality=Card(...) to enable multi-player roles."
                )
            for item in value:
                if not isinstance(item, self.player_entity_types):
                    allowed = ", ".join(pt.__name__ for pt in self.player_entity_types)
                    raise TypeError(
                        f"Role '{self.role_name}' expects types ({allowed}), "
                        f"got {type(item).__name__} in list"
                    )
            obj.__dict__[self.attr_name] = value
        else:
            # Single player
            if not isinstance(value, self.player_entity_types):
                allowed = ", ".join(pt.__name__ for pt in self.player_entity_types)
                raise TypeError(
                    f"Role '{self.role_name}' expects types ({allowed}), got {type(value).__name__}"
                )
            obj.__dict__[self.attr_name] = value

    @classmethod
    def multi(
        cls,
        role_name: str,
        player_type: type[T],
        *additional_player_types: type[T],
        cardinality: Card | None = None,
    ) -> Role[T]:
        """Define a role playable by multiple entity types.

        Args:
            role_name: The name of the role in TypeDB
            player_type: The first entity type that can play this role
            additional_player_types: Additional entity types allowed to play this role
            cardinality: Optional cardinality constraint for the role
        """
        if len((player_type, *additional_player_types)) < 2:
            raise ValueError("Role.multi requires at least two player types")
        return cls(role_name, player_type, *additional_player_types, cardinality=cardinality)

    @classmethod
    def __get_pydantic_core_schema__(
        cls, source_type: Any, handler: GetCoreSchemaHandler
    ) -> CoreSchema:
        """Define how Pydantic should validate Role fields.

        Accepts either:
        - A single entity instance for single-player roles
        - A list of entity instances for multi-player roles (cardinality > 1)

        Uses a custom validator that checks class names instead of isinstance,
        to handle generated code in different modules where the same class name
        exists but as a different Python object.
        """
        import types

        from pydantic_core import core_schema

        # Extract the entity type(s) from Role[T]
        # Handle both Role[Entity] and Role[Entity1 | Entity2] unions
        allowed_names: set[str] = set()

        if hasattr(source_type, "__args__") and source_type.__args__:
            for arg in source_type.__args__:
                # Check if it's a union type (e.g., Document | Email)
                if isinstance(arg, types.UnionType) or (
                    hasattr(arg, "__origin__") and arg.__origin__ is type(int | str)
                ):
                    # It's a union - get the individual types
                    if hasattr(arg, "__args__"):
                        for union_arg in arg.__args__:
                            if hasattr(union_arg, "__name__"):
                                allowed_names.add(union_arg.__name__)
                elif hasattr(arg, "__name__"):
                    allowed_names.add(arg.__name__)

        def validate_role_player(value: Any) -> Any:
            """Validate that value is an allowed entity type by class name.

            Checks the full inheritance chain (MRO) to support subclasses.
            E.g., if Document is allowed and Report is a subclass of Document,
            Report instances are accepted.
            """
            if not allowed_names:
                # No type constraints - allow anything
                return value

            def is_allowed_type(obj: Any) -> bool:
                """Check if obj's class or any base class matches allowed names."""
                # Check entire MRO (Method Resolution Order) for inheritance support
                for cls in type(obj).__mro__:
                    if cls.__name__ in allowed_names:
                        return True
                return False

            if isinstance(value, list):
                # List of entities for multi-player roles
                for item in value:
                    if not is_allowed_type(item):
                        raise ValueError(
                            f"Expected one of {allowed_names}, got {type(item).__name__}"
                        )
                return value
            else:
                # Single entity
                if not is_allowed_type(value):
                    raise ValueError(f"Expected one of {allowed_names}, got {type(value).__name__}")
                return value

        return core_schema.no_info_plain_validator_function(validate_role_player)
