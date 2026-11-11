"""Compare actual SDK plan stacks with committed twins and retained V2 answers."""

from __future__ import annotations

import hashlib
import os
import re
from operator import itemgetter
from pathlib import Path
from typing import Any, Never

from sdk_conformance_common import (
    ContractError,
    _json_value,
    _load_json,
    _regular_bytes,
    canonical_json_bytes,
)

ROOT = Path(__file__).resolve().parents[2]
BINDINGS = ("python", "node", "rust", "c")
FORMAT = "typebridge.sdk-plan-witness/v1"
DEFAULT_ADMISSION_WINDOW_MILLISECONDS = 60_000
IID = re.compile(r"0x(?:[0-9a-f]{2})+\Z")
IID_SHAPED = re.compile(r"0[xX][0-9a-fA-F]+\Z")
CALL = re.compile(r"[a-z][a-z0-9_.-]{0,159}\Z")


class WitnessRecorder:
    """Fixture-only recorder attached to the existing Python exchange."""

    def __init__(
        self,
        destination: Path,
        suite: str,
        binding: str,
        profile: str,
        *,
        advertisement: bytes = b"",
        caller: dict[str, Any] | None = None,
        session: dict[str, Any] | None = None,
        proxy_defaults: dict[str, Any] | None = None,
    ) -> None:
        if destination.exists():
            _fail("plan_witness_destination", "the witness destination must be new")
        self.destination = destination
        self.allowed_records = named_records(suite)
        self.value: dict[str, Any] = {
            "format": FORMAT,
            "suite": suite,
            "binding": binding,
            "semantic_profile": profile,
            "records": {},
            "calls": {},
        }
        self.advertisement = advertisement
        self.caller = caller or {}
        self.session = session or {}
        self.defaults = proxy_defaults or {}
        self.active: str | None = None

    @classmethod
    def requested(
        cls, suite: str, binding: str, profile: str, **options: Any
    ) -> WitnessRecorder | None:
        raw = os.environ.get("TYPE_BRIDGE_SDK_PLAN_WITNESS")
        return None if raw is None else cls(Path(raw), suite, binding, profile, **options)

    def record(self, name: str, iid: str) -> None:
        if name not in self.allowed_records or not IID.fullmatch(iid):
            _fail(
                "plan_record_identity", "the fixture must record the IID of a named journey record"
            )
        history = self.value["records"].setdefault(name, [])
        if iid not in history:
            history.append(iid)

    def mark(self, name: str) -> None:
        if not CALL.fullmatch(name) or name in self.value["calls"]:
            _fail("plan_witness_call", "fixture call labels must be unique canonical identifiers")
        self.value["calls"][name] = {"source": "exchange", "requests": []}
        self.active = name

    def exchange(self, request: bytes) -> None:
        if self.active is not None:
            name = self.active
            if self.value["calls"][name]["requests"]:
                _fail("plan_extra_exchange", "a selected typed terminal sent an extra exchange")
            self.value["calls"][name]["requests"].append(
                {
                    "request_hex": request.hex(),
                    "advertisement_hex": self.advertisement.hex(),
                    "caller": self.caller,
                    "session": self.session,
                    "proxy_defaults": self.defaults,
                    "caller_deadline_ms": min(
                        self.caller["timeout_milliseconds"], self.session["timeout_milliseconds"]
                    ),
                }
            )

    def stop(self) -> None:
        self.active = None

    def finish(self) -> None:
        self.stop()
        # Reuse the same validation before publishing a write-once witness.
        witness_calls(
            self.value, self.value["suite"], self.value["semantic_profile"], self.value["binding"]
        )
        with self.destination.open("xb") as output:
            output.write(canonical_json_bytes(self.value))


def _fail(code: str, message: str) -> Never:
    raise ContractError(code, message)


def named_records(suite: str, *, root: Path = ROOT) -> set[str]:
    """Use the journey's named records as the sole IID substitution authority."""
    journey = _load_json(
        root / f"tests/contracts/sdk_conformance/{suite}/journey.json", "journey"
    ).value
    return set(journey["create_order"])


