from __future__ import annotations

import ast
import hashlib
import json
import shutil
import struct
import subprocess
import sys
from collections.abc import Callable
from pathlib import Path

STAGE = Path(__file__).resolve().parent
SOURCE = STAGE / "generated_v2"
FOREIGN = STAGE / "generated_variant"
MAX_SCHEMA_AUTHORITY_BYTES = 16 * 1024 * 1024
PREFIX = "SCHEMA_AUTHORITY_BYTES: _Final[bytes] = "
SUFFIX = '.encode("utf-8")'


def canonical(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def envelope(package: Path) -> str:
    line = next(
        line
        for line in (package / "_authority.py").read_text().splitlines()
        if line.startswith(PREFIX)
    )
    expression = line.removeprefix(PREFIX).removesuffix(SUFFIX)
    value = ast.literal_eval(expression)
    if not isinstance(value, str):
        raise AssertionError("generated authority constant is not a string")
    return value


def fingerprint(content: object) -> str:
    digest = hashlib.sha256()
    digest.update(b"typebridge.fingerprint/v1\0")
    for value in (
        b"typebridge.schema.authority",
        b"typebridge.schema-authority/v1",
    ):
        digest.update(struct.pack(">Q", len(value)))
        digest.update(value)
    digest.update(b"\0")
    payload = canonical(content)
    digest.update(struct.pack(">Q", len(payload)))
    digest.update(payload)
    return digest.hexdigest()


def resign(value: dict[str, object]) -> None:
    authority_fingerprint = value["authority_fingerprint"]
    if not isinstance(authority_fingerprint, dict):
        raise AssertionError("authority fingerprint is not an object")
    authority_fingerprint["digest"] = fingerprint(value["content"])


def mutated(change: Callable[[dict[str, object]], None], *, resign_after: bool) -> str:
    value = json.loads(envelope(SOURCE))
    change(value)
    if resign_after:
        resign(value)
    return canonical(value).decode()


def reject(name: str, authority: str, expected: str) -> None:
    package_name = f"generated_rejected_{name}"
    package = STAGE / package_name
    shutil.rmtree(package, ignore_errors=True)
    shutil.copytree(SOURCE, package)
    (package / "_authority.py").write_text(
        "from __future__ import annotations\n\n"
        "from typing import Final as _Final\n\n"
        f"{PREFIX}{authority!r}{SUFFIX}\n"
    )
    completed = subprocess.run(
        [sys.executable, "-c", f"import {package_name}"],
        cwd=STAGE,
        capture_output=True,
        text=True,
        check=False,
    )
    output = completed.stdout + completed.stderr
    if completed.returncode == 0 or expected not in output:
        raise AssertionError(
            f"{name} authority returned {completed.returncode}, expected {expected!r}\n{output}"
        )


reject("malformed", "{", "malformed_canonical_json")
reject(
    "foreign",
    envelope(FOREIGN),
    "generated_schema_authority_semantic_mismatch",
)
reject(
    "stale",
    mutated(
        lambda value: value["content"]["declared_identity"].__setitem__("digest", "0" * 64),
        resign_after=False,
    ),
    "generated_schema_authority_integrity_mismatch",
)
reject(
    "missing_fingerprint",
    mutated(lambda value: value.pop("authority_fingerprint"), resign_after=False),
    "invalid_canonical_value",
)
reject(
    "managed_state",
    mutated(
        lambda value: value["content"]["managed_state"]["managed_semantic_schema"].__setitem__(
            "digest", "0" * 64
        ),
        resign_after=True,
    ),
    "generated_schema_authority_integrity_mismatch",
)
reject(
    "capability",
    mutated(
        lambda value: (
            value["content"]["required_capabilities"].append("query.future-feature"),
            value["content"]["required_capabilities"].sort(),
        ),
        resign_after=True,
    ),
    "unsupported_required_capability",
)
reject(
    "version",
    mutated(
        lambda value: value["content"].__setitem__(
            "authority_version", "typebridge.schema-authority/v2"
        ),
        resign_after=True,
    ),
    "generated_schema_authority_unsupported_version",
)
reject(
    "oversize",
    " " * (MAX_SCHEMA_AUTHORITY_BYTES + 1),
    "canonical_json_too_large",
)

print("generated Python authority rejection acceptance passed")
