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


def _subject_type_name(value: object) -> str:
    """Resolve a type name from an entity/relation model, attribute class, or ref."""
    get_type_name = getattr(value, "get_type_name", None)
    if get_type_name is not None:
        return str(get_type_name())
    return _attribute_name(value)


def _split_annotation_tokens(flags: str) -> list[tuple[str, str | None]]:
    """Split a flag string like ``@key @card(1..5) @doc("...")`` into
    ``(name, args)`` tokens. Escape-aware: parentheses inside double-quoted
    string literals do not terminate an argument list. Mirrors the Rust
    ``annotations::split_annotation_tokens``."""
    tokens: list[tuple[str, str | None]] = []
    i = 0
    length = len(flags)
    while i < length:
        if flags[i] != "@":
            i += 1
            continue
        i += 1
        name_start = i
        while i < length and (flags[i].isalnum() or flags[i] == "_"):
            i += 1
        if i == name_start:
            continue
        name = flags[name_start:i]
        args: str | None = None
        if i < length and flags[i] == "(":
            i += 1
            args_start = i
            depth = 1
            in_string = False
            while i < length:
                ch = flags[i]
                if in_string and ch == "\\":
                    i += 2
                    continue
                if ch == '"':
                    in_string = not in_string
                elif ch == "(" and not in_string:
                    depth += 1
                elif ch == ")" and not in_string:
                    depth -= 1
                    if depth == 0:
                        break
                i += 1
            args = flags[args_start : min(i, length)]
            if i < length:
                i += 1
        tokens.append((name, args))
    return tokens


def _first_string_literal(args: str) -> str | None:
    """Extract the first double-quoted string literal from an argument list,
    unescaped."""
    start = args.find('"')
    if start < 0:
        return None
    out: list[str] = []
    i = start + 1
    while i < len(args):
        ch = args[i]
        if ch == '"':
            return "".join(out)
        if ch == "\\" and i + 1 < len(args):
            follower = args[i + 1]
            out.append({"n": "\n", "t": "\t", "r": "\r"}.get(follower, follower))
            i += 2
            continue
        out.append(ch)
        i += 1
    return None


def _token_identity(name: str, args: str | None) -> str:
    """Change-grouping identity: ``@meta`` is keyed per meta key; everything
    else is at most one per subject."""
    if name == "meta" and args is not None:
        key = _first_string_literal(args)
        if key is not None:
            return f"meta:{key}"
    return name


def _render_token(name: str, args: str | None) -> str:
    return f"@{name}({args})" if args is not None else f"@{name}"


def _undefine_token_ref(name: str, args: str | None) -> str:
    """The ``@...`` reference used in ``undefine <ref> from <subject>``:
    ``@meta`` must be keyed; every other annotation undefines by bare name."""
    from type_bridge.typeql.annotations import escape_annotation_string

    if name == "meta" and args is not None:
        key = _first_string_literal(args)
        if key is not None:
            return f"@meta({escape_annotation_string(key)})"
    return f"@{name}"