def iid_records(records: Any, allowed: set[str]) -> dict[str, str]:
    """Accept only run-recorded canonical IIDs of known records, including reincarnations."""
    if type(records) is not dict or not set(records) <= allowed:
        _fail("plan_record_identity", "plan witness records must name journey records")
    result: dict[str, str] = {}
    for record, values in records.items():
        if (
            type(values) is not list
            or not values
            or not all(type(value) is str for value in values)
            or len(set(values)) != len(values)
        ):
            _fail("plan_record_identity", f"{record} must retain each recorded IID once")
        for value in values:
            if type(value) is not str or not IID.fullmatch(value):
                _fail("plan_record_identity", f"{record} contains a noncanonical IID")
            if value in result and result[value] != record:
                _fail("plan_record_identity", "one IID cannot identify different journey records")
            result[value] = record
    return result


def logical_iids(value: Any, records: dict[str, str], *, depth: int = 0) -> Any:
    """Change only values equal to this run's recorded IIDs; reject unknown IID-shaped values."""
    if depth > 128:
        _fail("plan_witness_depth", "plan witness exceeds the bounded nesting depth")
    if type(value) is str:
        if value in records:
            return f"@record/{records[value]}"
        if IID_SHAPED.fullmatch(value):
            _fail("plan_unknown_iid", "an IID-shaped value has no run-recorded journey identity")
        return value
    if type(value) is list:
        return [logical_iids(item, records, depth=depth + 1) for item in value]
    if type(value) is dict:
        return {key: logical_iids(item, records, depth=depth + 1) for key, item in value.items()}
    return value


def alpha_bindings(plan: dict[str, Any]) -> dict[str, Any]:
    """Rename declared bindings bijectively in first-reference order within each plan."""
    declarations = plan.get("bindings")
    if type(declarations) is not list:
        _fail("plan_binding_identity", "a plan must retain its binding declarations")
    by_id: dict[int, dict[str, Any]] = {}
    variables: set[str] = set()
    for declaration in declarations:
        if (
            type(declaration) is not dict
            or type(declaration.get("id")) is not int
            or declaration["id"] < 0
            or declaration["id"] in by_id
            or type(declaration.get("variable")) is not str
            or declaration["variable"] in variables
        ):
            _fail("plan_binding_identity", "binding IDs and variables must form a bijection")
        by_id[declaration["id"]] = declaration
        variables.add(declaration["variable"])
    renamed: dict[int, int] = {}

    def binding(value: Any) -> int:
        if type(value) is not int or value not in by_id:
            _fail("plan_binding_identity", "a binding reference has no unique declaration")
        if value not in renamed:
            renamed[value] = len(renamed)
        return renamed[value]

    singular = {
        "binding",
        "owner",
        "attribute",
        "relation",
        "player",
        "source",
        "target",
        "assigned",
        "thing",
    }

    def walk(value: Any) -> Any:
        if type(value) is list:
            return [walk(item) for item in value]
        if type(value) is not dict or value.get("kind") == "literal":
            return value
        result = {}
        for key in sorted(value):
            item = value[key]
            if key in singular and type(item) is int:
                result[key] = binding(item)
            elif key in {"bindings", "groups", "columns"} and type(item) is list:
                result[key] = [binding(entry) for entry in item]
            elif key == "input" and "reducer" in value and item is not None:
                result[key] = binding(item)
            elif key == "players" and type(item) is list:
                result[key] = [
                    [entry[0], binding(entry[1])] if type(entry) is list else walk(entry)
                    for entry in item
                ]
            else:
                result[key] = walk(item)
        return result

    # Declarations do not establish semantic occurrence order; their names have local scope.
    result = {
        key: walk(plan[key]) if key in {"pipeline", "output"} else plan[key]
        for key in sorted(plan)
        if key != "bindings"
    }
    for old in by_id:
        binding(old)
    result["bindings"] = [
        declaration | {"id": renamed[old], "variable": f"b{renamed[old]}"}
        for old, declaration in sorted(by_id.items(), key=lambda pair: renamed[pair[0]])
    ]
    return result


