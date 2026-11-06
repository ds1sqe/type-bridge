"""Consume staged fixtures using Rust for embedded plan semantics."""

import json
from pathlib import Path

from type_bridge_core import _query_plan_round_trip

ROOT = Path(__file__).resolve().parents[3]
FIXTURES = ROOT / "type-bridge-core/crates/contract/tests/fixtures/query-remote"


def test_query_remote_v3_fixtures() -> None:
    fixtures = {p.name: json.loads(p.read_bytes()) for p in FIXTURES.glob("*.json")}
    assert len(fixtures) == 14
    request = fixtures["request.json"]
    assert request["format"] == "typebridge.query-remote-request/v3"
    schema = (ROOT / "tests/contracts/plan/schema.json").read_bytes()
    for name in ["request.json", "request-result-ref.json"]:
        for entry in fixtures[name]["plans"]:
            original = bytes.fromhex(entry["plan_bytes"])
            encoded, fingerprint = _query_plan_round_trip(original, schema)
            assert encoded == original
            assert len(fingerprint) == 64
    for name in ["reply-committed.json", "reply-failed.json"]:
        outer = fixtures[name]
        assert outer["format"] == "typebridge.query-remote-signed-reply/v1"
        assert outer["payload"]["request"] == fixtures["request-fingerprint.json"]["digest"]
        assert outer["payload"]["nonce"] == request["nonce"]
