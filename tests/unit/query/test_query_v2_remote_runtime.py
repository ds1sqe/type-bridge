"""Offline unit coverage for the prepared V2 remote binding surface.

The declared-schema and plan bytes are the canonical fixtures the
cross-binding smoke test uses; everything here runs without TypeDB.
"""

from __future__ import annotations

import base64
import hashlib
import json
import struct
import sys
import threading
from pathlib import Path
from typing import cast

import pytest
import type_bridge_core as core

DECLARED_B64 = (
    "eyJkZWNsYXJlZF9pZGVudGl0eSI6eyJhbGdvcml0aG0iOiJzaGEyNTYiLCJjYW5vbmljYWxpemF0aW9u"
    "IjoidHlwZWJyaWRnZS5zY2hlbWEtY2Fub25pY2FsLWpzb24vdjEiLCJkaWdlc3QiOiJiZGFiNzEzOGE1"
    "NzIzOGVlMjNkZmNlYjY5ZTdmMDk4OTNjZmE3YjUzNmQ5ZTcwMzU2ZDFhOTg2YTEzMjQ5OWZlIiwiZG9t"
    "YWluIjoidHlwZWJyaWRnZS5zY2hlbWEuZGVjbGFyZWQtaWRlbnRpdHkifSwiZmFjdHMiOlt7ImtpbmQi"
    "OiJ0eXBlIiwidmFsdWUiOnsiaWQiOnsia2luZCI6ImVudGl0eSIsImxhYmVsIjoic21va2UtcGVyc29u"
    "In19fSx7ImtpbmQiOiJ0eXBlIiwidmFsdWUiOnsiaWQiOnsia2luZCI6ImF0dHJpYnV0ZSIsImxhYmVs"
    "Ijoic21va2UtbmFtZSJ9fX0seyJraW5kIjoidmFsdWUiLCJ2YWx1ZSI6eyJpZCI6InNtb2tlLW5hbWUi"
    "LCJ2YWx1ZV90eXBlIjoic3RyaW5nIn19LHsia2luZCI6Im93bnMiLCJ2YWx1ZSI6eyJpZCI6eyJhdHRy"
    "aWJ1dGUiOiJzbW9rZS1uYW1lIiwib3duZXIiOnsia2luZCI6ImVudGl0eSIsImxhYmVsIjoic21va2Ut"
    "cGVyc29uIn19fX1dLCJmb3JtYXRfdmVyc2lvbiI6MSwicmVxdWlyZWRfY2FwYWJpbGl0aWVzIjpbXX0="
)
PLAN_B64 = (
    "eyJiaW5kaW5ncyI6W3siaWQiOjAsInZhcmlhYmxlIjoicGVyc29uIn0seyJpZCI6MSwidmFyaWFibGUi"
    "OiJuYW1lIn1dLCJmb3JtYXQiOiJ0eXBlYnJpZGdlLnF1ZXJ5LXBsYW4vdjEiLCJmdW5jdGlvbnMiOltd"
    "LCJpbnB1dHMiOltdLCJtYW5hZ2VkX3NlbWFudGljcyI6eyJhbGdvcml0aG0iOiJzaGEyNTYiLCJjYW5v"
    "bmljYWxpemF0aW9uIjoidHlwZWJyaWRnZS5tYW5hZ2VkLXNlbWFudGljL3YxIiwiZGlnZXN0IjoiNTE2"
    "MDViOWYyNWQ5MDFiNjlhOGVkMmIwNGQ2NGZkN2IwMWJkZTZjOTIwYzJiMzA3YzJiMDE2NDRhMDc2ODk0"
    "YiIsImRvbWFpbiI6InR5cGVicmlkZ2Uuc2NoZW1hLm1hbmFnZWQtc2VtYW50aWMiLCJzZW1hbnRpY19w"
    "cm9maWxlIjoidHlwZWRiLTMuMTIuMS92MSJ9LCJvdXRwdXQiOnsiY29sdW1ucyI6WzAsMV0sImtpbmQi"
    "OiJyb3dzIn0sInBpcGVsaW5lIjpbeyJraW5kIjoibWF0Y2giLCJwYXR0ZXJucyI6W3siYmluZGluZyI6"
    "MCwiaW5jbHVkZV9zdWJ0eXBlcyI6dHJ1ZSwia2luZCI6ImlzYSIsInR5cGVfaWQiOnsia2luZCI6ImVu"
    "dGl0eSIsImxhYmVsIjoic21va2UtcGVyc29uIn19LHsiYXR0cmlidXRlIjoxLCJhdHRyaWJ1dGVfaWQi"
    "OiJzbW9rZS1uYW1lIiwia2luZCI6ImhhcyIsIm93bmVyIjowfV19LHsia2luZCI6InNvcnQiLCJ0ZXJt"
    "cyI6W3siYmluZGluZyI6MSwiZGlyZWN0aW9uIjoiYXNjZW5kaW5nIn1dfV0sInJlcXVpcmVkX2NhcGFi"
    "aWxpdGllcyI6WyJxdWVyeS5vdXRwdXQucm93cyIsInF1ZXJ5LnBhdHRlcm4uaGFzIiwicXVlcnkucGF0"
    "dGVybi5pc2EiLCJxdWVyeS5wYXR0ZXJuLmlzYS1zdWJ0eXBlcyIsInF1ZXJ5LnBsYW4iLCJxdWVyeS5z"
    "dGFnZS5zb3J0Il19"
)
SCOPE = "binding-smoke"
PROFILE = "typedb-3.12.1/v1"
INVOCATION = json.dumps({"operation": "rows", "rows": []})
FAILURE_CORPUS = json.loads(
    (Path(__file__).parents[3] / "tests/fixtures/query-v2-remote-failures.json").read_text(
        encoding="utf-8"
    )
)


