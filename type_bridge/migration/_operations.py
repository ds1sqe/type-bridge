"""Private operations retained for frozen migration recovery.

Operations define atomic schema changes that can be applied to a TypeDB database.
Each operation can generate forward TypeQL and optionally rollback TypeQL.

Example:
    from type_bridge.migration import _operations as ops

    operations = [
        ops.AddAttribute(Phone),
        ops.AddOwnership(Person, Phone, optional=True),
        ops.RunTypeQL(
            forward="match $p isa person; insert $p has phone 'unknown';",
            reverse="match $p isa person, has phone 'unknown'; delete $p has phone 'unknown';",
        ),
    ]
"""

from __future__ import annotations

from abc import ABC, abstractmethod
from collections.abc import Callable, Sequence
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from type_bridge.attribute.base import _QueryAttribute as Attribute
    from type_bridge.migration._ref import AttributeRef, EntityRef, RelationRef
    from type_bridge.models.entity import _QueryEntity as Entity
    from type_bridge.models.relation import _QueryRelation as Relation

    TypeLike = type[Entity | Relation] | EntityRef | RelationRef
    AttributeLike = type[Attribute] | AttributeRef


def _type_name(value: object) -> str:
    get_type_name = getattr(value, "get_type_name", None)
    if get_type_name is None:
        raise TypeError(f"{value!r} is not a TypeBridge type or migration type ref")
    return str(get_type_name())


class Operation(ABC):
    """Base class for migration operations.

    Operations must implement:
    - to_typeql(): Generate forward migration TypeQL
    - to_rollback_typeql(): Generate rollback TypeQL (or None if irreversible)
    """

    @abstractmethod
    def to_typeql(self) -> str:
        """Generate TypeQL for forward migration.

        Returns:
            TypeQL string to execute
        """
        pass

    @abstractmethod
    def to_rollback_typeql(self) -> str | None:
        """Generate TypeQL for rollback.

        Returns:
            TypeQL string to execute, or None if operation is irreversible
        """
        pass

    @property
    def reversible(self) -> bool:
        """Whether this operation can be rolled back.

        Returns:
            True if rollback TypeQL is available
        """
        return self.to_rollback_typeql() is not None

    def to_typeql_steps(self) -> list[str]:
        """Forward TypeQL, one query per step.

        TypeDB executes one define/redefine/undefine block per query, so
        operations that mix verbs (annotation changes) override this to
        return several steps. The default wraps ``to_typeql()``.
        """
        typeql = self.to_typeql()
        return [typeql] if typeql.strip() else []

    def to_rollback_typeql_steps(self) -> list[str] | None:
        """Rollback TypeQL, one query per step, or None if irreversible."""
        typeql = self.to_rollback_typeql()
        if typeql is None:
            return None
        return [typeql] if typeql.strip() else []


def _rendered_operation(operation: Operation) -> dict[str, Any]:
    from type_bridge import _rust_runtime
    from type_bridge.migration._lower import _model_class, _operation_spec

    # Copy synthesis must not ask _operation_spec for its own rendered text.
    if isinstance(operation, CopyAttribute):
        spec = {
            "kind": "copy_attribute",
            "owner": _type_name(operation.owner),
            "source": operation.source,
            "dest": operation.dest,
            "filter": operation.filter,
            "forward": None,
            "reverse": None,
        }
    elif isinstance(operation, AddEntity | AddRelation):
        from type_bridge.typeql._schema import _local_descriptor

        if isinstance(operation, AddEntity):
            spec = {
                "kind": "add_entity",
                "entity": _local_descriptor(_model_class(operation.entity)),
            }
        else:
            spec = {
                "kind": "add_relation",
                "relation": _local_descriptor(_model_class(operation.relation)),
            }
    else:
        spec = _operation_spec(operation)
    return _rust_runtime.rust_core()._render_migration_operation(spec)


@dataclass
class ModifyTypeAnnotations(Operation):
    """Modify @doc/@meta annotations on an entity, relation, or attribute type.

    TypeDB 3.12+. Annotation-only schema changes are metadata-safe: they never
    touch instance data.

    Example:
        ops.ModifyTypeAnnotations(
            Person,
            old_doc=None,
            new_doc="A person known to the system.",
        )
    """

    subject: TypeLike | AttributeLike
    old_doc: str | None = None
    new_doc: str | None = None
    old_meta: dict[str, str] = field(default_factory=dict)
    new_meta: dict[str, str] = field(default_factory=dict)

    def _steps(self) -> tuple[list[str], list[str]]:
        rendered = _rendered_operation(self)
        return rendered["forward_steps"], rendered["reverse_steps"]

    def to_typeql(self) -> str:
        return _rendered_operation(self)["forward"]

    def to_rollback_typeql(self) -> str | None:
        return _rendered_operation(self)["reverse"]

    def to_typeql_steps(self) -> list[str]:
        return _rendered_operation(self)["forward_steps"]

    def to_rollback_typeql_steps(self) -> list[str] | None:
        return _rendered_operation(self)["reverse_steps"]


