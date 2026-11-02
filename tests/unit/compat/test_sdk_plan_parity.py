"""Only run-recorded IIDs may vary in cross-binding canonical plan twins."""

from __future__ import annotations

import copy
import json
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / "scripts/ci"))
from scripts.ci import sdk_plan_parity as parity  # noqa: E402


def test_exchange_recorder_rejects_duplicate_calls_and_extra_io(tmp_path: Path) -> None:
    policy = {"timeout_milliseconds": 10}
    recorder = parity.WitnessRecorder(
        tmp_path / "witness.json",
        "queries",
        "python",
        "typedb-3.13.6/v1",
        caller=policy,
        session=policy,
    )
    recorder.mark("terminals.one")
    recorder.exchange(b"actual-first-request")
    with pytest.raises(parity.ContractError, match="plan_extra_exchange"):
        recorder.exchange(b"unexpected-second-request")
    with pytest.raises(parity.ContractError, match="plan_witness_call"):
        recorder.mark("terminals.one")
    recorder.stop()
    recorder.exchange(b"unselected-cleanup")
    assert len(recorder.value["calls"]["terminals.one"]["requests"]) == 1


def test_only_run_recorded_named_iids_change_and_reincarnations_keep_their_identity() -> None:
    records = parity.iid_records({"query-ada": ["0x01", "0x02"]}, {"query-ada"})
    value = {
        "plans": [{"iid": "0x01", "caller_value": "Ada"}, {"iid": "0x02"}],
        "capabilities": ["query.given"],
        "limits": {"items": 42},
    }
    assert parity.logical_iids(value, records) == {
        "plans": [
            {"iid": "@record/query-ada", "caller_value": "Ada"},
            {"iid": "@record/query-ada"},
        ],
        "capabilities": ["query.given"],
        "limits": {"items": 42},
    }
    with pytest.raises(parity.ContractError, match="plan_unknown_iid"):
        parity.logical_iids({"iid": "0x99"}, records)
    with pytest.raises(parity.ContractError, match="plan_record_identity"):
        parity.iid_records({"unknown-record": ["0x01"]}, {"query-ada"})
    with pytest.raises(parity.ContractError, match="plan_record_identity"):
        parity.iid_records(
            {"query-ada": ["0x01"], "query-dana": ["0x01"]}, {"query-ada", "query-dana"}
        )


def test_exchange_capture_keeps_plan_content_and_statement_order() -> None:
    plan = {
        "format": "typebridge.query-plan/v3",
        "pipeline": [{"kind": "match"}, {"kind": "fetch"}],
        "required_capabilities": ["query.given"],
        "given_rows": [[{"kind": "string", "value": "0x01"}]],
    }
    request = {
        "advertisement": "transport-identity",
        "expires_at_unix_ms": 2,
        "format": "typebridge.query-remote-request/v3",
        "kind": "read",
        "limits": {"max_items": 23},
        "nonce": "transport-nonce",
        "prepared_at_unix_ms": 1,
        "plans": [
            {"plan_bytes": parity.canonical_json_bytes(plan).removesuffix(b"\n").hex(), "rows": []}
        ],
    }
    stack = parity.exchange_stack(json.dumps(request).encode())
    assert stack == [{"plan": plan, "rows": []}]
    logical = parity.logical_iids(stack, {"0x01": "query-ada"})
    reordered = copy.deepcopy(logical)
    reordered[0]["plan"]["pipeline"].reverse()
    assert parity.canonical_json_bytes(reordered) != parity.canonical_json_bytes(logical)
    changed = copy.deepcopy(logical)
    changed[0]["plan"]["required_capabilities"] = ["query.other"]
    assert parity.canonical_json_bytes(changed) != parity.canonical_json_bytes(logical)
    request["plans"][0]["plan_bytes"] = "7B7D"
    with pytest.raises(parity.ContractError, match="plan_exchange_bytes"):
        parity.exchange_stack(json.dumps(request).encode())


