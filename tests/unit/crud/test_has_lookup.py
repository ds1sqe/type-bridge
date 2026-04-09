"""Unit tests for cross-type attribute lookup (has_lookup)."""

from __future__ import annotations

import pytest

from type_bridge import (
    Entity,
    Flag,
    Integer,
    Key,
    Relation,
    Role,
    String,
    TypeFlags,
)
from type_bridge.models.base import TypeDBType
from type_bridge.models.registry import ModelRegistry

# ── Test models ─────────────────────────────────────────────────────


class SharedName(String):
    pass


class Region(String):
    pass


class Salary(Integer):
    pass


class LookupPerson(Entity):
    flags = TypeFlags(name="lookup_person")
    name: SharedName = Flag(Key)


class LookupCompany(Entity):
    flags = TypeFlags(name="lookup_company")
    name: SharedName = Flag(Key)
    region: Region | None = None


class LookupEmployment(Relation):
    flags = TypeFlags(name="lookup_employment")
    employee: Role[LookupPerson] = Role("employee", LookupPerson)
    employer: Role[LookupCompany] = Role("employer", LookupCompany)
    name: SharedName | None = None
    region: Region | None = None
    salary: Salary | None = None


# ── ModelRegistry reverse index ─────────────────────────────────────


class TestAttributeOwners:
    def test_shared_attribute_has_all_owners(self):
        owners = ModelRegistry.get_attribute_owners(SharedName)
        owner_names = {c.__name__ for c in owners}
        assert "LookupPerson" in owner_names
        assert "LookupCompany" in owner_names
        assert "LookupEmployment" in owner_names

    def test_entity_only_attribute(self):
        owners = ModelRegistry.get_attribute_owners(Salary)
        owner_names = {c.__name__ for c in owners}
        assert "LookupEmployment" in owner_names
        assert "LookupPerson" not in owner_names

    def test_returns_copy(self):
        """Modifying the returned set must not affect the registry."""
        owners = ModelRegistry.get_attribute_owners(SharedName)
        owners.clear()
        assert len(ModelRegistry.get_attribute_owners(SharedName)) >= 3

    def test_unknown_attribute_returns_empty(self):
        class Unused(String):
            pass

        assert ModelRegistry.get_attribute_owners(Unused) == set()


# ── Attribute.get_owners() ──────────────────────────────────────────


class TestAttributeGetOwners:
    def test_get_owners_returns_owning_models(self):
        owners = SharedName.get_owners()
        owner_names = {c.__name__ for c in owners}
        assert "LookupPerson" in owner_names
        assert "LookupCompany" in owner_names
        assert "LookupEmployment" in owner_names

    def test_get_owners_salary(self):
        owners = Salary.get_owners()
        owner_names = {c.__name__ for c in owners}
        assert "LookupEmployment" in owner_names
        assert "LookupPerson" not in owner_names


# ── Query string generation ────────────────────────────────────────


