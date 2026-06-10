"""Shared fixtures for Python vs Rust benchmarks."""

from __future__ import annotations

import pytest

from type_bridge.query.ast import (
    ArithmeticValue,
    AttributePattern,
    DeleteClause,
    DeleteThingStatement,
    EntityPattern,
    FetchAttribute,
    FetchAttributeList,
    FetchClause,
    FetchFunction,
    FetchNestedWildcard,
    FetchVariable,
    FetchWildcard,
    FunctionCallValue,
    HasConstraint,
    HasPattern,
    HasStatement,
    IidConstraint,
    IidPattern,
    InsertClause,
    IsaConstraint,
    IsaStatement,
    LetAssignment,
    LiteralValue,
    MatchClause,
    MatchLetClause,
    NotPattern,
    OrPattern,
    ReduceAssignment,
    ReduceClause,
    RelationPattern,
    RelationStatement,
    RolePlayer,
    SubTypePattern,
    UpdateClause,
    ValueComparisonPattern,
)
from type_bridge.query.compiler import QueryCompiler

# ---------------------------------------------------------------------------
# Rust availability
# ---------------------------------------------------------------------------

try:
    from type_bridge_core import (
        QueryCompiler as RustQueryCompiler,
    )
    from type_bridge_core import (
        ValidationEngine as RustValidationEngine,
    )

    RUST_AVAILABLE = True
except ImportError:
    RUST_AVAILABLE = False

requires_rust = pytest.mark.skipif(not RUST_AVAILABLE, reason="type_bridge_core not installed")


# ---------------------------------------------------------------------------
# Compiler / validator instances
# ---------------------------------------------------------------------------


@pytest.fixture
def python_compiler() -> QueryCompiler:
    """A QueryCompiler that always uses the Python path."""
    return QueryCompiler()


@pytest.fixture
def rust_compiler():
    """The Rust QueryCompiler (skips if unavailable)."""
    pytest.importorskip("type_bridge_core")
    return RustQueryCompiler()


@pytest.fixture
def rust_validator():
    """The Rust ValidationEngine (skips if unavailable)."""
    pytest.importorskip("type_bridge_core")
    return RustValidationEngine()


# ---------------------------------------------------------------------------
# Validation data
# ---------------------------------------------------------------------------


@pytest.fixture
def type_names() -> list[str]:
    """1000 type names: mix of simple, hyphenated, long, and unicode."""
    names: list[str] = []
    for i in range(400):
        names.append(f"entity-type-{i}")
    for i in range(300):
        names.append(f"my-long-attribute-name-for-testing-{i}")
    for i in range(200):
        names.append(f"relation-{i}-data")
    for i in range(100):
        names.append(f"\u00e9l\u00e8ve-{i}")  # unicode names
    return names


# ---------------------------------------------------------------------------
# Query data — simple
# ---------------------------------------------------------------------------


@pytest.fixture
def simple_match() -> MatchClause:
    """Minimal match: $p isa person."""
    return MatchClause(patterns=[EntityPattern(variable="$p", type_name="person", constraints=[])])


@pytest.fixture
def match_with_constraints() -> MatchClause:
    """Match with IID + has constraints."""
    return MatchClause(
        patterns=[
            EntityPattern(
                variable="$p",
                type_name="person",
                constraints=[
                    IidConstraint(iid="0x1234567890abcdef"),
                    HasConstraint(attr_name="name", value=LiteralValue("Alice", "string")),
                    HasConstraint(attr_name="age", value=LiteralValue(30, "long")),
                ],
            )
        ]
    )


# ---------------------------------------------------------------------------
# Query data — complex (10 patterns)
# ---------------------------------------------------------------------------


