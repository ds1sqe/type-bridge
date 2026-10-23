#!/usr/bin/env python3
"""Shrink a cargo target directory before CI saves it as a cache.

The next run rebuilds the workspace's own crates anyway (their sources
change), so their artifacts only cost upload and restore time. This keeps
the compiled dependencies, which make the next build incremental, and
removes per profile directory (any directory holding a ``deps`` folder):

- every unit of a workspace package: its ``.fingerprint/<pkg>-<hash>`` and
  ``build/<pkg>-<hash>`` directories and every ``deps`` file carrying that
  ``-<hash>`` (libraries, test and bench executables, debug info);
- ``incremental/`` and ``examples/``;
- the files directly in the profile directory (uplifted copies).

Usage: prune_cargo_target.py TARGET_DIR [--manifest-path Cargo.toml] [--dry-run]
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
from collections.abc import Iterable
from pathlib import Path


def profile_dirs(target: Path, depth: int = 4) -> list[Path]:
    """Directories under ``target`` that hold a ``deps`` folder."""
    found = []
    frontier = [target]
    for _ in range(depth):
        nested = []
        for directory in frontier:
            for child in directory.iterdir():
                if child.is_dir() and not child.is_symlink():
                    if child.name == "deps":
                        found.append(directory)
                    else:
                        nested.append(child)
        frontier = nested
    return sorted(set(found))


def doomed(profile: Path, packages: Iterable[str]) -> list[Path]:
    """What to remove from one profile directory."""
    packages = set(packages)
    paths: list[Path] = []
    hashes = set()
    for kind in (".fingerprint", "build"):
        root = profile / kind
        if not root.is_dir():
            continue
        for unit in root.iterdir():
            package, _, unit_hash = unit.name.rpartition("-")
            if package in packages:
                paths.append(unit)
                hashes.add(unit_hash)
    for artifact in (profile / "deps").iterdir():
        # foo-<hash>, libfoo-<hash>.rlib, foo-<hash>.exe, foo-<hash>.dll.lib
        stem = artifact.name.split(".", 1)[0]
        if stem.rpartition("-")[2] in hashes:
            paths.append(artifact)
    for name in ("incremental", "examples"):
        if (profile / name).exists():
            paths.append(profile / name)
    paths += [child for child in profile.iterdir() if child.is_file()]
    return paths


def size(path: Path) -> int:
    if path.is_symlink() or path.is_file():
        return path.lstat().st_size
    return sum(child.lstat().st_size for child in path.rglob("*") if not child.is_dir())


def workspace_packages(manifest: Path) -> list[str]:
    output = subprocess.run(
        [
            "cargo",
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
            str(manifest),
        ],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return [package["name"] for package in json.loads(output)["packages"]]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("target", type=Path)
    parser.add_argument("--manifest-path", type=Path, default=Path("Cargo.toml"))
    parser.add_argument("--dry-run", action="store_true")
    arguments = parser.parse_args()
    if not arguments.target.is_dir():
        print(f"{arguments.target}: no target directory, nothing to prune")
        return
    packages = workspace_packages(arguments.manifest_path)
    before = size(arguments.target)
    removed = 0
    for profile in profile_dirs(arguments.target):
        for path in doomed(profile, packages):
            removed += size(path)
            if arguments.dry_run:
                continue
            if path.is_dir() and not path.is_symlink():
                shutil.rmtree(path)
            else:
                path.unlink()
    verb = "would remove" if arguments.dry_run else "removed"
    print(f"{arguments.target}: {before / 1e9:.2f} GB, {verb} {removed / 1e9:.2f} GB")


if __name__ == "__main__":
    main()
