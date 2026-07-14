"""Offline consistency tests for the versioned unified typed-query contract."""

from __future__ import annotations

import json
import re
from pathlib import Path
from typing import Any

ROOT = Path(__file__).parents[3]
FIXTURES = ROOT / "tests" / "contracts" / "typed_query"
SCHEMA_PATH = FIXTURES / "schema-v1.json"
CORPUS_PATH = FIXTURES / "corpus-v1.json"
RESULTS_PATH = FIXTURES / "expected-results-v1.json"
CONTRACT_PATH = ROOT / "docs" / "development" / "typed-query-contract.md"
PYTHON_DOCUMENTED_EXAMPLES = FIXTURES / "python" / "documented_examples.py"
TYPESCRIPT_DOCUMENTED_EXAMPLES = (
    ROOT
    / "type-bridge-core"
    / "crates"
    / "node"
    / "tests"
    / "compat"
    / "typed-query-contract"
    / "documented-examples.typecheck.ts"
)
LIVE_PARITY_CONTRACT = (
    ROOT / "tests" / "integration" / "parity" / "fixtures" / "typed-query" / "contract.json"
)

EXAMPLE_MARKER = re.compile(r"<!-- typed-query-example: ([a-z0-9-]+) -->")
EXAMPLE_BLOCK = re.compile(
    r"<!-- typed-query-example: ([a-z0-9-]+) -->\s*"
    r"```(python|typescript)\n(.*?)\n```",
    re.DOTALL,
)
CASE_ID = re.compile(r"^[a-z][a-z0-9-]*\.[a-z0-9-]+$")


