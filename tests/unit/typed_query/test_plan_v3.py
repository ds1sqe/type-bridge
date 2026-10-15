"""All SDKs consume the same Rust-generated canonical V3 plans."""

import json
from pathlib import Path

import pytest
from type_bridge_core import _query_plan_v3_round_trip

ROOT = Path(__file__).resolve().parents[2] / "contracts" / "plan_v3"
MANIFEST = json.loads((ROOT / "manifest.json").read_bytes())


@pytest.mark.parametrize("filename,fingerprint", sorted(MANIFEST.items()))
def test_plan_v3_bytes(filename: str, fingerprint: str) -> None:
    original = (ROOT / filename).read_bytes()
    encoded, actual = _query_plan_v3_round_trip(original, (ROOT / "schema.json").read_bytes())
    assert encoded == original
    assert actual == fingerprint


def test_plan_v3_rejects_noncanonical_bytes() -> None:
    original = (ROOT / next(iter(MANIFEST))).read_bytes()
    with pytest.raises(ValueError):
        _query_plan_v3_round_trip(original + b"\n", (ROOT / "schema.json").read_bytes())
