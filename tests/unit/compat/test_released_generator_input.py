"""Released-generator-input compatibility at the FFI boundary.

Every schema here is a shape the released 1.5.x generator accepted. The
native descriptor generator must return either a snapshot or a typed
``ValueError`` — never a panic-backed exception — and must not invent
unsupported constructs from comment or string text. The exhaustive
corpus lives in the Rust crate
(``crates/schema-compat/tests/released_input_corpus.rs``); this module
pins the same guarantees where 1.5.x callers actually observe them.
"""

from __future__ import annotations

import json

import pytest
import type_bridge_core as core

EXACT_RELEASED_REGRESSIONS = [
    pytest.param(
        'define attribute email, value string, @regex("^[a-z]+$");',
        id="comma-before-regex",
    ),
    pytest.param(
        'define attribute score @regex("x"), value string;',
        id="regex-before-value",
    ),
    pytest.param(
        "define attribute score, value string @abstract;",
        id="abstract-after-value",
    ),
    pytest.param(
        "define entity person, @abstract;",
        id="comma-before-type-abstract",
    ),
    pytest.param(
        "define relation friendship, relates friend; ghost plays friendship:friend;",
        id="plays-only-shell-entity",
    ),
]

DOMAIN_INCOHERENT_RELEASED_REGRESSIONS = [
    pytest.param(
        'define attribute score, value string @regex("x"), value integer;',
        '@regex("x")',
        id="earlier-regex-final-integer",
    ),
    pytest.param(
        'define attribute status, value string @values("a", "b"), value integer;',
        '@values("a", "b")',
        id="earlier-values-final-integer",
    ),
    pytest.param(
        "define attribute score, value integer @range(1..5), value double;",
        "@range(1..5)",
        id="earlier-range-final-double",
    ),
]

CANONICAL_CONTRACT_RELEASED_REGRESSIONS = [
    pytest.param(
        "define attribute name, value string; entity person, owns name @key @card(1..1);",
        ["@key"],
        id="key-with-cardinality",
    ),
    pytest.param(
        "define attribute name, value string; entity person, owns name @key @unique;",
        ["@key"],
        id="key-with-unique",
    ),
    pytest.param(
        "define relation event, relates participant @card(0);",
        ["@card(0)"],
        id="exact-zero-cardinality",
    ),
    pytest.param(
        "define relation event, relates participant @card(0..0);",
        ["@card(0..0)"],
        id="zero-range-cardinality",
    ),
    pytest.param(
        "define attribute score, value integer @range(5..5);",
        ["@range(5..5)"],
        id="equal-range-bounds",
    ),
    pytest.param(
        "define attribute score, value integer @range(5..2);",
        ["@range(5..2)"],
        id="reversed-range-bounds",
    ),
    pytest.param(
        "define attribute score, value integer @range(..);",
        ["@range(..)"],
        id="boundless-range",
    ),
]

EMPTY_RELEASED_SCHEMAS = [
    pytest.param("", id="empty"),
    pytest.param(" \t\r\n", id="whitespace-only"),
    pytest.param("# released comment only\n", id="hash-comment-only"),
    pytest.param("// released comment only\r\n", id="slash-comment-only"),
    pytest.param("/* released comment only */", id="block-comment-only"),
]

DEFINITION_ONLY_RELEASED_SCHEMAS = [
    pytest.param(
        "define\nfun answer() -> integer:\n  return 1;\n",
        id="function-only",
    ),
    pytest.param(
        "define\nstruct payload, value note string;\n",
        id="struct-only",
    ),
    pytest.param(
        "define\nfun answer() -> integer:\n  return 1;\nstruct payload, value note string;\n",
        id="function-and-struct-only",
    ),
]

OPEN_WORLD_REFERENCE_SCHEMA = (
    "define\n"
    "entity child, sub missing-parent, owns missing-attribute @card(0..1), "
    "plays missing-relation:member;\n"
    "relation base, relates existing;\n"
    "relation specialized, sub base, relates replacement as absent;\n"
    "entity player, plays base:missing-role;\n"
    "ghost plays missing-relation:missing-role;\n"
)

OPEN_WORLD_REFERENCE_EVIDENCE = [
    "sub missing-parent",
    "owns missing-attribute @card(0..1)",
    "plays missing-relation:member",
    "relates replacement as absent",
    "plays base:missing-role",
    "plays missing-relation:missing-role",
]