@pytest.fixture
def complex_query() -> MatchClause:
    """Complex match with 10 patterns: entities, relations, not, or, iid, has, comparison."""
    return MatchClause(
        patterns=[
            EntityPattern(
                variable="$p",
                type_name="person",
                constraints=[
                    HasConstraint(attr_name="name", value=LiteralValue("Alice", "string")),
                    HasConstraint(attr_name="age", value=LiteralValue(30, "long")),
                ],
            ),
            RelationPattern(
                variable="$r",
                type_name="employment",
                role_players=[
                    RolePlayer(role="employee", player_var="$p"),
                    RolePlayer(role="employer", player_var="$c"),
                ],
            ),
            EntityPattern(
                variable="$c",
                type_name="company",
                constraints=[
                    HasConstraint(attr_name="sector", value=LiteralValue("tech", "string"))
                ],
            ),
            HasPattern(thing_var="$p", attr_type="email", attr_var="$e"),
            ValueComparisonPattern(var="$age", operator=">=", value=LiteralValue(18, "long")),
            NotPattern(
                patterns=[
                    EntityPattern(
                        variable="$p",
                        type_name="retired-person",
                        constraints=[],
                        is_strict=True,
                    )
                ]
            ),
            OrPattern(
                alternatives=[
                    [HasPattern(thing_var="$p", attr_type="status", attr_var="$s1")],
                    [HasPattern(thing_var="$p", attr_type="active", attr_var="$s2")],
                ]
            ),
            EntityPattern(
                variable="$d",
                type_name="department",
                constraints=[
                    HasConstraint(attr_name="budget", value=LiteralValue(100000.0, "double")),
                ],
            ),
            EntityPattern(
                variable="$mgr",
                type_name="manager",
                constraints=[HasConstraint(attr_name="level", value=LiteralValue(3, "long"))],
            ),
            EntityPattern(
                variable="$proj",
                type_name="project",
                constraints=[
                    HasConstraint(attr_name="deadline", value=LiteralValue("2025-12-31", "date"))
                ],
            ),
        ]
    )


# ---------------------------------------------------------------------------
# Query data — relation insert
# ---------------------------------------------------------------------------


@pytest.fixture
def relation_insert() -> InsertClause:
    """Relation insert with role players and inline attributes."""
    return InsertClause(
        statements=[
            RelationStatement(
                variable="$rel",
                type_name="employment",
                role_players=[
                    RolePlayer(role="employee", player_var="$p"),
                    RolePlayer(role="employer", player_var="$c"),
                    RolePlayer(role="department", player_var="$d"),
                ],
                include_variable=True,
                attributes=[
                    HasStatement(
                        subject_var="$rel",
                        attr_name="start-date",
                        value=LiteralValue("2024-01-15", "date"),
                    ),
                    HasStatement(
                        subject_var="$rel",
                        attr_name="salary",
                        value=LiteralValue(95000, "long"),
                    ),
                ],
            )
        ]
    )


# ---------------------------------------------------------------------------
# Query data — batch (50 clauses)
# ---------------------------------------------------------------------------


@pytest.fixture
def batch_clauses() -> list[MatchClause | InsertClause | DeleteClause | FetchClause]:
    """50 mixed clauses: 20 match, 15 insert, 10 delete, 5 fetch."""
    clauses: list[MatchClause | InsertClause | DeleteClause | FetchClause] = []
    for i in range(20):
        clauses.append(
            MatchClause(
                patterns=[
                    EntityPattern(
                        variable=f"$e{i}",
                        type_name=f"entity-type-{i}",
                        constraints=[
                            HasConstraint(
                                attr_name=f"attr-{i}",
                                value=LiteralValue(f"value-{i}", "string"),
                            )
                        ],
                    )
                ]
            )
        )
    for i in range(15):
        clauses.append(
            InsertClause(
                statements=[
                    IsaStatement(variable=f"$n{i}", type_name=f"new-type-{i}"),
                    HasStatement(
                        subject_var=f"$n{i}",
                        attr_name=f"prop-{i}",
                        value=LiteralValue(i * 10, "long"),
                    ),
                ]
            )
        )
    for i in range(10):
        clauses.append(DeleteClause(statements=[DeleteThingStatement(variable=f"$d{i}")]))
    for i in range(5):
        clauses.append(
            FetchClause(
                items=[FetchAttribute(key=f"field-{i}", var=f"$f{i}", attr_name=f"data-{i}")]
            )
        )
    return clauses


# ---------------------------------------------------------------------------
# Query data — long query (30 patterns)
# ---------------------------------------------------------------------------