def exchange_stack(request: bytes) -> list[dict[str, Any]]:
    """Select ordered plan content; retain the full request separately as witness evidence."""
    wire = _json_value(request, "staged exchange request")
    required = {
        "advertisement",
        "expires_at_unix_ms",
        "format",
        "limits",
        "nonce",
        "plans",
        "prepared_at_unix_ms",
    }
    if (
        type(wire) is not dict
        or not required <= set(wire) <= required | {"kind"}
        or wire["format"] != "typebridge.query-remote-request/v3"
        or type(wire["limits"]) is not dict
        or type(wire["plans"]) is not list
        or not wire["plans"]
    ):
        _fail("plan_exchange_shape", "plan witness requires an actual staged request")
    plans = []
    for entry in wire["plans"]:
        if type(entry) is not dict or set(entry) != {"plan_bytes", "rows"}:
            _fail("plan_exchange_shape", "staged plan entry has the wrong shape")
        encoded = entry["plan_bytes"]
        if type(encoded) is not str:
            _fail("plan_exchange_bytes", "canonical plan bytes must be lowercase hexadecimal")
        try:
            raw = bytes.fromhex(encoded)
        except ValueError:
            _fail("plan_exchange_bytes", "canonical plan bytes are not hexadecimal")
        if raw.hex() != encoded:
            _fail("plan_exchange_bytes", "canonical plan bytes must be lowercase hexadecimal")
        plan = _json_value(raw, "canonical staged plan")
        if canonical_json_bytes(plan).removesuffix(b"\n") != raw:
            _fail("plan_exchange_bytes", "embedded plan bytes must be exactly canonical")
        plans.append({"plan": plan, "rows": entry["rows"]})
    return plans


def advertisement_digest(raw: bytes) -> str:
    """Independently check the contract's domain-separated advertisement identity."""
    value = _json_value(raw, "advertisement witness")
    if canonical_json_bytes(value).removesuffix(b"\n") != raw:
        _fail("plan_advertisement", "advertisement witness must retain the canonical bytes")
    digest = hashlib.sha256(b"typebridge.fingerprint/v1\0")
    for field in (
        b"typebridge.query.remote-capabilities",
        b"typebridge.query-remote-capabilities/v1",
    ):
        digest.update(len(field).to_bytes(8, "big"))
        digest.update(field)
    digest.update(b"\0")
    digest.update(len(raw).to_bytes(8, "big"))
    digest.update(raw)
    return digest.hexdigest()


