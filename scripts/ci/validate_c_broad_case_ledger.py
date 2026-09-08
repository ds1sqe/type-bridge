#!/usr/bin/env python3
"""Validate the digest-bound C behavioral coverage ledger."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path, PurePosixPath
from typing import Any, NoReturn

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_LEDGER = ROOT / "tests/contracts/c-broad-case-ledger-v1.json"
CAPABILITIES = ("G05", "G06", "G08", "G13")
SLICES = (
    "query-remote",
    "data-model",
    "admin-migration",
    "codec-archive",
    "artifact-cleanup",
)


class LedgerError(ValueError):
    """Stable fail-closed ledger rejection."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def reject(code: str, message: str) -> NoReturn:
    raise LedgerError(code, message)


def unique_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    value: dict[str, Any] = {}
    for key, item in pairs:
        if key in value:
            reject("duplicate_json_key", f"duplicate JSON key {key!r}")
        value[key] = item
    return value


def _sha256(path: Path) -> str:
    try:
        return hashlib.sha256(path.read_bytes()).hexdigest()
    except OSError as error:
        reject("unreadable_authority", f"cannot read {path}: {error}")


def validate(path: Path = DEFAULT_LEDGER, root: Path = ROOT) -> dict[str, Any]:
    try:
        ledger = json.loads(path.read_bytes(), object_pairs_hook=unique_object)
    except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
        reject("invalid_ledger_json", f"cannot load ledger: {error}")
    if not isinstance(ledger, dict) or set(ledger) != {
        "format",
        "authority_state",
        "publication_authority",
        "transition_capabilities",
        "source_slices",
        "ledgers",
    }:
        reject("invalid_ledger_shape", "top-level ledger keys are not exact")
    if ledger["format"] != "typebridge.c-broad-case-ledger/v1":
        reject("identity_drift", "ledger format drifted")
    if ledger["authority_state"] != "frozen" or ledger["publication_authority"] is not False:
        reject("authority_widening", "ledger must be frozen and non-publishing")
    if ledger["transition_capabilities"] != list(CAPABILITIES):
        reject("transition_scope_drift", "broad transition capability order drifted")
    sources = ledger["source_slices"]
    rows = ledger["ledgers"]
    if not isinstance(sources, dict) or tuple(sources) != SLICES:
        reject("slice_scope_drift", "source slice order or inventory drifted")
    if not isinstance(rows, dict) or tuple(rows) != CAPABILITIES:
        reject("transition_scope_drift", "capability ledger inventory drifted")

    authority_paths: set[str] = set()
    for slice_id, source in sources.items():
        if not isinstance(source, dict) or set(source) != {"source_commit", "authorities"}:
            reject("invalid_slice_shape", f"{slice_id} source shape is not exact")
        commit = source["source_commit"]
        if (
            not isinstance(commit, str)
            or len(commit) != 40
            or any(c not in "0123456789abcdef" for c in commit)
        ):
            reject("invalid_source_commit", f"{slice_id} source commit is not exact")
        authorities = source["authorities"]
        if not isinstance(authorities, list) or len(authorities) != 2:
            reject("invalid_authority_inventory", f"{slice_id} must bind two authorities")
        for authority in authorities:
            if not isinstance(authority, dict) or set(authority) != {"path", "sha256"}:
                reject("invalid_authority_shape", f"{slice_id} authority shape is not exact")
            relative = authority["path"]
            parsed = PurePosixPath(relative) if isinstance(relative, str) else PurePosixPath("/")
            if (
                parsed.is_absolute()
                or ".." in parsed.parts
                or not relative.startswith("tests/contracts/")
            ):
                reject("invalid_authority_path", f"unsafe authority path {relative!r}")
            if relative in authority_paths:
                reject("duplicate_authority_path", f"authority {relative!r} is repeated")
            authority_paths.add(relative)
            if _sha256(root / relative) != authority["sha256"]:
                reject("stale_authority_digest", f"stale authority digest for {relative}")

    for capability, entries in rows.items():
        if not isinstance(entries, list) or len(entries) != len(SLICES):
            reject("incomplete_capability_ledger", f"{capability} does not cover all slices")
        if tuple(entry.get("slice") for entry in entries if isinstance(entry, dict)) != SLICES:
            reject("slice_order_drift", f"{capability} slice order drifted")
        references: set[str] = set()
        for entry in entries:
            if not isinstance(entry, dict) or set(entry) != {"slice", "evidence_refs"}:
                reject("invalid_evidence_row", f"{capability} evidence row shape is not exact")
            refs = entry["evidence_refs"]
            if (
                not isinstance(refs, list)
                or len(refs) != 2
                or any(not isinstance(ref, str) or not ref or ref in references for ref in refs)
            ):
                reject("invalid_evidence_refs", f"{capability} evidence references are incomplete")
            references.update(refs)
    return ledger


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ledger", type=Path, default=DEFAULT_LEDGER)
    arguments = parser.parse_args()
    try:
        ledger = validate(arguments.ledger)
    except LedgerError as error:
        print(f"C broad-case ledger rejected [{error.code}]: {error}", file=sys.stderr)
        return 1
    print(
        f"validated {len(ledger['ledgers'])} broad capabilities across {len(ledger['source_slices'])} slices"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
