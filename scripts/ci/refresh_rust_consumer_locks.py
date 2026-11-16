#!/usr/bin/env python3
"""Refresh maintained consumer locks offline without silently changing registry pins."""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
CRATES = ROOT / "type-bridge-core/crates"
# These are the dependency shapes staged by rust_acceptance, rust_manager_live,
# c_provider, sdk_artifacts_surface_consumer, and rust_projection_live. No Rust
# source is compiled: empty targets suffice for Cargo's dependency resolution.
FIXTURES = (
    ("tests/support/rust-offline/Cargo.lock", "rust-projected-projection-parity", "offline"),
    ("tests/support/rust-live/Cargo.lock", "rust-manager-live", "live"),
    ("tests/support/provider/Cargo.lock", "type-bridge-test-provider", "provider"),
    (
        "tests/contracts/sdk-artifacts-rust-surface-Cargo.lock",
        "type-bridge-generated-schema",
        "surface",
    ),
    (
        "type-bridge-core/crates/schema-codegen/tests/rust_projection_live/consumer-Cargo.lock",
        "type-bridge-rust-projection-live-consumer",
        "legacy",
    ),
)


def registry_pins(body: bytes) -> dict[tuple[str, str, str], str | None]:
    """Retain exact registry identities, versions and checksums, excluding local paths."""
    return {
        (entry["name"], entry["version"], entry["source"]): entry.get("checksum")
        for entry in tomllib.loads(body.decode()).get("package", [])
        if entry.get("source", "").startswith("registry+")
    }


def validate_registry_changes(before: bytes, after: bytes, allowed: set[str]) -> None:
    """Refuse every changed registry package unless its name was explicitly selected."""
    old, new = registry_pins(before), registry_pins(after)
    changed = {
        key[0]
        for key in old.keys() | new.keys()
        if key not in old or key not in new or old[key] != new[key]
    }
    if denied := changed - allowed:
        raise ValueError(
            "registry pins would change without --update: " + ", ".join(sorted(denied))
        )


def path_dependency(crate: str, **options: Any) -> dict[str, Any]:
    return {"path": (CRATES / crate).as_posix(), **options}


def write_manifest(
    directory: Path,
    name: str,
    dependencies: dict[str, Any],
    *,
    library: bool,
    patch_sdk: bool = True,
) -> None:
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "src").mkdir(exist_ok=True)
    (directory / "src" / ("lib.rs" if library else "main.rs")).write_text(
        "" if library else "fn main() {}\n"
    )
    lines = [
        "[package]",
        f"name = {json.dumps(name)}",
        'version = "0.0.0"',
        'edition = "2024"',
        "publish = false",
        "",
        "[dependencies]",
    ]
    for key, value in dependencies.items():
        if isinstance(value, dict):
            encoded = "{ " + ", ".join(f"{k} = {json.dumps(v)}" for k, v in value.items()) + " }"
        else:
            encoded = json.dumps(value)
        lines.append(f"{key} = {encoded}")
    if patch_sdk:
        lines += [
            "",
            "[patch.crates-io]",
            f"type-bridge = {{ path = {json.dumps((CRATES / 'rust').as_posix())} }}",
        ]
    lines += ["", "[workspace]", ""]
    (directory / "Cargo.toml").write_text("\n".join(lines))


def stage_manifest(stage: Path, name: str, kind: str) -> Path:
    """Stage the same package names, dependencies and features as the real consumers."""
    sdk = tomllib.loads((CRATES / "rust/Cargo.toml").read_text())["package"]["version"]
    generated_dependency = {"type-bridge": {"version": f"={sdk}", "default-features": False}}
    generated = stage / "generated"
    foreign = stage / "foreign"
    write_manifest(generated, "type-bridge-generated-schema", generated_dependency, library=True)
    write_manifest(
        foreign, "type-bridge-generated-schema-foreign", generated_dependency, library=True
    )
    consumer = stage / "consumer"
    if kind == "surface":
        write_manifest(consumer, name, generated_dependency, library=True)
        return consumer / "Cargo.toml"
    tokio = {"version": "1", "features": ["macros", "rt-multi-thread"]}
    if kind == "provider":
        dependencies = {"type-bridge-orm": path_dependency("orm"), "tokio": tokio}
    else:
        dependencies = {
            "generated": {"package": "type-bridge-generated-schema", "path": generated.as_posix()},
            "serde_json": "1",
            "sha2": "0.10",
            "tokio": tokio,
        }
        if kind == "legacy":
            dependencies.update(
                {
                    "type-bridge": path_dependency("rust"),
                    "reqwest": {
                        "version": "0.12",
                        "default-features": False,
                        "features": ["rustls-tls"],
                    },
                }
            )
        else:
            options: dict[str, Any] = {"default-features": False}
            if kind == "live":
                options["features"] = ["band9"]
            dependencies.update(
                {
                    "foreign": {
                        "package": "type-bridge-generated-schema-foreign",
                        "path": foreign.as_posix(),
                    },
                    "type-bridge": path_dependency("rust", **options),
                    "type-bridge-orm": path_dependency("orm", **options),
                }
            )
            if kind == "offline":
                dependencies.update(
                    {
                        "type-bridge-contract": path_dependency("contract"),
                        "type-bridge-schema": path_dependency("schema"),
                    }
                )
    write_manifest(consumer, name, dependencies, library=False, patch_sdk=kind != "provider")
    return consumer / "Cargo.toml"


