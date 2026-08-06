"""Executable #189 generated-only application-operation parity gate."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

ROOT = Path(__file__).parents[3]
INVENTORY = ROOT / "tests/fixtures/generated-only-operation-parity-inventory.json"
BINDINGS = {"python", "node", "rust"}
REQUIRED_OPERATIONS = {
    "split_yaml_clean_generation_and_package_isolation",
    "model_construction_scalar_domains_optional_multivalue_and_references",
    "projected_attribute_and_ownership_constraint_validation",
    "entity_single_crud_iid_and_manager_terminals",
    "entity_insert_and_put_batches",
    "relation_single_crud_roles_iid_and_manager_terminals",
    "relation_insert_and_put_batches",
    "entity_relation_batch_update_delete_and_atomicity",
    "lifecycle_hook_ordering_cancellation_and_post_failure",
    "python_filtered_callback_update_and_filtered_delete",
    "python_cross_type_attribute_owner_lookup",
    "python_key_fallback_update_delete_without_iid",
    "borrowed_write_commit_rollback_and_reusable_read",
    "concise_single_type_filters_and_dunder_field_names",
    "optional_ownership_and_iid_set_predicates",
    "exact_subtype_binding_and_concrete_hydration",
    "comparison_string_boolean_and_field_predicates",
    "role_player_constraints_and_relation_references",
    "bounded_reachability_and_explicit_cross_join",
    "positional_named_collected_distinct_and_ordered_shapes",
    "ordered_windowed_one_first_rows_page_count_and_exists",
    "direct_reducers_and_grouping",
    "remote_rows_terminals_and_identical_materialization",
    "authenticated_remote_structured_diagnostics",
    "remote_reducers_fail_uniformly_before_exchange",
    "foreign_package_token_and_projection_evidence_rejection",
    "python_retained_raw_query_builder_generated_models",
    "integer_keys_polymorphic_roles_and_optional_role_survival",
    "plain_inherited_abstract_relation_role_lifecycle",
    "unkeyed_entity_iid_lifecycle_and_singular_query",
    "scalar_domain_equality_and_ordered_domain_predicates",
}
ALLOWED_EVIDENCE_ROOTS = {
    "python": (
        "tests/integration/schema/test_generated_projection_live.py",
        "type-bridge-core/crates/schema-codegen/tests/acceptance/",
    ),
    "node": (
        "type-bridge-core/crates/node/tests/projection-integration/generated-package-live.test.ts",
        "type-bridge-core/crates/schema-codegen/tests/typescript_acceptance/",
    ),
    "rust": (
        "type-bridge-core/crates/schema-codegen/tests/rust_projection_live.rs",
        "type-bridge-core/crates/schema-codegen/tests/rust_projection_live/",
        "type-bridge-core/crates/schema-codegen/tests/rust_acceptance.rs",
        "type-bridge-core/crates/schema-codegen/tests/rust_acceptance/",
        "type-bridge-core/crates/rust/src/remote.rs",
    ),
}


def _load() -> dict[str, Any]:
    return json.loads(INVENTORY.read_text(encoding="utf-8"))


def test_generated_only_operation_inventory_is_complete_and_source_tied() -> None:
    inventory = _load()
    assert inventory["format"] == "typebridge.generated-only-operation-parity/v1"
    assert set(inventory["bindings"]) == BINDINGS
    assert inventory["statuses"] == [
        "accepted_live",
        "accepted_offline",
        "uniform_unsupported",
    ]
    operations = inventory["operations"]
    assert {operation["id"] for operation in operations} == REQUIRED_OPERATIONS

    for operation in operations:
        assert operation["status"] in inventory["statuses"], operation["id"]
        required_bindings = set(operation.get("required_bindings", BINDINGS))
        assert required_bindings <= BINDINGS, operation["id"]
        assert set(operation["support"]) == required_bindings, operation["id"]
        if operation["status"] == "uniform_unsupported":
            assert operation["diagnostic"] == "query_remote_v2_native_only_operation"

        for binding, support in operation["support"].items():
            assert support["surface"], (operation["id"], binding)
            evidence_items = support["evidence"]
            assert evidence_items, (operation["id"], binding)
            if operation["status"] == "accepted_live":
                assert any(item["mode"] == "live" for item in evidence_items), (
                    operation["id"],
                    binding,
                )
            for evidence in evidence_items:
                assert evidence["mode"] in {"live", "offline"}, evidence
                relative = evidence["path"]
                assert relative.startswith(ALLOWED_EVIDENCE_ROOTS[binding]), (
                    operation["id"],
                    binding,
                    relative,
                )
                source_path = ROOT / relative
                assert source_path.is_file(), source_path
                source = source_path.read_text(encoding="utf-8")
                assert evidence["anchors"], evidence
                for anchor in evidence["anchors"]:
                    assert anchor in source, (
                        f"{operation['id']}: {binding}: {relative}: missing {anchor!r}"
                    )


def test_generated_only_inventory_does_not_use_handwritten_journeys_as_evidence() -> None:
    inventory = _load()
    forbidden = (
        "tests/integration/crud/",
        "tests/integration/queries/test_typed_",
        "type-bridge-core/crates/node/tests/integration/typed/",
        "type_bridge/models/",
        "_legacy",
    )
    evidence_paths = [
        evidence["path"]
        for operation in inventory["operations"]
        for support in operation["support"].values()
        for evidence in support["evidence"]
    ]
    assert all(not any(token in path for token in forbidden) for path in evidence_paths)


def test_remote_mutations_are_uniformly_not_advertised() -> None:
    inventory = _load()
    assert inventory["non_operations"] == [
        {
            "id": "remote_mutations",
            "disposition": "not_advertised",
            "bindings": {
                "python": "not_advertised",
                "node": "not_advertised",
                "rust": "not_advertised",
            },
            "reason": (
                "Generated remote sessions are query-only in every shipped binding; "
                "no remote mutation successor is promised by the cutover."
            ),
        }
    ]