def _canonical(payload: dict) -> bytes:
    return json.dumps(payload, separators=(",", ":"), sort_keys=True).encode()


_ED_FIELD = 2**255 - 19
_ED_ORDER = 2**252 + 27742317777372353535851937790883648493
_ED_D = (-121665 * pow(121666, _ED_FIELD - 2, _ED_FIELD)) % _ED_FIELD
_ED_I = pow(2, (_ED_FIELD - 1) // 4, _ED_FIELD)
_SIGNING_SEED = b"\x42" * 32


def _ed_xrecover(y: int) -> int:
    xx = ((y * y - 1) * pow(_ED_D * y * y + 1, _ED_FIELD - 2, _ED_FIELD)) % _ED_FIELD
    x = pow(xx, (_ED_FIELD + 3) // 8, _ED_FIELD)
    if (x * x - xx) % _ED_FIELD:
        x = (x * _ED_I) % _ED_FIELD
    return _ED_FIELD - x if x & 1 else x


_ED_BASE_Y = (4 * pow(5, _ED_FIELD - 2, _ED_FIELD)) % _ED_FIELD
_ED_BASE_X = _ed_xrecover(_ED_BASE_Y)
_ED_BASE = (_ED_BASE_X, _ED_BASE_Y, 1, (_ED_BASE_X * _ED_BASE_Y) % _ED_FIELD)
_ED_IDENTITY = (0, 1, 1, 0)


def _ed_add(
    left: tuple[int, int, int, int], right: tuple[int, int, int, int]
) -> tuple[int, int, int, int]:
    x1, y1, z1, t1 = left
    x2, y2, z2, t2 = right
    a = ((y1 - x1) * (y2 - x2)) % _ED_FIELD
    b = ((y1 + x1) * (y2 + x2)) % _ED_FIELD
    c = (2 * _ED_D * t1 * t2) % _ED_FIELD
    d = (2 * z1 * z2) % _ED_FIELD
    e = b - a
    f = d - c
    g = d + c
    h = b + a
    return (
        (e * f) % _ED_FIELD,
        (g * h) % _ED_FIELD,
        (f * g) % _ED_FIELD,
        (e * h) % _ED_FIELD,
    )


def _ed_scale(point: tuple[int, int, int, int], scalar: int) -> tuple[int, int, int, int]:
    result = _ED_IDENTITY
    addend = point
    while scalar:
        if scalar & 1:
            result = _ed_add(result, addend)
        addend = _ed_add(addend, addend)
        scalar >>= 1
    return result


def _ed_encode(point: tuple[int, int, int, int]) -> bytes:
    x, y, z, _ = point
    inverse = pow(z, _ED_FIELD - 2, _ED_FIELD)
    x = (x * inverse) % _ED_FIELD
    y = (y * inverse) % _ED_FIELD
    return (y | ((x & 1) << 255)).to_bytes(32, "little")


_SIGNING_HASH = hashlib.sha512(_SIGNING_SEED).digest()
_SIGNING_SCALAR_BYTES = bytearray(_SIGNING_HASH[:32])
_SIGNING_SCALAR_BYTES[0] &= 248
_SIGNING_SCALAR_BYTES[31] &= 63
_SIGNING_SCALAR_BYTES[31] |= 64
_SIGNING_SCALAR = int.from_bytes(_SIGNING_SCALAR_BYTES, "little")
_SIGNING_PUBLIC_KEY = _ed_encode(_ed_scale(_ED_BASE, _SIGNING_SCALAR))
_SIGNING_KEY_ID = hashlib.sha256(
    b"typebridge.query.remote-reply-key-id/v1\0" + _SIGNING_PUBLIC_KEY
).hexdigest()


def _ed_sign(message: bytes) -> bytes:
    nonce = (
        int.from_bytes(hashlib.sha512(_SIGNING_HASH[32:] + message).digest(), "little") % _ED_ORDER
    )
    encoded_nonce = _ed_encode(_ed_scale(_ED_BASE, nonce))
    challenge = (
        int.from_bytes(
            hashlib.sha512(encoded_nonce + _SIGNING_PUBLIC_KEY + message).digest(), "little"
        )
        % _ED_ORDER
    )
    scalar = (nonce + challenge * _SIGNING_SCALAR) % _ED_ORDER
    return encoded_nonce + scalar.to_bytes(32, "little")


def _fingerprint(domain: bytes, canonicalization: bytes, payload: bytes) -> str:
    digest = hashlib.sha256()
    digest.update(b"typebridge.fingerprint/v1\0")
    for field in (domain, canonicalization):
        digest.update(struct.pack(">Q", len(field)))
        digest.update(field)
    digest.update(b"\0")
    digest.update(struct.pack(">Q", len(payload)))
    digest.update(payload)
    return digest.hexdigest()


def _signed_reply(payload: dict, advertisement: bytes) -> bytes:
    payload_bytes = _canonical(payload)
    advertisement_fingerprint = _fingerprint(
        b"typebridge.query.remote-capabilities",
        b"typebridge.query-remote-capabilities/v1",
        advertisement,
    )
    key = _SIGNING_PUBLIC_KEY.hex()
    prefix = (
        f'{{"advertisement":"{advertisement_fingerprint}",'
        '"format":"typebridge.query-remote-signed-reply/v1",'
        f'"key":"{key}","key_id":"{_SIGNING_KEY_ID}","payload":'
    ).encode()
    digest = hashlib.sha256(
        b"typebridge.query.remote-reply-signature/v1\0" + prefix + payload_bytes + b"}"
    ).digest()
    return prefix + payload_bytes + b',"signature":"' + _ed_sign(digest).hex().encode() + b'"}'


def _request_fingerprint(request: bytes, case: dict) -> str:
    """Independently bind a fixture failure to exact Rust request bytes."""

    digest = hashlib.sha256()
    digest.update(b"typebridge.fingerprint/v1\0")
    for field in (
        case["fingerprint_domain"].encode(),
        case["fingerprint_canonicalization"].encode(),
    ):
        digest.update(struct.pack(">Q", len(field)))
        digest.update(field)
    digest.update(b"\0")  # no semantic profile
    digest.update(struct.pack(">Q", len(request)))
    digest.update(request)
    return digest.hexdigest()


def _advertisement(capabilities: list[str]) -> bytes:
    return _canonical(
        {
            "capabilities": capabilities,
            "executor": {
                "epoch": "python-binding-epoch-0001",
                "identity": "python-binding-executor",
            },
            "format": "typebridge.query-remote-capabilities/v1",
            "reply_key": _SIGNING_PUBLIC_KEY.hex(),
            "reply_key_id": _SIGNING_KEY_ID,
        }
    )


PLAN_CAPABILITIES = [
    "query.output.rows",
    "query.pattern.has",
    "query.pattern.isa",
    "query.pattern.isa-subtypes",
    "query.plan",
    "query.stage.sort",
]


def _authority() -> core.QueryV2Authority:
    return core.query_v2_authority(base64.b64decode(DECLARED_B64), SCOPE, PROFILE)


def test_shared_failure_corpus_is_bound_to_the_same_authority_fixture() -> None:
    assert FAILURE_CORPUS["format"] == "typebridge.query-remote-binding-failure-corpus/v1"
    assert FAILURE_CORPUS["declared_b64"] == DECLARED_B64
    assert FAILURE_CORPUS["plan_b64"] == PLAN_B64
    assert FAILURE_CORPUS["scope"] == SCOPE
    assert FAILURE_CORPUS["profile"] == PROFILE
    assert FAILURE_CORPUS["invocation"] == json.loads(INVOCATION)
    assert FAILURE_CORPUS["capabilities"] == PLAN_CAPABILITIES


def test_remote_capabilities_decode_and_reject_malformed_bytes() -> None:
    assert core.query_v2_remote_capabilities(_advertisement(PLAN_CAPABILITIES)) == (
        PLAN_CAPABILITIES
    )
    with pytest.raises(ValueError, match="malformed_canonical_json"):
        core.query_v2_remote_capabilities(b"not an advertisement")


def test_encode_checks_capabilities_against_the_exact_advertisement() -> None:
    authority = _authority()
    plan = base64.b64decode(PLAN_B64)
    pending = core.query_v2_prepare_remote(
        authority,
        plan,
        INVOCATION,
        _advertisement(PLAN_CAPABILITIES),
        10,
        1 << 20,
        1_000,
    )
    request = pending.request_bytes()
    assert b"typebridge.query-remote-request/v1" in bytes(request)
    parsed_request = json.loads(bytes(request))
    assert len(parsed_request["advertisement"]) == 64
    assert parsed_request["expires_at_unix_ms"] > parsed_request["prepared_at_unix_ms"]
    second = core.query_v2_prepare_remote(
        authority,
        plan,
        INVOCATION,
        _advertisement(PLAN_CAPABILITIES),
        10,
        1 << 20,
        1_000,
    )
    first_nonce = parsed_request["nonce"]
    second_nonce = json.loads(bytes(second.request_bytes()))["nonce"]
    assert len(first_nonce) == 32
    assert all(character in "0123456789abcdef" for character in first_nonce)
    assert first_nonce != second_nonce

    starved = _advertisement([c for c in PLAN_CAPABILITIES if c != "query.stage.sort"])
    with pytest.raises(ValueError, match="query_remote_capability_unsupported"):
        core.query_v2_prepare_remote(authority, plan, INVOCATION, starved, 10, 1 << 20, 1_000)


def test_negative_and_oversized_limits_fail_with_the_stable_diagnostic() -> None:
    authority = _authority()
    plan = base64.b64decode(PLAN_B64)
    advertisement = _advertisement(PLAN_CAPABILITIES)
    for max_items, max_bytes, max_collection_members, deadline in [
        (-1, 1 << 20, 1_000, None),
        (10, -1, 1_000, None),
        (10, 1 << 20, -1, None),
        (10, 1 << 20, 1_000, -1),
        (1 << 64, 1 << 20, 1_000, None),
        (1 << 200, 1 << 20, 1_000, None),
        (-(1 << 200), 1 << 20, 1_000, None),
        (10, 1 << 20, 1 << 200, None),
    ]:
        with pytest.raises(ValueError, match="query_remote_limit_invalid"):
            core.query_v2_prepare_remote(
                authority,
                plan,
                INVOCATION,
                advertisement,
                max_items,
                max_bytes,
                max_collection_members,
                deadline,
            )

    with pytest.raises(ValueError, match="query_remote_deadline_limit"):
        core.query_v2_prepare_remote(
            authority,
            plan,
            INVOCATION,
            advertisement,
            10,
            1 << 20,
            1_000,
            86_400_001,
        )


@pytest.mark.parametrize("case", FAILURE_CORPUS["cases"], ids=lambda case: case["name"])
def test_failure_binding_corpus_is_rejected_before_diagnostics_surface(case: dict) -> None:
    assert FAILURE_CORPUS["format"] == "typebridge.query-remote-binding-failure-corpus/v1"
    authority = _authority()
    plan = base64.b64decode(PLAN_B64)
    advertisement = _advertisement(PLAN_CAPABILITIES)
    pending = core.query_v2_prepare_remote(
        authority,
        plan,
        INVOCATION,
        advertisement,
        10,
        1 << 20,
        1_000,
    )
    failure = _signed_reply(case["reply"], advertisement)
    with pytest.raises(ValueError, match=case["expected"]):
        pending.decode_reply(failure)
    with pytest.raises(ValueError, match=case["replay_expected"]):
        pending.decode_reply(failure)


def test_success_byte_budget_authenticates_first_and_keeps_failures_decodable() -> None:
    authority = _authority()
    plan = base64.b64decode(PLAN_B64)
    advertisement = _advertisement(PLAN_CAPABILITIES)
    case = FAILURE_CORPUS["bound_case"]
    forged_pending = core.query_v2_prepare_remote(
        authority,
        plan,
        INVOCATION,
        advertisement,
        10,
        0,
        1_000,
    )
    forged_request = bytes(forged_pending.request_bytes())
    forged_nonce = json.loads(forged_request)["nonce"]
    signed_success = _signed_reply(
        {
            "format": "typebridge.query-remote-response/v1",
            "nonce": forged_nonce,
            "outcome": {"kind": "rows", "rows": []},
            "plan": "0" * 64,
            "request": _request_fingerprint(forged_request, case),
        },
        advertisement,
    )
    tampered = bytearray(signed_success)
    signature = signed_success.index(b'"signature":"') + len(b'"signature":"')
    tampered[signature] = ord("0") if tampered[signature] != ord("0") else ord("1")
    with pytest.raises(ValueError, match="query_remote_signature_invalid"):
        forged_pending.decode_reply(tampered)

    wrong_request_pending = core.query_v2_prepare_remote(
        authority,
        plan,
        INVOCATION,
        advertisement,
        10,
        0,
        1_000,
    )
    wrong_request = bytes(wrong_request_pending.request_bytes())
    wrong_request_nonce = json.loads(wrong_request)["nonce"]
    plan_fingerprint = _fingerprint(
        b"typebridge.query.plan",
        b"typebridge.query-plan-c14n/v1",
        plan,
    )
    signed_wrong_request = _signed_reply(
        {
            "format": "typebridge.query-remote-response/v1",
            "nonce": wrong_request_nonce,
            "outcome": {"kind": "rows", "rows": []},
            "plan": plan_fingerprint,
            "request": "0" * 64,
        },
        advertisement,
    )
    with pytest.raises(ValueError, match="query_remote_request_mismatch"):
        wrong_request_pending.decode_reply(signed_wrong_request)

    valid_pending = core.query_v2_prepare_remote(
        authority,
        plan,
        INVOCATION,
        advertisement,
        10,
        0,
        1_000,
    )
    valid_request = bytes(valid_pending.request_bytes())
    valid_nonce = json.loads(valid_request)["nonce"]
    signed_failure = _signed_reply(
        {
            **case["diagnostic"],
            "nonce": valid_nonce,
            "request": _request_fingerprint(valid_request, case),
        },
        advertisement,
    )
    assert len(signed_failure) > 1  # max_bytes=0 must not truncate failure evidence.
    with pytest.raises(ValueError) as error:
        valid_pending.decode_reply(signed_failure)
    assert str(error.value) == (f"{case['diagnostic']['code']}: {case['diagnostic']['message']}")


def test_request_bound_failure_surfaces_identically_and_consumes_reply_handle() -> None:
    case = FAILURE_CORPUS["bound_case"]
    authority = _authority()
    plan = base64.b64decode(PLAN_B64)
    pending = core.query_v2_prepare_remote(
        authority,
        plan,
        INVOCATION,
        (advertisement := _advertisement(PLAN_CAPABILITIES)),
        case["max_items"],
        1 << 20,
        1_000,
    )
    request = bytes(pending.request_bytes())
    parsed_request = json.loads(request)
    assert parsed_request["limits"]["max_items"] == 1
    reply = {
        **case["diagnostic"],
        "nonce": parsed_request["nonce"],
        "request": _request_fingerprint(request, case),
    }
    failure = _signed_reply(reply, advertisement)

    with pytest.raises(ValueError) as error:
        pending.decode_reply(failure)
    assert str(error.value) == (f"{case['diagnostic']['code']}: {case['diagnostic']['message']}")
    with pytest.raises(ValueError, match=case["replay_expected"]):
        pending.decode_reply(failure)


def test_invalid_first_reply_type_consumes_claim_and_replay_wins_before_type_inspection() -> None:
    case = FAILURE_CORPUS["bound_case"]
    pending = core.query_v2_prepare_remote(
        _authority(),
        base64.b64decode(PLAN_B64),
        INVOCATION,
        _advertisement(PLAN_CAPABILITIES),
        case["max_items"],
        1 << 20,
        1_000,
    )

    invalid_response = cast(bytes | bytearray, object())
    with pytest.raises(TypeError, match="argument 'response' must be bytes or bytearray"):
        pending.decode_reply(invalid_response)
    with pytest.raises(ValueError, match=case["replay_expected"]):
        pending.decode_reply(invalid_response)


def test_concurrent_reply_decodes_admit_exactly_one_one_shot_claimant() -> None:
    case = FAILURE_CORPUS["bound_case"]
    advertisement = _advertisement(PLAN_CAPABILITIES)
    pending = core.query_v2_prepare_remote(
        _authority(),
        base64.b64decode(PLAN_B64),
        INVOCATION,
        advertisement,
        case["max_items"],
        1 << 20,
        1_000,
    )
    request = bytes(pending.request_bytes())
    parsed_request = json.loads(request)
    failure = _signed_reply(
        {
            **case["diagnostic"],
            "nonce": parsed_request["nonce"],
            "request": _request_fingerprint(request, case),
        },
        advertisement,
    )
    gate = threading.Barrier(3)
    messages: list[str] = []

    def decode() -> None:
        gate.wait()
        try:
            pending.decode_reply(failure)
        except ValueError as error:
            messages.append(str(error))

    workers = [threading.Thread(target=decode) for _ in range(2)]
    for worker in workers:
        worker.start()
    gate.wait()
    for worker in workers:
        worker.join(timeout=5)
        assert not worker.is_alive()

    expected = f"{case['diagnostic']['code']}: {case['diagnostic']['message']}"
    assert messages.count(expected) == 1
    assert sum(case["replay_expected"] in message for message in messages) == 1


def test_large_forged_reply_is_binding_rejected_while_the_gil_is_released() -> None:
    authority = _authority()
    pending = core.query_v2_prepare_remote(
        authority,
        base64.b64decode(PLAN_B64),
        INVOCATION,
        (advertisement := _advertisement(PLAN_CAPABILITIES)),
        10,
        32 * 1024 * 1024,
        1_000,
    )
    forged = _signed_reply(
        {
            "format": "typebridge.query-remote-response/v1",
            "nonce": "foreign-nonce-0123456789abcdef",
            "outcome": "x" * (16 * 1024 * 1024),
            "plan": "0" * 64,
            "request": "0" * 64,
        },
        advertisement,
    )
    start = threading.Event()
    progressed = threading.Event()

    def probe() -> None:
        start.wait()
        progressed.set()

    worker = threading.Thread(target=probe)
    worker.start()
    previous_switch_interval = sys.getswitchinterval()
    sys.setswitchinterval(60.0)
    try:
        start.set()
        with pytest.raises(ValueError, match="query_remote_nonce_mismatch"):
            pending.decode_reply(forged)
        assert progressed.is_set(), "reply decoding held the GIL"
    finally:
        sys.setswitchinterval(previous_switch_interval)
        start.set()
        worker.join(timeout=5)
