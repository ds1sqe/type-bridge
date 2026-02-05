"""Field reference system for type-safe query building.

This module provides field descriptors and references that enable type-safe
query expressions like Person.age.gt(Age(30)) and Employment.employee.age.gt(Age(30)).
"""

from type_bridge.fields.base import (
    FieldDescriptor,
    FieldRef,
    NumericFieldRef,
    StringFieldRef,
)
from type_bridge.fields.role import (
    RolePlayerFieldRef,
    RolePlayerNumericFieldRef,
    RolePlayerStringFieldRef,
    RoleRef,
)

__all__ = [
    "FieldDescriptor",
    "FieldRef",
    "NumericFieldRef",
    "RolePlayerFieldRef",
    "RolePlayerNumericFieldRef",
    "RolePlayerStringFieldRef",
    "RoleRef",
    "StringFieldRef",
]