def checked_exchange(evidence: Any) -> list[dict[str, Any]]:
    """Check the excluded envelope against the run's actual advertisement and public policies."""
    if type(evidence) is not dict or set(evidence) != {
        "request_hex",
        "advertisement_hex",
        "caller",
        "session",
        "proxy_defaults",
        "caller_deadline_ms",
    }:
        _fail(
            "plan_envelope_shape", "exchange evidence must retain raw bytes and all policy inputs"
        )
    try:
        request = bytes.fromhex(evidence["request_hex"])
        advertisement = bytes.fromhex(evidence["advertisement_hex"])
    except (ValueError, TypeError):
        _fail("plan_envelope_shape", "raw exchange evidence must be hexadecimal")
    plans = exchange_stack(request)
    wire = _json_value(request, "request witness")
    advertised = _json_value(advertisement, "advertisement witness")
    if wire.get("kind") != "read" or wire["advertisement"] != advertisement_digest(advertisement):
        _fail(
            "plan_envelope_identity",
            "request kind or advertisement identity differs from its owner",
        )
    dimensions = {
        "timeout_milliseconds": "deadline_ms",
        "items": "max_items",
        "bytes": "max_bytes",
        "graph_nodes": "max_graph_nodes",
        "attribute_values": "max_attribute_values",
        "collection_members": "max_collection_members",
        "role_players": "max_role_players",
        "statements": "max_read_statements",
    }
    defaults, caller, session = (evidence[name] for name in ("proxy_defaults", "caller", "session"))
    if any(
        type(policy) is not dict or set(policy) != set(dimensions)
        for policy in (defaults, caller, session)
    ):
        _fail("plan_envelope_policy", "each public policy must retain every resource dimension")
    advertised_limits = advertised.get("answer_limits", {})
    if type(advertised_limits) is not dict or not set(advertised_limits) <= set(dimensions):
        _fail("plan_envelope_policy", "advertised limits contain an unknown dimension")

    def bound(policy: dict[str, Any], name: str) -> int:
        value = policy[name]
        if value is None and name == "statements":
            return max(3, len(plans))
        if type(value) is not int or value < 0:
            _fail("plan_envelope_policy", "resource policies must contain unsigned integer bounds")
        return value

    advertised_policy = defaults | advertised_limits
    limits = wire["limits"]
    required_limits = set(dimensions.values()) | {"max_staged_plans", "max_write_statements"}
    if not required_limits <= set(limits) <= required_limits | {"admission_window_milliseconds"}:
        _fail("plan_envelope_budget", "request limits must retain exactly the known dimensions")
    for name, field in dimensions.items():
        expected = min(bound(policy, name) for policy in (caller, session, advertised_policy))
        actual = limits.get(field)
        if type(actual) is not int:
            _fail("plan_envelope_budget", f"{field} is not an integer budget")
        if name == "timeout_milliseconds":
            deadline = evidence["caller_deadline_ms"]
            if type(deadline) is not int or not 0 < actual <= min(expected, deadline):
                _fail(
                    "plan_envelope_deadline",
                    "request deadline exceeds its caller or negotiated policy",
                )
        elif actual != expected:
            _fail(
                "plan_envelope_budget", f"{field} differs from min(caller, advertisement, session)"
            )
    if limits.get("max_staged_plans") != 64 or limits.get("max_write_statements") != 256:
        _fail(
            "plan_envelope_budget",
            "request must retain the staged protocol's plan and write ceilings",
        )
    admission_window = limits.get(
        "admission_window_milliseconds", DEFAULT_ADMISSION_WINDOW_MILLISECONDS
    )
    advertised_window = advertised.get(
        "admission_window_milliseconds", DEFAULT_ADMISSION_WINDOW_MILLISECONDS
    )
    if (
        type(admission_window) is not int
        or type(advertised_window) is not int
        or not 0 < admission_window <= advertised_window < 2**64
    ):
        _fail("plan_envelope_deadline", "request admission window exceeds its advertised ceiling")
    if (
        any(
            type(wire[field]) is not int or not 0 <= wire[field] < 2**64
            for field in ("prepared_at_unix_ms", "expires_at_unix_ms")
        )
        or wire["expires_at_unix_ms"] != wire["prepared_at_unix_ms"] + admission_window
    ):
        _fail(
            "plan_envelope_deadline",
            "request expiry must retain its exact preparation-time admission window",
        )
    return plans


def witness_calls(
    value: Any, suite: str, profile: str, binding: str, *, root: Path = ROOT
) -> dict[str, Any]:
    """Validate a producer witness and return its logical-identity stacks."""
    if (
        type(value) is not dict
        or set(value) != {"format", "suite", "binding", "semantic_profile", "records", "calls"}
        or value["format"] != FORMAT
        or value["suite"] != suite
        or value["binding"] != binding
        or value["semantic_profile"] != profile
        or type(value["calls"]) is not dict
        or not value["calls"]
        or not all(type(name) is str and CALL.fullmatch(name) for name in value["calls"])
        or not all(type(call) is dict for call in value["calls"].values())
    ):
        _fail("plan_witness_shape", f"{binding} witness has the wrong identity or shape")
    calls: dict[str, Any] = {}
    for name, call in value["calls"].items():
        if (
            call.get("source") == "exchange"
            and set(call) == {"source", "requests"}
            and type(call["requests"]) is list
            and len(call["requests"]) == 1
        ):
            calls[name] = [
                entry for request in call["requests"] for entry in checked_exchange(request)
            ]
        elif (
            call.get("source") == "trace"
            and set(call) == {"source", "plans"}
            and type(call["plans"]) is list
        ):
            calls[name] = [
                {"plan": plan, "rows": plan.get("given_rows", []) if type(plan) is dict else []}
                for plan in call["plans"]
            ]
        else:
            _fail(
                "plan_witness_call",
                "each call requires actual exchange bytes or the internal trace",
            )
    for entries in calls.values():
        if not entries:
            _fail("plan_witness_call", "each selected call must retain its actual native plans")
        for entry in entries:
            plan = entry["plan"]
            if (
                type(plan) is not dict
                or plan.get("format") != "typebridge.query-plan/v3"
                or type(plan.get("managed_semantics")) is not dict
                or plan["managed_semantics"].get("semantic_profile") != profile
            ):
                _fail(
                    "plan_witness_profile",
                    "each captured canonical plan must use the selected profile",
                )
    calls = logical_iids(calls, iid_records(value["records"], named_records(suite, root=root)))
    return {
        name: [entry | {"plan": alpha_bindings(entry["plan"])} for entry in entries]
        for name, entries in calls.items()
    }


