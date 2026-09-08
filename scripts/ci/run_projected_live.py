#!/usr/bin/env python3
"""Produce and compare four Projected reports on one exact TypeDB 3.12.3 server."""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import secrets
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time
from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType

sys.path.insert(0, str(Path(__file__).resolve().parent))
from persist_binding_reports import PublishError, publish  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
CORE = ROOT / "type-bridge-core"
SCHEMA = ROOT / "tests/contracts/sdk_conformance/sdk-v3/schema-v3.yaml"
NODE_PACKAGE = CORE / "crates/node"
COMPARATOR = ROOT / "scripts/ci/compare_projected_live.py"
PYTHON_PRODUCER = CORE / "crates/schema-codegen/tests/acceptance/projected_live_check.py"
NODE_PRODUCER = CORE / "crates/schema-codegen/tests/typescript_acceptance/projected_live_check.mjs"
NODE_HELPER = CORE / "crates/schema-codegen/tests/typescript_acceptance/live_support.mjs"
RUST_PRODUCER = CORE / "crates/schema-codegen/tests/rust_acceptance/projected_live.rs"
C_HARNESS = CORE / "crates/schema-codegen/tests/c_projected_live.rs"
C_CONSUMER = CORE / "crates/schema-codegen/tests/c_projected_live/consumer.c"
C_SETUP = ROOT / "tests/support/provider/main.rs"
C_PROVIDER_HELPER = CORE / "crates/schema-codegen/tests/support/c_provider.rs"

ADDRESS_ENV = "TYPE_BRIDGE_PROJECTED_LIVE_ADDRESS"
HTTP_PORT_ENV = "TYPE_BRIDGE_PROJECTED_LIVE_HTTP_PORT"
REPORT_ENV = "TYPE_BRIDGE_PROJECTED_LIVE_REPORT"
DATABASE_ENV = "TYPE_BRIDGE_PROJECTED_LIVE_DATABASE"
PYTHON_PACKAGE_ENV = "TYPE_BRIDGE_PROJECTED_PYTHON_PACKAGE_ROOT"
NODE_PACKAGE_ENV = "TYPE_BRIDGE_PROJECTED_NODE_PACKAGE_ROOT"
REPOSITORY_ENV = "TYPE_BRIDGE_PROJECTED_REPOSITORY_ROOT"
ACCEPTANCE_TARGET_ENV = "ACCEPTANCE_TARGET_DIR"
RUNNER_OWNED_ENV = (
    REPORT_ENV,
    DATABASE_ENV,
    PYTHON_PACKAGE_ENV,
    NODE_PACKAGE_ENV,
    REPOSITORY_ENV,
    ACCEPTANCE_TARGET_ENV,
)
PRODUCER_SOURCES = (
    ("Python producer", PYTHON_PRODUCER),
    ("Node producer", NODE_PRODUCER),
    ("Node helper", NODE_HELPER),
    ("Rust producer", RUST_PRODUCER),
    ("C harness", C_HARNESS),
    ("C consumer", C_CONSUMER),
    ("C setup", C_SETUP),
    ("C provider helper", C_PROVIDER_HELPER),
)
DATABASE_GUARD_SOURCE = r"""
import sys

from type_bridge import Database

action, address, raw_port, *names = sys.argv[1:]
if action not in {"assert-absent", "cleanup"} or not names:
    raise SystemExit("invalid Projected database guard invocation")
port = int(raw_port)
failures = []
for name in names:
    database = Database(address=address, database=name, http_port=port)
    connected = False
    try:
        database.connect()
        connected = True
        detected = database.detected_server_version()
        if detected != "3.12.3":
            failures.append(f"{name}: exact TypeDB 3.12.3 required, detected {detected!r}")
            continue
        exists = database.database_exists()
        if action == "assert-absent" and exists:
            failures.append(f"{name}: database is not absent")
        elif action == "cleanup" and exists:
            database.delete_database()
            if database.database_exists():
                failures.append(f"{name}: database remained after cleanup")
    except BaseException as error:
        failures.append(f"{name}: {type(error).__name__}: {error}")
    finally:
        if connected:
            try:
                database.close()
            except BaseException as error:
                failures.append(f"{name}: close {type(error).__name__}: {error}")
if failures:
    for failure in failures:
        print(failure, file=sys.stderr)
    raise SystemExit(1)
"""


