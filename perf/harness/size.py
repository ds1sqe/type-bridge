"""Record byte sizes of existing release artifacts without rebuilding them."""

from __future__ import annotations

import argparse
import os
import platform
import shutil
import subprocess
import sys
from collections.abc import Sequence
from dataclasses import replace
from pathlib import Path

from perf.harness.micro import collect_run
from perf.harness.producer import write_result
from perf.harness.schema import Metric, metric_id


def convert(artifacts: dict[str, Path]) -> list[Metric]:
    result = []
    for name, path in sorted(artifacts.items()):
        if not path.is_file():
            raise ValueError(f"{name}: artifact is not a file: {path}")
        size = path.stat().st_size
        if size == 0:
            raise ValueError(f"{name}: artifact is empty: {path}")
        result.append(Metric(metric_id("size", name, "bytes"), "size", "bytes", size))
    return result


def tool_version(tool: str) -> str | None:
    executable = shutil.which(tool)
    if executable is None:
        return None
    return subprocess.check_output([executable, "--version"], text=True).splitlines()[0]


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact", action="append", required=True, metavar="NAME=FILE")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--started-at")
    args = parser.parse_args(argv)
    try:
        artifacts: dict[str, Path] = {}
        for spec in args.artifact:
            name, separator, filename = spec.partition("=")
            if not separator or not name or not filename or name in artifacts:
                raise ValueError(f"expected unique NAME=FILE, got {spec!r}")
            path = Path(filename)
            if path.resolve() == args.out.resolve():
                raise ValueError("result output must not overwrite an artifact")
            artifacts[name] = path
        run = collect_run(os.environ, started_at=args.started_at)
        run = replace(
            run,
            toolchains=replace(
                run.toolchains,
                python=platform.python_version() if "wheel" in artifacts else None,
                node=tool_version("node") if "npm" in artifacts else None,
                cc=tool_version("cc")
                if any(name.startswith("c_runtime") for name in artifacts)
                else None,
            ),
        )
        write_result(run, convert(artifacts), args.out)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
