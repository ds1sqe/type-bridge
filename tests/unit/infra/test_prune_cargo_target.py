"""The cargo target prune drops workspace units and keeps dependencies."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location(
    "prune_cargo_target", ROOT / "scripts/ci/prune_cargo_target.py"
)
assert SPEC and SPEC.loader
prune = importlib.util.module_from_spec(SPEC)
sys.modules["prune_cargo_target"] = prune
SPEC.loader.exec_module(prune)


def touch(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"x")


def test_prune_drops_workspace_units_and_keeps_dependencies(tmp_path: Path) -> None:
    debug = tmp_path / "debug"
    nested = tmp_path / "rule-wire-standalone" / "debug"
    for profile in (debug, nested):
        # A workspace package: lib, test executable on Windows, build script.
        touch(profile / ".fingerprint/type-bridge-orm-aaa111/lib-type_bridge_orm")
        touch(profile / ".fingerprint/type-bridge-orm-bbb222/test-integration-test-serde")
        touch(profile / "build/type-bridge-orm-ccc333/build-script-build")
        touch(profile / ".fingerprint/type-bridge-orm-ccc333/build-script-build-script-build")
        touch(profile / "deps/libtype_bridge_orm-aaa111.rlib")
        touch(profile / "deps/serde-bbb222.exe")
        touch(profile / "deps/serde-bbb222.pdb")
        # A dependency that shares a name with the workspace bench `serde`.
        touch(profile / ".fingerprint/serde-ddd444/lib-serde")
        touch(profile / "build/serde-eee555/out/private.rs")
        touch(profile / "deps/libserde-ddd444.rlib")
        touch(profile / "deps/serde_derive-fff666.dll")
        touch(profile / "incremental/type_bridge_orm-x/s-1/query-cache.bin")
        touch(profile / "type-bridge.exe")
        touch(profile / ".cargo-lock")

    assert prune.profile_dirs(tmp_path) == [tmp_path / "debug", nested]
    for profile in (debug, nested):
        removed = set(prune.doomed(profile, ["type-bridge-orm", "type-bridge-c"]))
        assert removed == {
            profile / ".fingerprint/type-bridge-orm-aaa111",
            profile / ".fingerprint/type-bridge-orm-bbb222",
            profile / ".fingerprint/type-bridge-orm-ccc333",
            profile / "build/type-bridge-orm-ccc333",
            profile / "deps/libtype_bridge_orm-aaa111.rlib",
            profile / "deps/serde-bbb222.exe",
            profile / "deps/serde-bbb222.pdb",
            profile / "incremental",
            profile / "type-bridge.exe",
            profile / ".cargo-lock",
        }
