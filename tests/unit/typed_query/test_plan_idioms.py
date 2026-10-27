"""Context recording and shared-builder equality; generated signatures have acceptance checks."""

from __future__ import annotations

import asyncio
import json
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any

import pytest
from type_bridge_core import QueryLimits, QueryV2Authority, query_v2_authority

from type_bridge.plan import FieldToken, Handle, Plan, PlanError, Value, _match, literal

FIXTURE = json.loads(
    (Path(__file__).parents[2] / "fixtures/plan_builder/purchase.json").read_text()
)


class Token(FieldToken[Any]):
    owner: type[Any]
    fact: Mapping[str, object]

    def __init__(self, owner: type[Any], attribute: str) -> None:
        object.__setattr__(self, "owner", owner)
        object.__setattr__(
            self, "fact", {"id": {"owner": json.loads(owner.__type_id__), "attribute": attribute}}
        )


class Item:
    __type_id__ = '{"kind":"entity","label":"item"}'
    # Recording-only fixture; generated/native acceptance covers model hydration.
    __runtime_projection__ = None
    __plan_roles__: list[object] = []
    item_id: Token
    stock: Token


Item.item_id = Token(Item, "item-id")
Item.stock = Token(Item, "stock")


class ItemHandle(Handle[Item]):
    @property
    def stock(self) -> Value[int]:
        return self._value(Item.stock)

    def set_stock(self, value: Any) -> Plan[list[Item]]:
        return self._set({"stock": value})


@pytest.fixture
def authority() -> QueryV2Authority:
    return query_v2_authority(
        bytes.fromhex(FIXTURE["declared_hex"]), FIXTURE["scope"], FIXTURE["profile"]
    )


def item(authority: QueryV2Authority, key: str = "I1") -> ItemHandle:
    return _match(Item, ItemHandle, authority, "item_id", key, {})


def test_context_and_implicit_update_equal_the_explicit_core_form(
    authority: QueryV2Authority,
) -> None:
    with Plan():
        bound = item(authority).where(Item.stock > 0)
        updated = bound.set_stock(bound.stock - 1)
    explicit = Plan.builder()
    root = explicit.exact(Item)
    key = explicit.field(root, Item.item_id)
    key_input = explicit.input("item_key", "string")
    explicit.where_(key.eq(key_input))
    stock = explicit.field(root, Item.stock)
    minimum = explicit.input("item_stock_2", "long")
    explicit.where_(stock.gt(minimum))
    decrement = explicit.input("item_stock_3", "long")
    next_stock = explicit.let_("item_stock_4", stock - decrement)
    explicit.update(root, Item.stock, next_stock).fetch(root)
    explicit.row([key_input.value("I1"), minimum.value(0), decrement.value(1)])
    assert updated.canonical_bytes() == explicit.build(authority).canonical_bytes()
    assert len(json.loads(updated.canonical_bytes())["bindings"]) == 4


def test_held_delete_snapshot_equals_fetch_before_delete(authority: QueryV2Authority) -> None:
    with Plan():
        deleted = item(authority).delete_returning()
    explicit = Plan.builder()
    root = explicit.exact(Item)
    key = explicit.field(root, Item.item_id)
    value = explicit.input("item_key", "string")
    explicit.where_(key.eq(value)).fetch(root).delete(root).row([value.value("I1")])
    assert deleted.canonical_bytes() == explicit.build(authority).canonical_bytes()


def test_ignored_writes_do_not_select_a_result_for_deleted_bindings(
    authority: QueryV2Authority,
) -> None:
    with Plan():
        bound = item(authority)
        bound.set_stock(2)
        removed = bound.delete()
    plan = json.loads(removed.canonical_bytes())
    assert plan["operation"] == {"kind": "write", "returns": "matched"}
    assert not any(stage["kind"] == "fetch" for stage in plan["pipeline"])


def test_context_reset_on_nested_exception_and_after_exit(authority: QueryV2Authority) -> None:
    with pytest.raises(PlanError, match="plan_context_missing"):
        item(authority)
    with Plan() as outer:
        bound = item(authority)
        with pytest.raises(RuntimeError, match="abort"):
            with Plan():
                item(authority, "inner")
                raise RuntimeError("abort")
        result = outer.count()
        assert bound.stock is bound.stock
    assert result.canonical_bytes()
    with pytest.raises(PlanError, match="plan_context_missing"):
        bound.delete()


def test_async_tasks_keep_independent_plan_contexts(authority: QueryV2Authority) -> None:
    async def build(key: str) -> bytes:
        with Plan() as plan:
            item(authority, key)
            await asyncio.sleep(0)
            count = plan.count()
        return count.canonical_bytes()

    async def together() -> Sequence[bytes]:
        return await asyncio.gather(build("first"), build("second"))

    first, second = map(json.loads, asyncio.run(together()))
    assert first["given_rows"][0][0]["value"] == "first"
    assert second["given_rows"][0][0]["value"] == "second"


def test_conditions_refuse_accidental_python_truth_tests(authority: QueryV2Authority) -> None:
    with Plan():
        bound = item(authority)
        with pytest.raises(TypeError, match="where"):
            bool(bound.stock > 0)
        bound.set_stock(literal("long", 1))


def test_an_aborted_context_cannot_be_reentered(authority: QueryV2Authority) -> None:
    plan: Plan[object] = Plan()
    with pytest.raises(RuntimeError, match="abort"):
        with plan:
            item(authority)
            raise RuntimeError("abort")
    with pytest.raises(PlanError, match="plan_already_built"):
        with plan:
            pass


