"""Executable FULL-SDK capability and proof baseline for #110."""

from __future__ import annotations

import json
from collections import defaultdict
from pathlib import Path, PurePosixPath
from typing import Any

ROOT = Path(__file__).parents[3]
MANIFEST = ROOT / "tests/contracts/sdk_conformance/manifest-v1.json"
SEED_INVENTORY = ROOT / "tests/fixtures/generated-only-operation-parity-inventory.json"
TYPED_QUERY_CORPUS = ROOT / "tests/contracts/typed_query/corpus-v1.json"

BINDINGS = ["python", "node", "rust", "c", "kotlin-jvm", "haskell", "go", "dotnet"]
CURRENT_BINDINGS = {"python", "node", "rust"}
IMPLEMENTATION_ORDER = ["c", "kotlin-jvm", "haskell", "go", "dotnet"]
FUTURE_BINDINGS = ["kotlin-jvm", "haskell", "go", "dotnet"]
FOUR_LIVE_BINDINGS = {"python", "node", "rust", "c"}
FOUR_LIVE_PROFILE = "current_and_c_live_future_planned"
FINAL_BROAD_PROFILE = "terminal_broad_live_future_planned"
FINAL_DISTRIBUTION_PROFILE = "standalone_distribution_offline_future_planned"
STATUSES = ["accepted_offline", "accepted_live", "gap", "planned"]
PROOF_KINDS = [
    "compile_positive",
    "compile_negative",
    "direct_runtime",
    "remote_runtime",
    "diagnostic",
    "lifecycle",
    "artifact",
]

EXPECTED_CAPABILITIES = {
    "C01": "schema.generate.atomic-multibinding",
    "C02": "model.generated-values-and-references",
    "C03": "model.projected-constraint-validation",
    "C04": "crud.entity.single",
    "C05": "crud.entity.batch-insert-put",
    "C06": "crud.relation.single",
    "C07": "crud.relation.batch-insert-put",
    "C08": "crud.entity.batch-update-delete-atomic",
    "C09": "crud.relation.batch-update-delete-atomic",
    "C10": "transaction.borrowed-read-write-lifecycle",
    "C11": "manager.single-type-filter",
    "C12": "projection.field-name-identity",
    "C13": "query.owner-iid-set-predicates",
    "C14": "query.exact-subtype-hydration",
    "C15": "query.scalar-boolean-predicates",
    "C16": "query.role-traversal-and-relation-references",
    "C17": "query.reachability-and-explicit-cross-join",
    "C18": "query.selected-output-shapes",
    "C19": "query.ordered-windowed-model-terminals",
    "C20": "query.direct-reducers-and-grouping",
    "C21": "query.remote-model-terminals-one-exchange",
    "C22": "query.remote-reply-verification-and-hydration",
    "C23": "diagnostic.remote-structured",
    "C24": "model.integer-key-polymorphic-role-optionality",
    "C25": "model.inherited-relation-role-lifecycle",
    "C26": "crud.entity.unkeyed-iid-lifecycle",
    "C27": "crud.relation.unkeyed-iid-lifecycle",
    "C28": "value.scalar-domain-comparison",
    "C29": "projection.evidence-integrity",
    "C30": "projection.token-package-fencing",
    "C31": "query.remote-reducers-and-grouping",
    "G01": "schema.ordered-distinct-collections",
    "G02": "query.schema-function-invocation",
    "G03": "runtime.complete-connection-policy",
    "G04": "runtime.database-administration",
    "G05": "runtime.cancellation",
    "G06": "runtime.timeout-and-resource-limits",
    "G07": "model.canonical-serialization",
    "G08": "diagnostic.all-workflows-structured",
    "G09": "migration.reverse-cli",
    "G10": "migration.binding-neutral-backfill",
    "G11": "migration.sdk-runtime-facade",
    "G12": "distribution.standalone-cli",
    "G13": "runtime.explicit-close",
}
FOUR_LIVE_CAPABILITY_CODES = {
    "C01",
    "C02",
    "C03",
    "C04",
    "C05",
    "C06",
    "C07",
    "C08",
    "C09",
    "C10",
    "C11",
    "C12",
    "C13",
    "C14",
    "C15",
    "C16",
    "C17",
    "C18",
    "C19",
    "C20",
    "C21",
    "C22",
    "C23",
    "C24",
    "C25",
    "C26",
    "C27",
    "C28",
    "C29",
    "C30",
    "C31",
    "G01",
    "G02",
    "G03",
    "G04",
    "G07",
    "G09",
    "G10",
    "G11",
    "G05",
    "G06",
    "G08",
    "G13",
}
FINAL_DISTRIBUTION_CAPABILITY_CODES = {"G12"}

