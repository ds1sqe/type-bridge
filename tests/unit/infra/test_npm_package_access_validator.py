"""Hostile coverage for the npm package-write credential preflight."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from types import ModuleType

import pytest

ROOT = Path(__file__).resolve().parents[3]
VALIDATOR_PATH = ROOT / "scripts/ci/validate_npm_package_access.py"
PACKAGE = "@type-bridge/node"


def load_module(name: str, path: Path) -> ModuleType:
    """Load the standalone validator without making scripts a package."""
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


validator = load_module("validate_npm_package_access", VALIDATOR_PATH)


def test_exact_read_write_permission_is_accepted_with_other_packages() -> None:
    validator.validate_package_access(
        b'{"@type-bridge/node":"read-write","another-package":"read-only"}',
        package=PACKAGE,
    )


@pytest.mark.parametrize(
    ("body", "message"),
    (
        (b'{"@type-bridge/node":"read-only"}', "lacks read-write"),
        (b'{"another-package":"read-write"}', "does not contain"),
        (b"[]", "package-permission object"),
        (b'{"@type-bridge/node":true}', "invalid permission"),
        (b'{"@type-bridge/node":"admin"}', "invalid permission"),
        (b'{"Bad Package":"read-write"}', "invalid package key"),
        (b"not-json", "malformed"),
    ),
)
def test_insufficient_or_malformed_access_responses_fail_closed(
    body: bytes,
    message: str,
) -> None:
    with pytest.raises(validator.ValidationError, match=message):
        validator.validate_package_access(body, package=PACKAGE)


def test_duplicate_package_key_is_rejected_as_ambiguous() -> None:
    body = b'{"@type-bridge/node":"read-only","@type-bridge/node":"read-write"}'

    with pytest.raises(validator.ValidationError, match="duplicate key"):
        validator.validate_package_access(body, package=PACKAGE)


@pytest.mark.parametrize("package", ("", "@scope", "Bad Package", "name\nforged"))
def test_expected_package_name_must_be_safe(package: str) -> None:
    with pytest.raises(validator.ValidationError, match="Invalid expected npm package"):
        validator.validate_package_access(b"{}", package=package)


def test_access_response_must_be_a_regular_file(tmp_path: Path) -> None:
    target = tmp_path / "target.json"
    target.write_text('{"@type-bridge/node":"read-write"}')
    linked = tmp_path / "linked.json"
    linked.symlink_to(target)

    with pytest.raises(validator.ValidationError, match="linked or non-regular"):
        validator.read_access_json(linked)


def test_access_response_byte_budget_is_enforced(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    response = tmp_path / "access.json"
    response.write_bytes(b"12345")
    monkeypatch.setattr(validator, "MAX_ACCESS_JSON_BYTES", 4)

    with pytest.raises(validator.ValidationError, match="byte budget"):
        validator.read_access_json(response)