def test_advertisement_identity_matches_the_retained_core_capture() -> None:
    directory = ROOT / "tests/contracts/sdk_conformance/parity/v2-answers/wire/page-with-total"
    advertisement = (directory / "advertisement.json").read_bytes().removesuffix(b"\n")
    request = json.loads((directory / "request.json").read_bytes())
    assert parity.advertisement_digest(advertisement) == request["advertisement"]


def test_transport_budgets_are_checked_independently_from_the_plan_twin() -> None:
    policy = {
        "timeout_milliseconds": 30,
        "items": 10,
        "bytes": 1000,
        "graph_nodes": 20,
        "attribute_values": 30,
        "collection_members": 40,
        "role_players": 50,
        "statements": None,
    }
    advertisement = parity.canonical_json_bytes({"answer_limits": {"items": 7}}).removesuffix(b"\n")
    plan = {"format": "typebridge.query-plan/v3", "pipeline": []}
    limits = {
        "deadline_ms": 25,
        "max_items": 7,
        "max_bytes": 1000,
        "max_graph_nodes": 20,
        "max_attribute_values": 30,
        "max_collection_members": 40,
        "max_role_players": 50,
        "max_read_statements": 3,
        "max_staged_plans": 64,
        "max_write_statements": 256,
    }
    request = {
        "format": "typebridge.query-remote-request/v3",
        "kind": "read",
        "advertisement": parity.advertisement_digest(advertisement),
        "nonce": "test-nonce-00000001",
        "prepared_at_unix_ms": 1000,
        "expires_at_unix_ms": 1025,
        "limits": limits,
        "plans": [
            {"plan_bytes": parity.canonical_json_bytes(plan).removesuffix(b"\n").hex(), "rows": []}
        ],
    }

    def evidence(value: dict) -> dict:
        return {
            "request_hex": json.dumps(value).encode().hex(),
            "advertisement_hex": advertisement.hex(),
            "caller": policy,
            "session": policy,
            "proxy_defaults": policy,
            "caller_deadline_ms": 27,
        }

    assert parity.checked_exchange(evidence(request)) == [{"plan": plan, "rows": []}]
    for field, value, code in [
        ("max_items", 8, "plan_envelope_budget"),
        ("deadline_ms", 0, "plan_envelope_deadline"),
        ("deadline_ms", 28, "plan_envelope_deadline"),
    ]:
        changed = copy.deepcopy(request)
        changed["limits"][field] = value
        with pytest.raises(parity.ContractError, match=code):
            parity.checked_exchange(evidence(changed))
    for field, value in [("kind", "write"), ("advertisement", "0" * 64)]:
        changed = copy.deepcopy(request)
        changed[field] = value
        with pytest.raises(parity.ContractError, match="plan_envelope_identity"):
            parity.checked_exchange(evidence(changed))
    for field in ("prepared_at_unix_ms", "expires_at_unix_ms"):
        for value in ("1000", False, -1, 2**64):
            changed = copy.deepcopy(request)
            changed[field] = value
            with pytest.raises(parity.ContractError, match="plan_envelope_deadline"):
                parity.checked_exchange(evidence(changed))