class RunnerError(RuntimeError):
    """The exact live four-binding fan-in could not complete safely."""


class UnsafeProcessGroupError(RunnerError):
    """A timed-out process group could not be proven stopped before cleanup."""


@dataclass(frozen=True, slots=True)
class Fixture:
    address: str
    http_port: str


@dataclass(frozen=True, slots=True)
class Layout:
    root: Path
    python: Path
    node: Path
    reports: Path
    scratch: Path
    python_report: Path
    node_report: Path
    rust_report: Path
    c_report: Path
    python_database: str
    node_database: str
    rust_database: str
    c_database: str

    @classmethod
    def under(cls, root: Path, nonce: str) -> Layout:
        if len(nonce) != 24 or any(character not in "0123456789abcdef" for character in nonce):
            raise RunnerError("database nonce must be exactly 24 lowercase hexadecimal characters")
        reports = root / "reports"
        prefix = "typebridge_p2live"
        return cls(
            root=root,
            python=root / "python",
            node=root / "node",
            reports=reports,
            scratch=root / "scratch",
            python_report=reports / "python.json",
            node_report=reports / "node.json",
            rust_report=reports / "rust.json",
            c_report=reports / "c.json",
            python_database=f"{prefix}_python_{nonce}",
            node_database=f"{prefix}_node_{nonce}",
            rust_database=f"{prefix}_rust_{nonce}",
            c_database=f"{prefix}_c_{nonce}",
        )

    def report_paths(self) -> tuple[Path, Path, Path, Path]:
        return (
            self.python_report,
            self.node_report,
            self.rust_report,
            self.c_report,
        )

    def database_names(self) -> tuple[str, str, str, str]:
        return (
            self.python_database,
            self.node_database,
            self.rust_database,
            self.c_database,
        )


@dataclass(frozen=True, slots=True)
class CommandSpec:
    label: str
    arguments: tuple[str, ...]
    cwd: Path
    timeout_seconds: int
    environment: Mapping[str, str] | None = None


def _load_live_contract() -> ModuleType:
    spec = importlib.util.spec_from_file_location("projected_live_contract", COMPARATOR)
    if spec is None or spec.loader is None:
        raise RunnerError("the committed Projected live comparator cannot be loaded")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    try:
        spec.loader.exec_module(module)
    except (OSError, RuntimeError, ValueError) as error:
        raise RunnerError(f"the committed Projected live comparator rejected: {error}") from error
    return module


def _validate_producer_sources(contract: ModuleType) -> None:
    for label, path in PRODUCER_SOURCES:
        try:
            metadata = path.lstat()
        except OSError as error:
            raise RunnerError(f"{label} cannot be inspected: {error}") from error
        if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISREG(metadata.st_mode):
            raise RunnerError(f"{label} must be a regular non-symlink source file")
        try:
            contract.validate_producer_source(path.read_bytes())
        except (OSError, contract.ContractError) as error:
            raise RunnerError(f"{label} failed committed source validation: {error}") from error


def _required_fixture(environment: Mapping[str, str]) -> Fixture:
    present = [name for name in RUNNER_OWNED_ENV if name in environment]
    if present:
        raise RunnerError(f"runner-owned Projected live environment must be unset: {present}")
    address = environment.get(ADDRESS_ENV)
    if (
        not isinstance(address, str)
        or not address
        or len(address.encode("utf-8")) > 4096
        or address.strip() != address
        or any(character.isspace() or ord(character) < 32 for character in address)
        or "://" in address
        or "@" in address
    ):
        raise RunnerError(f"{ADDRESS_ENV} must be one bounded credential-free host:port")
    raw_port = environment.get(HTTP_PORT_ENV)
    if (
        not isinstance(raw_port, str)
        or not raw_port
        or not raw_port.isascii()
        or not raw_port.isdigit()
    ):
        raise RunnerError(f"{HTTP_PORT_ENV} must be an ASCII integer")
    port = int(raw_port)
    if port < 1 or port > 65535:
        raise RunnerError(f"{HTTP_PORT_ENV} must be in 1..65535")
    return Fixture(address=address, http_port=raw_port)


