"""Abstract base class for TypeDB entities and relations."""

from __future__ import annotations

from abc import ABC, abstractmethod
from collections.abc import Mapping
from typing import (
    TYPE_CHECKING,
    Any,
    ClassVar,
    Literal,
    Self,
    cast,
    dataclass_transform,
)

from pydantic import BaseModel, ConfigDict, PrivateAttr, model_validator

from type_bridge.attribute import AttributeFlags, TypeFlags
from type_bridge.attribute.flags import format_type_name
from type_bridge.crud.utils import format_value as _format_value_impl
from type_bridge.models.registry import ModelRegistry
from type_bridge.models.utils import (
    MatchClauseInfo,
    ModelAttrInfo,
    validate_type_name,
)

if TYPE_CHECKING:
    from type_bridge.attribute.base import Attribute
    from type_bridge.crud.typedb_manager import TypeDBManager
    from type_bridge.session import Connection


@dataclass_transform(kw_only_default=True, field_specifiers=(AttributeFlags, TypeFlags))
class TypeDBType(BaseModel, ABC):
    """Abstract base class for TypeDB entities and relations.

    This class provides common functionality for both Entity and Relation types,
    including type name management, abstract/base flags, and attribute ownership.

    Subclasses must implement:
    - get_supertype(): Get parent type in TypeDB hierarchy
    - to_schema_definition(): Generate TypeQL schema definition
    - to_insert_query(): Generate TypeQL insert query for instances
    """

    # Pydantic configuration (inherited by Entity and Relation)
    model_config = ConfigDict(
        arbitrary_types_allowed=True,
        validate_assignment=True,
        extra="allow",
        revalidate_instances="always",
    )

    # Internal metadata (class-level)
    _flags: ClassVar[TypeFlags] = TypeFlags()
    _owned_attrs: ClassVar[dict[str, ModelAttrInfo]] = {}

    # TypeDB internal ID - treated as private attribute by Pydantic
    _iid: str | None = PrivateAttr(default=None)

    @classmethod
    @abstractmethod
    def _get_manager_class(cls) -> type:
        """Get the CRUD manager class for this type."""
        ...

    @classmethod
    def manager(cls, connection: Connection) -> TypeDBManager[Self]:
        """Create a CRUD manager for this type.

        Args:
            connection: Database, Transaction, or TransactionContext

        Returns:
            Manager instance for this type
        """
        from type_bridge._backend import manager_class as selected_manager_class

        manager_class = selected_manager_class(cls._get_manager_class())
        return cast("TypeDBManager[Self]", manager_class(connection, cls))

    def _set_backend_iid(self, iid: str | None) -> None:
        """Set the backend-assigned IID through Pydantic's private storage."""
        private = self.__pydantic_private__
        if private is None:
            raise RuntimeError("Pydantic private storage is unavailable")
        private["_iid"] = iid

    def insert(self, connection: Connection) -> Self:
        """Insert this instance into the database.

        Args:
            connection: Database, Transaction, or TransactionContext

        Returns:
            Self for chaining
        """
        self.manager(connection).insert(self)
        return self

    def delete(self, connection: Connection) -> Self:
        """Delete this instance from the database.

        Args:
            connection: Database, Transaction, or TransactionContext

        Returns:
            Self for chaining
        """
        self.manager(connection).delete(self)
        return self

    @classmethod
    def has(
        cls,
        connection: Connection,
        attr_class: type[Attribute],
        value: Any | None = None,
    ) -> list[TypeDBType]:
        """Find all instances of this class (and its subtypes) that own *attr_class*.

        Behaviour depends on the receiver:

        * ``Entity.has(...)`` / ``Relation.has(...)``: cross-type lookup — returns
          instances across **all** concrete types of that kind.
        * ``<ConcreteType>.has(...)`` / ``<AbstractBase>.has(...)``: narrowed
          lookup — restricted to that type and its TypeDB subtypes via ``isa``
          polymorphism.

        Returned relation instances always have their role players hydrated
        (in addition to attributes). This is implemented by re-fetching each
        relation through ``concrete_class.manager(connection).get(_iid=...)``
        after the initial wildcard query, so the relation path is N+1 in the
        number of returned relations. Entity lookups remain single-query.

        Args:
            connection: Database, Transaction, or TransactionContext.
            attr_class: Attribute type to search for (e.g. ``Name``).
            value: Optional filter — raw value, Attribute instance,
                   or Expression (e.g. ``Name.gt(Name("B"))``).

        Returns:
            List of hydrated model instances (may contain mixed concrete types
            when called on the base ``Entity`` / ``Relation`` class or an
            abstract base subclass).

        Raises:
            TypeError: If called directly on :class:`TypeDBType` (use
                :class:`Entity` or :class:`Relation` instead).
        """
        from type_bridge.crud.has_lookup import has_lookup
        from type_bridge.models.entity import Entity
        from type_bridge.models.relation import Relation

        if cls is TypeDBType:
            raise TypeError("has() must be called on Entity or Relation, not TypeDBType directly")

        if issubclass(cls, Entity):
            kind: Literal["entity", "relation"] = "entity"
            base_cls: type[TypeDBType] = Entity
        elif issubclass(cls, Relation):
            kind = "relation"
            base_cls = Relation
        else:
            raise TypeError(f"has() requires an Entity or Relation class, got {cls.__name__}")

        # Narrow to the concrete (or abstract base) type when the caller is
        # not the bare Entity / Relation class. TypeDB's `isa` is polymorphic,
        # so subtypes of an abstract base are matched automatically.
        narrow_type = None if cls is base_cls else cls.get_type_name()

        return has_lookup(connection, attr_class, value, kind=kind, type_name=narrow_type)

    # Type context for name validation (entity, relation, etc.)
    _type_context: ClassVar[Literal["entity", "relation", "attribute", "role"]] = "entity"

    def __init_subclass__(cls) -> None:
        """Called when a TypeDBType subclass is created."""
        super().__init_subclass__()

        # Get TypeFlags if defined, otherwise create new default flags
        # Check if flags is defined directly on this class (not inherited)
        if "flags" in cls.__dict__ and isinstance(cls.__dict__["flags"], TypeFlags):
            # Explicitly set flags on this class
            cls._flags = cls.__dict__["flags"]
        else:
            # No explicit flags on this class - create new default flags
            # This ensures each subclass gets its own flags instance
            cls._flags = TypeFlags()

        # Validate type name doesn't conflict with TypeDB built-ins
        # Skip validation for:
        # 1. Base classes that won't appear in schema (base=True)
        # 2. The abstract base Entity and Relation classes themselves
        is_base_entity_or_relation = cls.__name__ in ("Entity", "Relation") and cls.__module__ in (
            "type_bridge.models",
            "type_bridge.models.entity",
            "type_bridge.models.relation",
        )
        if not cls._flags.base and not is_base_entity_or_relation:
            type_name = cls._flags.name or format_type_name(cls.__name__, cls._flags.case)
            validate_type_name(type_name, cls.__name__, cls._type_context)

        # Register model in the central registry
        ModelRegistry.register(cls)

    @classmethod
    def __pydantic_init_subclass__(cls, **kwargs: Any) -> None:
        """Called by Pydantic after model class initialization.

        Injects FieldDescriptor instances for class-level query access.
        This runs after Pydantic's setup is complete, so descriptors won't be removed.

        Example:
            Person.age  # Returns FieldRef for query building (class-level access)
            person.age  # Returns attribute value (instance-level access)
        """
        super().__pydantic_init_subclass__(**kwargs)

        from type_bridge.fields import FieldDescriptor

        # Inject FieldDescriptors for class-level query access
        for field_name, attr_info in cls._owned_attrs.items():
            descriptor = FieldDescriptor(field_name=field_name, attr_type=attr_info.typ)
            type.__setattr__(cls, field_name, descriptor)

    @model_validator(mode="wrap")
    @classmethod
    def _preserve_iid(cls, values: Any, handler: Any) -> Self:
        """Preserve _iid during revalidation.

        Pydantic resets private attributes when revalidating instances,
        so we capture _iid before validation and restore it after.

        Uses mode='wrap' to wrap around the entire validation chain,
        allowing us to capture state before and restore after.
        """
        # Capture _iid if values is an existing instance
        preserved_iid = None
        if isinstance(values, cls):
            preserved_iid = getattr(values, "_iid", None)

        # Run the rest of the validation chain (including _wrap_raw_values)
        instance = handler(values)

        # Restore _iid using Pydantic's official private attribute storage
        private = instance.__pydantic_private__
        if preserved_iid is not None and private is not None and private.get("_iid") is None:
            private["_iid"] = preserved_iid

        return instance

    @model_validator(mode="before")
    @classmethod
    def _wrap_raw_values(cls, values: Any) -> dict[str, Any]:
        """Ensure all attribute fields are wrapped in Attribute instances.

        Uses mode='before' to transform input BEFORE Pydantic validation.
        This avoids infinite recursion that would occur if we modified
        the instance after validation (with validate_assignment=True).

        The input can be either a dict or an existing model instance
        (when revalidating). We convert to dict and wrap raw values.
        """
        from type_bridge.fields.base import FieldRef

        # Convert instance to dict if needed
        if isinstance(values, cls):
            # Extract field values from existing instance
            data: dict[str, Any] = {}
            for field_name in cls.model_fields:
                if hasattr(values, field_name):
                    data[field_name] = getattr(values, field_name)
            # Include extra fields if allowed
            if cls.model_config.get("extra") == "allow" and values.__pydantic_extra__:
                data.update(values.__pydantic_extra__)
        elif isinstance(values, dict):
            data = dict(values)  # Copy to avoid mutating input
        else:
            # Let Pydantic handle other types (will likely fail validation)
            return values  # type: ignore[return-value]

        # Wrap raw values in Attribute instances
        all_attrs = cls.get_all_attributes()
        for field_name, attr_info in all_attrs.items():
            flags = attr_info.flags
            attr_class = attr_info.typ

            # Handle fields not in data - check for special default values
            # This happens for list fields with Flag(Card(...)) or inherited fields
            # where the descriptor's FieldRef was captured as the default
            if field_name not in data:
                field_info = cls.model_fields.get(field_name)
                if field_info is not None:
                    # AttributeFlags default: list field with Card()
                    if isinstance(field_info.default, AttributeFlags):
                        if field_info.default.has_explicit_card:
                            data[field_name] = []
                    # FieldRef default: inherited field where descriptor was
                    # accessed during subclass model building - use None
                    elif isinstance(field_info.default, FieldRef):
                        data[field_name] = None
                continue

            value = data[field_name]

            # Handle AttributeFlags passed as value (from Flag() without value)
            if isinstance(value, AttributeFlags):
                if flags.has_explicit_card:
                    data[field_name] = []
                else:
                    raise ValueError(
                        f"Field '{field_name}' received AttributeFlags as value. "
                        f"This usually means the field was not provided a value."
                    )
                continue

            if value is None:
                continue

            # Handle FieldRef (descriptor accessed as default value)
            if isinstance(value, FieldRef):
                data[field_name] = None
                continue

            # Wrap values in Attribute instances
            if isinstance(value, list):
                data[field_name] = [
                    item if isinstance(item, attr_class) else attr_class(item) for item in value
                ]
            elif not isinstance(value, attr_class):
                data[field_name] = attr_class(value)

        return data

    def model_copy(self, *, update: Mapping[str, Any] | None = None, deep: bool = False):
        """Override model_copy to ensure raw values are wrapped in Attribute instances.

        Pydantic's model_copy bypasses validators even with revalidate_instances='always',
        so we pre-wrap values in the update dict before copying.
        Also preserves _iid from original using Pydantic's __pydantic_private__.
        """
        # Preserve _iid before copy
        preserved_iid = getattr(self, "_iid", None)

        # Pre-wrap values in update dict before calling super()
        wrapped_update: dict[str, Any] | None = None
        if update:
            wrapped_update = {}
            owned_attrs = self.__class__.get_owned_attributes()
            for key, value in update.items():
                if key in owned_attrs and value is not None:
                    attr_info = owned_attrs[key]
                    attr_class = attr_info.typ
                    if isinstance(value, list):
                        wrapped_update[key] = [
                            item if isinstance(item, attr_class) else attr_class(item)
                            for item in value
                        ]
                    elif not isinstance(value, attr_class):
                        wrapped_update[key] = attr_class(value)
                    else:
                        wrapped_update[key] = value
                else:
                    wrapped_update[key] = value

        # Call parent model_copy with pre-wrapped update
        copied = super().model_copy(update=wrapped_update, deep=deep)

        # Restore _iid using Pydantic's official private attribute storage
        private = copied.__pydantic_private__
        if preserved_iid is not None and private is not None and private.get("_iid") is None:
            private["_iid"] = preserved_iid

        return copied

    @classmethod
    def get_type_name(cls) -> str:
        """Get the TypeDB type name for this type.

        If name is explicitly set in TypeFlags, it is used as-is.
        Otherwise, the class name is formatted according to the case parameter.
        """
        if cls._flags.name:
            return cls._flags.name
        return format_type_name(cls.__name__, cls._flags.case)

    @classmethod
    def _get_base_type_class(cls) -> type[TypeDBType]:
        """Get the root base class for this type hierarchy.

        Override in subclasses to return Entity or Relation.
        Used by get_supertype() to correctly identify inheritance boundaries.

        Returns:
            The base type class (Entity or Relation)
        """
        return TypeDBType

    @classmethod
    def get_supertype(cls) -> str | None:
        """Get the supertype from Python inheritance, skipping base classes.

        Base classes (with base=True) are Python-only and don't appear in TypeDB schema.
        This method skips them when determining the TypeDB supertype.

        Returns:
            Type name of the parent class, or None if direct subclass
        """
        base_class = cls._get_base_type_class()
        for base in cls.__bases__:
            if base is not base_class and issubclass(base, base_class):
                # Skip base classes - they don't appear in TypeDB schema
                if base.is_base():
                    # Recursively find the first non-base parent
                    return base.get_supertype()
                return base.get_type_name()
        return None

    @classmethod
    def is_abstract(cls) -> bool:
        """Check if this is an abstract type."""
        return cls._flags.abstract

    @classmethod
    def is_base(cls) -> bool:
        """Check if this is a Python base class (not in TypeDB schema)."""
        return cls._flags.base

    @classmethod
    def get_owned_attributes(cls) -> dict[str, ModelAttrInfo]:
        """Get attributes owned directly by this type (not inherited).

        Returns:
            Dictionary mapping field names to ModelAttrInfo (typ + flags)
        """
        return cls._owned_attrs.copy()

    @classmethod
    def get_all_attributes(cls) -> dict[str, ModelAttrInfo]:
        """Get all attributes including inherited ones.

        Traverses the class hierarchy to collect all owned attributes,
        including those from parent Entity/Relation classes.

        Returns:
            Dictionary mapping field names to ModelAttrInfo (typ + flags)
        """
        all_attrs: dict[str, ModelAttrInfo] = {}

        # Traverse MRO in reverse to get parent attributes first
        # Child attributes will override parent attributes with same name
        for base in reversed(cls.__mro__):
            if hasattr(base, "_owned_attrs") and isinstance(base._owned_attrs, dict):
                all_attrs.update(dict(base._owned_attrs))

        return all_attrs

    @classmethod
    def get_polymorphic_attributes(cls) -> dict[str, ModelAttrInfo]:
        """Get all attributes including those from registered subtypes.

        For polymorphic queries where the base class is used but concrete
        subtypes may be returned, this method collects attributes from all
        known subtypes so the query can fetch all possible attributes.

        Returns:
            Dictionary mapping field names to ModelAttrInfo, including
            attributes from all registered subtypes.
        """
        # Start with this class's attributes (including inherited)
        all_attrs = cls.get_all_attributes()

        # Recursively collect attributes from all subclasses
        def collect_subclass_attrs(klass: type[TypeDBType]) -> None:
            for subclass in klass.__subclasses__():
                # Skip if subclass is a base class (abstract, Python-only)
                if hasattr(subclass, "is_base") and subclass.is_base():
                    continue

                # Get subclass attributes and merge (subclass attrs take precedence)
                if hasattr(subclass, "get_all_attributes"):
                    subclass_attrs = subclass.get_all_attributes()
                    for field_name, attr_info in subclass_attrs.items():
                        if field_name not in all_attrs:
                            all_attrs[field_name] = attr_info

                # Recurse into further subclasses
                collect_subclass_attrs(subclass)

        collect_subclass_attrs(cls)
        return all_attrs

    @classmethod
    def _build_owns_lines(cls) -> list[str]:
        """Build TypeQL 'owns' lines for schema definition.

        This is a shared helper used by both Entity and Relation to generate
        the attribute ownership part of their schema definitions.

        Returns:
            List of TypeQL 'owns' lines with proper formatting
        """
        lines = []
        for _field_name, attr_info in cls._owned_attrs.items():
            attr_class = attr_info.typ
            flags = attr_info.flags
            attr_name = attr_class.get_attribute_name()

            ownership = f"    owns {attr_name}"
            annotations = flags.to_typeql_annotations()
            if annotations:
                ownership += " " + " ".join(annotations)
            lines.append(ownership)
        return lines

    @classmethod
    @abstractmethod
    def to_schema_definition(cls) -> str | None:
        """Generate TypeQL schema definition for this type.

        Returns:
            TypeQL schema definition string, or None if this is a base class
        """
        ...

    @abstractmethod
    def get_match_clause_info(self, var_name: str = "$x") -> MatchClauseInfo:
        """Get information to build a TypeQL match clause for this instance.

        Used by TypeDBManager for delete/update operations. Returns IID-based
        matching when available, otherwise falls back to type-specific
        identification (key attributes for entities, role players for relations).

        Args:
            var_name: Variable name to use in the match clause

        Returns:
            MatchClauseInfo with main clause, extra clauses, and variable name

        Raises:
            ValueError: If instance cannot be identified (no IID and no keys/role players)
        """
        ...

    @staticmethod
    def _format_value(value: Any) -> str:
        """Format a Python value for TypeQL.

        Delegates to the shared format_value utility in crud/utils.py.
        """
        return _format_value_impl(value)

    def _build_attribute_statements(self, var: str) -> list[Any]:
        """Build HasStatement AST nodes for all attributes on this instance.

        This is a shared helper used by both Entity.to_ast and Relation.to_ast
        to avoid code duplication in attribute serialization logic.

        Args:
            var: Variable name to use (e.g., "$e" or "$r")

        Returns:
            List of HasStatement AST nodes for non-None attribute values
        """
        from type_bridge.attribute import Attribute
        from type_bridge.models.utils import AstValueType, get_ast_value_type
        from type_bridge.query.ast import HasStatement, LiteralValue

        statements: list[HasStatement] = []

        for field_name, attr_info in self.get_all_attributes().items():
            value = getattr(self, field_name, None)
            if value is None:
                continue

            attr_class = attr_info.typ
            attr_name = attr_class.get_attribute_name()
            ast_type = get_ast_value_type(attr_class)

            # Handle lists (multi-value attributes)
            values = value if isinstance(value, list) else [value]

            for item in values:
                # Unwrap attribute value
                raw_val = item.value if isinstance(item, Attribute) else item

                # Refine type based on actual value if needed
                # (handles cases where base type is string but value is bool/int/float)
                item_type: AstValueType
                if ast_type == "string" and isinstance(raw_val, bool):
                    item_type = "boolean"
                elif ast_type == "string" and isinstance(raw_val, float):
                    item_type = "double"
                elif ast_type == "string" and isinstance(raw_val, int):
                    item_type = "long"
                else:
                    item_type = ast_type

                statements.append(
                    HasStatement(
                        subject_var=var,
                        attr_name=attr_name,
                        value=LiteralValue(value=raw_val, value_type=item_type),
                    )
                )

        return statements

    @abstractmethod
    def to_ast(self, var: str = "$x") -> Any:
        """Generate AST InsertClause for this instance.

        Args:
            var: Variable name to use

        Returns:
            InsertClause containing statements
        """
        ...

    def to_insert_query(self, var: str = "$e") -> str:
        """Generate TypeQL insert query string for this instance.

        This is a convenience method that uses the AST-based generation
        internally and compiles it to a string.

        Args:
            var: Variable name to use (default: "$e")

        Returns:
            TypeQL insert query string
        """
        from type_bridge.query.compiler import QueryCompiler

        insert_clause = self.to_ast(var=var)
        return QueryCompiler().compile(insert_clause)
