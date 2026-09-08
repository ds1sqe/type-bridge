"""Frozen source-contract tests for the four-binding sdk-v2 journey."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[3]
CONTRACT_ROOT = ROOT / "tests/contracts/sdk_conformance/sdk-v2"


def _load(name: str) -> dict[str, Any]:
    value = json.loads((CONTRACT_ROOT / name).read_text(encoding="utf-8"))
    assert isinstance(value, dict)
    return value


def test_sdk_v2_report_schema_freezes_four_bindings_and_34_rows() -> None:
    schema = _load("report-schema-v2.json")

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
    assert schema["properties"]["format"] == {"const": "typebridge.sdk-conformance-report/v2"}
    assert schema["properties"]["binding"] == {"enum": ["python", "node", "rust", "c"]}
    assert schema["properties"]["results"]["minItems"] == 34
    assert schema["properties"]["results"]["maxItems"] == 34
    fixture = schema["$defs"]["fixture"]
    assert fixture["additionalProperties"] is False
    assert fixture["properties"]["id"] == {"const": "sdk-v2"}
    assert fixture["properties"]["version"] == {"const": 2}
    assert fixture["properties"]["projection_target"] == {
        "enum": ["python", "typescript", "rust", "c"]
    }


def test_sdk_v2_proof_fragment_is_same_run_source_bound_and_closed() -> None:
    schema = _load("proof-fragment-schema-v1.json")

    assert schema["$schema"] == "https://json-schema.org/draft/2020-12/schema"
    assert schema["additionalProperties"] is False
    assert schema["required"] == [
        "format",
        "binding",
        "semantic_profile",
        "run_nonce",
        "contract",
        "producer",
        "results",
    ]
    properties = schema["properties"]
    assert properties["format"] == {"const": "typebridge.sdk-v2-proof-fragment/v1"}
    assert properties["binding"] == {"enum": ["python", "node", "rust", "c"]}
    assert properties["semantic_profile"] == {"const": "typedb-3.12.1/v1"}
    assert properties["run_nonce"]["pattern"] == "^[0-9a-f]{64}$"
    assert properties["results"]["minItems"] == 1
    assert properties["results"]["maxItems"] == 16
    assert properties["results"]["uniqueItems"] is True

    contract = schema["$defs"]["contract"]
    assert contract["additionalProperties"] is False
    assert contract["required"] == ["proof_schema", "allowlist", "journey"]
    producer = schema["$defs"]["producer"]
    assert producer["additionalProperties"] is False
    assert producer["required"] == ["id", "sources"]
    result = schema["$defs"]["result"]
    assert result["additionalProperties"] is False
    assert result["required"] == [
        "observation_ref",
        "proof_kind",
        "test_id",
        "outcome",
        "observation",
    ]
    assert result["properties"]["outcome"] == {"const": "passed"}

    allowlist = _load("proof-fragment-allowlist-v1.json")
    assert allowlist["format"] == "typebridge.sdk-v2-proof-fragment-allowlist/v1"
    assert allowlist["semantic_profile"] == "typedb-3.12.1/v1"
    assert set(allowlist["bindings"]) == {"python", "node", "rust", "c"}
    expected_lanes = {
        ("cancellation_direct", "direct_runtime"),
        ("cancellation_remote", "remote_runtime"),
        ("remote_structured_diagnostic", "diagnostic"),
    }
    expected_producers = {
        "python": {
            "python.generated_remote_acceptance": {
                "sources": [
                    "type-bridge-core/crates/python/src/match_runtime.rs",
                    "type-bridge-core/crates/python/src/query_v2_model_remote_runtime.rs",
                    "type-bridge-core/crates/schema-codegen/src/python/query.py",
                    "type-bridge-core/crates/schema-codegen/tests/acceptance/runtime_check.py",
                ],
                "results": {
                    ("cancellation_remote", "remote_runtime"): (
                        "python.generated_remote_cancellation"
                    ),
                    ("remote_structured_diagnostic", "diagnostic"): (
                        "python.generated_remote_structured_diagnostic"
                    ),
                },
            },
            "python.native_direct_cancellation": {
                "sources": [
                    "type-bridge-core/crates/orm/src/match_request/selected_result_executor.rs",
                    "type-bridge-core/crates/python/src/match_runtime.rs",
                ],
                "results": {
                    ("cancellation_direct", "direct_runtime"): ("python.native_direct_cancellation")
                },
            },
        },
        "node": {
            "node.generated_remote_acceptance": {
                "sources": [
                    "type-bridge-core/crates/node/src/match_runtime.rs",
                    "type-bridge-core/crates/node/src/query_v2_model_remote_runtime.rs",
                    "type-bridge-core/crates/node/typescript/native.ts",
                    "type-bridge-core/crates/node/typescript/runtime-projection.ts",
                    "type-bridge-core/crates/schema-codegen/src/typescript/runtime.ts",
                    "type-bridge-core/crates/schema-codegen/tests/typescript_acceptance/runtime_check.mjs",
                ],
                "results": {
                    ("cancellation_remote", "remote_runtime"): (
                        "node.generated_remote_cancellation"
                    ),
                    ("remote_structured_diagnostic", "diagnostic"): (
                        "node.generated_remote_structured_diagnostic"
                    ),
                },
            },
            "node.native_direct_cancellation": {
                "sources": [
                    "type-bridge-core/crates/node/src/match_runtime.rs",
                    "type-bridge-core/crates/orm/src/match_request/selected_result_executor.rs",
                ],
                "results": {
                    ("cancellation_direct", "direct_runtime"): ("node.native_direct_cancellation")
                },
            },
        },
        "rust": {
            "type-bridge-rust.generated-query-proof": {
                "sources": ["type-bridge-core/crates/rust/src/remote.rs"],
                "results": {
                    ("cancellation_direct", "direct_runtime"): (
                        "remote::tests::sdk_v2_rust_deterministic_proof_fragment"
                    ),
                    ("cancellation_remote", "remote_runtime"): (
                        "remote::tests::sdk_v2_rust_deterministic_proof_fragment"
                    ),
                    ("remote_structured_diagnostic", "diagnostic"): (
                        "remote::tests::sdk_v2_rust_deterministic_proof_fragment"
                    ),
                },
            }
        },
        "c": {
            "type-bridge-c.query-native-proof": {
                "sources": [
                    "type-bridge-core/crates/c/include/typebridge/type_bridge.h",
                    "type-bridge-core/crates/c/src/lib.rs",
                    "type-bridge-core/crates/c/src/query.rs",
                ],
                "results": {
                    ("cancellation_direct", "direct_runtime"): (
                        "query::tests::sdk_v2_c_deterministic_proof_fragment"
                    ),
                    ("cancellation_remote", "remote_runtime"): (
                        "query::tests::sdk_v2_c_deterministic_proof_fragment"
                    ),
                    ("remote_structured_diagnostic", "diagnostic"): (
                        "query::tests::sdk_v2_c_deterministic_proof_fragment"
                    ),
                },
            }
        },
    }
    for binding, producers in allowlist["bindings"].items():
        assert [producer["id"] for producer in producers] == sorted(expected_producers[binding])
        actual_lanes: set[tuple[str, str]] = set()
        for producer in producers:
            expected = expected_producers[binding][producer["id"]]
            assert producer["sources"] == expected["sources"]
            assert producer["sources"] == sorted(set(producer["sources"]))
            actual_results = {
                (result["observation_ref"], result["proof_kind"]): result["test_id"]
                for result in producer["results"]
            }
            assert actual_results == expected["results"]
            assert list(actual_results) == sorted(actual_results)
            assert actual_lanes.isdisjoint(actual_results)
            actual_lanes.update(actual_results)
        assert actual_lanes == expected_lanes


def test_sdk_v2_authored_dataset_is_deterministic_and_cleanup_reversed() -> None:
    journey = _load("journey-v2.json")

    assert journey["format"] == "typebridge.sdk-journey/v2"
    assert journey["fixture_id"] == "sdk-v2"
    assert journey["version"] == 2
    assert journey["semantic_profile"] == "typedb-3.12.1/v1"
    assert set(journey["records"]) == {
        "people",
        "employee",
        "manager",
        "membership",
        "network_link",
    }
    people = journey["records"]["people"]
    assert [person["fields"]["identifier"]["value"] for person in people] == [
        "query-ada",
        "query-dana",
    ]
    assert people[0]["fields"]["nickname"] == {"kind": "string", "value": "Ada"}
    assert "nickname" not in people[1]["fields"]
    assert people[0]["fields"]["score"] == {"kind": "long", "value": "38"}
    assert people[1]["fields"]["score"] == {"kind": "long", "value": "45"}
    assert journey["records"]["membership"]["roles"] == {
        "member": [{"model": "person", "key": "query-ada"}]
    }
    assert journey["records"]["network_link"]["roles"] == {
        "origin": [{"model": "person", "key": "query-ada"}],
        "destination": [{"model": "person", "key": "query-dana"}],
        "participant": [
            {"model": "person", "key": "query-ada"},
            {"model": "person", "key": "query-dana"},
        ],
    }
    assert journey["cleanup_order"] == list(reversed(journey["create_order"]))


def test_sdk_v2_freezes_21_normalized_observations_without_runtime_identity() -> None:
    journey = _load("journey-v2.json")
    observations = journey["expected_observations"]

    assert set(observations) == {
        "cancellation_direct",
        "cancellation_remote",
        "entity_lifecycle",
        "exact_subtypes",
        "grouped_reducer",
        "hydrated_result",
        "model_values_and_references",
        "owner_iid_set",
        "query_resource_lifecycle",
        "relation_lifecycle",
        "remote_one_exchange",
        "remote_structured_diagnostic",
        "resource_limits",
        "roles",
        "scalar_boolean",
        "scalar_domain",
        "schema_function",
        "selection_shapes",
        "structured_query_diagnostic",
        "terminals",
        "topology",
    }
    assert observations["schema_function"] == {
        "minimum": 30,
        "nested_values": [38, 45],
        "values": [38, 45],
    }
    assert observations["grouped_reducer"] == {
        "reducers": {
            "count": 2,
            "sum": 83,
            "min": 38,
            "max": 45,
            "mean_bits": "4044c00000000000",
            "median_bits": "4044c00000000000",
            "std_bits": "4013cc8a99af5453",
        },
        "groups": {
            "binding": [
                {"model": "person", "key": "query-ada", "count": 1},
                {"model": "person", "key": "query-dana", "count": 1},
            ],
            "field": [{"key": 38, "count": 1}, {"key": 45, "count": 1}],
            "field_tuple": [
                {"key": [38, 40], "count": 1},
                {"key": [45, 40], "count": 1},
            ],
        },
    }
    assert observations["resource_limits"]["hard_maxima"] == {
        "timeout_milliseconds": 30_000,
        "items": 65_536,
        "bytes": 33_554_432,
        "graph_nodes": 65_536,
        "attribute_values": 65_536,
        "collection_members": 65_536,
        "role_players": 65_536,
        "statements": 3,
    }
    assert observations["resource_limits"]["plus_one_clamped_all"] is True
    assert observations["resource_limits"]["zero_tightening_all"] is True
    assert observations["query_resource_lifecycle"]["query"]["post_close_io_count"] == 0
    assert (
        "clone_usable_after_source_close" not in observations["query_resource_lifecycle"]["query"]
    )

    serialized = json.dumps(journey, sort_keys=True)
    for forbidden in (
        '"address"',
        '"database"',
        '"database_name"',
        '"iid"',
        '"nonce"',
        '"port"',
        '"process_id"',
        '"timestamp"',
    ):
        assert forbidden not in serialized
