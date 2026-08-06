"""Private object shape used only while reading frozen Python migrations."""

from __future__ import annotations

from dataclasses import dataclass
from typing import TYPE_CHECKING, ClassVar

if TYPE_CHECKING:
    from type_bridge.migration._operations import Operation
    from type_bridge.models.entity import _QueryEntity as Entity
    from type_bridge.models.relation import _QueryRelation as Relation


@dataclass
class _ArchivedMigrationDependency:
    """Dependency identity decoded from a frozen migration source."""

    app_label: str
    migration_name: str

    def __str__(self) -> str:
        return f"{self.app_label}.{self.migration_name}"


class _ArchivedMigration:
    """Read-only in-memory representation of one frozen migration source."""

    name: str = ""
    app_label: str = ""
    dependencies: ClassVar[list[tuple[str, str]]] = []
    models: ClassVar[list[type[Entity | Relation]]] = []
    operations: ClassVar[list[Operation]] = []
    reversible: ClassVar[bool] = True

    def get_dependencies(self) -> list[_ArchivedMigrationDependency]:
        return [_ArchivedMigrationDependency(app, name) for app, name in self.dependencies]

    def describe(self) -> str:
        if self.models:
            model_names = [model.__name__ for model in self.models]
            return f"Initial migration with models: {', '.join(model_names)}"
        if self.operations:
            return f"Migration with {len(self.operations)} operation(s)"
        return "Empty migration"

    def __repr__(self) -> str:
        return f"<ArchivedMigration {self.app_label}.{self.name}>"


__all__: list[str] = []
