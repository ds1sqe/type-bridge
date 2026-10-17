"""Build every SDK driver against ``perf-stages`` bindings, on Linux.

    python -m perf.drivers.build --out DIR

Leaves in ``DIR``:

- ``site/``: the Python wheel built with ``perf-stages``, unpacked; put it
  first on ``PYTHONPATH``.
- ``type_bridge_node.node``: the Node native module built with
  ``perf-stages`` (``TYPE_BRIDGE_NODE_NATIVE_PATH``); the package itself is
  ``type-bridge-core/crates/node`` with its types built.
- ``clib/libtype_bridge_c.so`` and ``c-pipelines``: the C library built
  with ``perf-stages`` and the C driver linked against that copy.
- ``workspace/``: the purchase workspace with its generated C package, for
  the Node and C drivers (``perf.drivers.workspace``).

The Rust driver and the ``type-bridge`` CLI stay in the cargo target
directory. Each binding's ``perf-stages`` artifact is copied out right
after its build, so a later default build of the same crate cannot
replace what a driver loads.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import zipfile
from collections.abc import Mapping, Sequence
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CORE = ROOT / "type-bridge-core"
MATURIN = "maturin==1.14.1"


def target_dir(env: Mapping[str, str]) -> Path:
    """The cargo release directory, as cargo resolves it from ``env``."""
    return Path(env.get("CARGO_TARGET_DIR") or CORE / "target") / "release"


def commands(out: Path, env: Mapping[str, str]) -> list[tuple[list[str], Path]]:
    """Every build command, in order, with its working directory."""
    release = target_dir(env)
    cargo = ["cargo", "build", "--locked", "--release", "--manifest-path", str(CORE / "Cargo.toml")]
    cli = release / "type-bridge"
    generated = out / "workspace/generated/c"
    return [
        (
            [*cargo, "-p", "type-bridge-cli", "--bin", "type-bridge"]
            + ["-p", "type-bridge-perf", "--bin", "type-bridge-perf-pipelines"],
            ROOT,
        ),
        (
            ["uvx", "--from", MATURIN, "maturin", "build", "--release", "--locked"]
            + ["--features", "pyo3/extension-module,perf-stages", "-o", str(out / "wheels")],
            CORE,
        ),
        ([*cargo, "-p", "type-bridge-node", "--features", "perf-stages"], ROOT),
        (["npm", "ci", "--ignore-scripts"], CORE / "crates/node"),
        (["npm", "run", "build:types"], CORE / "crates/node"),
        ([*cargo, "-p", "type-bridge-c", "--features", "perf-stages"], ROOT),
        (
            [sys.executable, "-m", "perf.drivers.workspace", "--root", str(out), "--cli", str(cli)]
            + ["create"],
            ROOT,
        ),
        (
            ["cc", "-std=c17", "-O2", "-Wall", "-Wextra", "-Werror"]
            + ["-I", str(generated / "include"), "-I", str(CORE / "crates/c/include")]
            + [str(ROOT / "perf/drivers/c/pipelines.c"), str(generated / "src/models.c")]
            + ["-L", str(out / "clib"), "-ltype_bridge_c", f"-Wl,-rpath,{out / 'clib'}"]
            + ["-o", str(out / "c-pipelines")],
            ROOT,
        ),
    ]


def copy_artifact(command: Sequence[str], out: Path, env: Mapping[str, str]) -> None:
    """Copy a binding's ``perf-stages`` artifact out of the target directory."""
    release = target_dir(env)
    if "type-bridge-node" in command:
        shutil.copy2(release / "libtype_bridge_node.so", out / "type_bridge_node.node")
    elif "type-bridge-c" in command:
        (out / "clib").mkdir(exist_ok=True)
        shutil.copy2(release / "libtype_bridge_c.so", out / "clib/libtype_bridge_c.so")
    elif "maturin" in command:
        (wheel,) = (out / "wheels").glob("type_bridge_core-*.whl")
        with zipfile.ZipFile(wheel) as archive:
            archive.extractall(out / "site")


def main(argv: Sequence[str] | None = None, env: Mapping[str, str] = os.environ) -> int:
    parser = argparse.ArgumentParser(prog="python -m perf.drivers.build")
    parser.add_argument("--out", type=Path, required=True, help="empty directory for the drivers")
    arguments = parser.parse_args(argv)
    out = arguments.out.resolve()
    if out.exists() and any(out.iterdir()):
        parser.error(f"{out} is not empty")
    out.mkdir(parents=True, exist_ok=True)
    for command, cwd in commands(out, env):
        print("+", " ".join(command), file=sys.stderr, flush=True)
        subprocess.run(command, cwd=cwd, env=dict(env), check=True)
        copy_artifact(command, out, env)
    return 0


if __name__ == "__main__":
    sys.exit(main())