NON_NORMATIVE_WORKFLOWS = {
    "lifecycle_hook_ordering_cancellation_and_post_failure",
    "python_filtered_callback_update_and_filtered_delete",
    "python_cross_type_attribute_owner_lookup",
    "python_key_fallback_update_delete_without_iid",
    "python_retained_raw_query_builder_generated_models",
}
CANONICAL_GAP_WORKFLOWS = {
    "entity_relation_batch_update_delete_and_atomicity",
}


def _load(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def _expanded_binding_profile(manifest: dict[str, Any], profile_name: str) -> dict[str, str]:
    profile = manifest["binding_profiles"][profile_name]
    return {binding: status for status in STATUSES for binding in profile[status]}


def _seed_binding_status(operation: dict[str, Any], binding: str) -> str:
    if operation["status"] == "uniform_unsupported":
        return "gap"
    if binding not in operation["support"]:
        return "gap"
    return operation["status"]


def test_full_sdk_manifest_has_closed_profiles_and_real_rust_owners() -> None:
    manifest = _load(MANIFEST)

    assert manifest["format"] == "typebridge.sdk-conformance/v1"
    assert manifest["baseline"] == "shared-sdk"
    assert manifest["seed_inventory"] == SEED_INVENTORY.relative_to(ROOT).as_posix()
    assert manifest["canonical_case_catalog"] == {
        "state": "accepted_shared_runtime_baseline",
        "path": "tests/contracts/sdk_conformance/sdk-v1",
    }
    assert manifest["bindings"] == BINDINGS
    assert manifest["implementation_order"] == IMPLEMENTATION_ORDER
    assert manifest["statuses"] == STATUSES
    assert "uniform_unsupported" not in manifest["statuses"]
    assert manifest["proof_kinds"] == PROOF_KINDS
    assert manifest["binding_profiles"][FOUR_LIVE_PROFILE] == {
        "accepted_offline": [],
        "accepted_live": ["python", "node", "rust", "c"],
        "gap": [],
        "planned": FUTURE_BINDINGS,
    }
    assert (
        manifest["binding_profiles"][FINAL_BROAD_PROFILE]
        == manifest["binding_profiles"][FOUR_LIVE_PROFILE]
    )
    assert manifest["binding_profiles"][FINAL_DISTRIBUTION_PROFILE] == {
        "accepted_offline": ["python", "node", "rust", "c"],
        "accepted_live": [],
        "gap": [],
        "planned": FUTURE_BINDINGS,
    }

    for name, profile in manifest["proof_profiles"].items():
        assert set(profile) == set(PROOF_KINDS), name
        for kind, disposition in profile.items():
            assert disposition == "required" or disposition.startswith("not_applicable: "), (
                name,
                kind,
                disposition,
            )
            if disposition.startswith("not_applicable: "):
                assert disposition.removeprefix("not_applicable: ").strip(), (name, kind)

    for name, profile in manifest["binding_profiles"].items():
        assert set(profile) == set(STATUSES), name
        assigned = [binding for status in STATUSES for binding in profile[status]]
        assert len(assigned) == len(set(assigned)), name
        assert set(assigned) == set(BINDINGS), name
        expected_planned = (
            set(FUTURE_BINDINGS)
            if name in {FOUR_LIVE_PROFILE, FINAL_BROAD_PROFILE, FINAL_DISTRIBUTION_PROFILE}
            else set(IMPLEMENTATION_ORDER)
        )
        assert set(profile["planned"]) == expected_planned, name

    capabilities = manifest["capabilities"]
    used_owners = {capability["owner"] for capability in capabilities}
    assert used_owners == set(manifest["owners"])
    for owner_id, owner in manifest["owners"].items():
        relative = PurePosixPath(owner["path"])
        assert not relative.is_absolute(), owner_id
        assert ".." not in relative.parts, owner_id
        assert relative.parts[:2] == ("type-bridge-core", "crates"), owner_id
        assert relative.suffix == ".rs", owner_id
        source_path = ROOT / relative
        assert source_path.is_file(), source_path
        source = source_path.read_text(encoding="utf-8")
        assert owner["symbol"] in source, (owner_id, source_path, owner["symbol"])


def test_full_sdk_capability_catalog_is_granular_and_fail_closed() -> None:
    manifest = _load(MANIFEST)
    capabilities = manifest["capabilities"]

    assert {item["code"]: item["id"] for item in capabilities} == EXPECTED_CAPABILITIES
    assert len({item["id"] for item in capabilities}) == len(capabilities)
    assert all(item["classification"] == "canonical" for item in capabilities)
    assert all(item["owner"] in manifest["owners"] for item in capabilities)
    assert all(item["proof_profile"] in manifest["proof_profiles"] for item in capabilities)
    assert all(item["binding_profile"] in manifest["binding_profiles"] for item in capabilities)
    assert {
        item["code"]
        for item in capabilities
        if item["binding_profile"] in {FOUR_LIVE_PROFILE, FINAL_BROAD_PROFILE}
    } == FOUR_LIVE_CAPABILITY_CODES

    case_ids = [case for item in capabilities for case in item["case_ids"]]
    assert len(case_ids) == len(set(case_ids))
    assert all(case.startswith("sdk.") for case in case_ids)

    for capability in capabilities:
        expanded = _expanded_binding_profile(manifest, capability["binding_profile"])
        assert set(expanded) == set(BINDINGS), capability["code"]
        assert all(expanded[target] == "planned" for target in FUTURE_BINDINGS)
        if capability["code"] in FOUR_LIVE_CAPABILITY_CODES:
            assert all(expanded[binding] == "accepted_live" for binding in FOUR_LIVE_BINDINGS)
        elif capability["code"] in FINAL_DISTRIBUTION_CAPABILITY_CODES:
            assert all(expanded[binding] == "accepted_offline" for binding in FOUR_LIVE_BINDINGS)
        else:
            assert expanded["c"] == "planned"

        if capability["code"].startswith("C"):
            assert capability["origin"] == "seed_inventory"
            assert capability["seed_operations"], capability["code"]
        else:
            assert capability["code"].startswith("G")
            assert capability["origin"] == "capability_audit"
            assert capability["seed_operations"] == []
            if capability["code"] not in (
                FOUR_LIVE_CAPABILITY_CODES | FINAL_DISTRIBUTION_CAPABILITY_CODES
            ):
                assert capability["gap_reason"].strip()
                assert all(expanded[binding] == "gap" for binding in CURRENT_BINDINGS)

        if any(expanded[binding] == "gap" for binding in CURRENT_BINDINGS):
            assert capability.get("gap_reason", "").strip(), capability["code"]


def test_seed_workflows_classify_every_existing_row_without_parity_exemptions() -> None:
    manifest = _load(MANIFEST)
    inventory = _load(SEED_INVENTORY)
    operations = {operation["id"]: operation for operation in inventory["operations"]}
    workflows = {workflow["id"]: workflow for workflow in manifest["seed_workflows"]}
    capabilities = {capability["id"]: capability for capability in manifest["capabilities"]}

    assert set(workflows) == set(operations)
    assert {
        workflow_id
        for workflow_id, workflow in workflows.items()
        if workflow["classification"] == "non_normative"
    } == NON_NORMATIVE_WORKFLOWS
    assert {
        workflow_id
        for workflow_id, workflow in workflows.items()
        if workflow["classification"] == "canonical_gap"
    } == CANONICAL_GAP_WORKFLOWS

    reverse_mapping: dict[str, set[str]] = defaultdict(set)
    for workflow_id, workflow in workflows.items():
        assert workflow["classification"] in {
            "canonical",
            "canonical_gap",
            "non_normative",
        }
        if workflow["classification"] == "non_normative":
            assert workflow["capabilities"] == []
            assert workflow["reason"].strip()
            continue

        assert workflow["capabilities"], workflow_id
        for capability_id in workflow["capabilities"]:
            assert capability_id in capabilities, (workflow_id, capability_id)
            assert capabilities[capability_id]["origin"] == "seed_inventory"
            reverse_mapping[capability_id].add(workflow_id)

    seeded_capabilities = {
        capability["id"]: capability
        for capability in manifest["capabilities"]
        if capability["origin"] == "seed_inventory"
    }
    assert set(reverse_mapping) == set(seeded_capabilities)
    for capability_id, capability in seeded_capabilities.items():
        assert set(capability["seed_operations"]) == reverse_mapping[capability_id]

        expanded = _expanded_binding_profile(manifest, capability["binding_profile"])
        for binding in CURRENT_BINDINGS:
            statuses = {
                _seed_binding_status(operations[operation_id], binding)
                for operation_id in capability["seed_operations"]
            }
            rank = {"gap": 0, "accepted_offline": 1, "accepted_live": 2}
            expected = min(statuses, key=lambda status: rank[status])
            if capability["code"] in FOUR_LIVE_CAPABILITY_CODES:
                expected = "accepted_live"
            assert expanded[binding] == expected, (
                capability["code"],
                binding,
                expanded[binding],
                expected,
            )


def test_remote_mutations_and_legacy_query_surfaces_cannot_count_as_support() -> None:
    manifest = _load(MANIFEST)
    inventory = _load(SEED_INVENTORY)
    corpus = _load(TYPED_QUERY_CORPUS)
    non_operations = {item["id"]: item for item in manifest["non_operations"]}

    assert non_operations == {
        "remote_mutations": {
            "id": "remote_mutations",
            "disposition": "outside-canonical-contract",
            "reason": "the FULL remote persona is query-only; direct writes remain the canonical mutation path",
        },
        "compat.legacy-root-surfaces": {
            "id": "compat.legacy-root-surfaces",
            "disposition": "compatibility-evidence-only",
            "reason": "the retained V1/raw query surface cannot satisfy generated-only V2 conformance",
        },
    }
    assert inventory["non_operations"][0]["id"] == "remote_mutations"
    assert any(case["id"] == "compat.legacy-root-surfaces" for case in corpus["cases"])
