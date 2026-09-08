"""Test-only access to private V1/query execution representations.

These aliases keep internal engine tests without restoring any application
authoring import.  Generated-package acceptance remains the only application
operation evidence for the #189 cutover.
"""

from __future__ import annotations

import type_bridge.attribute.flags as _flags
from type_bridge.attribute.base import _QueryAttribute as Attribute
from type_bridge.attribute.boolean import _QueryBoolean as Boolean
from type_bridge.attribute.date import _QueryDate as Date
from type_bridge.attribute.datetime import _QueryDateTime as DateTime
from type_bridge.attribute.datetimetz import _QueryDateTimeTZ as DateTimeTZ
from type_bridge.attribute.decimal import _QueryDecimal as Decimal
from type_bridge.attribute.double import _QueryDouble as Double
from type_bridge.attribute.duration import _QueryDuration as Duration
from type_bridge.attribute.flags import (
    _QueryAttributeFlags as AttributeFlags,
)
from type_bridge.attribute.flags import _QueryCard as Card
from type_bridge.attribute.flags import _QueryDistinct as Distinct
from type_bridge.attribute.flags import _QueryDoc as Doc
from type_bridge.attribute.flags import _QueryKey as Key
from type_bridge.attribute.flags import _QueryMeta as Meta
from type_bridge.attribute.flags import _QueryOrdered as Ordered
from type_bridge.attribute.flags import _QueryTypeFlags as TypeFlags
from type_bridge.attribute.flags import _QueryTypeNameCase as TypeNameCase
from type_bridge.attribute.flags import _QueryUnique as Unique
from type_bridge.attribute.integer import _QueryInteger as Integer
from type_bridge.attribute.string import _QueryString as String
from type_bridge.fields.base import _QueryFieldDescriptor as FieldDescriptor
from type_bridge.fields.base import _QueryFieldRef as FieldRef
from type_bridge.fields.base import _QueryNumericFieldRef as NumericFieldRef
from type_bridge.fields.base import _QueryStringFieldRef as StringFieldRef
from type_bridge.fields.role import (
    _QueryRolePlayerNumericFieldRef as RolePlayerNumericFieldRef,
)
from type_bridge.fields.role import (
    _QueryRolePlayerStringFieldRef as RolePlayerStringFieldRef,
)
from type_bridge.fields.role import _QueryRoleRef as RoleRef
from type_bridge.models.base import _QueryTypeDBType as TypeDBType
from type_bridge.models.entity import _QueryEntity as Entity
from type_bridge.models.registry import _QueryModelRegistry as ModelRegistry
from type_bridge.models.relation import _QueryRelation as Relation
from type_bridge.models.role import _QueryRole as Role
from type_bridge.session import Database

Flag = _flags._query_flag

__all__ = [
    "Attribute",
    "AttributeFlags",
    "Boolean",
    "Card",
    "Database",
    "Date",
    "DateTime",
    "DateTimeTZ",
    "Decimal",
    "Distinct",
    "Doc",
    "Double",
    "Duration",
    "Entity",
    "Flag",
    "FieldDescriptor",
    "FieldRef",
    "Integer",
    "Key",
    "ModelRegistry",
    "Meta",
    "NumericFieldRef",
    "Ordered",
    "Relation",
    "Role",
    "RolePlayerNumericFieldRef",
    "RolePlayerStringFieldRef",
    "RoleRef",
    "String",
    "StringFieldRef",
    "TypeDBType",
    "TypeFlags",
    "TypeNameCase",
    "Unique",
]