def _require_tools() -> None:
    if os.name != "posix":
        raise RunnerError("the Projected live fan-in requires POSIX process-group isolation")
    for executable in ("cargo", "maturin", "npm", "node", "gcc", "clang"):
        if shutil.which(executable) is None:
            raise RunnerError(f"required Projected live tool is unavailable: {executable}")
    tsc = NODE_PACKAGE / "node_modules/.bin/tsc"
    if not tsc.is_file() or not os.access(tsc, os.X_OK):
        raise RunnerError(
            "required TypeScript compiler is unavailable; run npm ci in "
            "type-bridge-core/crates/node"
        )


def _prepare(layout: Layout) -> None:
    metadata = layout.root.lstat()
    if stat.S_ISLNK(metadata.st_mode) or not stat.S_ISDIR(metadata.st_mode):
        raise RunnerError("the Projected live stage root must be a real directory")
    layout.python.mkdir()
    layout.node.mkdir()
    layout.reports.mkdir()
    layout.scratch.mkdir()
    node_scope = layout.node / "node_modules/@type-bridge"
    node_scope.mkdir(parents=True)
    (node_scope / "node").symlink_to(NODE_PACKAGE, target_is_directory=True)
    reports = layout.report_paths()
    databases = layout.database_names()
    if len(set(reports)) != 4 or len(set(databases)) != 4:
        raise RunnerError("Projected live report paths and database names must be unique")
    for report in reports:
        if not report.is_absolute() or report.exists() or report.is_symlink():
            raise RunnerError(f"report destination must be an absent absolute path: {report}")
    for database in databases:
        if len(database) > 64 or not all(
            character.isascii() and (character.isalnum() or character == "_")
            for character in database
        ):
            raise RunnerError(f"generated live database name is not bounded ASCII: {database}")


def _cargo_run_example(example: str, output: Path) -> tuple[str, ...]:
    return (
        "cargo",
        "run",
        "--locked",
        "--quiet",
        "--manifest-path",
        str(CORE / "Cargo.toml"),
        "--package",
        "type-bridge-schema-codegen",
        "--example",
        example,
        "--",
        str(SCHEMA),
        str(output),
    )


def _live_environment(
    layout: Layout,
    fixture: Fixture,
    report: Path,
    database: str,
) -> dict[str, str]:
    return {
        ADDRESS_ENV: fixture.address,
        HTTP_PORT_ENV: fixture.http_port,
        REPORT_ENV: str(report),
        DATABASE_ENV: database,
        REPOSITORY_ENV: str(ROOT),
        "TMPDIR": str(layout.scratch),
    }


def _database_guard(
    layout: Layout,
    fixture: Fixture,
    action: str,
    databases: tuple[str, ...],
    label: str,
) -> CommandSpec:
    return CommandSpec(
        label=label,
        arguments=(
            sys.executable,
            "-c",
            DATABASE_GUARD_SOURCE,
            action,
            fixture.address,
            fixture.http_port,
            *databases,
        ),
        cwd=ROOT,
        timeout_seconds=180,
        environment={"TMPDIR": str(layout.scratch)},
    )


