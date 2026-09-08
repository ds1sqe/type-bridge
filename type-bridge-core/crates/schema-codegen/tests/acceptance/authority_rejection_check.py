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
SOURCE = STAGE / "generated_ordered"
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


EXPECTED_DIAGNOSTIC = {
    "category": "integrity",
    "sdk_category": "integrity",
    "code": "projection_evidence_mismatch",
    "message": "Generated projection evidence does not match the verified schema package",
    "path": [{"kind": "argument", "value": "projection_evidence"}],
}


def reject(name: str, authority: str) -> None:
    package_name = f"generated_rejected_{name}"
    package = STAGE / package_name
    shutil.rmtree(package, ignore_errors=True)
    shutil.copytree(SOURCE, package)
    (package / "_authority.py").write_text(
        "from __future__ import annotations\n\n"
        "from typing import Final as _Final\n\n"
        f"{PREFIX}{authority!r}{SUFFIX}\n"
    )
    probe = (
        "import importlib, json\n"
        "try:\n"
        f"    importlib.import_module({package_name!r})\n"
        "except BaseException as error:\n"
        "    print(json.dumps({\n"
        "        'category': error.category,\n"
        "        'sdk_category': error.sdk_category,\n"
        "        'code': error.code,\n"
        "        'message': error.message,\n"
        "        'path': error.path,\n"
        "    }, sort_keys=True))\n"
        "else:\n"
        "    raise AssertionError('hostile ordered package import succeeded')\n"
    )
    completed = subprocess.run(
        [sys.executable, "-c", probe],
        cwd=STAGE,
        capture_output=True,
        text=True,
        check=False,
    )
    try:
        diagnostic = json.loads(completed.stdout)
    except json.JSONDecodeError:
        diagnostic = None
    if completed.returncode != 0 or diagnostic != EXPECTED_DIAGNOSTIC:
        raise AssertionError(
            f"{name} authority returned {completed.returncode}, "
            f"expected {EXPECTED_DIAGNOSTIC!r}, received {diagnostic!r}\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )


ordered_models = (SOURCE / "_models.py").read_text()
native_install = ordered_models.index("\n_install_runtime_projection(\n")
if "from ._query import " in ordered_models[:native_install]:
    raise AssertionError("ordered models imported query authority before native admission")
query_aliases = ordered_models.find("\nfrom ._query import ", native_install)
if query_aliases != -1 and query_aliases <= native_install:
    raise AssertionError("ordered model query aliases were not deferred past native admission")

ordered_init = (SOURCE / "__init__.py").read_text()
if ordered_init.index("from ._models import ") >= ordered_init.index("from ._query import "):
    raise AssertionError("ordered package init did not trigger model admission before query")
if (SOURCE / "_query.py").read_bytes() != (STAGE / "generated_v2" / "_query.py").read_bytes():
    raise AssertionError("ordered package changed the fixed Python query resource")

completed = subprocess.run(
    [sys.executable, "-c", "import generated_ordered"],
    cwd=STAGE,
    capture_output=True,
    text=True,
    check=False,
)
if completed.returncode != 0:
    raise AssertionError(
        "exact ordered generated package import failed\n"
        f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
    )

reject("malformed", "{")
reject("foreign", envelope(FOREIGN))
reject(
    "stale",
    mutated(
        lambda value: value["content"]["declared_identity"].__setitem__("digest", "0" * 64),
        resign_after=False,
    ),
)
reject(
    "missing_fingerprint",
    mutated(lambda value: value.pop("authority_fingerprint"), resign_after=False),
)
reject(
    "managed_state",
    mutated(
        lambda value: value["content"]["managed_state"]["managed_semantic_schema"].__setitem__(
            "digest", "0" * 64
        ),
        resign_after=True,
    ),
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
)
reject(
    "version",
    mutated(
        lambda value: value["content"].__setitem__(
            "authority_version", "typebridge.schema-authority/v2"
        ),
        resign_after=True,
    ),
)
reject(
    "oversize",
    " " * (MAX_SCHEMA_AUTHORITY_BYTES + 1),
)

print("generated Python authority rejection acceptance passed")