class TestQueryGeneration:
    """Verify the real ``_build_has_query`` builder, not a mirror.

    Calling the production builder directly is intentional: a duplicated
    mirror in the tests is exactly how the original concrete-class narrowing
    bug shipped without anyone noticing.
    """

    def test_entity_no_value(self):
        from type_bridge.crud.has_lookup import _build_has_query

        q = _build_has_query(SharedName, kind="entity")
        assert "entity $e" in q
        assert "has SharedName $n" in q
        assert "label($e)" in q

    def test_entity_exact_match(self):
        from type_bridge.crud.has_lookup import _build_has_query

        q = _build_has_query(SharedName, "Alice", kind="entity")
        assert 'has SharedName "Alice"' in q

    def test_entity_with_attribute_instance(self):
        from type_bridge.crud.has_lookup import _build_has_query

        q = _build_has_query(SharedName, SharedName("Alice"), kind="entity")
        assert 'has SharedName "Alice"' in q

    def test_relation_kind(self):
        from type_bridge.crud.has_lookup import _build_has_query

        q = _build_has_query(SharedName, kind="relation")
        assert "relation $r" in q
        assert "isa $r" in q
        assert "label($r)" in q

    def test_comparison_expression_no_duplicate_has(self):
        """Expression path should NOT emit a redundant has clause."""
        from type_bridge.crud.has_lookup import _build_has_query

        expr = SharedName.gt(SharedName("B"))
        q = _build_has_query(SharedName, expr, kind="entity")
        # The expression generates its own `has SharedName $x__sharedname`
        assert "$x isa $e" in q
        assert '> "B"' in q
        # Must NOT have the hardcoded `has SharedName $n` — only the expression's has
        assert "has SharedName $n" not in q

    def test_integer_exact_match(self):
        from type_bridge.crud.has_lookup import _build_has_query

        q = _build_has_query(Salary, 120000, kind="relation")
        assert "has Salary 120000" in q

    # ── Concrete-class narrowing ────────────────────────────────────

    def test_concrete_entity_narrows_to_type_name(self):
        """Passing type_name must restrict the match to that concrete type.

        Narrowed form uses ``$t sub <type_name>; $x isa! $t`` so that
        ``label($t)`` can recover the most-specific subtype label.
        ``label($x)`` is illegal in TypeDB 3 because $x is an Object variable.
        """
        from type_bridge.crud.has_lookup import _build_has_query

        q = _build_has_query(
            SharedName,
            "Alice",
            kind="entity",
            type_name="lookup_person",
        )
        # Narrowed match: $t sub lookup_person; $x isa! $t
        assert "$t sub lookup_person" in q
        assert "$x isa! $t" in q
        assert 'has SharedName "Alice"' in q
        # Must NOT bind a kind variable
        assert "entity $e" not in q
        # Label must come from the type variable $t, not from $x or a kind var
        assert "label($t)" in q
        assert "label($e)" not in q
        assert "label($x)" not in q

    def test_concrete_relation_narrows_to_type_name(self):
        """Concrete relation narrowing emits $t sub <type_name>; $x isa! $t."""
        from type_bridge.crud.has_lookup import _build_has_query

        q = _build_has_query(
            SharedName,
            value=None,
            kind="relation",
            type_name="lookup_employment",
        )
        assert "$t sub lookup_employment" in q
        assert "$x isa! $t" in q
        assert "has SharedName $n" in q
        assert "relation $r" not in q
        assert "label($t)" in q
        assert "label($r)" not in q

    def test_base_entity_query_stays_cross_type(self):
        """Regression guard: type_name=None preserves the cross-type form."""
        from type_bridge.crud.has_lookup import _build_has_query

        q = _build_has_query(SharedName, kind="entity", type_name=None)
        assert "entity $e" in q
        assert "label($e)" in q

    def test_base_relation_query_stays_cross_type(self):
        """Regression guard: type_name=None preserves the cross-type relation form."""
        from type_bridge.crud.has_lookup import _build_has_query

        q = _build_has_query(SharedName, kind="relation", type_name=None)
        assert "relation $r" in q
        assert "label($r)" in q

    def test_concrete_narrowing_with_comparison_expression(self):
        """Narrowed expression path still avoids the duplicate has clause."""
        from type_bridge.crud.has_lookup import _build_has_query

        expr = SharedName.gt(SharedName("B"))
        q = _build_has_query(
            SharedName,
            expr,
            kind="entity",
            type_name="lookup_person",
        )
        assert "$t sub lookup_person" in q
        assert "$x isa! $t" in q
        assert '> "B"' in q
        # Expression path must not emit the hardcoded `has ... $n` clause
        assert "has SharedName $n" not in q
        # And must not fall back to the kind variable
        assert "entity $e" not in q


# ── TypeDBType.has() kind detection & guards ────────────────────────


class TestHasKindDetection:
    def test_entity_base_is_entity_kind(self):
        from type_bridge.models.entity import Entity as EntityCls

        assert issubclass(EntityCls, Entity)

    def test_relation_base_is_relation_kind(self):
        from type_bridge.models.relation import Relation as RelationCls

        assert issubclass(RelationCls, Relation)

    def test_entity_subclass_is_entity_kind(self):
        assert issubclass(LookupPerson, Entity)
        assert not issubclass(LookupPerson, Relation)

    def test_relation_subclass_is_relation_kind(self):
        assert issubclass(LookupEmployment, Relation)

    def test_typedbtype_has_raises_typeerror(self):
        """Calling has() directly on TypeDBType must raise TypeError."""
        with pytest.raises(TypeError, match="must be called on Entity or Relation"):
            TypeDBType.has(object(), SharedName)  # type: ignore[arg-type]


# ── Hydration edge cases ───────────────────────────────────────────


