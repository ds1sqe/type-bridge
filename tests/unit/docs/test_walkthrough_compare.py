"""The walkthrough comparator must preserve statement and identity differences."""

from __future__ import annotations

import importlib.util
import json
from copy import deepcopy
from pathlib import Path

import pytest

PATH = Path(__file__).parents[3] / "examples/walkthrough/compare.py"
SPEC = importlib.util.spec_from_file_location("walkthrough_compare", PATH)
assert SPEC is not None and SPEC.loader is not None
compare = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(compare)


def test_iid_normalization_preserves_references_and_user_values() -> None:
    assert compare.normalize({"iid": "0xaa", "buyer_iid": "0xaa", "name": "0xaa"}) == {
        "iid": "iid:0",
        "buyer_iid": "iid:0",
        "name": "0xaa",
    }
    assert compare.normalize([{"iid": "0xaa"}, {"iid": "0xbb"}]) != compare.normalize(
        [{"iid": "0xaa"}, {"iid": "0xaa"}]
    )


def test_comparison_rejects_typeql_and_hydration_changes() -> None:
    first = {
        "purchase": {
            "plan": {"pipeline": [{"kind": "insert"}]},
            "explain": {"statements": ["insert $x isa item;"]},
            "models": [{"iid": "0xaa", "stock": 4}],
            "raw": [{"_iid": "0xaa", "attributes": {"stock": 4}}],
        }
    }
    second = deepcopy(first)
    second["purchase"]["models"][0]["iid"] = "0xbb"
    compare.compare({"python": first, "typescript": second})
    second["purchase"]["explain"]["statements"][0] += " "
    with pytest.raises(ValueError, match="purchase/explain"):
        compare.compare({"python": first, "typescript": second})
    second = deepcopy(first)
    second["purchase"]["models"][0]["stock"] = 5
    with pytest.raises(ValueError, match="purchase/models"):
        compare.compare({"python": first, "typescript": second})


def test_transcript_rejects_missing_steps(tmp_path: Path) -> None:
    path = tmp_path / "python.jsonl"
    path.write_text(json.dumps({"case": "purchase", "step": "call", "value": "insert()"}))
    with pytest.raises(ValueError, match="steps must be"):
        compare.read(path)


def test_comparison_requires_authored_raw_documents_and_compares_their_values() -> None:
    first = {"purchase": {"plan": {}, "explain": {}, "models": [], "raw": [{"stock": 4}]}}
    second = deepcopy(first)
    second["purchase"]["raw"] = compare.RAW_SEAM
    with pytest.raises(ValueError, match="must expose"):
        compare.compare({"python": first, "typescript": second})
    second["purchase"]["raw"] = [{"stock": 5}]
    with pytest.raises(ValueError, match="purchase/raw differs"):
        compare.compare({"python": first, "typescript": second})