def native_witness(
    suite: str,
    profile: str,
    binding: str,
    trace: Path,
    output: Path,
    *,
    identities: Path | None = None,
) -> None:
    """Publish the exact selected native lowerings from the private conformance sink."""
    calls: dict[str, Any] = {}
    for line in _regular_bytes(trace, "native trace", 16 * 1024 * 1024).splitlines():
        frame = _json_value(line, "native trace frame")
        if (
            type(frame) is not dict
            or set(frame) != {"call", "plans"}
            or type(frame["call"]) is not str
            or not CALL.fullmatch(frame["call"])
            or type(frame["plans"]) is not list
        ):
            _fail("plan_trace_frame", "native trace must retain named calls and ordered plans")
        calls.setdefault(frame["call"], {"source": "trace", "plans": []})["plans"].extend(
            frame["plans"]
        )
    records: dict[str, list[str]] = {}
    if identities is not None:
        for line in _regular_bytes(identities, "named native identities", 1024 * 1024).splitlines():
            identity = _json_value(line, "named native identity")
            if (
                type(identity) is not list
                or len(identity) != 2
                or not all(type(item) is str for item in identity)
            ):
                _fail(
                    "plan_record_identity",
                    "native identity records must retain a name and actual IID",
                )
            history = records.setdefault(identity[0], [])
            if identity[1] not in history:
                history.append(identity[1])
    value = {
        "format": FORMAT,
        "suite": suite,
        "binding": binding,
        "semantic_profile": profile,
        "records": records,
        "calls": calls,
    }
    witness_calls(value, suite, profile, binding)
    with output.open("xb") as destination:
        destination.write(canonical_json_bytes(value))


def merge_witnesses(
    suite: str,
    profile: str,
    binding: str,
    paths: list[Path],
    output: Path,
) -> None:
    """Combine disjoint selected programs while retaining every original call and identity."""
    value: dict[str, Any] = {
        "format": FORMAT,
        "suite": suite,
        "binding": binding,
        "semantic_profile": profile,
        "records": {},
        "calls": {},
    }
    for path in paths:
        fragment = _json_value(
            _regular_bytes(path, "native witness fragment", 16 * 1024 * 1024),
            "native witness fragment",
        )
        witness_calls(fragment, suite, profile, binding)
        if value["calls"].keys() & fragment["calls"].keys():
            _fail("plan_witness_call", "selected model programs must have disjoint call names")
        value["calls"].update(fragment["calls"])
        for name, identities in fragment["records"].items():
            history = value["records"].setdefault(name, [])
            history.extend(identity for identity in identities if identity not in history)
    witness_calls(value, suite, profile, binding)
    with output.open("xb") as destination:
        destination.write(canonical_json_bytes(value))