def _load(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def _nonblank_source(source: str) -> str:
    return "\n".join(line.rstrip() for line in source.splitlines() if line.strip())


def test_semantic_corpus_conforms_to_schema_v1() -> None:
    schema = _load(SCHEMA_PATH)
    corpus = _load(CORPUS_PATH)

    assert schema["schema_id"] == "type-bridge.typed-query.semantic-corpus"
    assert schema["schema_version"] == corpus["schema_version"] == corpus["version"] == 1
    assert set(schema["required_top_level"]) <= corpus.keys()
    assert corpus["languages"] == schema["enums"]["language"]

    required_case = set(schema["required_case_fields"])
    required_expected = set(schema["required_expected_fields"])
    case_ids: set[str] = set()

    for case in corpus["cases"]:
        assert required_case <= case.keys(), case["id"]
        assert required_expected <= case["expected"].keys(), case["id"]
        assert CASE_ID.fullmatch(case["id"]), case["id"]
        assert case["id"] not in case_ids, case["id"]
        case_ids.add(case["id"])

        assert case["description"].strip()
        assert case["features"]
        assert case["operation"] in schema["enums"]["operation"]
        assert case["transaction_mode"] in schema["enums"]["transaction_mode"]
        assert isinstance(case["input"], dict)

        expected = case["expected"]
        assert expected["outcome"] in schema["enums"]["outcome"]
        assert expected["identity_basis"] in schema["enums"]["identity_basis"]
        assert expected["multiplicity"] in schema["enums"]["multiplicity"]
        assert isinstance(expected["zero_executor_invocations"], bool)
        assert isinstance(expected["zero_data_statements"], bool)
        if expected["zero_executor_invocations"]:
            assert expected["zero_data_statements"], case["id"]

        if expected["outcome"] == "success":
            assert expected["error_category"] is None, case["id"]
            assert expected["error_code"] is None, case["id"]
        else:
            assert expected["error_category"] in schema["enums"]["error_category"]
            assert isinstance(expected["error_code"], str) and expected["error_code"]

        for language in schema["enums"]["language"]:
            assert case["examples"][language], (case["id"], language)


def test_semantic_corpus_covers_the_normative_vocabulary() -> None:
    schema = _load(SCHEMA_PATH)
    cases = _load(CORPUS_PATH)["cases"]
    required = schema["required_coverage"]

    assert set(required["operations"]) <= {case["operation"] for case in cases}
    assert set(required["transaction_modes"]) <= {case["transaction_mode"] for case in cases}
    assert set(required["identity_bases"]) <= {case["expected"]["identity_basis"] for case in cases}
    assert set(required["error_categories"]) <= {
        case["expected"]["error_category"]
        for case in cases
        if case["expected"]["error_category"] is not None
    }
    assert set(required["features"]) <= {feature for case in cases for feature in case["features"]}
    assert set(required["selection_arities"]) <= {
        case["input"]["selection_arity"] for case in cases if "selection_arity" in case["input"]
    }

    invalid_plans = [case for case in cases if case["expected"]["error_category"] == "invalid_plan"]
    assert invalid_plans
    target_preflight = next(
        case for case in invalid_plans if case["id"] == "transaction.non-read-context"
    )
    assert not target_preflight["expected"]["zero_executor_invocations"]
    assert target_preflight["expected"]["zero_data_statements"]
    construction_errors = [case for case in invalid_plans if case is not target_preflight]
    assert all(case["expected"]["zero_executor_invocations"] for case in construction_errors)
    assert all(case["expected"]["zero_data_statements"] for case in construction_errors)

    capability = next(case for case in cases if case["id"] == "preflight.missing-capability")
    stale = next(case for case in cases if case["id"] == "preflight.stale-schema")
    for case in (capability, stale):
        assert not case["expected"]["zero_executor_invocations"]
        assert case["expected"]["zero_data_statements"]


def test_error_cases_pin_real_boundary_and_canonical_codes() -> None:
    cases = _load(CORPUS_PATH)["cases"]
    actual = {
        case["id"]: (case["expected"]["error_category"], case["expected"]["error_code"])
        for case in cases
        if case["expected"]["outcome"] == "error"
    }

    assert actual == {
        "selection.duplicate-handle": ("invalid_plan", "duplicate_selection"),
        "selection.seventeen-slot-rejection": ("invalid_plan", "selection_cap_exceeded"),
        "topology.disconnected": ("invalid_plan", "disconnected_plan"),
        "boolean.or-definite-binding": ("invalid_plan", "partial_or_binding"),
        "boolean.not-unattached-reference": ("invalid_plan", "unattached_binding"),
        "references.cross-owner-field": ("invalid_plan", "cross_owner_field"),
        "references.cross-owner-role": ("invalid_plan", "cross_owner_role"),
        "references.incompatible-role-player": ("invalid_plan", "incompatible_role_player"),
        "shape.page-non-root-singular": ("invalid_plan", "singular_non_root_page_slot"),
        "bounds.public-invalid-limit": ("invalid_plan", "invalid_window_limit"),
        "bounds.public-invalid-offset": ("invalid_plan", "invalid_window_offset"),
        "bounds.canonical-zero-limit": ("invalid_plan", "zero_window_limit"),
        "bounds.window-overflow": ("invalid_plan", "window_overflow"),
        "order.non-scalar-field": ("invalid_plan", "non_scalar_order_field"),
        "order.missing-stable-root-key": ("invalid_plan", "missing_stable_unique_key"),
        "order.missing-stable-collection-key": (
            "invalid_plan",
            "missing_stable_unique_key",
        ),
        "cardinality.one-zero": ("cardinality", "no_result"),
        "cardinality.one-many": ("cardinality", "not_unique"),
        "transaction.non-read-context": (
            "invalid_plan",
            "borrowed_target_not_read_only",
        ),
        "preflight.missing-capability": (
            "unsupported_capability",
            "missing_provider_capability",
        ),
        "preflight.stale-schema": ("stale_schema", "stale_schema"),
        "execution.resource-limit": ("resource_limit", "collected_concept_limit"),
        "execution.provider-failure": ("provider", "provider_statement_failed"),
        "execution.result-decode": ("result_decode", "result_shape_mismatch"),
    }


def test_documented_example_ids_are_unique_paired_and_referenced() -> None:
    corpus = _load(CORPUS_PATH)
    contract = CONTRACT_PATH.read_text(encoding="utf-8")
    markers = EXAMPLE_MARKER.findall(contract)

    assert len(markers) == len(set(markers))
    declared = {
        example_id
        for language_ids in corpus["documented_examples"].values()
        for example_id in language_ids
    }
    assert set(markers) == declared

    parity_ids: set[str] = set()
    for group in corpus["example_parity_groups"]:
        assert group["id"] not in parity_ids
        parity_ids.add(group["id"])
        assert group["python"] in corpus["documented_examples"]["python"]
        assert group["typescript"] in corpus["documented_examples"]["typescript"]

    for case in corpus["cases"]:
        for language in corpus["languages"]:
            assert set(case["examples"][language]) <= set(
                corpus["documented_examples"][language]
            ), case["id"]


def test_all_ten_documented_blocks_are_exact_compiler_inputs() -> None:
    corpus = _load(CORPUS_PATH)
    contract = CONTRACT_PATH.read_text(encoding="utf-8")
    blocks = {
        example_id: (language, source)
        for example_id, language, source in EXAMPLE_BLOCK.findall(contract)
    }

    assert len(blocks) == 10
    paths = {
        "python": PYTHON_DOCUMENTED_EXAMPLES,
        "typescript": TYPESCRIPT_DOCUMENTED_EXAMPLES,
    }
    for language, example_ids in corpus["documented_examples"].items():
        expected_fence = language
        assert all(blocks[example_id][0] == expected_fence for example_id in example_ids)
        concatenated = "\n\n".join(blocks[example_id][1] for example_id in example_ids)
        assert _nonblank_source(paths[language].read_text(encoding="utf-8")) == (
            _nonblank_source(concatenated)
        )


def test_expected_result_references_pin_distinct_identity_semantics() -> None:
    corpus = _load(CORPUS_PATH)
    results = _load(RESULTS_PATH)
    expected = results["expected"]

    assert results["fixture_id"] == "identity-main"
    for case in corpus["cases"]:
        result_ref = case["expected"].get("result_ref")
        if result_ref is None:
            continue
        fixture_id, key = result_ref.split(".", maxsplit=1)
        assert fixture_id == results["fixture_id"], case["id"]
        assert key in expected, case["id"]

    assert len(results["solutions"]) == 4
    assert len(expected["selected_rows"]) == 3
    assert expected["distinct_roots"] == ["person:alice", "person:bob"]
    assert expected["page_by_person_offset_0_limit_1"] == {
        "roots": ["person:alice"],
        "offset": 0,
        "limit": 1,
        "total": 2,
    }
    assert expected["alice_collect_employments"] == [
        "employment:alice-acme-1",
        "employment:alice-acme-1",
        "employment:alice-acme-2",
    ]
    assert expected["alice_collect_distinct_employments"] == [
        "employment:alice-acme-1",
        "employment:alice-acme-2",
    ]
    assert expected["count_by_person"] == 2
    assert expected["exists_by_person"] is True


def test_live_public_artifact_projection_is_derived_from_the_identity_manifest() -> None:
    results = _load(RESULTS_PATH)
    expected = results["expected"]
    live = _load(LIVE_PARITY_CONTRACT)["semantic_corpus_projection"]

    assert live == {
        "source_fixture": results["fixture_id"],
        "distinct_roots": expected["distinct_roots"],
        "page_by_person_offset_0_limit_1": expected["page_by_person_offset_0_limit_1"],
        "alice_collect_count": len(expected["alice_collect_employments"]),
        "alice_collect_distinct_count": len(expected["alice_collect_distinct_employments"]),
        "count_by_person": expected["count_by_person"],
        "exists_by_person": expected["exists_by_person"],
    }


def test_contract_document_pins_required_sections_and_implemented_import_status() -> None:
    contract = CONTRACT_PATH.read_text(encoding="utf-8")
    required_headings = {
        "## Public Surface and Compatibility Boundary",
        "## Complete Python Example",
        "## Complete TypeScript Example",
        "## Row Shapes and Terminal Semantics",
        "## Identity, Multiplicity, and Hydration",
        "## Graph Topology and Boolean Binding",
        "## Output Shapes and Collections",
        "## Ordering and Bounds",
        "## String Operators",
        "## Transaction Ownership and Resources",
        "## Stable Error Categories",
        "## Legacy Compatibility Baseline",
        "## Fixture and Implementation Handoff",
    }

    assert required_headings <= set(contract.splitlines())
    assert "The immutable facade is available from `type_bridge.typed`" in contract
    assert "@type-bridge/node/typed" in contract
    assert "Future API — activated by #174" not in contract
    assert "between 1 and 16 selections" in contract
    assert "A seventeenth" in contract
    assert "no SQL wildcard meaning" in contract
    assert "explicit migration choice" in contract
