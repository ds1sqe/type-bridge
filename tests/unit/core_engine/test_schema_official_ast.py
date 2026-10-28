"""Native schema boundaries preserve archive inputs and checked failures."""

import pytest
import type_bridge_core as core


def test_archived_schema_reader_warns_and_keeps_released_only_syntax() -> None:
    source = "define attribute name value string; entity person @abstract owns name;"
    with pytest.warns(DeprecationWarning, match="schema archive reader"):
        schema = core._ArchivedTypeSchema.from_typeql(source)
    assert '"person"' in schema.to_json()


def test_checked_schema_generation_propagates_invalid_identifier() -> None:
    info = {
        "attributes": {},
        "entities": {
            "bad; entity injected": {
                "type_name": "bad; entity injected",
                "is_abstract": False,
                "parent_type": None,
                "owned_attributes": [],
                "plays_cardinalities": {},
            }
        },
        "relations": {},
    }
    with pytest.raises(ValueError, match="invalid TypeQL identifier"):
        core._archived_generate_define_block(info)


def test_native_ast_comparison_distinguishes_semantics_and_invalid_syntax() -> None:
    assert core._typeql_queries_equivalent("define entity person;", "define\nentity person;\n")
    assert not core._typeql_queries_equivalent("define entity person;", "define entity company;")
    with pytest.raises(ValueError, match="invalid TypeQL syntax"):
        core._typeql_queries_equivalent("define entity person;", "invalid query")


@pytest.mark.parametrize("player_kind", ["entities", "relations"])
@pytest.mark.parametrize(
    ("middle_maximum", "leaf_maximum"),
    [(None, None), (None, 3), (None, 1), (2, None), (2, 2), (2, 1)],
)
def test_inherited_plays_preserves_nearest_cardinality_override(
    player_kind: str, middle_maximum: int | None, leaf_maximum: int | None
) -> None:
    players = {
        name: {
            "type_name": name,
            "is_abstract": True,
            "parent_type": parent,
            "owned_attributes": [],
            "roles": [],
            "plays_cardinalities": cards,
        }
        for name, parent, cards in (
            ("base", None, {"membership:member": [0, 3]}),
            (
                "middle",
                "base",
                {} if middle_maximum is None else {"membership:member": [0, middle_maximum]},
            ),
            (
                "leaf",
                "middle",
                {} if leaf_maximum is None else {"membership:member": [0, leaf_maximum]},
            ),
        )
    }
    info = {
        "attributes": {},
        "entities": players if player_kind == "entities" else {},
        "relations": {
            **(players if player_kind == "relations" else {}),
            "membership": {
                "type_name": "membership",
                "is_abstract": False,
                "parent_type": None,
                "owned_attributes": [],
                "roles": [{"role_name": "member", "player_type_names": ["base", "middle", "leaf"]}],
                "plays_cardinalities": {},
            },
        },
    }
    rendered = core._archived_generate_define_block(info)
    assert "base, plays membership:member @card(0..3);" in rendered
    if middle_maximum is None:
        assert "middle, plays" not in rendered
    else:
        assert "middle, plays membership:member @card(0..2);" in rendered
    inherited_maximum = middle_maximum if middle_maximum is not None else 3
    if leaf_maximum in (None, inherited_maximum):
        assert "leaf, plays" not in rendered
    else:
        assert f"leaf, plays membership:member @card(0..{leaf_maximum});" in rendered