def command_plan(layout: Layout, fixture: Fixture) -> tuple[CommandSpec, ...]:
    """Return the exact same-stage live producer and comparator ledger."""

    python_environment = _live_environment(
        layout,
        fixture,
        layout.python_report,
        layout.python_database,
    )
    python_environment[PYTHON_PACKAGE_ENV] = str(layout.python)
    node_environment = _live_environment(
        layout,
        fixture,
        layout.node_report,
        layout.node_database,
    )
    node_environment[NODE_PACKAGE_ENV] = str(layout.node)
    rust_environment = _live_environment(
        layout,
        fixture,
        layout.rust_report,
        layout.rust_database,
    )
    rust_environment[ACCEPTANCE_TARGET_ENV] = str(layout.scratch / "rust-target")
    c_environment = _live_environment(
        layout,
        fixture,
        layout.c_report,
        layout.c_database,
    )
    c_environment[ACCEPTANCE_TARGET_ENV] = str(layout.scratch / "c-target")
    c_environment["CARGO_TARGET_DIR"] = str(layout.scratch / "c-target")
    tsc = NODE_PACKAGE / "node_modules/.bin/tsc"
    return (
        CommandSpec(
            "install the Python native facade",
            ("maturin", "develop"),
            CORE,
            600,
            {"TMPDIR": str(layout.scratch)},
        ),
        _database_guard(
            layout,
            fixture,
            "assert-absent",
            layout.database_names(),
            "verify exact fixture and four initially absent databases",
        ),
        CommandSpec(
            "generate the Python package",
            _cargo_run_example(
                "emit_python_acceptance",
                layout.python / "generated_projected",
            ),
            ROOT,
            600,
            {"TMPDIR": str(layout.scratch)},
        ),
        CommandSpec(
            "produce the Python report",
            (sys.executable, str(PYTHON_PRODUCER)),
            ROOT,
            600,
            python_environment,
        ),
        _database_guard(
            layout,
            fixture,
            "assert-absent",
            (layout.python_database,),
            "verify the Python database was removed",
        ),
        CommandSpec(
            "build the Node package",
            ("npm", "run", "build"),
            NODE_PACKAGE,
            600,
            {"TMPDIR": str(layout.scratch)},
        ),
        CommandSpec(
            "generate the TypeScript package",
            _cargo_run_example(
                "emit_typescript_acceptance",
                layout.node / "generated_projected",
            ),
            ROOT,
            600,
            {"TMPDIR": str(layout.scratch)},
        ),
        CommandSpec(
            "compile the TypeScript package",
            (str(tsc), "--project", str(layout.node / "generated_projected/tsconfig.json")),
            ROOT,
            600,
            {"TMPDIR": str(layout.scratch)},
        ),
        CommandSpec(
            "produce the Node report",
            ("node", str(NODE_PRODUCER)),
            ROOT,
            600,
            node_environment,
        ),
        _database_guard(
            layout,
            fixture,
            "assert-absent",
            (layout.node_database,),
            "verify the Node database was removed",
        ),
        CommandSpec(
            "produce the Rust report",
            (
                "cargo",
                "test",
                "--locked",
                "--manifest-path",
                str(CORE / "Cargo.toml"),
                "-p",
                "type-bridge-schema-codegen",
                "--test",
                "rust_projected_live",
                "generated_rust_projected_live_producer_runs_when_explicitly_configured",
                "--",
                "--exact",
                "--nocapture",
            ),
            ROOT,
            1200,
            rust_environment,
        ),
        _database_guard(
            layout,
            fixture,
            "assert-absent",
            (layout.rust_database,),
            "verify the Rust database was removed",
        ),
        CommandSpec(
            "build the C shared library",
            (
                "cargo",
                "build",
                "--locked",
                "--manifest-path",
                str(CORE / "Cargo.toml"),
                "-p",
                "type-bridge-c",
                "--lib",
            ),
            ROOT,
            600,
            {
                "TMPDIR": str(layout.scratch),
                ACCEPTANCE_TARGET_ENV: str(layout.scratch / "c-target"),
                "CARGO_TARGET_DIR": str(layout.scratch / "c-target"),
            },
        ),
        CommandSpec(
            "produce the C report",
            (
                "cargo",
                "test",
                "--locked",
                "--manifest-path",
                str(CORE / "Cargo.toml"),
                "-p",
                "type-bridge-schema-codegen",
                "--test",
                "c_projected_live",
                "generated_c17_projected_live_subset_round_trips_exact_3_12_1",
                "--",
                "--ignored",
                "--exact",
                "--nocapture",
            ),
            ROOT,
            1200,
            c_environment,
        ),
        _database_guard(
            layout,
            fixture,
            "assert-absent",
            (layout.c_database,),
            "verify the C database was removed",
        ),
        CommandSpec(
            "compare exactly four live reports",
            (
                sys.executable,
                str(COMPARATOR),
                *(str(path) for path in layout.report_paths()),
            ),
            ROOT,
            60,
            {"TMPDIR": str(layout.scratch)},
        ),
    )


