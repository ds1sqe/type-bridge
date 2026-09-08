"""Executable audit for tests removed by the generated-only cutover."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

ROOT = Path(__file__).parents[3]
AUDIT = ROOT / "tests/fixtures/generated-only-test-removal-audit.json"
ALLOWED_DISPOSITIONS = {
    "canonical_migration_and_archive_successors",
    "cutover_artifact_replacement",
    "generated_and_retained_query_successors",
    "generated_successor",
    "removed_authoring_contract",
    "split_yaml_schema_successor",
}


def _load(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def test_every_additional_removed_test_path_has_source_tied_disposition() -> None:
    audit = _load(AUDIT)
    assert audit["format"] == "typebridge.generated-only-test-removal-audit/v1"
    operation_map = _load(ROOT / audit["operation_removal_map"])
    operation_paths = {item["path"] for item in operation_map["case_inventory"]}
    assert len(operation_paths) == audit["operation_inventory_paths"]
    removed_operation_paths = {path for path in operation_paths if not (ROOT / path).exists()}
    assert len(removed_operation_paths) == audit["operation_removed_paths"]

    family_ids: set[str] = set()
    removed_paths: set[str] = set()
    for family in audit["families"]:
        family_id = family["id"]
        assert family_id not in family_ids
        family_ids.add(family_id)
        assert family["disposition"] in ALLOWED_DISPOSITIONS, family_id
        assert family["reason"].strip(), family_id
        assert family["removed_paths"], family_id
        assert family["evidence"], family_id

        for removed in family["removed_paths"]:
            relative = Path(removed)
            assert relative.is_relative_to(Path("."))
            assert ".." not in relative.parts
            assert removed not in removed_paths, removed
            assert removed not in operation_paths, removed
            removed_paths.add(removed)
            assert not (ROOT / relative).exists(), f"retired test surface is active: {removed}"

        for evidence in family["evidence"]:
            relative = Path(evidence["path"])
            assert ".." not in relative.parts
            path = ROOT / relative
            assert path.is_file(), f"{family_id} successor evidence is absent: {path}"
            needles = evidence["needles"]
            assert needles, f"{family_id} successor has no source-tied assertion"
            source = path.read_text(encoding="utf-8")
            for needle in needles:
                assert needle in source, f"{family_id}: {path} no longer contains {needle!r}"

    assert len(removed_paths) == audit["additional_removed_paths"]
    assert (
        len(removed_paths | removed_operation_paths)
        == audit["total_removed_test_and_support_paths"]
    )


def test_retained_query_tests_are_active_and_not_reclassified_as_authoring() -> None:
    audit = _load(AUDIT)
    removed = {path for family in audit["families"] for path in family["removed_paths"]}
    retained = {
        "tests/unit/typed_query/test_query.py",
        "tests/unit/typed_query/test_references.py",
        "tests/unit/typed_query/test_remote_query.py",
        "tests/unit/typed_query/test_results.py",
        "tests/unit/query/test_query_v2_authoring_facade.py",
        "type-bridge-core/crates/node/tests/unit/query-v2-authoring.test.ts",
        "type-bridge-core/crates/node/tests/unit/query-v2-remote-failures.test.ts",
        "type-bridge-core/crates/orm/tests/query_tests.rs",
    }
    assert removed.isdisjoint(retained)
    assert all((ROOT / path).is_file() for path in retained)


def test_generated_successor_evidence_never_uses_private_handwritten_models() -> None:
    audit = _load(AUDIT)
    generated_dispositions = {
        "generated_and_retained_query_successors",
        "generated_successor",
        "split_yaml_schema_successor",
    }
    for family in audit["families"]:
        if family["disposition"] not in generated_dispositions:
            continue
        for evidence in family["evidence"]:
            source = (ROOT / evidence["path"]).read_text(encoding="utf-8")
            assert "tests.utils.handwritten" not in source, family["id"]
            assert "tests/utils/handwritten" not in source, family["id"]
            assert "_legacy_models" not in source, family["id"]