def test_binding_alpha_renaming_is_bijective_and_keeps_all_semantic_values() -> None:
    plan = {
        "bindings": [{"id": 8, "variable": "person"}, {"id": 3, "variable": "score"}],
        "given_rows": [[{"kind": "long", "value": "8"}]],
        "output": {"kind": "rows", "columns": [8]},
        "pipeline": [
            {
                "kind": "match",
                "patterns": [
                    {"kind": "has", "owner": 8, "attribute": 3, "attribute_id": "score"},
                    {
                        "kind": "value",
                        "left": {"kind": "binding", "binding": 3},
                        "right": {"kind": "input", "column": 8},
                        "comparator": "gte",
                    },
                ],
            }
        ],
    }
    renamed = copy.deepcopy(plan)
    renamed["bindings"] = [{"id": 99, "variable": "other"}, {"id": 10, "variable": "value"}]
    renamed["output"]["columns"] = [99]
    renamed["pipeline"][0]["patterns"][0].update(owner=99, attribute=10)
    renamed["pipeline"][0]["patterns"][1]["left"]["binding"] = 10
    actual = parity.alpha_bindings(plan)
    assert actual == parity.alpha_bindings(renamed)
    assert actual["given_rows"] == plan["given_rows"]
    assert actual["pipeline"][0]["patterns"][1]["right"]["column"] == 8
    for bad in [
        [{"id": 8, "variable": "same"}, {"id": 3, "variable": "same"}],
        [{"id": 8, "variable": "a"}, {"id": 8, "variable": "b"}],
        [{"id": 8, "variable": "a"}],
    ]:
        with pytest.raises(parity.ContractError, match="plan_binding_identity"):
            parity.alpha_bindings(plan | {"bindings": bad})


def test_native_trace_capture_keeps_plan_order_and_requires_valid_frames(tmp_path: Path) -> None:
    trace = tmp_path / "native.jsonl"
    output = tmp_path / "witness.json"
    plan = {
        "format": "typebridge.query-plan/v3",
        "managed_semantics": {"semantic_profile": "typedb-3.13.6/v1"},
        "bindings": [{"id": 0, "variable": "person"}],
        "pipeline": [],
        "output": {"kind": "rows", "columns": [0]},
    }
    trace.write_text(
        "\n".join(
            json.dumps(
                {
                    "call": "manager_field_token_filter.first",
                    "plans": [plan | {"operation": {"kind": kind}}],
                }
            )
            for kind in ("count", "rows")
        )
    )
    parity.native_witness("models", "typedb-3.13.6/v1", "python", trace, output)
    witness = json.loads(output.read_text())
    calls = parity.witness_calls(witness, "models", "typedb-3.13.6/v1", "python")
    assert [
        entry["plan"]["operation"]["kind"] for entry in calls["manager_field_token_filter.first"]
    ] == ["count", "rows"]
    for malformed in (None, [], "typedb-3.13.6/v1"):
        changed = copy.deepcopy(witness)
        changed["calls"]["manager_field_token_filter.first"]["plans"][0]["managed_semantics"] = (
            malformed
        )
        with pytest.raises(parity.ContractError, match="plan_witness_profile"):
            parity.witness_calls(changed, "models", "typedb-3.13.6/v1", "python")
    with pytest.raises(FileExistsError):
        parity.native_witness("models", "typedb-3.13.6/v1", "python", trace, output)
    trace.write_text(json.dumps({"call": "unknown\ncall", "plans": []}))
    with pytest.raises(parity.ContractError, match="plan_trace_frame"):
        parity.native_witness("models", "typedb-3.13.6/v1", "python", trace, tmp_path / "bad.json")