def test_entity_bulk_equals_explicit_inputs_and_rows(authority: QueryV2Authority) -> None:
    from type_bridge.plan import _bulk_insert

    with Plan():
        bulk = _bulk_insert(
            Item, authority, [{"stock": 2, "item_id": "I1"}, {"item_id": "I2", "stock": 3}]
        )
    p = Plan.builder()
    created = p.insert(Item)
    key = p.input("item_item_id", "string")
    stock = p.input("item_stock", "long")
    created.field(Item.item_id, key).field(Item.stock, stock)
    p.row([key.value("I1"), stock.value(2)]).row([key.value("I2"), stock.value(3)])
    p.fetch(created.thing())
    assert bulk.canonical_bytes() == p.build(authority).canonical_bytes()


def test_bulk_ownership_metadata_is_lowered_once_per_field(
    authority: QueryV2Authority, monkeypatch: pytest.MonkeyPatch
) -> None:
    import type_bridge.plan as module

    original = getattr(module, "_owns_json")
    ownerships: list[str] = []

    def record(token: Any) -> str:
        result = original(token)
        ownerships.append(result)
        return result

    monkeypatch.setattr(module, "_owns_json", record)
    with Plan():
        bulk = module._bulk_insert(
            Item, authority, ({"stock": 2, "item_id": f"I{index}"} for index in range(10_000))
        )
    assert bulk.writes()
    assert len(ownerships) == 2
    assert len(set(ownerships)) == 2


def test_empty_and_mixed_shape_bulk_are_not_a_single_wire_plan(authority: QueryV2Authority) -> None:
    from type_bridge.plan import _bulk_insert

    for rows in [[], [{"item_id": "I1", "stock": 2}, {"item_id": "I2"}]]:
        with Plan():
            bulk = _bulk_insert(Item, authority, rows)
        assert bulk.writes()
        with pytest.raises(PlanError, match="plan_batch_split"):
            bulk.canonical_bytes()


def test_cached_fields_still_check_the_tokens_nominal_owner(authority: QueryV2Authority) -> None:
    class OtherItem(Item):
        pass

    with Plan() as p:
        bound = item(authority)
        assert bound.stock is bound.stock
        wrong_owner = Token(OtherItem, "stock")
        with pytest.raises(TypeError, match="does not belong"):
            bound.where(wrong_owner > 0)
        counted = p.count()
    assert counted.canonical_bytes()


def test_explicit_template_rebinds_without_mutating_the_original(
    authority: QueryV2Authority,
) -> None:
    with Plan.template() as template:
        minimum = template.input("minimum", "long")
        _match(Item, ItemHandle, authority, "item_id", literal("string", "I1"), {}).where(
            Item.stock > minimum
        )
        counted = template.count()
    first = counted.bind([[minimum.value(2)]])
    second = counted.bind([[minimum.value(3)]])
    assert first.canonical_bytes() != second.canonical_bytes()
    assert counted.bind([[minimum.value(2)]]).canonical_bytes() == first.canonical_bytes()
    assert Plan(first).bind([[minimum.value(3)]]).canonical_bytes() == second.canonical_bytes()
    unrelated = Plan.builder().input("minimum", "long")
    with pytest.raises(TypeError, match="does not belong"):
        counted.bind([[unrelated.value(2)]])


def test_optional_trailing_input_can_be_absent(authority: QueryV2Authority) -> None:
    p = Plan.builder()
    p.exact(Item)
    required = p.input("minimum", "long")
    p.optional_input("unused", "string")
    p.row([required.value(2)]).count()
    original = p.build(authority)
    assert original.bind([[required.value(2)]]).canonical_bytes() == original.canonical_bytes()


def test_built_template_refuses_mutation(authority: QueryV2Authority) -> None:
    with Plan.template() as template:
        item(authority)
    for operation in [
        lambda: template.input("late", "long"),
        lambda: template.optional_input("late", "long"),
        lambda: template.row([]),
    ]:
        with pytest.raises(PlanError, match="plan_already_built"):
            operation()


@pytest.mark.parametrize("template", [False, True])
def test_recording_captures_selected_query_limits(
    authority: QueryV2Authority, template: bool
) -> None:
    limits = QueryLimits(canonical_bytes=512, canonical_string_bytes=512)
    recording = Plan.template(query_limits=limits) if template else Plan(query_limits=limits)
    with pytest.raises(ValueError, match="canonical_json_too_large"):
        with recording:
            item(authority)


def test_scalar_returning_uses_core_column_order_and_host_decoding(
    authority: QueryV2Authority,
) -> None:
    from type_bridge.plan import Rows

    with Plan() as p:
        bound = item(authority)
        selected = p.returning(bound.stock)
    wire = json.loads(selected.canonical_bytes())
    column = wire["output"]["columns"][0]
    name = wire["bindings"][column]["variable"]
    decoder = selected._scalar_rows
    assert decoder(Rows([{name: {"category": "value", "value_type": "integer", "value": 7}}])) == [
        (7,)
    ]
    assert decoder(Rows([{name: {"kind": "value", "value": {"kind": "long", "value": "7"}}}])) == [
        (7,)
    ]
    assert decoder(Rows([])) == []
    with pytest.raises(PlanError):
        decoder(
            Rows([{name: {"category": "value", "value_type": "string", "value": "wrong domain"}}])
        )