@dataclass
class ModifyRoleAnnotations(Operation):
    """Modify @doc/@meta annotations on a relation role (TypeDB 3.12+).

    Example:
        ops.ModifyRoleAnnotations(
            Employment, "employee",
            new_doc="The employed party.",
        )
    """

    relation: TypeLike
    role_name: str
    old_doc: str | None = None
    new_doc: str | None = None
    old_meta: dict[str, str] = field(default_factory=dict)
    new_meta: dict[str, str] = field(default_factory=dict)

    def _steps(self) -> tuple[list[str], list[str]]:
        rendered = _rendered_operation(self)
        return rendered["forward_steps"], rendered["reverse_steps"]

    def to_typeql(self) -> str:
        return _rendered_operation(self)["forward"]

    def to_rollback_typeql(self) -> str | None:
        return _rendered_operation(self)["reverse"]

    def to_typeql_steps(self) -> list[str]:
        return _rendered_operation(self)["forward_steps"]

    def to_rollback_typeql_steps(self) -> list[str] | None:
        return _rendered_operation(self)["reverse_steps"]


# --- Attribute Operations ---


@dataclass
class AddAttribute(Operation):
    """Add a new attribute type.

    Example:
        ops.AddAttribute(Phone)  # Creates: define attribute phone, value string;
    """

    attribute: AttributeLike

    def to_typeql(self) -> str:
        return _rendered_operation(self)["forward"]

    def to_rollback_typeql(self) -> str | None:
        return _rendered_operation(self)["reverse"]


@dataclass
class RemoveAttribute(Operation):
    """Remove an attribute type.

    WARNING: This is a BREAKING change. Ensure all attribute instances
    and ownerships are removed first.
    """

    attribute: AttributeLike

    def to_typeql(self) -> str:
        return _rendered_operation(self)["forward"]

    def to_rollback_typeql(self) -> str | None:
        # Cannot restore deleted data
        return _rendered_operation(self)["reverse"]


# --- Entity Operations ---


@dataclass
class AddEntity(Operation):
    """Add a new entity type.

    Example:
        ops.AddEntity(Person)
    """

    entity: TypeLike

    def to_typeql(self) -> str:
        return _rendered_operation(self)["forward"]

    def to_rollback_typeql(self) -> str | None:
        return _rendered_operation(self)["reverse"]


@dataclass
class RemoveEntity(Operation):
    """Remove an entity type.

    WARNING: This is a BREAKING change. Ensure all entity instances
    are deleted first.
    """

    entity: TypeLike

    def to_typeql(self) -> str:
        return _rendered_operation(self)["forward"]

    def to_rollback_typeql(self) -> str | None:
        # Cannot restore deleted data
        return _rendered_operation(self)["reverse"]


# --- Ownership Operations ---


@dataclass
class AddOwnership(Operation):
    """Add attribute ownership to an entity or relation.

    Example:
        ops.AddOwnership(Person, Phone, optional=True)
        # Creates: define person owns phone @card(0..1);

        ops.AddOwnership(Person, Email, key=True)
        # Creates: define person owns email @key;
    """

    owner: TypeLike
    attribute: AttributeLike
    optional: bool = False
    key: bool = False
    unique: bool = False
    card_min: int | None = None
    card_max: int | None = None

    def to_typeql(self) -> str:
        return _rendered_operation(self)["forward"]

    def to_rollback_typeql(self) -> str | None:
        return _rendered_operation(self)["reverse"]


@dataclass
class RemoveOwnership(Operation):
    """Remove attribute ownership from an entity or relation.

    WARNING: This may orphan attribute data. Ensure attribute values
    are removed from instances first.
    """

    owner: TypeLike
    attribute: AttributeLike

    def to_typeql(self) -> str:
        return _rendered_operation(self)["forward"]

    def to_rollback_typeql(self) -> str | None:
        # Would need to know original flags (key, unique, cardinality)
        return _rendered_operation(self)["reverse"]


