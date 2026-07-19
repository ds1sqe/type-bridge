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

ACCEPTED_RELEASED_SCHEMAS = [
    pytest.param("define\n# café — résumé ✓\nentity person;\n", id="unicode-comment"),
    pytest.param(
        'define\nattribute name, value string @regex("café .* ✓");\n'
        "entity person, owns name;\n",
        id="unicode-string",
    ),
    pytest.param(
        "define attribute name, value string; entity person; "
        "entity person, owns name;",
        id="explicit-reopen",
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
]


@pytest.mark.parametrize("source", ACCEPTED_RELEASED_SCHEMAS)
def test_released_schema_generates_closed_world(source: str) -> None:
    snapshot = json.loads(core.generated_declared_descriptors_json(source))
    assert snapshot["closed_world"] is True
    assert snapshot["unsupported_constructs"] == []


@pytest.mark.parametrize(
    ("source", "recorded"),
    [
        pytest.param(
            "define attribute name, value string; entity person, owns name @cascade;",
            ["@cascade"],
            id="cascade",
        ),
        pytest.param(
            "define attribute name, value string; "
            "entity person, owns name @subkey(primary);",
            ["@subkey(primary)"],
            id="subkey",
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