@pytest.fixture
def long_query() -> MatchClause:
    """30-pattern match simulating a wide join across many entities."""
    patterns = []
    for i in range(10):
        patterns.append(
            EntityPattern(
                variable=f"$e{i}",
                type_name=f"entity-type-{i}",
                constraints=[
                    HasConstraint(attr_name=f"name-{i}", value=LiteralValue(f"val-{i}", "string")),
                    HasConstraint(attr_name=f"count-{i}", value=LiteralValue(i * 100, "long")),
                ],
            )
        )
    for i in range(5):
        patterns.append(
            RelationPattern(
                variable=f"$r{i}",
                type_name=f"link-{i}",
                role_players=[
                    RolePlayer(role=f"source-{i}", player_var=f"$e{i}"),
                    RolePlayer(role=f"target-{i}", player_var=f"$e{i + 5}"),
                ],
                constraints=[
                    HasConstraint(attr_name=f"weight-{i}", value=LiteralValue(i * 0.5, "double"))
                ],
            )
        )
    for i in range(5):
        patterns.append(HasPattern(thing_var=f"$e{i}", attr_type=f"tag-{i}", attr_var=f"$t{i}"))
    for i in range(5):
        patterns.append(
            ValueComparisonPattern(
                var=f"$t{i}",
                operator=">=",
                value=LiteralValue(f"threshold-{i}", "string"),
            )
        )
    for i in range(3):
        patterns.append(IidPattern(variable=f"$x{i}", iid=f"0x{'ab' * 8}{i:02x}"))
    patterns.append(
        AttributePattern(
            variable="$salary",
            type_name="salary-amount",
            value=LiteralValue(50000, "long"),
        )
    )
    patterns.append(SubTypePattern(variable="$t", parent_type="base-entity"))
    return MatchClause(patterns=patterns)


# ---------------------------------------------------------------------------
# Query data — deeply nested (or-of-not-of-or, 3 levels)
# ---------------------------------------------------------------------------


@pytest.fixture
def deeply_nested_query() -> MatchClause:
    """Deeply nested boolean logic: or { not { or { ... } } }."""
    # Level 3 (innermost): simple entity patterns
    inner_entities = [
        [
            EntityPattern(
                variable=f"$deep{i}",
                type_name=f"leaf-type-{i}",
                constraints=[
                    HasConstraint(attr_name=f"prop-{i}", value=LiteralValue(f"v{i}", "string"))
                ],
            )
        ]
        for i in range(4)
    ]
    level3_or = OrPattern(alternatives=inner_entities)

    # Level 2: not wrapping an or, repeated
    level2_blocks = []
    for i in range(3):
        not_block = NotPattern(
            patterns=[
                level3_or,
                EntityPattern(
                    variable=f"$guard{i}",
                    type_name=f"guard-type-{i}",
                    constraints=[
                        HasConstraint(attr_name="active", value=LiteralValue(True, "boolean"))
                    ],
                ),
            ]
        )
        level2_blocks.append(
            [
                not_block,
                RelationPattern(
                    variable=f"$rel{i}",
                    type_name=f"context-rel-{i}",
                    role_players=[
                        RolePlayer(role="subject", player_var=f"$guard{i}"),
                        RolePlayer(role="object", player_var=f"$deep{i}"),
                    ],
                ),
            ]
        )

    # Level 1: top-level or over the not blocks
    top_or = OrPattern(alternatives=level2_blocks)

    return MatchClause(
        patterns=[
            EntityPattern(
                variable="$root",
                type_name="root-entity",
                constraints=[
                    HasConstraint(attr_name="name", value=LiteralValue("start", "string")),
                    HasConstraint(attr_name="priority", value=LiteralValue(1, "long")),
                    HasConstraint(attr_name="score", value=LiteralValue(99.5, "double")),
                ],
            ),
            top_or,
            NotPattern(
                patterns=[
                    EntityPattern(
                        variable="$excluded",
                        type_name="blacklisted",
                        constraints=[],
                        is_strict=True,
                    )
                ]
            ),
            HasPattern(thing_var="$root", attr_type="timestamp", attr_var="$ts"),
            ValueComparisonPattern(
                var="$ts", operator=">=", value=LiteralValue("2024-01-01", "date")
            ),
        ]
    )


# ---------------------------------------------------------------------------
# Query data — graph traversal (chain of relations)
# ---------------------------------------------------------------------------


