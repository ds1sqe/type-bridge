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
from dataclasses import dataclass, field
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from type_bridge.attribute.base import Attribute
    from type_bridge.models import Entity, Relation


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

    attribute: type[Attribute]

    def to_typeql(self) -> str:
        return f"define\n{self.attribute.to_schema_definition()}"

    def to_rollback_typeql(self) -> str | None:
        name = self.attribute.get_attribute_name()
        return f"undefine\nattribute {name};"


@dataclass
class RemoveAttribute(Operation):
    """Remove an attribute type.

    WARNING: This is a BREAKING change. Ensure all attribute instances
    and ownerships are removed first.
    """

    attribute: type[Attribute]

    def to_typeql(self) -> str:
        name = self.attribute.get_attribute_name()
        return f"undefine\nattribute {name};"

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

    entity: type[Entity]

    def to_typeql(self) -> str:
        schema = self.entity.to_schema_definition()
        if schema:
            return f"define\n{schema}"
        return ""

    def to_rollback_typeql(self) -> str | None:
        name = self.entity.get_type_name()
        return f"undefine\nentity {name};"


@dataclass
class RemoveEntity(Operation):
    """Remove an entity type.

    WARNING: This is a BREAKING change. Ensure all entity instances
    are deleted first.
    """

    entity: type[Entity]

    def to_typeql(self) -> str:
        name = self.entity.get_type_name()
        return f"undefine\nentity {name};"

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

    owner: type[Entity | Relation]
    attribute: type[Attribute]
    optional: bool = False
    key: bool = False
    unique: bool = False
    card_min: int | None = None
    card_max: int | None = None

    def to_typeql(self) -> str:
        from type_bridge.typeql.annotations import format_card_annotation

        owner_name = self.owner.get_type_name()
        attr_name = self.attribute.get_attribute_name()

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
        owner_name = self.owner.get_type_name()
        attr_name = self.attribute.get_attribute_name()
        return f"undefine\n{owner_name} owns {attr_name};"


@dataclass
class RemoveOwnership(Operation):
    """Remove attribute ownership from an entity or relation.

    WARNING: This may orphan attribute data. Ensure attribute values
    are removed from instances first.
    """

    owner: type[Entity | Relation]
    attribute: type[Attribute]

    def to_typeql(self) -> str:
        owner_name = self.owner.get_type_name()
        attr_name = self.attribute.get_attribute_name()
        return f"undefine\n{owner_name} owns {attr_name};"

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

    owner: type[Entity | Relation]
    attribute: type[Attribute]
    old_annotations: str
    new_annotations: str

    def to_typeql(self) -> str:
        owner_name = self.owner.get_type_name()
        attr_name = self.attribute.get_attribute_name()
        # TypeDB 3.x uses redefine for modifications
        return f"redefine\n{owner_name} owns {attr_name} {self.new_annotations};"

    def to_rollback_typeql(self) -> str | None:
        owner_name = self.owner.get_type_name()
        attr_name = self.attribute.get_attribute_name()
        return f"redefine\n{owner_name} owns {attr_name} {self.old_annotations};"


# --- Relation Operations ---


@dataclass
class AddRelation(Operation):
    """Add a new relation type with its roles.

    Example:
        ops.AddRelation(Employment)
    """

    relation: type[Relation]

    def to_typeql(self) -> str:
        lines = []
        schema = self.relation.to_schema_definition()
        if schema:
            lines.append(f"define\n{schema}")

            # Add role player definitions
            for role_name, role in self.relation._roles.items():
                for player_type in role.player_types:
                    lines.append(
                        f"{player_type} plays {self.relation.get_type_name()}:{role.role_name};"
                    )

        return "\n".join(lines)

    def to_rollback_typeql(self) -> str | None:
        name = self.relation.get_type_name()
        return f"undefine\nrelation {name};"


@dataclass
class RemoveRelation(Operation):
    """Remove a relation type.

    WARNING: This is a BREAKING change. Ensure all relation instances
    are deleted first.
    """

    relation: type[Relation]

    def to_typeql(self) -> str:
        name = self.relation.get_type_name()
        return f"undefine\nrelation {name};"

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

    relation: type[Relation]
    role_name: str
    player_types: list[str] = field(default_factory=list)

    def to_typeql(self) -> str:
        rel_name = self.relation.get_type_name()
        lines = [f"define\n{rel_name} relates {self.role_name};"]
        for player in self.player_types:
            lines.append(f"{player} plays {rel_name}:{self.role_name};")
        return "\n".join(lines)

    def to_rollback_typeql(self) -> str | None:
        rel_name = self.relation.get_type_name()
        return f"undefine\n{rel_name} relates {self.role_name};"


@dataclass
class RemoveRole(Operation):
    """Remove a role from a relation.

    WARNING: This is a BREAKING change. Ensure no relation instances
    have role players for this role.
    """

    relation: type[Relation]
    role_name: str

    def to_typeql(self) -> str:
        rel_name = self.relation.get_type_name()
        return f"undefine\n{rel_name} relates {self.role_name};"

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

    relation: type[Relation]
    role_name: str
    player_type: str

    def to_typeql(self) -> str:
        rel_name = self.relation.get_type_name()
        return f"define\n{self.player_type} plays {rel_name}:{self.role_name};"

    def to_rollback_typeql(self) -> str | None:
        rel_name = self.relation.get_type_name()
        return f"undefine\n{self.player_type} plays {rel_name}:{self.role_name};"


@dataclass
class RemoveRolePlayer(Operation):
    """Remove a player type from a role.

    WARNING: This is a BREAKING change. Ensure no relation instances
    have this player type in this role.
    """

    relation: type[Relation]
    role_name: str
    player_type: str

    def to_typeql(self) -> str:
        rel_name = self.relation.get_type_name()
        return f"undefine\n{self.player_type} plays {rel_name}:{self.role_name};"

    def to_rollback_typeql(self) -> str | None:
        rel_name = self.relation.get_type_name()
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

    owner: type[Entity | Relation]
    source: str
    dest: str
    filter: str | None = None

    def to_typeql(self) -> str:
        """Generate insert-if-absent backfill TypeQL.

        Emits a match+insert that copies ``source`` values to ``dest`` for every
        owner instance that does not already have the destination attribute.
        """
        owner_name = self.owner.get_type_name()
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
        owner_name = self.owner.get_type_name()
        return f"match $x isa {owner_name}, has {self.dest} $v;\ndelete $v of $x;"