def test_native_identities_and_disjoint_fragments_keep_the_measured_write_stack(
    tmp_path: Path,
) -> None:
    profile = "typedb-3.13.6/v1"
    plan = {
        "format": "typebridge.query-plan/v3",
        "managed_semantics": {"semantic_profile": profile},
        "bindings": [{"id": 0, "variable": "counter"}],
        "pipeline": [{"kind": "match", "patterns": [{"kind": "iid", "binding": 0, "iid": "0x01"}]}],
        "output": {"kind": "rows", "columns": [0]},
    }
    identities = tmp_path / "records.jsonl"
    identities.write_text(json.dumps(["counter-left", "0x01"]) + "\n")
    fragments = []
    for name in ("insert", "update"):
        trace = tmp_path / f"{name}.jsonl"
        trace.write_text(
            json.dumps({"call": f"unkeyed_entity_iid_lifecycle.{name}", "plans": [plan]})
        )
        fragment = tmp_path / f"{name}.json"
        parity.native_witness("models", profile, "c", trace, fragment, identities=identities)
        fragments.append(fragment)
    output = tmp_path / "merged.json"
    parity.merge_witnesses("models", profile, "c", fragments, output)
    value = json.loads(output.read_text())
    assert value["records"] == {"counter-left": ["0x01"]}
    calls = parity.witness_calls(value, "models", profile, "c")
    assert len(calls) == 2
    assert (
        calls["unkeyed_entity_iid_lifecycle.update"][0]["plan"]["pipeline"][0]["patterns"][0]["iid"]
        == "@record/counter-left"
    )
    with pytest.raises(parity.ContractError, match="plan_witness_call"):
        parity.merge_witnesses(
            "models", profile, "c", [fragments[0], fragments[0]], tmp_path / "duplicate.json"
        )
    identities.write_text(json.dumps(["unknown-record", "0x01"]))
    with pytest.raises(parity.ContractError, match="plan_record_identity"):
        parity.native_witness(
            "models", profile, "c", trace, tmp_path / "bad-identity.json", identities=identities
        )


def test_plan_comparator_requires_every_binding_and_the_exact_committed_bytes(
    tmp_path: Path,
) -> None:
    suite = "queries"
    profile = "typedb-3.13.6/v1"
    directory = tmp_path / "tests/contracts/sdk_conformance" / suite
    directory.mkdir(parents=True)
    (directory / "journey.json").write_text(
        json.dumps({"create_order": ["query-ada"], "expected_observations": ["terminals"]})
    )
    twins = directory / "plans/typedb-3.13.6_v1"
    twins.mkdir(parents=True)
    plan = {
        "format": "typebridge.query-plan/v3",
        "managed_semantics": {"semantic_profile": profile},
        "bindings": [{"id": 5, "variable": "person"}],
        "output": {"kind": "rows", "columns": [5]},
        "pipeline": [{"kind": "match", "patterns": [{"kind": "iid", "binding": 5, "iid": "0x01"}]}],
    }
    paths = {}
    for binding in parity.BINDINGS:
        value = {
            "format": parity.FORMAT,
            "suite": suite,
            "binding": binding,
            "semantic_profile": profile,
            "records": {"query-ada": ["0x01"]},
            "calls": {"terminals.one": {"source": "trace", "plans": [plan]}},
        }
        paths[binding] = tmp_path / f"{binding}.json"
        paths[binding].write_bytes(parity.canonical_json_bytes(value))
    canonical = parity.logical_iids(
        [{"plan": parity.alpha_bindings(plan), "rows": []}], {"0x01": "query-ada"}
    )
    (twins / "terminals.one.json").write_bytes(parity.canonical_json_bytes(canonical))
    coverage = {
        "format": "typebridge.sdk-plan-coverage/v1",
        "suite": suite,
        "observations": {"terminals": {"kind": "native", "calls": ["terminals.one"]}},
    }
    coverage_path = directory / "plans/coverage.json"
    coverage_path.write_text(json.dumps(coverage))
    assert parity.compare_twins(suite, profile, paths, root=tmp_path)["calls"] == 1
    coverage["observations"]["terminals"]["calls"] = ["terminals.count"]
    coverage_path.write_text(json.dumps(coverage))
    with pytest.raises(parity.ContractError, match="plan_coverage_inventory"):
        parity.compare_twins(suite, profile, paths, root=tmp_path)
    coverage_path.unlink()
    with pytest.raises(parity.ContractError, match="invalid_source_file"):
        parity.compare_twins(suite, profile, paths, root=tmp_path)
    coverage["observations"]["terminals"]["calls"] = ["terminals.one"]
    coverage_path.write_text(json.dumps(coverage))
    with pytest.raises(parity.ContractError, match="plan_witness_bindings"):
        parity.compare_twins(suite, profile, {"rust": paths["rust"]}, root=tmp_path)
    (twins / "terminals.one.json").write_text("[]\n")
    with pytest.raises(parity.ContractError, match="plan_twin_drift"):
        parity.compare_twins(suite, profile, paths, root=tmp_path)
    value = json.loads(paths["node"].read_text())
    value["calls"]["terminals.one"]["plans"][0]["pipeline"][0]["patterns"][0]["iid"] = "0x99"
    paths["node"].write_text(json.dumps(value))
    with pytest.raises(parity.ContractError, match="plan_unknown_iid"):
        parity.compare_twins(suite, profile, paths, root=tmp_path)


