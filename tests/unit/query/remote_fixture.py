"""Shared authority and authenticated remote fixtures for native plan tests."""

from __future__ import annotations

import base64
import hashlib
import json
import struct

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
SCOPE = "binding-smoke"
PROFILE = "typedb-3.12.1/v1"


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


# Native read plan and staged envelope capabilities.
PLAN_CAPABILITIES = [
    "query.output.rows",
    "query.pattern.has",
    "query.pattern.isa",
    "query.pattern.isa-subtypes",
    "query.plan",
    "query.plan.v3",
    "query.remote.envelope-v3",
    "query.remote.structured-diagnostic",
    "query.stage.sort",
]


def _authority() -> core.PlanAuthority:
    return core.plan_authority(base64.b64decode(DECLARED_B64), SCOPE, PROFILE)