@dataclass
class ModifyOwnership(Operation):
    """Modify ownership annotations (cardinality, key, unique).

    TypeDB can only ``redefine`` parameterized annotations (``@card``);
    parameterless ones (``@key``, ``@unique``, ``@distinct``) must be
    added with ``define`` and removed with ``undefine``. The operation
    decomposes the transition into per-annotation schema steps (one
    query each) via ``to_typeql_steps()``; both the direct-execution
    path and the planner-backed path run those steps in order.

    Example:
        ops.ModifyOwnership(
            Person, Phone,
            old_annotations="@card(0..1)",
            new_annotations="@card(1..1)"
        )
    """

    owner: TypeLike
    attribute: AttributeLike
    old_annotations: str
    new_annotations: str

    def _steps(self) -> tuple[list[str], list[str]]:
        rendered = _rendered_operation(self)
        return rendered["forward_steps"], rendered["reverse_steps"]

    def to_typeql(self) -> str:
        return _rendered_operation(self)["forward"]

    def to_rollback_typeql(self) -> str | None:
        return _rendered_operation(self)["reverse"]

    def to_typeql_steps(self) -> list[str]:
        return _rendered_operation(self)["forward_steps"]

    def to_rollback_typeql_steps(self) -> list[str] | None:
        return _rendered_operation(self)["reverse_steps"]


# --- Relation Operations ---


@dataclass
class AddRelation(Operation):
    """Add a new relation type with its roles.

    Example:
        ops.AddRelation(Employment)
    """

    relation: TypeLike

    def to_typeql(self) -> str:
        return _rendered_operation(self)["forward"]

    def to_rollback_typeql(self) -> str | None:
        return _rendered_operation(self)["reverse"]


@dataclass
class RemoveRelation(Operation):
    """Remove a relation type.

    WARNING: This is a BREAKING change. Ensure all relation instances
    are deleted first.
    """

    relation: TypeLike

    def to_typeql(self) -> str:
        return _rendered_operation(self)["forward"]

    def to_rollback_typeql(self) -> str | None:
        # Cannot restore deleted data
        return _rendered_operation(self)["reverse"]


# --- Role Operations ---


@dataclass
class AddRole(Operation):
    """Add a new role to an existing relation.

    Example:
        ops.AddRole(Employment, "manager", ["person"])
    """

    relation: TypeLike
    role_name: str
    player_types: list[str] = field(default_factory=list)

    def to_typeql(self) -> str:
        return _rendered_operation(self)["forward"]

    def to_rollback_typeql(self) -> str | None:
        return _rendered_operation(self)["reverse"]


@dataclass
class RemoveRole(Operation):
    """Remove a role from a relation.

    WARNING: This is a BREAKING change. Ensure no relation instances
    have role players for this role.
    """

    relation: TypeLike
    role_name: str

    def to_typeql(self) -> str:
        return _rendered_operation(self)["forward"]

    def to_rollback_typeql(self) -> str | None:
        # Would need to know player types
        return _rendered_operation(self)["reverse"]


@dataclass
class AddRolePlayer(Operation):
    """Add a player type to an existing role.

    Example:
        ops.AddRolePlayer(Employment, "employee", "contractor")
        # Allows Contractor entities to play the employee role
    """

    relation: TypeLike
    role_name: str
    player_type: str

    def to_typeql(self) -> str:
        return _rendered_operation(self)["forward"]

    def to_rollback_typeql(self) -> str | None:
        return _rendered_operation(self)["reverse"]


@dataclass
class RemoveRolePlayer(Operation):
    """Remove a player type from a role.

    WARNING: This is a BREAKING change. Ensure no relation instances
    have this player type in this role.
    """

    relation: TypeLike
    role_name: str
    player_type: str

    def to_typeql(self) -> str:
        return _rendered_operation(self)["forward"]

    def to_rollback_typeql(self) -> str | None:
        return _rendered_operation(self)["reverse"]


# --- Custom TypeQL Operations ---


@dataclass
class RunTypeQL(Operation):
    """Execute arbitrary TypeQL for complex migrations.

    Use this for:
    - Data migrations (updating existing data)
    - Complex schema changes not covered by other operations
    - Renaming attributes (requires data migration)

    Example:
        ops.RunTypeQL(
            forward=\"\"\"
                match $p isa person;
                not { $p has phone $ph; };
                insert $p has phone "unknown";
            \"\"\",
            reverse=\"\"\"
                match $p isa person, has phone "unknown";
                delete $p has phone "unknown";
            \"\"\"
        )
    """

    forward: str
    reverse: str | None = None

    def to_typeql(self) -> str:
        return self.forward.strip()

    def to_rollback_typeql(self) -> str | None:
        return self.reverse.strip() if self.reverse else None


PythonMigrationCallable = Callable[[Any], None]