@pytest.fixture
def graph_traversal_query() -> MatchClause:
    """Simulates a multi-hop graph traversal: A->B->C->D->E with relations."""
    hop_count = 8
    patterns = []

    # The anchor entity
    patterns.append(
        EntityPattern(
            variable="$n0",
            type_name="person",
            constraints=[
                HasConstraint(attr_name="name", value=LiteralValue("Alice", "string")),
                IidConstraint(iid="0x0000000000000001"),
            ],
        )
    )

    # Chain of hops: each hop is a relation + target entity
    for i in range(hop_count):
        patterns.append(
            RelationPattern(
                variable=f"$hop{i}",
                type_name="knows" if i % 2 == 0 else "works-with",
                role_players=[
                    RolePlayer(role="from", player_var=f"$n{i}"),
                    RolePlayer(role="to", player_var=f"$n{i + 1}"),
                ],
                constraints=[
                    HasConstraint(
                        attr_name="since",
                        value=LiteralValue(f"20{15 + i}-01-01", "date"),
                    )
                ],
            )
        )
        patterns.append(
            EntityPattern(
                variable=f"$n{i + 1}",
                type_name="person",
                constraints=[
                    HasConstraint(
                        attr_name="age",
                        value=LiteralValue(25 + i, "long"),
                    )
                ],
            )
        )

    # Fetch attributes along the chain
    for i in range(hop_count + 1):
        patterns.append(HasPattern(thing_var=f"$n{i}", attr_type="email", attr_var=f"$email{i}"))

    return MatchClause(patterns=patterns)


# ---------------------------------------------------------------------------
# Query data — heavy insert (100 entities with attributes)
# ---------------------------------------------------------------------------


@pytest.fixture
def heavy_insert() -> InsertClause:
    """Insert 100 entities, each with 5 attributes."""
    statements = []
    for i in range(100):
        var = f"$new{i}"
        statements.append(IsaStatement(variable=var, type_name=f"data-record-{i % 10}"))
        statements.append(
            HasStatement(
                subject_var=var, attr_name="name", value=LiteralValue(f"Record #{i}", "string")
            )
        )
        statements.append(
            HasStatement(subject_var=var, attr_name="index", value=LiteralValue(i, "long"))
        )
        statements.append(
            HasStatement(
                subject_var=var,
                attr_name="score",
                value=LiteralValue(i * 1.5, "double"),
            )
        )
        statements.append(
            HasStatement(
                subject_var=var,
                attr_name="active",
                value=LiteralValue(i % 2 == 0, "boolean"),
            )
        )
        statements.append(
            HasStatement(
                subject_var=var,
                attr_name="created-at",
                value=LiteralValue(f"2025-01-{(i % 28) + 1:02d}", "date"),
            )
        )
    return InsertClause(statements=statements)


# ---------------------------------------------------------------------------
# Query data — large fetch (20 items, mixed types)
# ---------------------------------------------------------------------------


@pytest.fixture
def large_fetch() -> FetchClause:
    """Fetch clause with 20 items mixing all fetch item types."""
    items = []
    for i in range(6):
        items.append(FetchAttribute(key=f"attr-{i}", var="$p", attr_name=f"field-{i}"))
    for i in range(4):
        items.append(FetchAttributeList(key=f"list-{i}", var="$p", attr_name=f"tags-{i}"))
    for i in range(3):
        items.append(FetchFunction(key=f"fn-{i}", func_name="label", var=f"$t{i}"))
    for i in range(3):
        items.append(FetchWildcard(key=f"all-{i}", var=f"$e{i}"))
    for i in range(2):
        items.append(FetchNestedWildcard(key=f"nested-{i}", var=f"$r{i}"))
    items.append(FetchFunction(key="_iid", func_name="iid", var="$p"))
    items.append(FetchFunction(key="_label", func_name="label", var="$p"))
    return FetchClause(items=items)


# ---------------------------------------------------------------------------
# Query data — reduce with multiple aggregations
# ---------------------------------------------------------------------------


