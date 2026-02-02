"""CRUD operation exceptions for TypeBridge.

This module provides exception classes for CRUD operations to provide
clear and consistent error handling across entity and relation operations.
"""


class NotFoundError(LookupError):
    """Base class for not-found errors in CRUD operations.

    Raised when an entity or relation that was expected to exist
    cannot be found in the database.
    """

    pass


class EntityNotFoundError(NotFoundError):
    """Raised when an entity does not exist in the database.

    This exception is raised during delete or update operations
    when the target entity cannot be found using its @key attributes
    or matched attributes.

    Example:
        try:
            manager.delete(nonexistent_entity)
        except EntityNotFoundError:
            print("Entity was already deleted or never existed")
    """

    pass


class RelationNotFoundError(NotFoundError):
    """Raised when a relation does not exist in the database.

    This exception is raised during delete or update operations
    when the target relation cannot be found using its role players'
    @key attributes.

    Example:
        try:
            manager.delete(nonexistent_relation)
        except RelationNotFoundError:
            print("Relation was already deleted or never existed")
    """

    pass


class NotUniqueError(ValueError):
    """Raised when an operation requires exactly one match but finds multiple.

    This exception is raised when attempting to delete an entity without
    @key attributes and multiple matching records are found. Use
    filter().delete() for bulk deletion instead.

    Example:
        try:
            manager.delete(keyless_entity)
        except NotUniqueError:
            print("Multiple entities matched - use filter().delete() for bulk deletion")
    """

    pass


class HydrationError(RuntimeError):
    """Raised when hydrating database results into model instances fails.

    This exception is raised when the ORM cannot convert raw database
    results into Python model instances. This typically indicates:
    - Schema mismatch between database and Python models
    - Corrupt or unexpected data in the database
    - Missing or invalid type information

    Attributes:
        model_type: Name of the model class being hydrated
        raw_data: The raw database result that failed to hydrate
        cause: The underlying exception that caused the failure

    Example:
        try:
            results = manager.all()
        except HydrationError as e:
            print(f"Failed to hydrate {e.model_type}: {e.cause}")
            print(f"Raw data: {e.raw_data}")
    """

    def __init__(
        self,
        model_type: str,
        raw_data: object,
        cause: Exception,
    ):
        self.model_type = model_type
        self.raw_data = raw_data
        self.cause = cause

        message = (
            f"Failed to hydrate {model_type} from database result: {cause}. "
            f"This may indicate a schema mismatch or corrupt data. "
            f"Raw data: {raw_data!r}"
        )
        super().__init__(message)


class KeyAttributeError(ValueError):
    """Raised when @key attribute validation fails during update/delete.

    This exception is raised when:
    - A @key attribute has a None value
    - No @key attributes are defined on the entity

    Attributes:
        entity_type: Name of the entity class
        operation: The operation that failed ("update" or "delete")
        field_name: The @key field that was None (if applicable)
        all_fields: List of all defined fields (when no @key exists)

    Example:
        try:
            manager.update(entity_with_none_key)
        except KeyAttributeError as e:
            print(f"Key validation failed: {e}")
            print(f"Entity type: {e.entity_type}")
            print(f"Operation: {e.operation}")
    """

    def __init__(
        self,
        entity_type: str,
        operation: str,
        field_name: str | None = None,
        all_fields: list[str] | None = None,
    ):
        self.entity_type = entity_type
        self.operation = operation
        self.field_name = field_name
        self.all_fields = all_fields

        if field_name is not None:
            # Key attribute is None
            message = (
                f"Cannot {operation} {entity_type}: "
                f"key attribute '{field_name}' is None. "
                f"Ensure the entity has a valid '{field_name}' value "
                f"before calling {operation}()."
            )
        else:
            # No @key attributes defined
            message = (
                f"Cannot {operation} {entity_type}: no @key attributes found. "
                f"The {operation}() method requires at least one @key attribute "
                f"to identify the entity. "
                f"Defined attributes: {all_fields} (none marked as @key). "
                f"Hint: Add Flag(Key) to an attribute, e.g., `id: Id = Flag(Key)`"
            )

        super().__init__(message)
