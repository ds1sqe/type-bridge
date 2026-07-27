#!/usr/bin/env python3
"""Fail closed unless npm reports read-write access to one exact package."""

from __future__ import annotations

import argparse
import json
import re
import stat
import sys
from collections.abc import Sequence
from pathlib import Path
from typing import Any

MAX_ACCESS_JSON_BYTES = 1024 * 1024
PACKAGE_NAME_PATTERN = re.compile(r"^(?:@[a-z0-9][a-z0-9._~-]*/)?[a-z0-9][a-z0-9._~-]*$")
KNOWN_PERMISSIONS = frozenset({"read-only", "read-write"})


class ValidationError(RuntimeError):
    """The npm access response is unsafe, malformed, or insufficient."""


def valid_package_name(value: str) -> bool:
    """Return whether a package name is safe to compare and print."""
    return len(value.encode("utf-8")) <= 214 and PACKAGE_NAME_PATTERN.fullmatch(value) is not None


def unique_json_object(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    """Reject duplicate JSON keys instead of silently accepting the last value."""
    result: dict[str, Any] = {}
    for key, value in pairs:
        if key in result:
            raise ValidationError(f"npm access JSON contains a duplicate key: {key!r}")
        result[key] = value
    return result


def read_access_json(path: Path) -> bytes:
    """Read one bounded regular response without following a symlink."""
    try:
        file_stat = path.lstat()
    except OSError as error:
        raise ValidationError(f"Could not inspect npm access JSON {path}: {error}") from error
    if stat.S_ISLNK(file_stat.st_mode) or not stat.S_ISREG(file_stat.st_mode):
        raise ValidationError(f"npm access JSON is linked or non-regular: {path}")
    if file_stat.st_size < 0 or file_stat.st_size > MAX_ACCESS_JSON_BYTES:
        raise ValidationError(
            "npm access JSON exceeds the byte budget: "
            f"size={file_stat.st_size}, maximum={MAX_ACCESS_JSON_BYTES}"
        )
    try:
        body = path.read_bytes()
    except OSError as error:
        raise ValidationError(f"Could not read npm access JSON {path}: {error}") from error
    if len(body) != file_stat.st_size:
        raise ValidationError(f"npm access JSON changed while it was read: {path}")
    return body


def validate_package_access(body: bytes, *, package: str) -> None:
    """Require a unique exact package key whose permission is read-write."""
    if not valid_package_name(package):
        raise ValidationError(f"Invalid expected npm package name: {package!r}")
    try:
        payload = json.loads(
            body.decode("utf-8"),
            object_pairs_hook=unique_json_object,
        )
    except ValidationError:
        raise
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValidationError(f"npm access JSON is malformed: {error}") from error
    if not isinstance(payload, dict):
        raise ValidationError("npm access JSON must be one package-permission object")
    for actual_package, permission in payload.items():
        if not isinstance(actual_package, str) or not valid_package_name(actual_package):
            raise ValidationError(
                f"npm access JSON contains an invalid package key: {actual_package!r}"
            )
        if not isinstance(permission, str) or permission not in KNOWN_PERMISSIONS:
            raise ValidationError(
                "npm access JSON contains an invalid permission: "
                f"package={actual_package!r}, permission={permission!r}"
            )
    permission = payload.get(package)
    if permission is None:
        raise ValidationError(f"npm access JSON does not contain the expected package: {package}")
    if permission != "read-write":
        raise ValidationError(
            f"npm credential lacks read-write access to {package}: permission={permission!r}"
        )


def build_parser() -> argparse.ArgumentParser:
    """Build the command-line parser."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--access-json", type=Path, required=True)
    parser.add_argument("--package", required=True)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    """Validate one npm access response without echoing registry credentials."""
    args = build_parser().parse_args(argv)
    validate_package_access(read_access_json(args.access_json.resolve()), package=args.package)
    print(f"npm credential has read-write access to {args.package}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ValidationError as error:
        print(f"npm package-access validation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