ACCEPTED_RELEASED_SCHEMAS = [
    pytest.param("define\n# café — résumé ✓\nentity person;\n", id="unicode-comment"),
    pytest.param(
        'define\nattribute name, value string @regex("café .* ✓");\nentity person, owns name;\n',
        id="unicode-string",
    ),
    pytest.param(
        "define attribute name, value string; entity person; entity person, owns name;",
        id="explicit-reopen",
    ),
    pytest.param(
        'define attribute name, value string; entity person @doc("same"), owns name; '
        'define entity person @doc("same"), owns name;',
        id="repeated-reopen-facts-and-annotations",
    ),
    pytest.param(
        "define attribute name, value string; "
        "relation interaction, relates participant @card(0..2), "
        "relates participant @card(1..1); "
        "entity person, owns name @card(0..1), owns name @key, "
        "plays interaction:participant @card(0..3), "
        "plays interaction:participant @card(1..1);",
        id="repeated-facts-inside-one-declaration",
    ),
    pytest.param(
        "define relation friendship, relates friend; "
        "person plays friendship:friend; entity person;",
        id="kindless-reopen-first",
    ),
    pytest.param(
        "define\n# mention thing[] and @distinct and fun f()\nentity person;\n",
        id="stripped-syntax-in-comment",
    ),
    pytest.param(
        "define\n// mention @subkey(fake)\n/* mention @cascade */\nentity person;\n",
        id="stripped-syntax-in-slash-comments",
    ),
    *EXACT_RELEASED_REGRESSIONS,
]


@pytest.mark.parametrize("source", EMPTY_RELEASED_SCHEMAS)
def test_released_empty_input_uses_one_canonical_snapshot(source: str) -> None:
    expected = core.generated_declared_descriptors_json("")
    assert core.generated_declared_descriptors_json(source) == expected

    for target in ("python", "typescript", "rust"):
        package = json.loads(core.render_models_json(source, target, json.dumps({})))
        assert package == json.loads(core.render_models_json("", target, json.dumps({})))
        files = {file["path"]: file["contents"] for file in package["files"]}
        assert files["declared-schema.json"] == expected


@pytest.mark.parametrize("source", DEFINITION_ONLY_RELEASED_SCHEMAS)
def test_released_definition_only_input_attaches_canonical_empty_snapshot(source: str) -> None:
    expected = core.generated_declared_descriptors_json("")
    assert core.generated_declared_descriptors_json(source) == expected

    for target in ("python", "typescript", "rust"):
        package = json.loads(core.render_models_json(source, target, json.dumps({})))
        files = {file["path"]: file["contents"] for file in package["files"]}
        assert files["declared-schema.json"] == expected
        if target == "python":
            for path, contents in files.items():
                if path.endswith(".py"):
                    compile(contents, path, "exec")
            assert ("functions.py" in files) is ("fun answer" in source)
            assert ("structs.py" in files) is ("struct payload" in source)


def test_released_unresolved_references_attach_open_world_snapshot() -> None:
    encoded = core.generated_declared_descriptors_json(OPEN_WORLD_REFERENCE_SCHEMA)
    snapshot = json.loads(encoded)
    assert snapshot["closed_world"] is False
    assert snapshot["unsupported_constructs"] == OPEN_WORLD_REFERENCE_EVIDENCE
    assert snapshot["plays"] == []

    child = next(entity for entity in snapshot["entities"] if entity["label"] == "child")
    assert child["parent"] is None
    assert child["owns"] == []
    assert any(entity["label"] == "ghost" for entity in snapshot["entities"])

    for target in ("python", "typescript", "rust"):
        package = json.loads(
            core.render_models_json(OPEN_WORLD_REFERENCE_SCHEMA, target, json.dumps({}))
        )
        files = {file["path"]: file["contents"] for file in package["files"]}
        assert files["declared-schema.json"] == encoded


def test_released_unresolved_references_render_compilable_python_without_phantoms() -> None:
    package = json.loads(
        core.render_models_json(OPEN_WORLD_REFERENCE_SCHEMA, "python", json.dumps({}))
    )
    files = {file["path"]: file["contents"] for file in package["files"]}

    for path, contents in files.items():
        if path.endswith(".py"):
            compile(contents, path, "exec")

    assert "class Child(Entity):" in files["entities.py"]
    assert "missing-" not in files["entities.py"]
    assert "plays: ClassVar" not in files["entities.py"]
    assert "class Base(Relation):" in files["relations.py"]
    assert "class Specialized(Base):" in files["relations.py"]
    assert "absent" not in files["relations.py"]
    assert "replacement" not in files["relations.py"]


@pytest.mark.parametrize("source", ACCEPTED_RELEASED_SCHEMAS)
def test_released_schema_generates_closed_world(source: str) -> None:
    snapshot = json.loads(core.generated_declared_descriptors_json(source))
    assert snapshot["closed_world"] is True
    assert snapshot["unsupported_constructs"] == []