@pytest.fixture
def reduce_query() -> ReduceClause:
    """Reduce clause with 5 aggregation assignments and groupby."""
    return ReduceClause(
        assignments=[
            ReduceAssignment(
                variable="$count",
                expression=FunctionCallValue(function="count", args=["$p"]),
            ),
            ReduceAssignment(
                variable="$total_salary",
                expression=FunctionCallValue(function="sum", args=["$salary"]),
            ),
            ReduceAssignment(
                variable="$max_age",
                expression=FunctionCallValue(function="max", args=["$age"]),
            ),
            ReduceAssignment(
                variable="$min_age",
                expression=FunctionCallValue(function="min", args=["$age"]),
            ),
            ReduceAssignment(
                variable="$avg_score",
                expression=FunctionCallValue(function="mean", args=["$score"]),
            ),
        ],
        group_by="$dept",
    )


# ---------------------------------------------------------------------------
# Query data — realistic multi-clause pipeline
# ---------------------------------------------------------------------------


@pytest.fixture
def realistic_pipeline() -> list:
    """Realistic pipeline: match + fetch, simulating a real ORM query."""
    match = MatchClause(
        patterns=[
            EntityPattern(
                variable="$p",
                type_name="person",
                constraints=[
                    HasConstraint(attr_name="status", value=LiteralValue("active", "string")),
                ],
            ),
            RelationPattern(
                variable="$emp",
                type_name="employment",
                role_players=[
                    RolePlayer(role="employee", player_var="$p"),
                    RolePlayer(role="employer", player_var="$c"),
                ],
            ),
            EntityPattern(
                variable="$c",
                type_name="company",
                constraints=[
                    HasConstraint(attr_name="sector", value=LiteralValue("tech", "string")),
                    HasConstraint(attr_name="size", value=LiteralValue("large", "string")),
                ],
            ),
            HasPattern(thing_var="$p", attr_type="name", attr_var="$name"),
            HasPattern(thing_var="$p", attr_type="email", attr_var="$email"),
            HasPattern(thing_var="$p", attr_type="age", attr_var="$age"),
            ValueComparisonPattern(var="$age", operator=">=", value=LiteralValue(21, "long")),
            ValueComparisonPattern(var="$age", operator="<=", value=LiteralValue(65, "long")),
            NotPattern(
                patterns=[
                    EntityPattern(
                        variable="$p",
                        type_name="contractor",
                        constraints=[],
                        is_strict=True,
                    )
                ]
            ),
        ]
    )
    fetch = FetchClause(
        items=[
            FetchFunction(key="_iid", func_name="iid", var="$p"),
            FetchAttribute(key="name", var="$p", attr_name="name"),
            FetchAttribute(key="email", var="$p", attr_name="email"),
            FetchAttribute(key="age", var="$p", attr_name="age"),
            FetchNestedWildcard(key="company", var="$c"),
        ]
    )
    return [match, fetch]


# ---------------------------------------------------------------------------
# Query data — large batch (200 clauses)
# ---------------------------------------------------------------------------


@pytest.fixture
def large_batch() -> list:
    """200 mixed clauses simulating a bulk operation."""
    clauses = []
    # 80 matches with 3 patterns each
    for i in range(80):
        clauses.append(
            MatchClause(
                patterns=[
                    EntityPattern(
                        variable=f"$e{i}",
                        type_name=f"type-{i % 20}",
                        constraints=[
                            HasConstraint(
                                attr_name="key",
                                value=LiteralValue(f"k-{i}", "string"),
                            ),
                            HasConstraint(
                                attr_name="seq",
                                value=LiteralValue(i, "long"),
                            ),
                        ],
                    ),
                    HasPattern(thing_var=f"$e{i}", attr_type="label", attr_var=f"$lbl{i}"),
                    ValueComparisonPattern(
                        var=f"$lbl{i}",
                        operator="!=",
                        value=LiteralValue("", "string"),
                    ),
                ]
            )
        )
    # 60 inserts with isa + 3 has each
    for i in range(60):
        var = f"$ins{i}"
        clauses.append(
            InsertClause(
                statements=[
                    IsaStatement(variable=var, type_name=f"record-{i % 15}"),
                    HasStatement(
                        subject_var=var,
                        attr_name="name",
                        value=LiteralValue(f"item-{i}", "string"),
                    ),
                    HasStatement(
                        subject_var=var,
                        attr_name="value",
                        value=LiteralValue(i * 3.14, "double"),
                    ),
                    HasStatement(
                        subject_var=var,
                        attr_name="timestamp",
                        value=LiteralValue(f"2025-06-{(i % 28) + 1:02d}", "date"),
                    ),
                ]
            )
        )
    # 40 deletes
    for i in range(40):
        clauses.append(DeleteClause(statements=[DeleteThingStatement(variable=f"$del{i}")]))
    # 20 updates
    for i in range(20):
        clauses.append(
            UpdateClause(
                statements=[
                    HasStatement(
                        subject_var=f"$upd{i}",
                        attr_name="modified",
                        value=LiteralValue("2025-12-31", "date"),
                    )
                ]
            )
        )
    return clauses