def _annotation_change_steps(
    subject: str,
    old_tokens: list[tuple[str, str | None]],
    new_tokens: list[tuple[str, str | None]],
) -> tuple[list[str], list[str]]:
    """Lower an annotation-set change on ``subject`` to (forward, rollback) steps.

    TypeDB 3.12 semantics: removals must use ``undefine`` (one block for all,
    ``@meta`` keyed), value changes must use ``redefine`` (exactly one element
    per query; parameterless annotations can never be redefined), and
    additions must use ``define`` (one block for all). Removals run first —
    adding ``@key`` while a conflicting explicit ``@card`` is still declared
    fails schema validation. Mirrors the Rust planner's
    ``annotation_token_steps``.
    """
    old_map = {_token_identity(name, args): (name, args) for name, args in old_tokens}
    new_map = {_token_identity(name, args): (name, args) for name, args in new_tokens}

    added = [new_map[key] for key in sorted(new_map) if key not in old_map]
    removed = [old_map[key] for key in sorted(old_map) if key not in new_map]
    changed = [
        (old_map[key], new_map[key])
        for key in sorted(old_map)
        if key in new_map and _render_token(*old_map[key]) != _render_token(*new_map[key])
    ]

    forward: list[str] = []
    if removed:
        forward.append(
            "undefine\n"
            + "\n".join(f"{_undefine_token_ref(*token)} from {subject};" for token in removed)
        )
    for _, new_token in changed:
        forward.append(f"redefine\n{subject} {_render_token(*new_token)};")
    if added:
        forward.append(
            "define\n" + "\n".join(f"{subject} {_render_token(*token)};" for token in added)
        )

    # Rollback steps are listed in rollback execution order: each forward
    # step mirrored, walked back-to-front.
    rollback: list[str] = []
    if added:
        rollback.append(
            "undefine\n"
            + "\n".join(f"{_undefine_token_ref(*token)} from {subject};" for token in added)
        )
    for old_token, _ in reversed(changed):
        rollback.append(f"redefine\n{subject} {_render_token(*old_token)};")
    if removed:
        rollback.append(
            "define\n" + "\n".join(f"{subject} {_render_token(*token)};" for token in removed)
        )
    return forward, rollback


def _doc_meta_tokens(doc: str | None, meta: dict[str, str]) -> list[tuple[str, str | None]]:
    """Build the @doc/@meta token list for one side of an annotation change."""
    from type_bridge.typeql.annotations import escape_annotation_string

    tokens: list[tuple[str, str | None]] = []
    if doc is not None:
        tokens.append(("doc", escape_annotation_string(doc)))
    for key in sorted(meta):
        tokens.append(
            ("meta", f"{escape_annotation_string(key)}, {escape_annotation_string(meta[key])}")
        )
    return tokens


def _doc_meta_annotation_steps(
    subject: str,
    old_doc: str | None,
    new_doc: str | None,
    old_meta: dict[str, str],
    new_meta: dict[str, str],
) -> tuple[list[str], list[str]]:
    """Lower a @doc/@meta change on ``subject`` to (forward, rollback) steps."""
    return _annotation_change_steps(
        subject,
        _doc_meta_tokens(old_doc, old_meta),
        _doc_meta_tokens(new_doc, new_meta),
    )


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
        return _doc_meta_annotation_steps(
            _subject_type_name(self.subject),
            self.old_doc,
            self.new_doc,
            self.old_meta,
            self.new_meta,
        )

    def to_typeql(self) -> str:
        return "\n\n".join(self._steps()[0])

    def to_rollback_typeql(self) -> str | None:
        return "\n\n".join(self._steps()[1])

    def to_typeql_steps(self) -> list[str]:
        return self._steps()[0]

    def to_rollback_typeql_steps(self) -> list[str] | None:
        return self._steps()[1]


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
        subject = f"{_type_name(self.relation)} relates {self.role_name}"
        return _doc_meta_annotation_steps(
            subject, self.old_doc, self.new_doc, self.old_meta, self.new_meta
        )

    def to_typeql(self) -> str:
        return "\n\n".join(self._steps()[0])

    def to_rollback_typeql(self) -> str | None:
        return "\n\n".join(self._steps()[1])

    def to_typeql_steps(self) -> list[str]:
        return self._steps()[0]

    def to_rollback_typeql_steps(self) -> list[str] | None:
        return self._steps()[1]


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
        subject = f"{_type_name(self.owner)} owns {_attribute_name(self.attribute)}"
        return _annotation_change_steps(
            subject,
            _split_annotation_tokens(self.old_annotations),
            _split_annotation_tokens(self.new_annotations),
        )

    def to_typeql(self) -> str:
        return "\n\n".join(self._steps()[0])

    def to_rollback_typeql(self) -> str | None:
        return "\n\n".join(self._steps()[1])

    def to_typeql_steps(self) -> list[str]:
        return self._steps()[0]

    def to_rollback_typeql_steps(self) -> list[str] | None:
        return self._steps()[1]


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