@pytest.mark.parametrize("source", EXACT_RELEASED_REGRESSIONS)
def test_released_schema_renders_python_package(source: str) -> None:
    package = json.loads(core.render_models_json(source, "python", json.dumps({})))
    files = {file["path"]: file["contents"] for file in package["files"]}
    assert "declared-schema.json" in files
    snapshot = json.loads(files["declared-schema.json"])
    assert snapshot["closed_world"] is True
    assert snapshot["unsupported_constructs"] == []
    assert "GENERATED_DECLARED_DESCRIPTORS_JSON" in files["registry.py"]


@pytest.mark.parametrize(("source", "annotation"), DOMAIN_INCOHERENT_RELEASED_REGRESSIONS)
def test_domain_incoherent_released_schema_remains_generatable(
    source: str, annotation: str
) -> None:
    snapshot = json.loads(core.generated_declared_descriptors_json(source))
    assert snapshot["closed_world"] is False
    assert snapshot["unsupported_constructs"] == [annotation]

    package = json.loads(core.render_models_json(source, "python", json.dumps({})))
    files = {file["path"]: file["contents"] for file in package["files"]}
    assert json.loads(files["declared-schema.json"])["unsupported_constructs"] == [annotation]


@pytest.mark.parametrize(("source", "evidence"), CANONICAL_CONTRACT_RELEASED_REGRESSIONS)
def test_released_annotation_contract_mismatch_remains_generatable(
    source: str, evidence: list[str]
) -> None:
    snapshot = json.loads(core.generated_declared_descriptors_json(source))
    assert snapshot["closed_world"] is False
    assert snapshot["unsupported_constructs"] == evidence

    package = json.loads(core.render_models_json(source, "python", json.dumps({})))
    files = {file["path"]: file["contents"] for file in package["files"]}
    assert json.loads(files["declared-schema.json"])["unsupported_constructs"] == evidence


def test_annotation_recovery_ignores_markers_in_comments_and_strings() -> None:
    source = (
        'define attribute note, value string @doc("literal @key @card(0)");\n'
        "attribute score, value integer @range(5..2);\n"
        "relation event, relates participant @card(0);\n"
        "entity sample, owns note @key /* @unique in comment */ @card(1..1);\n"
    )
    snapshot = json.loads(core.generated_declared_descriptors_json(source))
    assert snapshot["closed_world"] is False
    assert snapshot["unsupported_constructs"] == ["@range(5..2)", "@card(0)", "@key"]
    assert snapshot["attributes"][0]["doc"] == "literal @key @card(0)"


def test_nonportable_released_identifiers_attach_open_world_snapshot() -> None:
    long_attribute = "a" * 256
    long_relation = "r" * 256
    long_role = "p" * 256
    source = (
        f"define attribute {long_attribute}, value string;\n"
        f"relation {long_relation}, relates visible;\n"
        f"entity owner, owns {long_attribute}, plays {long_relation}:visible;\n"
        f"relation portable, relates {long_role};\n"
        f"entity player, plays portable:{long_role};\n"
    )
    snapshot = json.loads(core.generated_declared_descriptors_json(source))
    assert snapshot["closed_world"] is False
    assert snapshot["unsupported_constructs"] == [
        f"attribute {long_attribute}, value string",
        f"relation {long_relation}, relates visible",
        f"owns {long_attribute}",
        f"plays {long_relation}:visible",
        f"relates {long_role}",
        f"plays portable:{long_role}",
    ]

    package = json.loads(core.render_models_json(source, "python", json.dumps({})))
    files = {file["path"]: file["contents"] for file in package["files"]}
    assert (
        json.loads(files["declared-schema.json"])["unsupported_constructs"]
        == snapshot["unsupported_constructs"]
    )


@pytest.mark.parametrize(
    ("source", "recorded"),
    [
        pytest.param(
            "define attribute name, value string; entity person, owns name @cascade;",
            ["@cascade"],
            id="cascade",
        ),
        pytest.param(
            "define attribute name, value string; entity person, owns name @subkey(primary);",
            ["@subkey(primary)"],
            id="subkey",
        ),
        pytest.param(
            "define attribute name, value string; "
            "entity person, owns name @subkey # comment contains )\r\n ( primary );",
            ["@subkey # comment contains )\r\n ( primary )"],
            id="subkey-comment-trivia",
        ),
        pytest.param(
            "define attribute name, value string; "
            "entity person, owns name @subkey /* ) */ (primary /* ( */ );",
            ["@subkey /* ) */ (primary /* ( */ )"],
            id="subkey-block-comment-trivia",
        ),
    ],
)
def test_released_annotations_record_open_world(source: str, recorded: list[str]) -> None:
    snapshot = json.loads(core.generated_declared_descriptors_json(source))
    assert snapshot["closed_world"] is False
    assert snapshot["unsupported_constructs"] == recorded


def test_invalid_input_is_a_typed_error_not_a_panic() -> None:
    with pytest.raises(ValueError):
        core.generated_declared_descriptors_json("define\nnonsense ✓ nonsense\n")