@dataclass
class RunPython(Operation):
    """Run Python ORM code during migration execution.

    ``RunPython`` is for migrations that need the normal TypeBridge ORM surface
    rather than portable TypeQL, for example loading JSON/TOML data, creating
    many entities/relations, or querying existing data before writing derived
    values.  The callable receives the migration executor's database connection,
    so existing code such as ``User.manager(db).filter(...).execute()`` works.

    Example:
        def forwards(db):
            users = User.manager(db).filter(name__startswith="A").execute()
            ...

        operations = [ops.RunPython(forwards)]
    """

    code: PythonMigrationCallable
    reverse: PythonMigrationCallable | None = None
    description: str | None = None
    resources: Sequence[str] = ()
    import_checks: Sequence[str] = ()

    def to_typeql(self) -> str:
        name = self.description or self._callable_name(self.code)
        return self._preview("RunPython", name)

    def to_rollback_typeql(self) -> str | None:
        if self.reverse is None:
            return None
        name = self.description or self._callable_name(self.reverse)
        return self._preview("RunPython reverse", name)

    @property
    def reversible(self) -> bool:
        return self.reverse is not None

    def run(self, db: Any) -> None:
        self.code(db)

    def rollback(self, db: Any) -> None:
        if self.reverse is None:
            raise RuntimeError(
                f"RunPython operation {self._callable_name(self.code)} is not reversible"
            )
        self.reverse(db)

    @staticmethod
    def _callable_name(func: PythonMigrationCallable) -> str:
        module = getattr(func, "__module__", "")
        name = getattr(func, "__qualname__", getattr(func, "__name__", repr(func)))
        return f"{module}:{name}" if module else name

    def _preview(self, prefix: str, name: str) -> str:
        lines = [f"# {prefix}: {name}"]
        if self.resources:
            lines.append(f"# resources: {', '.join(self.resources)}")
        if self.import_checks:
            lines.append(f"# import checks: {', '.join(self.import_checks)}")
        return "\n".join(lines)


@dataclass
class RenameAttribute(Operation):
    """Rename an attribute type — placeholder without an executable lowering.

    A real rename is a staged multi-step change (define new attribute,
    plain ownerships, data backfill, annotation tightening, old-value
    cleanup, removal) that needs the full owner list from a schema. This
    single operation cannot carry that, so it has no executable TypeQL and
    the migration planner refuses to lower it.

    Use ``author_migration(..., attribute_renames=[(old, new)])`` to author
    the staged expansion from two schemas, or spell out the primitive
    operations (``AddAttribute``, ``AddOwnership``, ``CopyAttribute``,
    ``RunTypeQL``, ``RemoveOwnership``, ``RemoveAttribute``) by hand.
    """

    old_name: str
    new_name: str
    value_type: str

    def to_typeql(self) -> str:
        # Historically this emitted the define plus comment-guide lines;
        # executing that silently created the new attribute and skipped the
        # data migration entirely. Failing loudly is the only safe lowering.
        raise NotImplementedError(
            f"RenameAttribute({self.old_name!r} -> {self.new_name!r}) has no executable "
            "TypeQL lowering; author the rename via "
            "author_migration(..., attribute_renames=[(old, new)]) or use the primitive "
            "operations explicitly"
        )

    def to_rollback_typeql(self) -> str | None:
        # Rename operations are not easily reversible
        return None


# --- Backfill Operations ---


@dataclass
class CopyAttribute(Operation):
    """Copy an attribute value from source to destination on all instances of the owner type.

    This is a DML (data manipulation) backfill operation that copies attribute values from
    one attribute to another for every instance of the owning type. The forward operation
    uses an insert-if-absent pattern (idempotent — safe to re-run). The reverse deletes all
    destination attribute values added by this operation.

    No transform function is supported in v1. Use RunTypeQL for value transforms.

    Example:
        ops.CopyAttribute(
            owner=Person,
            source="legacy-name",
            dest="display-name",
        )
        # Backfills: match $x isa person, has legacy-name $v;
        #            not { $x has display-name $d; };
        #            insert $x has display-name == $v;

    Note: ``dest`` must already be owned by ``owner`` via a prior schema op.
    """

    owner: TypeLike
    source: str
    dest: str
    filter: str | None = None

    def to_typeql(self) -> str:
        """Generate insert-if-absent backfill TypeQL.

        Emits a match+insert that copies ``source`` values to ``dest`` for every
        owner instance that does not already have the destination attribute.
        """
        return _rendered_operation(self)["forward"]

    def to_rollback_typeql(self) -> str | None:
        """Generate the inverse delete that removes all dest values added by this op."""
        return _rendered_operation(self)["reverse"]
