"""Migration operations for TypeDB schema changes.

Operations define atomic schema changes that can be applied to a TypeDB database.
Each operation can generate forward TypeQL and optionally rollback TypeQL.

Example:
    from type_bridge.migration import operations as ops

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
    from type_bridge.attribute.base import Attribute
    from type_bridge.migration.ref import AttributeRef, EntityRef, RelationRef
    from type_bridge.models import Entity, Relation

    TypeLike = type[Entity | Relation] | EntityRef | RelationRef
    AttributeLike = type[Attribute] | AttributeRef


def _type_name(value: object) -> str:
    get_type_name = getattr(value, "get_type_name", None)
    if get_type_name is None:
        raise TypeError(f"{value!r} is not a TypeBridge type or migration type ref")
    return str(get_type_name())


def _attribute_name(value: object) -> str:
    get_attribute_name = getattr(value, "get_attribute_name", None)
    if get_attribute_name is None:
        raise TypeError(f"{value!r} is not a TypeBridge attribute or migration attribute ref")
    return str(get_attribute_name())


def _schema_definition(value: object, operation: str) -> str:
    to_schema_definition = getattr(value, "to_schema_definition", None)
    if to_schema_definition is None:
        raise TypeError(
            f"{operation} requires a full model class when no sidecar execution spec is present"
        )
    return str(to_schema_definition())


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


# --- Attribute Operations ---


@dataclass
class AddAttribute(Operation):
    """Add a new attribute type.

    Example:
        ops.AddAttribute(Phone)  # Creates: define attribute phone, value string;
    """

    attribute: AttributeLike

    def to_typeql(self) -> str:
        return f"define\n{_schema_definition(self.attribute, 'AddAttribute')}"

    def to_rollback_typeql(self) -> str | None:
        name = _attribute_name(self.attribute)
        return f"undefine\n{name};"


@dataclass
class RemoveAttribute(Operation):
    """Remove an attribute type.

    WARNING: This is a BREAKING change. Ensure all attribute instances
    and ownerships are removed first.
    """

    attribute: AttributeLike

    def to_typeql(self) -> str:
        name = _attribute_name(self.attribute)
        return f"undefine\n{name};"

    def to_rollback_typeql(self) -> str | None:
        # Cannot restore deleted data
        return None


# --- Entity Operations ---


@dataclass
class AddEntity(Operation):
    """Add a new entity type.

    Example:
        ops.AddEntity(Person)
    """

    entity: TypeLike

    def to_typeql(self) -> str:
        schema = _schema_definition(self.entity, "AddEntity")
        if schema:
            return f"define\n{schema}"
        return ""

    def to_rollback_typeql(self) -> str | None:
        name = _type_name(self.entity)
        return f"undefine\n{name};"


@dataclass
class RemoveEntity(Operation):
    """Remove an entity type.

    WARNING: This is a BREAKING change. Ensure all entity instances
    are deleted first.
    """

    entity: TypeLike

    def to_typeql(self) -> str:
        name = _type_name(self.entity)
        return f"undefine\n{name};"

    def to_rollback_typeql(self) -> str | None:
        # Cannot restore deleted data
        return None


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
        from type_bridge.typeql.annotations import format_card_annotation

        owner_name = _type_name(self.owner)
        attr_name = _attribute_name(self.attribute)

        annotations = []
        if self.key:
            annotations.append("@key")
        elif self.unique:
            annotations.append("@unique")
        elif self.card_min is not None or self.card_max is not None:
            card_annotation = format_card_annotation(self.card_min, self.card_max)
            if card_annotation:
                annotations.append(card_annotation)
        elif self.optional:
            annotations.append("@card(0..1)")

        ann_str = " " + " ".join(annotations) if annotations else ""
        return f"define\n{owner_name} owns {attr_name}{ann_str};"

    def to_rollback_typeql(self) -> str | None:
        owner_name = _type_name(self.owner)
        attr_name = _attribute_name(self.attribute)
        return f"undefine\nowns {attr_name} from {owner_name};"


@dataclass
class RemoveOwnership(Operation):
    """Remove attribute ownership from an entity or relation.

    WARNING: This may orphan attribute data. Ensure attribute values
    are removed from instances first.
    """

    owner: TypeLike
    attribute: AttributeLike

    def to_typeql(self) -> str:
        owner_name = _type_name(self.owner)
        attr_name = _attribute_name(self.attribute)
        return f"undefine\nowns {attr_name} from {owner_name};"

    def to_rollback_typeql(self) -> str | None:
        # Would need to know original flags (key, unique, cardinality)
        return None


@dataclass
class ModifyOwnership(Operation):
    """Modify ownership annotations (cardinality, key, unique).

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

    def to_typeql(self) -> str:
        owner_name = _type_name(self.owner)
        attr_name = _attribute_name(self.attribute)
        # TypeDB 3.x uses redefine for modifications
        return f"redefine\n{owner_name} owns {attr_name} {self.new_annotations};"

    def to_rollback_typeql(self) -> str | None:
        owner_name = _type_name(self.owner)
        attr_name = _attribute_name(self.attribute)
        return f"redefine\n{owner_name} owns {attr_name} {self.old_annotations};"