def test_binding_scoped_twins_keep_shared_manager_calls_required_in_every_sdk(
    tmp_path: Path,
) -> None:
    suite = "models"
    profile = "typedb-3.13.6/v1"
    directory = tmp_path / "tests/contracts/sdk_conformance/models"
    twins = directory / "plans/typedb-3.13.6_v1"
    twins.mkdir(parents=True)
    (directory / "journey.json").write_text(
        json.dumps(
            {"create_order": ["counter-left"], "expected_observations": ["shared", "returning"]}
        )
    )
    plan = {
        "format": "typebridge.query-plan/v3",
        "managed_semantics": {"semantic_profile": profile},
        "bindings": [{"id": 0, "variable": "counter"}],
        "output": {"kind": "rows", "columns": [0]},
        "pipeline": [{"kind": "match", "patterns": [{"kind": "iid", "binding": 0, "iid": "0x01"}]}],
    }
    canonical = parity.logical_iids(
        [{"plan": parity.alpha_bindings(plan), "rows": []}], {"0x01": "counter-left"}
    )
    paths = {}
    for binding in parity.BINDINGS:
        calls = {"shared.one": {"source": "trace", "plans": [plan]}}
        if binding != "c":
            calls["returning.one"] = {"source": "trace", "plans": [plan]}
        value = {
            "format": parity.FORMAT,
            "suite": suite,
            "binding": binding,
            "semantic_profile": profile,
            "records": {"counter-left": ["0x01"]},
            "calls": calls,
        }
        paths[binding] = tmp_path / f"{binding}.json"
        paths[binding].write_bytes(parity.canonical_json_bytes(value))
    for call in ("shared.one", "returning.one"):
        (twins / f"{call}.json").write_bytes(parity.canonical_json_bytes(canonical))
    coverage = {
        "format": "typebridge.sdk-plan-coverage/v1",
        "suite": suite,
        "observations": {
            "shared": {"kind": "native", "calls": ["shared.one"]},
            "returning": {
                "kind": "native",
                "calls": ["returning.one"],
                "bindings": ["python", "node", "rust"],
            },
        },
    }
    coverage_path = directory / "plans/coverage.json"
    coverage_path.write_text(json.dumps(coverage))
    assert parity.compare_twins(suite, profile, paths, root=tmp_path)["calls"] == 2
    original = paths["c"].read_bytes()
    value = json.loads(original)
    value["calls"]["shared.one"]["plans"].append(plan)
    paths["c"].write_bytes(parity.canonical_json_bytes(value))
    with pytest.raises(parity.ContractError, match="plan_manager_count"):
        parity.compare_twins(suite, profile, paths, root=tmp_path)
    paths["c"].write_bytes(original)
    value = json.loads(original)
    value["calls"] = {"unexpected.one": value["calls"]["shared.one"]}
    paths["c"].write_bytes(parity.canonical_json_bytes(value))
    with pytest.raises(parity.ContractError, match="plan_coverage_inventory"):
        parity.compare_twins(suite, profile, paths, root=tmp_path)
    paths["c"].write_bytes(original)
    del coverage["observations"]["returning"]["bindings"]
    coverage_path.write_text(json.dumps(coverage))
    with pytest.raises(parity.ContractError, match="plan_coverage_inventory"):
        parity.compare_twins(suite, profile, paths, root=tmp_path)