def cargo(toolchain: str, arguments: list[str]) -> None:
    completed = subprocess.run(
        ["cargo", f"+{toolchain}", *arguments],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        text=True,
    )
    if completed.returncode:
        raise ValueError(completed.stderr.strip())


def refresh(*, check: bool, updates: list[str], toolchain: str) -> int:
    allowed = {spec.split("@", 1)[0] for spec in updates}
    present: set[str] = set()
    results: list[tuple[Path, bytes, bytes]] = []
    with tempfile.TemporaryDirectory(prefix="typebridge-consumer-locks-") as temporary:
        for number, (relative, name, kind) in enumerate(FIXTURES):
            path = ROOT / relative
            original = path.read_bytes()
            manifest = stage_manifest(Path(temporary) / str(number), name, kind)
            staged = manifest.with_name("Cargo.lock")
            staged.write_bytes(original)
            cargo(
                toolchain,
                ["metadata", "--offline", "--format-version=1", "--manifest-path", str(manifest)],
            )
            names = {key[0] for key in registry_pins(staged.read_bytes())}
            present |= names
            for spec in updates:
                if spec.split("@", 1)[0] in names:
                    cargo(
                        toolchain,
                        [
                            "update",
                            "--offline",
                            "--manifest-path",
                            str(manifest),
                            "--package",
                            spec,
                        ],
                    )
            regenerated = staged.read_bytes()
            validate_registry_changes(original, regenerated, allowed)
            results.append((path, original, regenerated))
    if absent := allowed - present:
        raise ValueError(
            "selected registry packages are absent from all consumer graphs: "
            + ", ".join(sorted(absent))
        )
    # Validate all graphs before publishing any lock, and never overwrite a
    # concurrent source edit made while Cargo was resolving the fixtures.
    if any(path.read_bytes() != original for path, original, _ in results):
        raise ValueError("a consumer lock changed during regeneration; retry")
    stale = False
    for path, original, regenerated in results:
        changed = original != regenerated
        stale |= changed
        relative = path.relative_to(ROOT)
        if changed and not check:
            candidate = None
            try:
                with tempfile.NamedTemporaryFile(
                    dir=path.parent, prefix=".consumer-lock-", delete=False
                ) as output:
                    candidate = Path(output.name)
                    output.write(regenerated)
                    output.flush()
                    os.fsync(output.fileno())
                candidate.chmod(path.stat().st_mode & 0o777)
                os.replace(candidate, path)
            finally:
                if candidate is not None:
                    candidate.unlink(missing_ok=True)
        print(
            f"{'stale' if check and changed else 'refreshed' if changed else 'unchanged'}: {relative}"
        )
    return int(check and stale)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check", action="store_true", help="refuse stale locks without writing them"
    )
    parser.add_argument(
        "--update",
        action="append",
        default=[],
        metavar="PACKAGE[@VERSION]",
        help="explicitly permit and request a named registry package update; repeat for each allowed package",
    )
    parser.add_argument(
        "--toolchain", default="1.94.1", help="exact Cargo toolchain (default: CI's 1.94.1)"
    )
    args = parser.parse_args()
    if not re.fullmatch(r"\d+\.\d+\.\d+", args.toolchain):
        parser.error("--toolchain must be an exact numeric Rust version")
    if args.check and args.update:
        parser.error("--check cannot update registry packages")
    if any(not re.fullmatch(r"[A-Za-z0-9_-]+(?:@[A-Za-z0-9.+_-]+)?", spec) for spec in args.update):
        parser.error("--update requires a Cargo package name, optionally followed by @VERSION")
    try:
        return refresh(check=args.check, updates=args.update, toolchain=args.toolchain)
    except (OSError, ValueError) as error:
        print(f"consumer lock refresh refused: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
