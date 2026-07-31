#!/usr/bin/env python3
"""Validate a publisher's downloaded payloads against the recovery ledger."""

from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Sequence
from pathlib import Path

from validate_release_recovery import (
    ValidationError,
    read_json,
    validate_manifest,
    validate_manifest_digest,
    validate_payload_selection,
)


def build_parser() -> argparse.ArgumentParser:
    """Build the command-line parser."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--expected-manifest-sha256", required=True)
    parser.add_argument("--artifact-root", type=Path, required=True)
    parser.add_argument(
        "--artifact",
        action="append",
        required=True,
        help="exact manifest artifact name; repeat for a merged payload directory",
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Validate one exact payload selection and print a canonical summary."""
    arguments = build_parser().parse_args(argv)
    manifest_value = read_json(arguments.manifest, label="manifest")
    manifest_sha256 = validate_manifest_digest(
        manifest_value,
        expected_sha256=arguments.expected_manifest_sha256,
    )
    manifest = validate_manifest(manifest_value)
    verified_file_count = validate_payload_selection(
        arguments.artifact_root,
        expected_artifacts=manifest["artifacts"],
        artifact_names=arguments.artifact,
    )
    summary = {
        "artifacts": sorted(arguments.artifact),
        "manifest_sha256": manifest_sha256,
        "verified_payload_file_count": verified_file_count,
    }
    print(json.dumps(summary, ensure_ascii=False, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValidationError as error:
        print(f"release recovery payload validation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