class TestHydrateResults:
    def test_empty_results(self):
        from type_bridge.crud.has_lookup import _hydrate_results

        assert _hydrate_results([], ModelRegistry, connection=None) == []

    def test_missing_type_label_skipped(self):
        from type_bridge.crud.has_lookup import _hydrate_results

        results = [{"_iid": "0x1", "_type": None, "attributes": {"SharedName": "Alice"}}]
        assert _hydrate_results(results, ModelRegistry, connection=None) == []

    def test_empty_type_label_skipped(self):
        from type_bridge.crud.has_lookup import _hydrate_results

        results = [{"_iid": "0x1", "_type": "", "attributes": {"SharedName": "Alice"}}]
        assert _hydrate_results(results, ModelRegistry, connection=None) == []

    def test_unregistered_type_skipped(self):
        from type_bridge.crud.has_lookup import _hydrate_results

        results = [
            {"_iid": "0x1", "_type": "no_such_type_xyz", "attributes": {"SharedName": "Alice"}}
        ]
        assert _hydrate_results(results, ModelRegistry, connection=None) == []

    def test_iid_none_not_set(self):
        from type_bridge.crud.has_lookup import _hydrate_results

        results = [{"_iid": None, "_type": "lookup_person", "attributes": {"SharedName": "Alice"}}]
        instances = _hydrate_results(results, ModelRegistry, connection=None)
        assert len(instances) == 1
        assert instances[0]._iid is None

    def test_iid_dict_unwrapped(self):
        from type_bridge.crud.has_lookup import _hydrate_results

        results = [
            {
                "_iid": {"value": "0xABC"},
                "_type": "lookup_person",
                "attributes": {"SharedName": "Alice"},
            }
        ]
        instances = _hydrate_results(results, ModelRegistry, connection=None)
        assert instances[0]._iid == "0xABC"

    def test_entity_hydration(self):
        from type_bridge.crud.has_lookup import _hydrate_results

        results = [
            {
                "_iid": "0x1",
                "_type": "lookup_person",
                "attributes": {"SharedName": "Alice"},
            }
        ]
        instances = _hydrate_results(results, ModelRegistry, connection=None)
        assert len(instances) == 1
        assert isinstance(instances[0], LookupPerson)
        assert instances[0].name.value == "Alice"

    def test_relation_hydration_without_role_players(self):
        # Stub LookupEmployment.manager so the relation path can route through
        # manager.get(_iid=...) without touching a real database. Returns a
        # pre-built relation that pretends to have role players already set.
        from unittest.mock import MagicMock

        from type_bridge.crud.has_lookup import _hydrate_results

        prebuilt = LookupEmployment(name=SharedName("Engineer"), salary=Salary(100000))
        fake_manager = MagicMock()
        fake_manager.get = MagicMock(return_value=[prebuilt])

        original_manager = LookupEmployment.manager
        LookupEmployment.manager = classmethod(lambda cls, conn: fake_manager)  # type: ignore[method-assign]
        try:
            results = [
                {
                    "_iid": "0x2",
                    "_type": "lookup_employment",
                    "attributes": {"SharedName": "Engineer", "Salary": 100000},
                }
            ]
            instances = _hydrate_results(results, ModelRegistry, connection=object())
        finally:
            LookupEmployment.manager = original_manager  # type: ignore[method-assign]

        assert len(instances) == 1
        assert instances[0] is prebuilt

    def test_relation_path_uses_manager_get_for_role_players(self):
        """Relations must route through manager.get(_iid=...) for role players.

        This pins the contract that ``_hydrate_results`` delegates relation
        hydration to the existing relation manager (which already extracts
        role players via ``crud/role_players.py``).
        """
        from unittest.mock import MagicMock

        from type_bridge.crud.has_lookup import _hydrate_results

        prebuilt = LookupEmployment(name=SharedName("Mocked"))
        fake_manager = MagicMock()
        fake_manager.get = MagicMock(return_value=[prebuilt])

        original_manager = LookupEmployment.manager
        LookupEmployment.manager = classmethod(lambda cls, conn: fake_manager)  # type: ignore[method-assign]
        try:
            results = [
                {
                    "_iid": "0xABC",
                    "_type": "lookup_employment",
                    "attributes": {"SharedName": "Engineer"},
                }
            ]
            sentinel_connection = object()
            instances = _hydrate_results(
                results,
                ModelRegistry,
                connection=sentinel_connection,
            )
        finally:
            LookupEmployment.manager = original_manager  # type: ignore[method-assign]

        # manager.get must have been called with the IID from the wildcard fetch
        fake_manager.get.assert_called_once_with(_iid="0xABC")
        assert instances == [prebuilt]

    def test_entity_path_does_not_call_manager_get(self):
        """Entities must stay single-query — no N+1 regression.

        The entity hydration path uses the wildcard ``$x.*`` payload directly
        and must NOT issue a follow-up ``manager.get(_iid=...)`` call.
        """
        from unittest.mock import MagicMock

        from type_bridge.crud.has_lookup import _hydrate_results

        fake_manager = MagicMock()
        fake_manager.get = MagicMock(
            side_effect=AssertionError("entity should not call manager.get")
        )

        original_manager = LookupPerson.manager
        LookupPerson.manager = classmethod(lambda cls, conn: fake_manager)  # type: ignore[method-assign]
        try:
            results = [
                {
                    "_iid": "0xDEF",
                    "_type": "lookup_person",
                    "attributes": {"SharedName": "Alice"},
                }
            ]
            instances = _hydrate_results(
                results,
                ModelRegistry,
                connection=object(),
            )
        finally:
            LookupPerson.manager = original_manager  # type: ignore[method-assign]

        fake_manager.get.assert_not_called()
        assert len(instances) == 1
        assert isinstance(instances[0], LookupPerson)
        assert instances[0].name.value == "Alice"
