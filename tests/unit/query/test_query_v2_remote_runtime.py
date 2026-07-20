"""Offline unit coverage for the prepared V2 remote binding surface.

The declared-schema and plan bytes are the canonical fixtures the
cross-binding smoke test uses; everything here runs without TypeDB.
"""

from __future__ import annotations

import base64
import json

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
NONCE = "runtime-unit-nonce-0123456789"
INVOCATION = json.dumps({"operation": "rows", "rows": []})


def _canonical(payload: dict) -> bytes:
    return json.dumps(payload, separators=(",", ":"), sort_keys=True).encode()


def _advertisement(capabilities: list[str]) -> bytes:
    return _canonical(
        {
            "capabilities": capabilities,
            "format": "typebridge.query-remote-capabilities/v1",
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


def test_remote_capabilities_decode_and_reject_malformed_bytes() -> None:
    assert core.query_v2_remote_capabilities(_advertisement(PLAN_CAPABILITIES)) == (
        PLAN_CAPABILITIES
    )
    with pytest.raises(ValueError, match="malformed_canonical_json"):
        core.query_v2_remote_capabilities(b"not an advertisement")


def test_encode_checks_capabilities_against_the_exact_advertisement() -> None:
    authority = _authority()
    plan = base64.b64decode(PLAN_B64)
    request = core.query_v2_encode_remote_request(
        authority, plan, INVOCATION, _advertisement(PLAN_CAPABILITIES), NONCE, 10, 1 << 20
    )
    assert b"typebridge.query-remote-request/v1" in bytes(request)

    starved = _advertisement([c for c in PLAN_CAPABILITIES if c != "query.stage.sort"])
    with pytest.raises(ValueError, match="query_remote_capability_unsupported"):
        core.query_v2_encode_remote_request(
            authority, plan, INVOCATION, starved, NONCE, 10, 1 << 20
        )


def test_negative_and_oversized_limits_fail_with_the_stable_diagnostic() -> None:
    authority = _authority()
    plan = base64.b64decode(PLAN_B64)
    advertisement = _advertisement(PLAN_CAPABILITIES)
    for max_items, max_bytes, deadline in [
        (-1, 1 << 20, None),
        (10, -1, None),
        (10, 1 << 20, -1),
        (1 << 64, 1 << 20, None),
    ]:
        with pytest.raises(ValueError, match="query_remote_limit_invalid"):
            core.query_v2_encode_remote_request(
                authority, plan, INVOCATION, advertisement, NONCE, max_items, max_bytes, deadline
            )
        with pytest.raises(ValueError, match="query_remote_limit_invalid"):
            core.query_v2_decode_remote_outcome(
                authority, plan, INVOCATION, b"{}", NONCE, max_items, max_bytes, deadline
            )


def test_failure_envelopes_surface_their_stable_server_diagnostic() -> None:
    authority = _authority()
    plan = base64.b64decode(PLAN_B64)
    failure = _canonical(
        {
            "category": "resource_limit",
            "code": "query_remote_deadline_exceeded",
            "format": "typebridge.query-remote-failure/v1",
            "message": "the executor deadline elapsed",
            "nonce": None,
            "request": None,
        }
    )
    with pytest.raises(ValueError, match="query_remote_deadline_exceeded"):
        core.query_v2_decode_remote_outcome(
            authority, plan, INVOCATION, failure, NONCE, 10, 1 << 20
        )


def test_replies_bound_to_a_foreign_nonce_are_rejected_not_surfaced() -> None:
    authority = _authority()
    plan = base64.b64decode(PLAN_B64)
    foreign = _canonical(
        {
            "category": "resource_limit",
            "code": "query_remote_deadline_exceeded",
            "format": "typebridge.query-remote-failure/v1",
            "message": "the executor deadline elapsed",
            "nonce": "some-other-nonce-9876543210zz",
            "request": None,
        }
    )
    with pytest.raises(ValueError, match="query_remote_nonce_mismatch"):
        core.query_v2_decode_remote_outcome(
            authority, plan, INVOCATION, foreign, NONCE, 10, 1 << 20
        )
