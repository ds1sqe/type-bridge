#!/usr/bin/env python3
"""Run generated package runtime acceptance against extracted candidate wheels."""

from __future__ import annotations

import argparse
import importlib
import json
import runpy
import sys
from pathlib import Path
from types import ModuleType


def within(path: Path, root: Path) -> bool:
    candidate = path.resolve()
    boundary = root.resolve()
    return candidate == boundary or boundary in candidate.parents


def module_paths(module: ModuleType) -> tuple[Path, ...]:
    paths: list[Path] = []
    module_file = getattr(module, "__file__", None)
    if module_file is not None:
        paths.append(Path(module_file))
    module_path = getattr(module, "__path__", None)
    if module_path is not None:
        paths.extend(Path(path) for path in module_path)
    return tuple(path.resolve() for path in paths)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifact-root", required=True, type=Path)
    parser.add_argument("--generated-root", required=True, type=Path)
    parser.add_argument("--runtime-check", required=True, type=Path)
    parser.add_argument("--source-root", required=True, type=Path)
    args = parser.parse_args()

    artifact_root = args.artifact_root.resolve()
    generated_root = args.generated_root.resolve()
    source_root = args.source_root.resolve()
    sys.path[:] = [str(generated_root), str(artifact_root), *sys.path]

    runpy.run_path(str(args.runtime_check.resolve()), run_name="__main__")

    type_bridge = importlib.import_module("type_bridge")
    type_bridge_core = importlib.import_module("type_bridge_core")
    generated = importlib.import_module("generated_v2")
    for name in (
        "Attribute",
        "Entity",
        "Relation",
        "Role",
        "TypeDBManager",
        "TypeDBType",
    ):
        if hasattr(type_bridge, name):
            raise AssertionError(f"removed handwritten root export is present: {name}")
    for name in ("Database", "Query", "QueryBuilder"):
        if not hasattr(type_bridge, name):
            raise AssertionError(f"retained root operation is missing: {name}")

    locations: dict[str, list[str]] = {}
    for name, module, expected_root in (
        ("type_bridge", type_bridge, artifact_root),
        ("type_bridge_core", type_bridge_core, artifact_root),
        ("generated_v2", generated, generated_root),
    ):
        paths = module_paths(module)
        if not paths or not all(within(path, expected_root) for path in paths):
            raise AssertionError(f"{name} escaped its candidate root: {paths}")
        if any(within(path, source_root) for path in paths):
            raise AssertionError(f"{name} leaked from the source checkout: {paths}")
        locations[name] = [str(path) for path in paths]

    print(json.dumps({"locations": locations, "status": "ok"}, sort_keys=True))


if __name__ == "__main__":
    main()
