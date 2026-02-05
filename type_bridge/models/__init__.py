"""TypeDB model classes - Entity, Relation, and Role."""

from type_bridge.models.base import TypeDBType
from type_bridge.models.entity import Entity
from type_bridge.models.relation import Relation
from type_bridge.models.role import Role
from type_bridge.models.utils import (
    FieldInfo,
    MatchClauseInfo,
    ModelAttrInfo,
    WriteQueryInfo,
)

__all__ = [
    "Entity",
    "FieldInfo",
    "MatchClauseInfo",
    "ModelAttrInfo",
    "Relation",
    "Role",
    "TypeDBType",
    "WriteQueryInfo",
]
