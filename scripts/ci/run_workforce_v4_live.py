#!/usr/bin/env python3
"""Run the four Workforce V4 producers against one exact isolated TypeDB."""

from __future__ import annotations

import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CORE = ROOT / "type-bridge-core"
COMPARATOR = ROOT / "scripts/ci/compare_workforce_conformance_v4.py"
REPORTS = tuple(
    f"{binding}-workforce-v4-report.json" for binding in ("python", "node", "rust", "c")
)
TESTS = tuple(f"workforce_v4_{binding}_live" for binding in ("python", "node", "rust", "c"))


class RunnerError(RuntimeError):
    """The exact-live fan-in could not complete safely."""


def run(command: list[str], *, env: dict[str, str] | None = None, capture: bool = False) -> str:
    result = subprocess.run(
        command,
        cwd=ROOT,
        env=env,
        check=False,
        text=True,
        stdout=subprocess.PIPE if capture else None,
    )
    if result.returncode != 0:
        raise RunnerError(f"command failed with exit {result.returncode}: {' '.join(command)}")
    return result.stdout.strip() if capture else ""


def published_port(project: str, container_port: str) -> str:
    value = run(
        [
            "docker",
            "compose",
            "-p",
            project,
            "-f",
            str(ROOT / "docker-compose.yml"),
            "port",
            "typedb",
            container_port,
        ],
        capture=True,
    )
    if not value.startswith("0.0.0.0:") and not value.startswith("[::]:"):
        raise RunnerError(f"unexpected published TypeDB port: {value!r}")
    return value.rsplit(":", 1)[1]


def main() -> int:
    project = f"tb-plan06-v4-fanin-{os.getpid()}"
    temporary = Path(tempfile.mkdtemp(prefix="typebridge-workforce-v4-"))
    compose = [
        "docker",
        "compose",
        "-p",
        project,
        "-f",
        str(ROOT / "docker-compose.yml"),
    ]
    interrupted = False

    def mark_interrupted(_signum: int, _frame: object) -> None:
        nonlocal interrupted
        interrupted = True
        raise KeyboardInterrupt

    previous = signal.signal(signal.SIGTERM, mark_interrupted)
    try:
        env = os.environ.copy()
        env.update(
            {
                "TYPEDB_IMAGE": "typedb/typedb:3.12.3",
                "TYPEDB_PORT": "0",
                "TYPEDB_HTTP_PORT": "0",
            }
        )
        run([*compose, "up", "-d", "--wait", "typedb"], env=env)
        driver_port = published_port(project, "1729")
        http_port = published_port(project, "8000")
        producer_env = env | {
            "TYPEDB_ADDRESS": f"127.0.0.1:{driver_port}",
            "TYPEDB_HTTP_PORT": http_port,
            "TYPEDB_USERNAME": "admin",
            "TYPEDB_PASSWORD": "password",
            "TYPE_BRIDGE_WORKFORCE_V4_REPORT_DIR": str(temporary),
        }
        for test in TESTS:
            run(
                [
                    "cargo",
                    "test",
                    "--manifest-path",
                    str(CORE / "Cargo.toml"),
                    "-p",
                    "type-bridge-cli",
                    "--test",
                    test,
                    "--",
                    "--ignored",
                    "--nocapture",
                ],
                env=producer_env,
            )
        paths = [temporary / report for report in REPORTS]
        missing = [path.name for path in paths if not path.is_file()]
        if missing:
            raise RunnerError(f"validated report publication is incomplete: {missing}")
        comparison = run(
            [sys.executable, str(COMPARATOR), *(str(path) for path in paths)],
            capture=True,
        )
        print(json.dumps(json.loads(comparison), indent=2, sort_keys=True))
        return 0
    except (KeyboardInterrupt, RunnerError) as error:
        if interrupted:
            print("Workforce V4 live fan-in interrupted", file=sys.stderr)
        else:
            print(f"Workforce V4 live fan-in failed: {error}", file=sys.stderr)
        return 1
    finally:
        signal.signal(signal.SIGTERM, previous)
        subprocess.run(
            [*compose, "down", "-v", "--remove-orphans"],
            cwd=ROOT,
            env=os.environ.copy(),
            check=False,
        )
        shutil.rmtree(temporary, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
