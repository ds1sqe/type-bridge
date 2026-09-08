"""Frozen source-contract tests for the four-binding sdk-v3 journey."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import yaml

ROOT = Path(__file__).resolve().parents[3]
CONTRACT_ROOT = ROOT / "tests/contracts/sdk_conformance/sdk-v3"

EXPECTED_SELECTED_PROOFS = [
    ("sdk.crud.entity-batch-insert-put", "direct_runtime", "entity_batch_insert_put"),
    (
        "sdk.crud.entity-batch-update-delete",
        "direct_runtime",
        "entity_batch_update_delete_atomic",
    ),
    (
        "sdk.crud.relation-batch-insert-put",
        "direct_runtime",
        "relation_batch_insert_put",
    ),
    (
        "sdk.crud.relation-batch-update-delete",
        "direct_runtime",
        "relation_batch_update_delete_atomic",
    ),
    ("sdk.crud.unkeyed-entity", "direct_runtime", "unkeyed_entity_iid_lifecycle"),
    (
        "sdk.crud.unkeyed-relation",
        "direct_runtime",
        "unkeyed_relation_iid_lifecycle",
    ),
    (
        "sdk.diagnostic.all-workflows",
        "diagnostic",
        "data_operation_structured_diagnostic",
    ),
    ("sdk.manager.filter", "direct_runtime", "manager_field_token_filter"),
    ("sdk.model.constraints", "diagnostic", "projected_constraint_validation"),
    (
        "sdk.model.inherited-relation-role",
        "direct_runtime",
        "inherited_relation_role_lifecycle",
    ),
    (
        "sdk.model.integer-key-polymorphic-role",
        "direct_runtime",
        "integer_key_polymorphic_role",
    ),
    (
        "sdk.projection.evidence-integrity",
        "diagnostic",
        "projection_evidence_integrity",
    ),
    ("sdk.projection.field-name-identity", "artifact", "field_name_identity"),
    (
        "sdk.projection.token-package-fencing",
        "diagnostic",
        "token_package_fencing",
    ),
    ("sdk.runtime.cancellation", "direct_runtime", "data_operation_cancellation"),
    (
        "sdk.runtime.connection-policy",
        "direct_runtime",
        "complete_connection_policy",
    ),
    ("sdk.runtime.explicit-close", "lifecycle", "data_resource_lifecycle"),
    (
        "sdk.runtime.timeout-resource-limits",
        "direct_runtime",
        "data_operation_resource_limits",
    ),
    ("sdk.schema.generate.atomic", "artifact", "atomic_multibinding_generation"),
    (
        "sdk.schema.ordered-distinct",
        "direct_runtime",
        "ordered_distinct_collections",
    ),
    (
        "sdk.transaction.borrowed",
        "lifecycle",
        "borrowed_transaction_lifecycle",
    ),
]
EXPECTED_TRANSITION_CASES = [
    "sdk.schema.generate.atomic",
    "sdk.model.constraints",
    "sdk.crud.entity-batch-insert-put",
    "sdk.crud.relation-batch-insert-put",
    "sdk.crud.entity-batch-update-delete",
    "sdk.crud.relation-batch-update-delete",
    "sdk.transaction.borrowed",
    "sdk.manager.filter",
    "sdk.projection.field-name-identity",
    "sdk.model.integer-key-polymorphic-role",
    "sdk.model.inherited-relation-role",
    "sdk.crud.unkeyed-entity",
    "sdk.crud.unkeyed-relation",
    "sdk.projection.evidence-integrity",
    "sdk.projection.token-package-fencing",
    "sdk.schema.ordered-distinct",
    "sdk.runtime.connection-policy",
]
EVIDENCE_ONLY_CASES = {
    "sdk.diagnostic.all-workflows",
    "sdk.runtime.cancellation",
    "sdk.runtime.explicit-close",
    "sdk.runtime.timeout-resource-limits",
}
PACKAGE_LANES = {
    ("projected_constraint_validation", "diagnostic"),
    ("projection_evidence_integrity", "diagnostic"),
    ("token_package_fencing", "diagnostic"),
}
DATA_LANES = {
    ("complete_connection_policy", "direct_runtime"),
    ("data_operation_cancellation", "direct_runtime"),
    ("data_operation_resource_limits", "direct_runtime"),
    ("data_operation_structured_diagnostic", "diagnostic"),
}
EXPECTED_REPORT_PRODUCERS = {
    "python": {
        "id": "python.generated-data-model-runtime-v3-live",
        "test_id": "python.generated_data_model_runtime_v3_live",
    },
    "node": {
        "id": "node.generated-data-model-runtime-v3-live",
        "test_id": "node.generated_data_model_runtime_v3_live",
    },
    "rust": {
        "id": "type-bridge-rust.generated-data-model-runtime-v3-live",
        "test_id": "rust_projection_live::generated_data_model_runtime_v3_live",
    },
    "c": {
        "id": "type-bridge-c.generated-data-model-runtime-v3-live",
        "test_id": "c_projection_live::generated_data_model_runtime_v3_live",
    },
}
COMMON_DATA_SOURCES: set[str] = {
    "type-bridge-core/crates/contract/src/sdk_diagnostic.rs",
    "type-bridge-core/crates/orm/src/execution_diagnostic.rs",
    "type-bridge-core/crates/orm/src/manager/dynamic.rs",
    "type-bridge-core/crates/orm/src/projected_crud.rs",
    "type-bridge-core/crates/orm/src/session/context.rs",
    "type-bridge-core/crates/orm/src/session/database.rs",
    "type-bridge-core/crates/orm/src/session/mod.rs",
    "type-bridge-core/crates/orm/src/session/transaction.rs",
    "type-bridge-core/crates/typedb-runtime/src/lib.rs",
}
EXPECTED_FRAGMENT_PRODUCERS: dict[str, dict[str, dict[str, Any]]] = {
    "python": {
        "python.generated-data-v3-proof": {
            "sources": COMMON_DATA_SOURCES
            | {
                "type-bridge-core/crates/python/src/orm_runtime.rs",
                "type-bridge-core/crates/python/src/runtime_projection.rs",
            },
            "lanes": DATA_LANES,
            "test_id": "runtime_projection::tests::sdk_v3_python_data_plane_fragment",
        },
        "python.generated-package-v3-proof": {
            "sources": {
                "type-bridge-core/crates/python/src/runtime_projection.rs",
                "type-bridge-core/crates/schema-codegen/src/python/runtime.py",
                "type-bridge-core/crates/schema-codegen/tests/acceptance/runtime_check.py",
            },
            "lanes": PACKAGE_LANES,
            "test_id": "python.generated_package_v3_integrity",
        },
    },
    "node": {
        "node.generated-data-v3-proof": {
            "sources": COMMON_DATA_SOURCES
            | {
                "type-bridge-core/crates/node/src/lib.rs",
                "type-bridge-core/crates/node/src/runtime_projection.rs",
            },
            "lanes": DATA_LANES,
            "test_id": "runtime_projection::tests::sdk_v3_node_data_plane_fragment",
        },
        "node.generated-package-v3-proof": {
            "sources": {
                "type-bridge-core/crates/node/src/runtime_projection.rs",
                "type-bridge-core/crates/schema-codegen/src/typescript/runtime.ts",
                "type-bridge-core/crates/schema-codegen/tests/typescript_acceptance/runtime_check.mjs",
            },
            "lanes": PACKAGE_LANES,
            "test_id": "node.generated_package_v3_integrity",
        },
    },
    "rust": {
        "type-bridge-rust.generated-data-v3-proof": {
            "sources": COMMON_DATA_SOURCES
            | {
                "type-bridge-core/crates/rust/src/entity_manager.rs",
                "type-bridge-core/crates/rust/src/relation_manager.rs",
                "type-bridge-core/crates/rust/src/session.rs",
                "type-bridge-core/crates/rust/src/transaction.rs",
                "type-bridge-core/crates/rust/src/transaction/tests.rs",
            },
            "lanes": DATA_LANES,
            "test_id": "transaction::tests::sdk_v3_rust_data_plane_fragment",
        },
        "type-bridge-rust.generated-package-v3-proof": {
            "sources": {
                "type-bridge-core/crates/rust/src/entity_codec.rs",
                "type-bridge-core/crates/rust/src/relation_codec.rs",
                "type-bridge-core/crates/rust/src/schema.rs",
                "type-bridge-core/crates/schema-codegen/tests/rust_acceptance.rs",
            },
            "lanes": PACKAGE_LANES,
            "test_id": "rust_acceptance::sdk_v3_generated_package_integrity",
        },
    },
    "c": {
        "type-bridge-c.generated-data-v3-proof": {
            "sources": COMMON_DATA_SOURCES
            | {
                "type-bridge-core/crates/c/include/typebridge/type_bridge.h",
                "type-bridge-core/crates/c/src/entity_crud.rs",
                "type-bridge-core/crates/c/src/projected_model.rs",
                "type-bridge-core/crates/c/src/relation_crud.rs",
                "type-bridge-core/crates/c/src/runtime.rs",
            },
            "lanes": DATA_LANES,
            "test_id": "runtime::tests::sdk_v3_c_data_plane_fragment",
        },
        "type-bridge-c.generated-package-v3-proof": {
            "sources": {
                "type-bridge-core/crates/c/include/typebridge/type_bridge.h",
                "type-bridge-core/crates/c/src/projected_model.rs",
                "type-bridge-core/crates/c/src/projected_token.rs",
                "type-bridge-core/crates/c/src/projected_value.rs",
                "type-bridge-core/crates/c/src/schema_package.rs",
                "type-bridge-core/crates/schema-codegen/src/c/render.rs",
                "type-bridge-core/crates/schema-codegen/tests/c_emitter.rs",
            },
            "lanes": PACKAGE_LANES,
            "test_id": "c_emitter::sdk_v3_generated_package_integrity",
        },
    },
}


def _load(name: str) -> dict[str, Any]:
    value = json.loads((CONTRACT_ROOT / name).read_text(encoding="utf-8"))
    assert isinstance(value, dict)
    return value


def test_sdk_v3_report_schema_freezes_four_bindings_and_exact_21_rows() -> None:
    schema = _load("report-schema-v3.json")

    assert schema["$schema"] == "https://json-schema.org/draft/2020-12/schema"
    assert schema["additionalProperties"] is False
    assert schema["required"] == [
        "format",
        "binding",
        "manifest",
        "catalog",
        "fixture",
        "results",
    ]
    assert schema["properties"]["format"] == {"const": "typebridge.sdk-conformance-report/v3"}
    assert schema["properties"]["binding"] == {"enum": ["python", "node", "rust", "c"]}
    assert schema["properties"]["results"]["minItems"] == 21
    assert schema["properties"]["results"]["maxItems"] == 21
    fixture = schema["$defs"]["fixture"]
    assert fixture["additionalProperties"] is False
    assert fixture["properties"]["id"] == {"const": "sdk-v3"}
    assert fixture["properties"]["version"] == {"const": 3}
    assert fixture["properties"]["projection_target"] == {
        "enum": ["python", "typescript", "rust", "c"]
    }


def test_sdk_v3_catalog_freezes_final_fingerprint_authority() -> None:
    catalog = _load("catalog-v3.json")
    manifest = json.loads(
        (ROOT / "tests/contracts/sdk_conformance/manifest-v1.json").read_text(encoding="utf-8")
    )

    assert catalog["format"] == "typebridge.sdk-catalog/v3"
    assert catalog["authority_state"] == "finalized"
    assert catalog["expected_fingerprints"] == {
        "semantic": {
            "domain": "typebridge.schema.semantic",
            "algorithm": "sha256",
            "canonicalization": "typebridge.schema-canonical-json/v1",
            "semantic_profile": "typedb-3.12.1/v1",
            "digest": "3c8d072b60c575b4c0381c1ea44088a9c18e01ed52e90730311aadccb72cd0c8",
        },
        "projections": {
            binding: {
                "domain": "typebridge.binding.projection",
                "algorithm": "sha256",
                "canonicalization": "typebridge.binding-projection/v1",
                "semantic_profile": "typedb-3.12.1/v1",
                "digest": digest,
            }
            for binding, digest in {
                "python": "61713e741c3db3179d1bcb0a6d4880d1a7cd4cc37a19cdeb19bccd81c009bcc2",
                "node": "924b685239100ddbebd3456d2b85c3543af9e950a29501af3a48b9b375fd3c47",
                "rust": "4196b70e742635adf51b61aea9b5fa4e9c2966d48d3d0410faf4db4066be2011",
                "c": "f406ca5fb5194bc516fb816aaf58fd4430a6ec39f8767cd7221618c60887fcd4",
            }.items()
        },
    }
    assert catalog["fixture"] == {
        "id": "sdk-v3",
        "version": 3,
        "semantic_profile": "typedb-3.12.1/v1",
        "schema_path": "tests/contracts/sdk_conformance/sdk-v3/schema-v3.yaml",
        "provider_schema_path": ("tests/contracts/sdk_conformance/sdk-v3/provider-3.12.1-v3.tql"),
    }
    assert catalog["report_bindings"] == ["python", "node", "rust", "c"]
    assert catalog["report_producers"] == EXPECTED_REPORT_PRODUCERS
    selected = [
        (row["case_id"], row["proof_kind"], row["observation_ref"])
        for row in catalog["selected_proofs"]
    ]
    assert selected == EXPECTED_SELECTED_PROOFS
    assert selected == sorted(selected)
    assert catalog["manifest_transition_cases"] == EXPECTED_TRANSITION_CASES
    assert {row[0] for row in selected} - set(EXPECTED_TRANSITION_CASES) == EVIDENCE_ONLY_CASES
    assert [(case["id"], case["capability_id"]) for case in catalog["cases"]] == [
        (capability["case_ids"][0], capability["id"]) for capability in manifest["capabilities"]
    ]
    selected_cases = {row[0] for row in selected}
    assert {
        case["id"] for case in catalog["cases"] if case["disposition"] == "shared_smoke"
    } == selected_cases


def test_sdk_v3_fixture_is_ordered_without_mutating_the_preserved_v1_v2_fixture() -> None:
    fixture = yaml.safe_load((CONTRACT_ROOT / "schema-v3.yaml").read_text(encoding="utf-8"))
    aliases = fixture["entities"]["person"]["owns"]["aliases"]
    participants = fixture["relations"]["network-link"]["relates"]["participant"]
    assert aliases["ordered"] is True
    assert aliases["distinct"] is True
    assert participants["ordered"] is True
    assert participants["distinct"] is True

    preserved = yaml.safe_load(
        (ROOT / "type-bridge-core/crates/schema-codegen/tests/acceptance/schema.yaml").read_text(
            encoding="utf-8"
        )
    )
    assert "ordered" not in preserved["entities"]["person"]["owns"]["aliases"]
    assert "ordered" not in preserved["relations"]["network-link"]["relates"]["participant"]
    preserved["attributes"]["nickname"] = {
        "value": {
            "type": "string",
            "regex": "^[A-Z][a-z]+$",
            "values": ["Ada", "Dana"],
        }
    }
    preserved["attributes"]["val_constrained"] = {
        "value": {"type": "integer", "range": {"min": 0, "max": 80}}
    }
    preserved["entities"]["person"]["owns"]["aliases"].update({"ordered": True, "distinct": True})
    preserved["relations"]["network-link"]["relates"]["participant"].update(
        {"ordered": True, "distinct": True}
    )
    assert fixture == preserved

    provider = (CONTRACT_ROOT / "provider-3.12.1-v3.tql").read_text(encoding="utf-8")
    preserved_provider = (
        ROOT / "type-bridge-core/crates/schema-codegen/tests/acceptance/provider-3.12.1.tql"
    ).read_text(encoding="utf-8")
    expected_provider = (
        preserved_provider.replace(
            "attribute nickname, value string;",
            'attribute nickname, value string @regex("^[A-Z][a-z]+$") @values("Ada", "Dana");',
            1,
        )
        .replace(
            "attribute val_constrained, value integer;",
            "attribute val_constrained, value integer @range(0..80);",
            1,
        )
        .replace(
            "    relates participant @card(0..3);",
            "    relates participant[] @distinct @card(0..3);",
            1,
        )
        .replace(
            "    owns aliases @unique @card(0..3),",
            "    owns aliases[] @unique @distinct @card(0..3),",
            1,
        )
    )
    assert provider == expected_provider


def test_sdk_v3_journey_freezes_21_complete_observation_shapes() -> None:
    journey = _load("journey-v3.json")

    assert journey["format"] == "typebridge.sdk-journey/v3"
    assert journey["fixture_id"] == "sdk-v3"
    assert journey["version"] == 3
    assert set(journey["records"]) == {
        "people",
        "robots",
        "counters",
        "memberships",
        "network_links",
        "interactions",
        "plain_activity",
        "event",
        "container",
    }
    assert journey["cleanup_order"] == list(reversed(journey["create_order"]))
    assert journey["records"]["plain_activity"] == {
        "ref": "plain-activity-ada",
        "model": "plain-activity",
        "roles": {"participant": [{"model": "person", "key": "data-ada"}]},
    }
    observations = journey["expected_observations"]
    assert set(observations) == {row[2] for row in EXPECTED_SELECTED_PROOFS}
    assert len(observations) == 21
    assert all(isinstance(value, dict) and value for value in observations.values())
    expected_fields = {
        "atomic_multibinding_generation": {
            "targets",
            "common_authority_identity",
            "package_identities_distinct",
            "generated_sidecars",
            "no_sidecar_runtime_dependency",
            "deterministic_rerun",
            "injected_failure",
        },
        "borrowed_transaction_lifecycle": {
            "read",
            "commit_visibility",
            "rollback_visibility",
            "poison",
            "post_rollback",
        },
        "complete_connection_policy": {
            "endpoint_input",
            "database_input",
            "credentials",
            "http_probe",
            "trust",
            "compatibility",
            "deadline_clock",
            "resources_tighten_only",
            "configuration_rejections",
            "version_rejection",
            "diagnostics_redacted",
            "close_idempotent",
        },
        "data_operation_cancellation": {
            "pre_dispatch",
            "owned_in_flight",
            "borrowed_in_flight",
            "after_commit",
        },
        "data_operation_resource_limits": {
            "dimensions",
            "absolute_deadline_reused",
            "connection_ceiling_inherited",
            "operation_policy_tightens_only",
            "representative_crossing",
            "owned_failure",
            "borrowed_failure",
        },
        "data_operation_structured_diagnostic": {
            "version",
            "representative",
            "provider_text_exposed",
            "secrets_exposed",
            "deterministic_field_order",
        },
        "data_resource_lifecycle": {
            "resources",
            "close_contract",
            "parent_child",
            "session_rules",
            "result_survival",
            "cancellation_close_idempotent",
            "projected_value_close_idempotent",
            "projected_thing_close_idempotent",
        },
        "entity_batch_insert_put": {"empty", "insert", "duplicate_key", "put", "late_failure"},
        "entity_batch_update_delete_atomic": {
            "update",
            "duplicate_target",
            "delete_failure",
            "delete_success",
        },
        "field_name_identity": {
            "generated_tokens",
            "token_identities_distinct",
            "package_branded",
            "compatibility_lookup",
            "generated_token_string_parser_used",
        },
        "inherited_relation_role_lifecycle": {
            "model",
            "inherited_relation",
            "inherited_role",
            "player_model",
            "created",
            "read_after_create",
            "role_identity_preserved",
            "deleted",
            "read_after_delete",
            "count_after_delete",
        },
        "integer_key_polymorphic_role": {
            "integer_keys",
            "relation",
            "optional_role",
            "polymorphic_players_observed",
            "present",
            "absent",
            "relation_as_player",
        },
        "manager_field_token_filter": {
            "model",
            "field_token",
            "operator_literal",
            "operator_outcomes",
            "conjunction",
            "terminals",
            "first",
            "rejections",
            "borrowed_read",
        },
        "ordered_distinct_collections": {
            "owns",
            "relates",
            "scalar_duplicate",
            "player_duplicate",
            "unordered_compatibility_default",
        },
        "projected_constraint_validation": {
            "scalar_domains",
            "rejection_families",
            "provider_enforced_families",
            "representative_diagnostic",
        },
        "projection_evidence_integrity": {
            "rejected_mutations",
            "representative_mutation",
            "diagnostic",
            "rejected_before_provider_io",
        },
        "relation_batch_insert_put": {
            "empty",
            "insert",
            "duplicate_key",
            "put",
            "late_failure",
        },
        "relation_batch_update_delete_atomic": {
            "update",
            "duplicate_target",
            "delete_failure",
            "delete_success",
        },
        "token_package_fencing": {
            "accepted_local",
            "rejections",
            "rejected_token_states",
            "provider_text_exposed",
        },
        "unkeyed_entity_iid_lifecycle": {
            "model",
            "identity_kind",
            "surface",
            "insert",
            "get_by_identity",
            "update_by_identity",
            "delete_by_identity",
            "count_after_cleanup",
        },
        "unkeyed_relation_iid_lifecycle": {
            "model",
            "identity_kind",
            "surface",
            "insert",
            "get_by_identity",
            "update_by_identity",
            "delete_by_identity",
            "count_after_cleanup",
        },
    }
    assert {name: set(value) for name, value in observations.items()} == expected_fields

    generation = observations["atomic_multibinding_generation"]
    assert generation["common_authority_identity"] == {
        "schema_source_equal": True,
        "semantic_profile": "typedb-3.12.1/v1",
        "semantic_fingerprint_equal": True,
        "resource_ledger_equal": True,
    }
    assert generation["generated_sidecars"] == []
    assert generation["no_sidecar_runtime_dependency"] is True

    constraints = observations["projected_constraint_validation"]
    assert {item["family"] for item in constraints["rejection_families"]} == {
        "abstract_constructibility",
        "allowed_values",
        "field_constructibility",
        "inherited_owns",
        "inherited_plays",
        "inherited_relates",
        "invalid_player_type",
        "key",
        "maximum_cardinality",
        "ordered_distinct_player",
        "ordered_distinct_scalar",
        "ownership_cardinality",
        "range",
        "regex",
        "required_cardinality",
        "role_cardinality",
        "role_constructibility",
        "scalar_domain",
    }
    assert all(
        item["rejected"] and item["rejected_before_provider_io"]
        for item in constraints["rejection_families"]
    )
    assert constraints["provider_enforced_families"] == [
        {
            "family": "unique",
            "projection_fact_retained": True,
            "local_preflight": "not_applicable",
            "provider_enforced": True,
        }
    ]
    assert constraints["representative_diagnostic"]["path"]
    assert constraints["representative_diagnostic"]["details"]

    for name in ("entity_batch_insert_put", "relation_batch_insert_put"):
        batch = observations[name]
        assert batch["duplicate_key"]["code"] == "duplicate_batch_key"
        assert batch["duplicate_key"]["rejected_before_provider_io"] is True
        assert batch["put"]["inserted_keys"]
        assert batch["put"]["replaced_keys"]
        assert batch["late_failure"] == {
            "rollback_completed": True,
            "committed_prefix": False,
            "persisted_keys": [],
            "published_results": 0,
        }
    for name in ("entity_batch_update_delete_atomic", "relation_batch_update_delete_atomic"):
        batch = observations[name]
        assert batch["update"]["identity_kind"] == "iid"
        assert batch["update"]["identity_preserved"] == [True, True]
        assert batch["duplicate_target"]["code"] == "duplicate_batch_target"
        assert batch["duplicate_target"]["rejected_before_provider_io"] is True
        assert batch["delete_failure"]["all_targets_remain"] is True
        assert batch["delete_failure"]["committed_prefix"] is False
        assert batch["delete_success"]["outcome"] == "unit"
        assert batch["delete_success"]["affected_count_exposed"] is False

    transaction = observations["borrowed_transaction_lifecycle"]
    assert transaction["commit_visibility"]["before_commit"]["outside_transaction_visible"] is False
    assert transaction["commit_visibility"]["after_commit"]["outside_transaction_visible"] is True
    assert (
        transaction["rollback_visibility"]["after_rollback"]["outside_transaction_visible"] is False
    )
    assert transaction["poison"]["first_cause"] == transaction["poison"]["retained_cause"]
    assert transaction["poison"]["commit_rejection"]["provider_commit_calls"] == 0
    assert transaction["post_rollback"]["state"] == "rolled_back"

    manager = observations["manager_field_token_filter"]
    assert {item["operator"]: item["normalized_keys"] for item in manager["operator_outcomes"]} == {
        "eq": ["data-ada"],
        "ne": ["data-dana"],
        "gt": ["data-dana"],
        "gte": ["data-ada", "data-dana"],
        "lt": [],
        "lte": ["data-ada"],
    }
    assert manager["conjunction"]["normalized_keys"] == ["data-dana"]
    assert manager["first"]["strict_singular"] is True
    assert manager["first"]["nonsingular_rejection"]["code"] == ("manager_first_requires_identity")
    assert {item["kind"] for item in manager["rejections"]} >= {
        "wrong_field_owner",
        "wrong_scalar_domain",
        "wrong_package",
    }
    assert manager["borrowed_read"]["final_state"] == "active"

    fields = observations["field_name_identity"]
    assert fields["generated_tokens"][0]["canonical_owner"] == "entity:person"
    assert fields["generated_tokens"][0]["canonical_attribute"] == "attribute:foo__bar"
    assert fields["compatibility_lookup"] == [
        {
            "input": "foo__bar",
            "resolved_attribute": "foo__bar",
            "operator": "eq",
            "literal_double_underscore": True,
        },
        {
            "input": "score__gte",
            "resolved_attribute": "score",
            "operator": "gte",
            "operator_suffix": True,
        },
        {
            "input": "score__gte__eq",
            "resolved_attribute": "score__gte",
            "operator": "eq",
            "literal_double_underscore": True,
        },
    ]

    integer_roles = observations["integer_key_polymorphic_role"]
    assert [item["value"] for item in integer_roles["integer_keys"]] == ["-7", "7"]
    assert len(integer_roles["present"]) == 2
    assert integer_roles["absent"]["role_present"] is False
    assert integer_roles["relation_as_player"]["preserved"] is True
    inherited = observations["inherited_relation_role_lifecycle"]
    assert inherited["model"] == "plain-activity"
    assert inherited["inherited_relation"] == "base-activity"
    assert inherited["inherited_role"] == "participant"
    assert inherited["read_after_delete"] is False
    assert inherited["count_after_delete"] == 0

    for name in ("unkeyed_entity_iid_lifecycle", "unkeyed_relation_iid_lifecycle"):
        lifecycle = observations[name]
        assert lifecycle["identity_kind"] == "iid"
        assert lifecycle["surface"] == {"key_present": False, "put_present": False}
        assert lifecycle["get_by_identity"]["found"] is True
        assert lifecycle["update_by_identity"]["identity_preserved"] is True
        assert lifecycle["delete_by_identity"]["read_after_delete"] is False
        assert lifecycle["count_after_cleanup"] == 0

    evidence = observations["projection_evidence_integrity"]
    assert "foreign" in evidence["rejected_mutations"]
    assert evidence["representative_mutation"] == {
        "evidence": "semantic_schema_fingerprint",
        "kind": "missing",
    }
    assert evidence["diagnostic"] == {
        "category": "integrity",
        "code": "projection_evidence_mismatch",
        "path": [
            {"kind": "argument", "value": "projection_evidence"},
            {"kind": "index", "value": 0},
            {
                "kind": "contract_identity",
                "value": "semantic_schema_fingerprint",
            },
        ],
        "details": {
            "actual_occurrence_count": {"kind": "count", "value": "0"},
            "expected_occurrence_count": {"kind": "count", "value": "1"},
            "foreign_package": {"kind": "boolean", "value": False},
        },
    }
    tokens = observations["token_package_fencing"]
    assert set(tokens["accepted_local"]) == {"construction", "batch", "filter", "hydration"}
    assert set(tokens["rejections"]) == {"construction", "batch", "filter", "hydration"}

    assert observations["ordered_distinct_collections"]["owns"]["hydrated"] == [
        "analyst",
        "mathematician",
    ]
    assert observations["ordered_distinct_collections"]["relates"]["hydrated"] == [
        "data-ada",
        "data-dana",
    ]
    assert (
        observations["ordered_distinct_collections"]["scalar_duplicate"][
            "rejected_before_provider_io"
        ]
        is True
    )
    assert (
        observations["ordered_distinct_collections"]["player_duplicate"][
            "rejected_before_provider_io"
        ]
        is True
    )

    connection = observations["complete_connection_policy"]
    assert connection["endpoint_input"]["credential_free"] is True
    assert connection["database_input"]["max_bytes"] == 256
    assert connection["http_probe"]["authoritative"] is True
    assert connection["credentials"]["username_max_bytes"] == 4096
    assert connection["credentials"]["password_max_bytes"] == 65536
    assert connection["trust"]["custom_root_max_bytes"] == 1048576
    assert connection["compatibility"]["detected_provider_required"] == "3.12.1"
    assert connection["configuration_rejections"]["before_network_io"] is True
    assert connection["version_rejection"]["database_provider_calls"] == 0

    cancellation = observations["data_operation_cancellation"]
    assert cancellation["owned_in_flight"]["rollback_completed"] is True
    assert cancellation["owned_in_flight"]["committed_prefix"] is False
    assert cancellation["borrowed_in_flight"]["state"] == "rollback_only"
    dimensions = observations["data_operation_resource_limits"]["dimensions"]
    assert [item["dimension"] for item in dimensions] == [
        "timeout_milliseconds",
        "items",
        "bytes",
        "graph_nodes",
        "attribute_values",
        "collection_members",
        "role_players",
        "statements",
    ]
    assert all(item["hard_boundary"] == "accepted" for item in dimensions)
    assert all(item["zero_tightening"] == "first_charge_rejected" for item in dimensions)
    assert all(item["hard_plus_one"] == "clamped_to_hard_max" for item in dimensions)

    diagnostics = observations["data_operation_structured_diagnostic"]
    assert set(diagnostics["representative"]) == {
        "input",
        "provider",
        "hydration",
        "commit_unknown",
        "close",
    }
    assert {item["category"] for item in diagnostics["representative"].values()} == {
        "invalid_input",
        "provider",
        "integrity",
        "transaction",
    }
    assert diagnostics["representative"]["commit_unknown"]["rollback_attempted"] is False
    resources = observations["data_resource_lifecycle"]
    assert {"cancellation", "projected_value", "projected_thing"} <= set(resources["resources"])
    assert resources["parent_child"]["database_close_with_transaction"] == "in_use"
    assert resources["session_rules"]["sibling_filter_usable"] is True
    assert all(resources["result_survival"].values())

    encoded = json.dumps(observations, sort_keys=True)
    assert "provider_iid" not in encoded
    assert "0x" not in encoded


def test_sdk_v3_fragment_contract_freezes_two_producers_and_seven_lanes() -> None:
    schema = _load("proof-fragment-schema-v1.json")
    assert schema["additionalProperties"] is False
    assert schema["properties"]["format"] == {"const": "typebridge.sdk-v3-proof-fragment/v1"}
    assert schema["properties"]["run_nonce"]["pattern"] == "^[0-9a-f]{64}$"
    assert schema["properties"]["results"]["minItems"] == 1
    assert schema["properties"]["results"]["maxItems"] == 4
    assert schema["$defs"]["producer"]["properties"]["sources"]["maxItems"] == 16
    assert schema["$defs"]["result"]["properties"]["proof_kind"] == {
        "enum": ["direct_runtime", "diagnostic"]
    }

    allowlist = _load("proof-fragment-allowlist-v1.json")
    assert allowlist["format"] == "typebridge.sdk-v3-proof-fragment-allowlist/v1"
    assert set(allowlist["bindings"]) == {"python", "node", "rust", "c"}
    for binding, producers in allowlist["bindings"].items():
        assert len(producers) == 2
        expected_producers = EXPECTED_FRAGMENT_PRODUCERS[binding]
        assert [producer["id"] for producer in producers] == sorted(expected_producers)
        lanes_by_producer: list[set[tuple[str, str]]] = []
        for producer in producers:
            expected = expected_producers[producer["id"]]
            assert producer["sources"] == sorted(set(producer["sources"]))
            assert set(producer["sources"]) == expected["sources"]
            assert all((ROOT / source).is_file() for source in producer["sources"]), binding
            lanes = {
                (result["observation_ref"], result["proof_kind"]) for result in producer["results"]
            }
            assert lanes == expected["lanes"]
            assert {result["test_id"] for result in producer["results"]} == {expected["test_id"]}
            assert [
                (result["observation_ref"], result["proof_kind"]) for result in producer["results"]
            ] == sorted(lanes)
            lanes_by_producer.append(lanes)
        assert set.union(*lanes_by_producer) == PACKAGE_LANES | DATA_LANES
        assert set.intersection(*lanes_by_producer) == set()
        assert sorted(map(len, lanes_by_producer)) == [3, 4]
        assert not any(
            source.endswith(("projected_batch.rs", "filter.rs", "result.rs"))
            for producer in producers
            for source in producer["sources"]
        )


def test_sdk_v3_environment_and_nonce_names_are_independent_from_v2() -> None:
    readme = (CONTRACT_ROOT / "README.md").read_text(encoding="utf-8")

    assert "TYPE_BRIDGE_SDK_REPORT_V3" in readme
    assert "TYPE_BRIDGE_SDK_V3_PROOF_RUN_NONCE" in readme
    assert "TYPE_BRIDGE_SDK_V3_PROOF_FRAGMENT" in readme
    assert "TYPE_BRIDGE_SDK_V3_PROOF_FRAGMENTS" in readme
    assert "TYPE_BRIDGE_SDK_REPORT_V2" not in readme
    assert "TYPE_BRIDGE_SDK_V2_PROOF_RUN_NONCE" not in readme
