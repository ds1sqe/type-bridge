#!/usr/bin/env python3
"""Keep the temporary eager ORM feature inside its migration boundary."""

import json
import subprocess
import sys
from pathlib import Path

metadata = json.loads(
    subprocess.check_output(
        [
            "cargo",
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
            "type-bridge-core/Cargo.toml",
        ],
        text=True,
    )
)
allowed = {"type-bridge-orm", "type-bridge-schema-migration-typedb"}
violations = []
for package in metadata["packages"]:
    if package["name"] in allowed:
        continue
    references = subprocess.run(
        [
            "rg",
            "--files-with-matches",
            r"session::eager",
            str(Path(package["manifest_path"]).parent),
        ],
        text=True,
        capture_output=True,
        check=False,
    )
    if references.returncode not in (0, 1):
        sys.exit(references.stderr)
    if references.stdout:
        violations.append(package["name"])
    aliases = {
        dependency.get("rename") or dependency["name"]
        for dependency in package["dependencies"]
        if dependency["name"] == "type-bridge-orm"
    }
    for dependency in package["dependencies"]:
        if dependency["name"] == "type-bridge-orm" and "internal-eager" in dependency["features"]:
            violations.append(package["name"])
    for members in package["features"].values():
        for member in members:
            if any(
                member in {f"{alias}/internal-eager", f"{alias}?/internal-eager"}
                for alias in aliases
            ):
                violations.append(package["name"])
if violations:
    sys.exit(
        "internal-eager enabled outside ORM/migration boundary: "
        + ", ".join(sorted(set(violations)))
    )
print("internal-eager dependency boundary passed")