# --- Relation Operations ---


@dataclass
class AddRelation(Operation):
    """Add a new relation type with its roles.

    Example:
        ops.AddRelation(Employment)
    """

    relation: TypeLike

    def to_typeql(self) -> str:
        lines = []
        schema = _schema_definition(self.relation, "AddRelation")
        if schema:
            lines.append(f"define\n{schema}")

            # Add role player definitions
            for role_name, role in getattr(self.relation, "_roles", {}).items():
                for player_type in role.player_types:
                    lines.append(f"{player_type} plays {_type_name(self.relation)}:{role_name};")

        return "\n".join(lines)

    def to_rollback_typeql(self) -> str | None:
        name = _type_name(self.relation)
        return f"undefine\n{name};"


@dataclass
class RemoveRelation(Operation):
    """Remove a relation type.

    WARNING: This is a BREAKING change. Ensure all relation instances
    are deleted first.
    """

    relation: TypeLike

    def to_typeql(self) -> str:
        name = _type_name(self.relation)
        return f"undefine\n{name};"

    def to_rollback_typeql(self) -> str | None:
        # Cannot restore deleted data
        return None


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
        rel_name = _type_name(self.relation)
        lines = [f"define\n{rel_name} relates {self.role_name};"]
        for player in self.player_types:
            lines.append(f"{player} plays {rel_name}:{self.role_name};")
        return "\n".join(lines)

    def to_rollback_typeql(self) -> str | None:
        rel_name = _type_name(self.relation)
        return f"undefine\nrelates {self.role_name} from {rel_name};"


@dataclass
class RemoveRole(Operation):
    """Remove a role from a relation.

    WARNING: This is a BREAKING change. Ensure no relation instances
    have role players for this role.
    """

    relation: TypeLike
    role_name: str

    def to_typeql(self) -> str:
        rel_name = _type_name(self.relation)
        return f"undefine\nrelates {self.role_name} from {rel_name};"

    def to_rollback_typeql(self) -> str | None:
        # Would need to know player types
        return None


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
        rel_name = _type_name(self.relation)
        return f"define\n{self.player_type} plays {rel_name}:{self.role_name};"

    def to_rollback_typeql(self) -> str | None:
        rel_name = _type_name(self.relation)
        return f"undefine\nplays {rel_name}:{self.role_name} from {self.player_type};"


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
        rel_name = _type_name(self.relation)
        return f"undefine\nplays {rel_name}:{self.role_name} from {self.player_type};"

    def to_rollback_typeql(self) -> str | None:
        rel_name = _type_name(self.relation)
        return f"define\n{self.player_type} plays {rel_name}:{self.role_name};"


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
    """Rename an attribute type.

    WARNING: This is a complex operation that requires both schema
    and data migration. Consider using RunTypeQL for full control.

    This operation:
    1. Creates new attribute type
    2. Migrates data from old to new
    3. Removes old attribute type

    Note: Rollback is not supported for this operation.
    """

    old_name: str
    new_name: str
    value_type: str

    def to_typeql(self) -> str:
        # This is a complex multi-step operation
        # For simplicity, we generate the TypeQL that would need to be executed
        # In practice, the executor would need to handle this specially
        lines = [
            "# Step 1: Create new attribute",
            "define",
            f"attribute {self.new_name}, value {self.value_type};",
            "",
            "# Step 2: Migrate ownership (manual step required)",
            f"# For each type that owns {self.old_name}, add: <type> owns {self.new_name};",
            "",
            "# Step 3: Migrate data (manual step required)",
            f"# match $x has {self.old_name} $v; insert $x has {self.new_name} $v;",
            "",
            "# Step 4: Remove old attribute (after data migration)",
            f"# undefine attribute {self.old_name};",
        ]
        return "\n".join(lines)

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
        owner_name = _type_name(self.owner)
        filter_line = f"\n  {self.filter};" if self.filter else ""
        # `has <dest> == $v` assigns the *value* of the matched source attribute
        # to a new destination attribute. Writing `has <dest> $v` instead would
        # fail TypeDB type inference: `$v` is typed as the source attribute and
        # cannot simultaneously be a destination-attribute instance.
        return (
            f"match\n"
            f"  $x isa {owner_name}, has {self.source} $v;\n"
            f"  not {{ $x has {self.dest} $d; }};{filter_line}\n"
            f"insert\n"
            f"  $x has {self.dest} == $v;"
        )

    def to_rollback_typeql(self) -> str | None:
        """Generate the inverse delete that removes all dest values added by this op."""
        owner_name = _type_name(self.owner)
        return f"match $x isa {owner_name}, has {self.dest} $v;\ndelete $v of $x;"