def _output_text(value: str | bytes | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return value


def _signal_process_group(process: subprocess.Popen[str], signal_number: int) -> bool:
    try:
        os.killpg(process.pid, signal_number)
    except ProcessLookupError:
        return False
    except OSError as error:
        raise UnsafeProcessGroupError(
            f"could not signal process group {process.pid}: {error}"
        ) from error
    return True


def _process_group_exists(process: subprocess.Popen[str]) -> bool:
    try:
        os.killpg(process.pid, 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    except OSError as error:
        raise UnsafeProcessGroupError(
            f"could not probe process group {process.pid}: {error}"
        ) from error
    return True


def _require_process_group_stopped(
    process: subprocess.Popen[str],
    timeout_seconds: float = 10.0,
) -> None:
    deadline = time.monotonic() + timeout_seconds
    while _process_group_exists(process):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise UnsafeProcessGroupError(f"process group {process.pid} still exists after SIGKILL")
        time.sleep(min(0.05, remaining))


def _terminate_process_group(
    process: subprocess.Popen[str],
) -> tuple[str, str]:
    _signal_process_group(process, signal.SIGTERM)
    try:
        process.communicate(timeout=10)
    except subprocess.TimeoutExpired:
        pass
    if _process_group_exists(process):
        _signal_process_group(process, signal.SIGKILL)
    try:
        stdout, stderr = process.communicate(timeout=10)
    except subprocess.TimeoutExpired as error:
        raise UnsafeProcessGroupError(
            "process group did not terminate and drain after SIGTERM plus SIGKILL\n"
            f"stdout:\n{_output_text(error.stdout)}\n"
            f"stderr:\n{_output_text(error.stderr)}"
        ) from error
    _require_process_group_stopped(process)
    return stdout, stderr


def _terminate_process_group_or_unsafe(
    process: subprocess.Popen[str],
) -> tuple[str, str]:
    try:
        return _terminate_process_group(process)
    except UnsafeProcessGroupError:
        raise
    except BaseException as error:
        raise UnsafeProcessGroupError(
            "process-group termination was interrupted; cleanup is unsafe"
        ) from error


def _run(command: CommandSpec) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    if command.environment is not None:
        environment.update(command.environment)
    print(f"Projected live: {command.label}", file=sys.stderr, flush=True)
    process: subprocess.Popen[str] | None = None
    stopped_descendants = False
    try:
        process = subprocess.Popen(
            command.arguments,
            cwd=command.cwd,
            env=environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            start_new_session=True,
        )
        stdout, stderr = process.communicate(timeout=command.timeout_seconds)
        if _process_group_exists(process):
            _terminate_process_group_or_unsafe(process)
            stopped_descendants = True
    except subprocess.TimeoutExpired as error:
        if process is None:
            raise UnsafeProcessGroupError(
                f"{command.label} launch timed out before child absence could be proven"
            ) from error
        try:
            stdout, stderr = _terminate_process_group_or_unsafe(process)
        except UnsafeProcessGroupError as termination:
            raise UnsafeProcessGroupError(
                f"{command.label} exceeded {command.timeout_seconds} seconds; "
                f"cleanup is unsafe\n{termination}"
            ) from termination
        raise RunnerError(
            f"{command.label} exceeded {command.timeout_seconds} seconds\n"
            f"stdout:\n{stdout}\n"
            f"stderr:\n{stderr}"
        ) from error
    except OSError as error:
        if process is None:
            raise RunnerError(f"could not launch {command.label}: {error}") from error
        _terminate_process_group_or_unsafe(process)
        raise RunnerError(f"{command.label} process I/O failed: {error}") from error
    except BaseException as error:
        if process is None:
            raise UnsafeProcessGroupError(
                f"{command.label} launch was interrupted; child absence cannot be proven"
            ) from error
        _terminate_process_group_or_unsafe(process)
        raise
    if stopped_descendants:
        raise RunnerError(f"{command.label} left a live descendant process group")
    completed = subprocess.CompletedProcess(
        command.arguments,
        process.returncode,
        stdout=stdout,
        stderr=stderr,
    )
    if completed.returncode != 0:
        raise RunnerError(
            f"{command.label} failed with exit {completed.returncode}\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    return completed


def _require_regular_report(path: Path, label: str, contract: ModuleType) -> None:
    try:
        metadata = path.lstat()
    except OSError as error:
        raise RunnerError(f"{label} did not publish its report: {error}") from error
    if (
        stat.S_ISLNK(metadata.st_mode)
        or not stat.S_ISREG(metadata.st_mode)
        or metadata.st_size > contract.MAX_REPORT_BYTES
    ):
        raise RunnerError(f"{label} report is not one bounded regular non-symlink file")


def _cleanup_after_failure(layout: Layout, fixture: Fixture) -> str:
    cleanup = _database_guard(
        layout,
        fixture,
        "cleanup",
        layout.database_names(),
        "remove runner-owned databases after failure",
    )
    try:
        _run(cleanup)
    except RunnerError as error:
        return f"\nrunner cleanup also failed:\n{error}"
    return "\nrunner cleanup confirmed all four generated database names are absent"


def run(environment: Mapping[str, str] = os.environ, output: Path | None = None) -> str:
    fixture = _required_fixture(environment)
    contract = _load_live_contract()
    _validate_producer_sources(contract)
    _require_tools()
    with tempfile.TemporaryDirectory(prefix="typebridge-projected-live-") as temporary:
        layout = Layout.under(Path(temporary).resolve(), secrets.token_hex(12))
        _prepare(layout)
        producer_reports = {
            "produce the Python report": layout.python_report,
            "produce the Node report": layout.node_report,
            "produce the Rust report": layout.rust_report,
            "produce the C report": layout.c_report,
        }
        comparison = ""
        owns_database_names = False
        try:
            for command in command_plan(layout, fixture):
                report = producer_reports.get(command.label)
                if report is not None and (report.exists() or report.is_symlink()):
                    raise RunnerError(f"report path exists before {command.label}: {report}")
                completed = _run(command)
                if command.label == "verify exact fixture and four initially absent databases":
                    owns_database_names = True
                if report is not None:
                    _require_regular_report(report, command.label, contract)
                if command.label == "compare exactly four live reports":
                    comparison = completed.stdout
        except RunnerError as error:
            if owns_database_names and not isinstance(error, UnsafeProcessGroupError):
                raise RunnerError(f"{error}{_cleanup_after_failure(layout, fixture)}") from error
            raise
        except BaseException as error:
            if owns_database_names:
                cleanup = _cleanup_after_failure(layout, fixture)
                raise RunnerError(
                    f"Projected live fan-in was interrupted by {type(error).__name__}{cleanup}"
                ) from error
            raise
        try:
            summary = json.loads(comparison)
        except (json.JSONDecodeError, RecursionError) as error:
            raise RunnerError("the four-binding comparator emitted malformed JSON") from error
        if comparison.encode() != contract.canonical_json_bytes(summary):
            raise RunnerError("the four-binding comparator did not emit one canonical summary")
        if output is not None:
            try:
                publish(layout.report_paths(), output, checkout=ROOT)
            except PublishError as error:
                raise RunnerError(f"validated report publication failed: {error}") from error
        return comparison


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path)
    arguments = parser.parse_args(argv)
    try:
        summary = run(output=arguments.output)
    except RunnerError as error:
        print(f"Projected exact-live fan-in failed: {error}", file=sys.stderr)
        return 1
    sys.stdout.write(summary)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