def compare_twins(
    suite: str, profile: str, paths: dict[str, Path], *, root: Path = ROOT
) -> dict[str, Any]:
    """Require every binding's actual stacks to equal each other and the committed twin bytes."""
    if set(paths) != set(BINDINGS):
        _fail("plan_witness_bindings", "plan parity requires exactly the four SDK bindings")
    calls = {
        binding: witness_calls(
            _json_value(
                _regular_bytes(path, f"{binding} plan witness", 16 * 1024 * 1024), "plan witness"
            ),
            suite,
            profile,
            binding,
            root=root,
        )
        for binding, path in paths.items()
    }
    coverage_path = root / f"tests/contracts/sdk_conformance/{suite}/plans/coverage.json"
    coverage = _load_json(coverage_path, "selected plan coverage").value
    journey = _load_json(
        root / f"tests/contracts/sdk_conformance/{suite}/journey.json", "journey"
    ).value
    if (
        type(coverage) is not dict
        or set(coverage) != {"format", "suite", "observations"}
        or coverage["format"] != "typebridge.sdk-plan-coverage/v1"
        or coverage["suite"] != suite
        or type(coverage["observations"]) is not dict
        or set(coverage["observations"]) != set(journey["expected_observations"])
    ):
        _fail(
            "plan_coverage_inventory",
            "coverage must annotate every unchanged journey observation",
        )
    required: dict[str, set[str]] = {}
    for observation in coverage["observations"].values():
        if (
            type(observation) is not dict
            or type(observation.get("calls")) is not list
            or observation.get("kind")
            not in {"native", "host_empty", "raw_remainder", "answer_gate_auxiliary"}
            or not all(type(call) is str and CALL.fullmatch(call) for call in observation["calls"])
            or (observation["kind"] == "native") != bool(observation["calls"])
            or (observation["kind"] != "native" and not observation.get("reason"))
        ):
            _fail(
                "plan_coverage_inventory",
                "each observation needs selected native calls or an explicit empty/raw/auxiliary reason",
            )
        bindings = observation.get("bindings", list(BINDINGS))
        if (
            type(bindings) is not list
            or len(bindings) < 2
            or not all(type(binding) is str for binding in bindings)
            or len(bindings) != len(set(bindings))
            or not set(bindings) <= set(BINDINGS)
            or ("bindings" in observation and observation["kind"] != "native")
        ):
            _fail("plan_coverage_inventory", "native coverage must name its exposed SDK bindings")
        for call in observation["calls"]:
            selected = set(bindings)
            if call in required and required[call] != selected:
                _fail("plan_coverage_inventory", "one call has conflicting binding coverage")
            required[call] = selected
    for binding, actual in calls.items():
        selected = {call for call, bindings in required.items() if binding in bindings}
        if set(actual) != selected:
            _fail(
                "plan_coverage_inventory",
                f"{binding} native call witnesses differ from its required coverage",
            )
    if suite == "models":
        for binding, actual in calls.items():
            for call, entries in actual.items():
                if len(entries) != 1:
                    _fail(
                        "plan_manager_count",
                        f"{binding}/{call} must execute exactly one native plan",
                    )
    expected = {}
    for call, bindings in required.items():
        reference = (
            "rust"
            if "rust" in bindings
            else next(binding for binding in BINDINGS if binding in bindings)
        )
        expected[call] = calls[reference][call]
        for binding in bindings:
            if canonical_json_bytes(calls[binding][call]) != canonical_json_bytes(expected[call]):
                _fail(
                    "plan_binding_drift",
                    f"{suite}/{profile} {binding}/{call} plan stack differs from {reference}",
                )
    directory = root / f"tests/contracts/sdk_conformance/{suite}/plans" / profile.replace("/", "_")
    if set(path.stem for path in directory.glob("*.json")) != set(expected):
        _fail("plan_twin_inventory", f"{suite}/{profile} twin files differ from the measured calls")
    for call, stacks in expected.items():
        path = directory / f"{call}.json"
        if path.read_bytes() != canonical_json_bytes(stacks):
            _fail("plan_twin_drift", f"{suite}/{profile}/{call} differs from its committed twin")
    return {"bindings": list(BINDINGS), "calls": len(expected), "semantic_profile": profile}


def compare_oracle_answers(
    suite: str, reports: dict[str, dict[str, Any]], *, root: Path = ROOT
) -> None:
    """Keep the frozen pre-rewire observations as an independent oracle."""
    capture = _load_json(
        root / f"tests/contracts/sdk_conformance/parity/oracle-answers/{suite}.json",
        "frozen oracle answers",
    ).value
    for binding in BINDINGS:
        order = itemgetter("case_id", "proof_kind")
        actual = sorted(reports[binding]["results"], key=order)
        expected = sorted(capture["bindings"][binding], key=order)
        if canonical_json_bytes(actual) != canonical_json_bytes(expected):
            _fail(
                "oracle_answer_drift",
                f"{suite}/{binding} observations differ from the frozen oracle answers",
            )
