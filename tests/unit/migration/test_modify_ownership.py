"""ModifyOwnership annotation-transition lowering.

TypeDB can only ``redefine`` parameterized annotations; parameterless ones
(``@key``, ``@unique``, ``@distinct``) are added with ``define`` and removed
with ``undefine``. The historical blanket ``redefine`` lowering failed live
(REX28) for every transition involving a parameterless annotation.
"""

from __future__ import annotations

from type_bridge import Entity, Flag, Key, String, TypeFlags
from type_bridge.attribute import AttributeFlags
from type_bridge.migration import operations as ops


class MoNickname(String):
    flags = AttributeFlags(name="mo-nickname")


class MoPerson(Entity):
    flags = TypeFlags(name="mo-person")
    nickname: MoNickname = Flag(Key)


def test_card_change_still_lowers_to_redefine() -> None:
    op = ops.ModifyOwnership(
        MoPerson,
        MoNickname,
        old_annotations="@card(0..1)",
        new_annotations="@card(1..1)",
    )
    assert op.to_typeql() == "redefine\nmo-person owns mo-nickname @card(1..1);"
    assert op.to_rollback_typeql() == "redefine\nmo-person owns mo-nickname @card(0..1);"


def test_adding_a_parameterless_annotation_lowers_to_define() -> None:
    op = ops.ModifyOwnership(MoPerson, MoNickname, old_annotations="", new_annotations="@key")
    assert op.to_typeql() == "define\nmo-person owns mo-nickname @key;"
    assert op.to_rollback_typeql() == "undefine\n@key from mo-person owns mo-nickname;"


def test_removing_a_parameterless_annotation_lowers_to_undefine() -> None:
    op = ops.ModifyOwnership(MoPerson, MoNickname, old_annotations="@unique", new_annotations="")
    assert op.to_typeql() == "undefine\n@unique from mo-person owns mo-nickname;"
    assert op.to_rollback_typeql() == "define\nmo-person owns mo-nickname @unique;"


def test_mixed_transition_decomposes_into_per_annotation_steps() -> None:
    """@card(0..1) -> @key removes one kind and adds another: two schema
    queries. Removal runs first — defining @key while the conflicting
    explicit @card is still declared fails schema validation."""
    op = ops.ModifyOwnership(
        MoPerson,
        MoNickname,
        old_annotations="@card(0..1)",
        new_annotations="@key",
    )
    assert op.to_typeql_steps() == [
        "undefine\n@card from mo-person owns mo-nickname;",
        "define\nmo-person owns mo-nickname @key;",
    ]
    assert op.to_rollback_typeql_steps() == [
        "undefine\n@key from mo-person owns mo-nickname;",
        "define\nmo-person owns mo-nickname @card(0..1);",
    ]


def test_no_op_transition_lowers_to_no_steps() -> None:
    op = ops.ModifyOwnership(MoPerson, MoNickname, old_annotations="@key", new_annotations="@key")
    assert op.to_typeql_steps() == []
    assert op.to_rollback_typeql_steps() == []