# ---------------------------------------------------------------------------
# Query data — arithmetic expressions
# ---------------------------------------------------------------------------


@pytest.fixture
def arithmetic_match() -> MatchClause:
    """Match with ArithmeticValue: $salary > ($base + $bonus) * 1.5."""
    return MatchClause(
        patterns=[
            EntityPattern(
                variable="$p",
                type_name="employee",
                constraints=[
                    HasConstraint(
                        attr_name="department",
                        value=LiteralValue("engineering", "string"),
                    ),
                ],
            ),
            HasPattern(thing_var="$p", attr_type="salary", attr_var="$salary"),
            HasPattern(thing_var="$p", attr_type="base-pay", attr_var="$base"),
            HasPattern(thing_var="$p", attr_type="bonus", attr_var="$bonus"),
            ValueComparisonPattern(
                var="$salary",
                operator=">",
                value=ArithmeticValue(
                    left=ArithmeticValue(
                        left="$base",
                        operator="+",
                        right="$bonus",
                    ),
                    operator="*",
                    right=LiteralValue(1.5, "double"),
                ),
            ),
        ]
    )


@pytest.fixture
def nested_arithmetic() -> MatchClause:
    """4-level nested arithmetic: (($a + $b) * ($c - $d)) / (($e % $f) ^ 2)."""
    return MatchClause(
        patterns=[
            EntityPattern(variable="$x", type_name="calculation", constraints=[]),
            HasPattern(thing_var="$x", attr_type="a", attr_var="$a"),
            HasPattern(thing_var="$x", attr_type="b", attr_var="$b"),
            HasPattern(thing_var="$x", attr_type="c", attr_var="$c"),
            HasPattern(thing_var="$x", attr_type="d", attr_var="$d"),
            HasPattern(thing_var="$x", attr_type="e", attr_var="$e"),
            HasPattern(thing_var="$x", attr_type="f", attr_var="$f"),
            ValueComparisonPattern(
                var="$result",
                operator=">=",
                value=ArithmeticValue(
                    left=ArithmeticValue(
                        left=ArithmeticValue(left="$a", operator="+", right="$b"),
                        operator="*",
                        right=ArithmeticValue(left="$c", operator="-", right="$d"),
                    ),
                    operator="/",
                    right=ArithmeticValue(
                        left=ArithmeticValue(left="$e", operator="%", right="$f"),
                        operator="^",
                        right=LiteralValue(2, "long"),
                    ),
                ),
            ),
        ]
    )


# ---------------------------------------------------------------------------
# Query data — MatchLetClause
# ---------------------------------------------------------------------------


@pytest.fixture
def match_let_single() -> MatchLetClause:
    """Single let assignment: let $count = count($p)."""
    return MatchLetClause(
        assignments=[
            LetAssignment(
                variables=["$count"],
                expression=FunctionCallValue(function="count", args=["$p"]),
                is_stream=False,
            )
        ]
    )


@pytest.fixture
def match_let_multiple() -> MatchLetClause:
    """Multiple let assignments including a stream."""
    return MatchLetClause(
        assignments=[
            LetAssignment(
                variables=["$count"],
                expression=FunctionCallValue(function="count", args=["$p"]),
                is_stream=False,
            ),
            LetAssignment(
                variables=["$total"],
                expression=FunctionCallValue(function="sum", args=["$salary"]),
                is_stream=False,
            ),
            LetAssignment(
                variables=["$x"],
                expression=FunctionCallValue(function="values", args=["$attr"]),
                is_stream=True,
            ),
        ]
    )


