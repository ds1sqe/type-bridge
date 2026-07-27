#!/usr/bin/env python3
"""Shared lockstep checks for TypeBridge's Python and Node release identities."""

from __future__ import annotations

import ast
import json
import re
import tomllib
from pathlib import Path
from typing import Any

CORE_DISTRIBUTION = "type-bridge-core"


class ContractError(RuntimeError):
    """A checked-in release authority violates the cross-package contract."""


def normalize_distribution_name(value: str) -> str:
    """Return the PEP 503 comparison form for one distribution name."""
    return re.sub(r"[-_.]+", "-", value).lower()


def _read_text(path: Path, *, label: str) -> str:
    if path.is_symlink() or not path.is_file():
        raise ContractError(f"{label} is missing, non-regular, or symbolic: {path}")
    try:
        return path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as error:
        raise ContractError(f"Could not read {label} {path}: {error}") from error


def _read_toml(path: Path, *, label: str) -> dict[str, Any]:
    try:
        payload = tomllib.loads(_read_text(path, label=label))
    except tomllib.TOMLDecodeError as error:
        raise ContractError(f"Could not parse {label} {path}: {error}") from error
    if not isinstance(payload, dict):
        raise ContractError(f"{label} must contain a TOML table: {path}")
    return payload


def _reject_duplicate_json_keys(pairs: list[tuple[str, Any]]) -> dict[str, Any]:
    payload: dict[str, Any] = {}
    for key, value in pairs:
        if key in payload:
            raise ContractError(f"JSON authority contains a duplicate key: {key!r}")
        payload[key] = value
    return payload


def _read_json(path: Path, *, label: str) -> dict[str, Any]:
    try:
        payload = json.loads(
            _read_text(path, label=label),
            object_pairs_hook=_reject_duplicate_json_keys,
        )
    except json.JSONDecodeError as error:
        raise ContractError(f"Could not parse {label} {path}: {error}") from error
    if not isinstance(payload, dict):
        raise ContractError(f"{label} must contain a JSON object: {path}")
    return payload


def validate_root_python_manifest_lockstep(
    manifest: Path,
    expected_version: str,
) -> str:
    """Require one canonical, unmarked, exact dependency on the same-version core."""
    payload = _read_toml(manifest, label="root Python manifest")
    project = payload.get("project")
    if not isinstance(project, dict):
        raise ContractError(f"Root Python manifest has no [project] table: {manifest}")
    project_version = project.get("version")
    if project_version != expected_version:
        raise ContractError(
            "Root Python project version disagrees with the release identity: "
            f"actual={project_version!r}, expected={expected_version!r}"
        )
    dependencies = project.get("dependencies")
    if not isinstance(dependencies, list) or not all(
        isinstance(requirement, str) for requirement in dependencies
    ):
        raise ContractError("Root Python project.dependencies must be a list of strings")

    core_requirements: list[str] = []
    for requirement in dependencies:
        name_match = re.match(r"^\s*([A-Za-z0-9][A-Za-z0-9._-]*)", requirement)
        if name_match is None:
            continue
        if normalize_distribution_name(name_match.group(1)) == CORE_DISTRIBUTION:
            core_requirements.append(requirement)

    if len(core_requirements) != 1:
        raise ContractError(
            "Root Python manifest must declare exactly one type-bridge-core requirement: "
            f"actual={core_requirements!r}"
        )
    expected_requirement = f"{CORE_DISTRIBUTION}=={project_version}"
    if core_requirements[0] != expected_requirement:
        raise ContractError(
            "Root Python manifest type-bridge-core dependency must be the canonical, "
            "unmarked, exact same-version requirement: "
            f"actual={core_requirements[0]!r}, expected={expected_requirement!r}"
        )
    return expected_requirement


def _assignment_targets_name(target: ast.expr, name: str) -> bool:
    if isinstance(target, ast.Name):
        return target.id == name
    if isinstance(target, (ast.Tuple, ast.List)):
        return any(_assignment_targets_name(element, name) for element in target.elts)
    return False


def validate_python_package_version(package_init: Path, expected_version: str) -> str:
    """Bind the import-visible ``type_bridge.__version__`` literal to the release."""
    source = _read_text(package_init, label="type_bridge package initializer")
    try:
        module = ast.parse(source, filename=str(package_init))
    except SyntaxError as error:
        raise ContractError(f"Could not parse type_bridge package initializer: {error}") from error

    assignments: list[ast.expr | None] = []
    for node in ast.walk(module):
        if isinstance(node, ast.Assign) and any(
            _assignment_targets_name(target, "__version__") for target in node.targets
        ):
            assignments.append(node.value)
        elif isinstance(node, ast.AnnAssign) and _assignment_targets_name(
            node.target, "__version__"
        ):
            assignments.append(node.value)
        elif isinstance(node, (ast.AugAssign, ast.NamedExpr)) and _assignment_targets_name(
            node.target, "__version__"
        ):
            assignments.append(None)

    if len(assignments) != 1:
        raise ContractError(
            "type_bridge.__version__ must have exactly one source assignment: "
            f"actual={len(assignments)}"
        )
    value = assignments[0]
    actual = (
        value.value if isinstance(value, ast.Constant) and isinstance(value.value, str) else None
    )
    if actual != expected_version:
        raise ContractError(
            "type_bridge.__version__ disagrees with the release identity: "
            f"actual={actual!r}, expected={expected_version!r}"
        )
    return actual


def validate_node_package_lockstep(
    package_json: Path,
    package_lock: Path,
    expected_version: str,
) -> str:
    """Bind package.json and both package-lock root versions to the release tag."""
    package = _read_json(package_json, label="Node package.json")
    lock = _read_json(package_lock, label="Node package-lock.json")
    package_name = package.get("name")
    if not isinstance(package_name, str) or not package_name:
        raise ContractError("Node package.json has no package name")
    if package.get("version") != expected_version:
        raise ContractError(
            "Node package version disagrees with the release identity: "
            f"actual={package.get('version')!r}, expected={expected_version!r}"
        )
    if lock.get("name") != package_name or lock.get("version") != expected_version:
        raise ContractError(
            "Node package-lock root identity disagrees with package.json/tag: "
            f"actual_name={lock.get('name')!r}, actual_version={lock.get('version')!r}, "
            f"expected_name={package_name!r}, expected_version={expected_version!r}"
        )
    packages = lock.get("packages")
    root_package = packages.get("") if isinstance(packages, dict) else None
    if not isinstance(root_package, dict):
        raise ContractError("Node package-lock has no packages[''] root package")
    if root_package.get("name") != package_name or root_package.get("version") != expected_version:
        raise ContractError(
            "Node package-lock packages[''] identity disagrees with package.json/tag: "
            f"actual_name={root_package.get('name')!r}, "
            f"actual_version={root_package.get('version')!r}, "
            f"expected_name={package_name!r}, expected_version={expected_version!r}"
        )
    return expected_version