# ---------------------------------------------------------------------------
# Query data — FetchVariable
# ---------------------------------------------------------------------------


@pytest.fixture
def fetch_with_variable() -> FetchClause:
    """Fetch clause using FetchVariable items."""
    return FetchClause(
        items=[
            FetchVariable(key="person", var="$p"),
            FetchVariable(key="company", var="$c"),
            FetchAttribute(key="name", var="$p", attr_name="name"),
            FetchFunction(key="_iid", func_name="iid", var="$p"),
        ]
    )


# ---------------------------------------------------------------------------
# Query data — IsaConstraint
# ---------------------------------------------------------------------------


@pytest.fixture
def isa_constraint_match() -> MatchClause:
    """Match with IsaConstraint (strict and non-strict)."""
    return MatchClause(
        patterns=[
            EntityPattern(
                variable="$p",
                type_name="person",
                constraints=[
                    IsaConstraint(type_name="employee", strict=False),
                    HasConstraint(attr_name="name", value=LiteralValue("Alice", "string")),
                ],
            ),
            EntityPattern(
                variable="$a",
                type_name="animal",
                constraints=[
                    IsaConstraint(type_name="mammal", strict=True),
                ],
            ),
            EntityPattern(
                variable="$v",
                type_name="vehicle",
                constraints=[
                    IsaConstraint(type_name="electric-vehicle", strict=False),
                    HasConstraint(attr_name="range", value=LiteralValue(300, "long")),
                ],
            ),
        ]
    )


# ---------------------------------------------------------------------------
# Query data — standalone clause types
# ---------------------------------------------------------------------------


@pytest.fixture
def standalone_insert() -> InsertClause:
    """Standalone insert: 5 entities x 4 statements each."""
    statements = []
    for i in range(5):
        var = f"$e{i}"
        statements.extend(
            [
                IsaStatement(variable=var, type_name=f"person-{i}"),
                HasStatement(
                    subject_var=var,
                    attr_name="name",
                    value=LiteralValue(f"Person {i}", "string"),
                ),
                HasStatement(
                    subject_var=var,
                    attr_name="age",
                    value=LiteralValue(20 + i, "long"),
                ),
                HasStatement(
                    subject_var=var,
                    attr_name="active",
                    value=LiteralValue(True, "boolean"),
                ),
            ]
        )
    return InsertClause(statements=statements)


@pytest.fixture
def standalone_delete() -> DeleteClause:
    """Standalone delete: 10 deletions."""
    return DeleteClause(statements=[DeleteThingStatement(variable=f"$d{i}") for i in range(10)])


@pytest.fixture
def standalone_update() -> UpdateClause:
    """Standalone update: 20 has-reassignments."""
    statements = []
    for i in range(10):
        statements.append(
            HasStatement(
                subject_var=f"$u{i}",
                attr_name="modified-at",
                value=LiteralValue("2025-12-31", "date"),
            )
        )
    for i in range(10):
        statements.append(
            HasStatement(
                subject_var=f"$u{i}",
                attr_name="status",
                value=LiteralValue("updated", "string"),
            )
        )
    return UpdateClause(statements=statements)


# ---------------------------------------------------------------------------
# Validation data — variable names
# ---------------------------------------------------------------------------


@pytest.fixture
def variable_names() -> list[str]:
    """500 variable names for batch validation."""
    names: list[str] = []
    for i in range(200):
        names.append(f"$entity-var-{i}")
    for i in range(150):
        names.append(f"$long-variable-name-for-testing-purpose-{i}")
    for i in range(100):
        names.append(f"$r{i}")
    for i in range(50):
        names.append(f"$\u00e9l\u00e8ve-{i}")
    return names


@pytest.fixture
def type_names_5000() -> list[str]:
    """5000 type names for large batch validation."""
    names: list[str] = []
    for i in range(2000):
        names.append(f"entity-type-{i}")
    for i in range(1500):
        names.append(f"my-long-attribute-name-for-testing-{i}")
    for i in range(1000):
        names.append(f"relation-{i}-data")
    for i in range(500):
        names.append(f"\u00e9l\u00e8ve-{i}")
    return names
