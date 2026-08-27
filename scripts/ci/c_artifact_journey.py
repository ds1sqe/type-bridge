#!/usr/bin/env python3
"""Run and validate the Plan 08 clean C consumer journey from candidate archives."""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import tempfile
from collections.abc import Mapping, Sequence
from pathlib import Path, PurePosixPath
from typing import Any

import c_package_candidates as packages
import standalone_cli_candidate as cli

ROOT = Path(__file__).resolve().parents[2]
FORMAT = "typebridge.c-artifact-clean-consumer/v1"
DISPOSITION = "candidate-only-unpublished-unsupported"
FULL_CONSUMER = ROOT / "type-bridge-core/crates/schema-codegen/tests/c_projection_live/consumer.c"
PHASE4_CONSUMER = (
    ROOT / "type-bridge-core/crates/schema-codegen/tests/c_projection_live/phase4_consumer.c"
)
PHASE4_PACKAGE = (
    ROOT / "type-bridge-core/crates/schema-codegen/tests/c_projection_live/phase4_package.c"
)


class JourneyError(RuntimeError):
    """The clean-consumer journey or its evidence is incomplete."""


def run(
    command: Sequence[str],
    *,
    cwd: Path = ROOT,
    env: Mapping[str, str] | None = None,
    expect_success: bool = True,
) -> str:
    try:
        result = subprocess.run(
            command,
            cwd=cwd,
            env=None if env is None else dict(env),
            check=False,
            capture_output=True,
            text=True,
            timeout=300,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise JourneyError(f"command could not run: {' '.join(command)}: {error}") from error
    if (result.returncode == 0) != expect_success:
        outcome = "unexpectedly passed" if result.returncode == 0 else "failed"
        raise JourneyError(
            f"command {outcome}: {' '.join(command)}\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result.stdout


def adapted_fixture(path: Path) -> bytes:
    """Adapt the frozen acceptance consumer to the independently packaged prefix."""
    source = path.read_text(encoding="utf-8")
    if source.count("#include <fixture/models.h>") != 1:
        raise JourneyError(f"fixture include authority drifted: {path.name}")
    if "fixture_" not in source:
        raise JourneyError(f"fixture symbol authority drifted: {path.name}")
    source = source.replace(
        "#include <fixture/models.h>",
        "#include <tb_workforcev3/tb_workforcev3.h>",
    )
    source = source.replace("fixture_", "tb_workforcev3_")
    source = source.replace("FIXTURE_", "TB_WORKFORCEV3_")
    return source.encode()


def adapted_flat_package() -> bytes:
    source = PHASE4_PACKAGE.read_text(encoding="utf-8")
    if source.count('#include "src/models.c"') != 1:
        raise JourneyError("flat package source include authority drifted")
    source = source.replace('#include "src/models.c"', '#include "src/tb_workforcev3.c"')
    source = source.replace("fixture_", "tb_workforcev3_")
    return source.encode()


def _write(files: Mapping[PurePosixPath, bytes], destination: Path) -> None:
    packages.write_files(files, destination)


def _compile(
    compiler: str,
    standard: str,
    sources: Sequence[Path],
    output: Path,
    runtime: Path,
    generated: Path,
    *,
    extra: Sequence[str] = (),
) -> None:
    run(
        [
            compiler,
            standard,
            "-O1",
            "-g",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic-errors",
            *extra,
            f"-I{runtime / 'include'}",
            f"-I{generated / 'include'}",
            *[str(source) for source in sources],
            f"-L{runtime / 'lib'}",
            "-ltype_bridge_c",
            f"-Wl,-rpath,{runtime / 'lib'}",
            "-o",
            str(output),
        ]
    )


def lifecycle_source() -> bytes:
    return b"""#include <string.h>\n#include <tb_workforcev3/tb_workforcev3.h>\nint main(void) {\n  type_bridge_schema_package_t *package = NULL;\n  type_bridge_execution_diagnostics_t *diagnostics = NULL;\n  type_bridge_cancellation_t *cancellation = NULL;\n  tb_workforcev3_identifier *identifier = NULL;\n  type_bridge_byte_view_t input = {(const uint8_t *)"artifact", 8u};\n  type_bridge_byte_view_t output = {0};\n  uint8_t requested = 0u;\n  if (tb_workforcev3_schema_package_open_v2(&package, &diagnostics) != TYPE_BRIDGE_STATUS_OK || diagnostics != NULL) return 10;\n  if (tb_workforcev3_identifier_open(package, input, &identifier, &diagnostics) != TYPE_BRIDGE_STATUS_OK || diagnostics != NULL) return 11;\n  if (tb_workforcev3_identifier_value(identifier, &output, &diagnostics) != TYPE_BRIDGE_STATUS_OK || output.length != 8u || memcmp(output.data, "artifact", 8u) != 0) return 12;\n  if (tb_workforcev3_identifier_close(&identifier) != TYPE_BRIDGE_STATUS_OK || identifier != NULL) return 13;\n  if (tb_workforcev3_identifier_close(&identifier) != TYPE_BRIDGE_STATUS_OK) return 14;\n  if (type_bridge_cancellation_open(&cancellation) != TYPE_BRIDGE_STATUS_OK) return 15;\n  if (type_bridge_cancellation_request(cancellation) != TYPE_BRIDGE_STATUS_OK || type_bridge_cancellation_is_requested(cancellation, &requested) != TYPE_BRIDGE_STATUS_OK || requested != 1u) return 16;\n  if (type_bridge_cancellation_close(&cancellation) != TYPE_BRIDGE_STATUS_OK || cancellation != NULL) return 17;\n  if (type_bridge_cancellation_close(&cancellation) != TYPE_BRIDGE_STATUS_OK) return 18;\n  if (type_bridge_schema_package_close(&package) != TYPE_BRIDGE_STATUS_OK || package != NULL) return 19;\n  return diagnostics == NULL ? 0 : 20;\n}\n"""


def negative_source() -> bytes:
    return b"""#include <tb_workforcev3/tb_workforcev3.h>\nvoid rejected(tb_workforcev3_person *person) {\n  tb_workforcev3_membership *relation = person;\n  (void)relation;\n}\n"""


def loader_source() -> bytes:
    return b"""#define _GNU_SOURCE\n#include <dlfcn.h>\n#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\ntypedef struct { const unsigned char *data; size_t length; } byte_view;\ntypedef unsigned int (*version_fn)(byte_view *);\nint main(int argc, char **argv) {\n  void *library; version_fn version; byte_view value = {0}; char maps[4096]; FILE *stream;\n  if (argc != 2) return 10;\n  library = dlopen(argv[1], RTLD_NOW | RTLD_LOCAL); if (library == NULL) return 11;\n  *(void **)(&version) = dlsym(library, "type_bridge_runtime_version");\n  if (version == NULL || version(&value) != 0u || value.length != 5u || memcmp(value.data, "2.1.0", 5u) != 0) return 12;\n  if (dlclose(library) != 0) return 13;\n  stream = fopen("/proc/self/maps", "r"); if (stream == NULL) return 14;\n  while (fgets(maps, sizeof(maps), stream) != NULL) {\n    if (strstr(maps, "libtype_bridge_c.so") != NULL) { fclose(stream); return 15; }\n  }\n  return fclose(stream) == 0 ? 0 : 16;\n}\n"""


def provider_free(
    cli_archive: Path, runtime_archive: Path, generated_archive: Path
) -> dict[str, Any]:
    cli_manifest = cli.validate(cli_archive)
    runtime_manifest = packages.validate_runtime(runtime_archive)
    generated_manifest = packages.validate_generated(generated_archive)
    package_smoke = packages.smoke(runtime_archive, generated_archive)
    runtime_files = packages.safe_archive_files(runtime_archive, packages.RUNTIME_MEMBERS)
    generated_files = packages.safe_archive_files(generated_archive, packages.PACKAGE_MEMBERS)
    with tempfile.TemporaryDirectory(prefix="type-bridge-c-artifact-journey-") as temporary:
        root = Path(temporary)
        runtime = root / "runtime prefix"
        generated = root / "generated prefix"
        runtime.mkdir()
        generated.mkdir()
        _write(runtime_files, runtime)
        _write(generated_files, generated)
        source = root / "consumer"
        source.mkdir()
        generated_source = generated / "src/tb_workforcev3.c"

        lifecycle = source / "lifecycle.c"
        lifecycle.write_bytes(lifecycle_source())
        sanitized = root / "sanitized-lifecycle"
        _compile(
            "clang",
            "-std=c17",
            [generated_source, lifecycle],
            sanitized,
            runtime,
            generated,
            extra=("-fsanitize=address,undefined", "-fno-omit-frame-pointer"),
        )
        run(
            [str(sanitized)],
            env={
                **os.environ,
                "ASAN_OPTIONS": "detect_leaks=1:halt_on_error=1:abort_on_error=1",
                "UBSAN_OPTIONS": "halt_on_error=1:print_stacktrace=1",
            },
        )

        negative = source / "negative.c"
        negative.write_bytes(negative_source())
        for compiler, standard in (
            ("gcc", "-std=c17"),
            ("clang", "-std=c17"),
            ("g++", "-std=c++17"),
            ("clang++", "-std=c++17"),
        ):
            run(
                [
                    compiler,
                    standard,
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-pedantic-errors",
                    f"-I{runtime / 'include'}",
                    f"-I{generated / 'include'}",
                    "-fsyntax-only",
                    str(negative),
                ],
                expect_success=False,
            )

        loader = source / "loader.c"
        loader.write_bytes(loader_source())
        loader_executable = root / "loader-unload"
        run(
            [
                "cc",
                "-std=c17",
                "-Wall",
                "-Wextra",
                "-Werror",
                str(loader),
                "-ldl",
                "-o",
                str(loader_executable),
            ]
        )
        run([str(loader_executable), str(runtime / "lib/libtype_bridge_c.so")])

        # Both canonical live sources must compile directly against the archive
        # package. Execution is the separately reported connected lane.
        full = source / "full.c"
        full.write_bytes(adapted_fixture(FULL_CONSUMER))
        run(
            [
                "cc",
                "-std=c17",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic-errors",
                "-DTYPE_BRIDGE_WORKFORCE_V5_C_CODEC",
                f"-I{runtime / 'include'}",
                f"-I{generated / 'include'}",
                "-fsyntax-only",
                str(full),
            ]
        )
        phase4 = source / "phase4.c"
        phase4.write_bytes(adapted_fixture(PHASE4_CONSUMER))
        for compiler, standard in (("cc", "-std=c17"), ("c++", "-std=c++17")):
            run(
                [
                    compiler,
                    standard,
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-pedantic-errors",
                    f"-I{runtime / 'include'}",
                    f"-I{generated / 'include'}",
                    "-fsyntax-only",
                    str(phase4),
                ]
            )

    report = {
        "artifacts": {
            "cli": {
                "candidate-id": cli_manifest["candidate-id"],
                "sha256": cli.sha256(cli_archive.read_bytes()),
            },
            "generated-package": {
                "candidate-id": generated_manifest["candidate-id"],
                "sha256": packages.sha256(generated_archive.read_bytes()),
            },
            "runtime": {
                "candidate-id": runtime_manifest["candidate-id"],
                "sha256": packages.sha256(runtime_archive.read_bytes()),
            },
        },
        "connected": None,
        "format": FORMAT,
        "platform": packages.TARGET,
        "provider-free": {
            "archive-package-smoke": package_smoke,
            "canonical-live-sources-compile": {"c17": True, "cpp17": True},
            "compile-negative": ["gcc-c17", "clang-c17", "gcc-cpp17", "clang-cpp17"],
            "loader-unload": "unmapped-after-dlclose",
            "sanitizers": ["address", "leak", "undefined"],
        },
        "publication-disposition": DISPOSITION,
    }
    validate_report(report, cli_archive, runtime_archive, generated_archive)
    return report


def validate_report(
    report: dict[str, Any], cli_archive: Path, runtime_archive: Path, generated_archive: Path
) -> None:
    if set(report) != {
        "artifacts",
        "connected",
        "format",
        "platform",
        "provider-free",
        "publication-disposition",
    }:
        raise JourneyError("clean-consumer report member set drifted")
    if report["format"] != FORMAT or report["platform"] != packages.TARGET:
        raise JourneyError("clean-consumer report identity drifted")
    if report["publication-disposition"] != DISPOSITION:
        raise JourneyError("clean-consumer report widened publication authority")
    expected = {
        "cli": (cli.validate(cli_archive)["candidate-id"], cli.sha256(cli_archive.read_bytes())),
        "runtime": (
            packages.validate_runtime(runtime_archive)["candidate-id"],
            packages.sha256(runtime_archive.read_bytes()),
        ),
        "generated-package": (
            packages.validate_generated(generated_archive)["candidate-id"],
            packages.sha256(generated_archive.read_bytes()),
        ),
    }
    artifacts = report.get("artifacts")
    if not isinstance(artifacts, dict) or set(artifacts) != set(expected):
        raise JourneyError("clean-consumer artifact inventory drifted")
    for name, (candidate_id, digest) in expected.items():
        if artifacts[name] != {"candidate-id": candidate_id, "sha256": digest}:
            raise JourneyError(f"clean-consumer artifact binding drifted: {name}")
    provider = report.get("provider-free")
    if not isinstance(provider, dict) or provider.get("loader-unload") != "unmapped-after-dlclose":
        raise JourneyError("loader-unload evidence is missing")
    if provider.get("sanitizers") != ["address", "leak", "undefined"]:
        raise JourneyError("sanitizer evidence is incomplete")
    if provider.get("compile-negative") != ["gcc-c17", "clang-c17", "gcc-cpp17", "clang-cpp17"]:
        raise JourneyError("compile-negative matrix is incomplete")


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    provider = commands.add_parser("provider-free")
    provider.add_argument("cli", type=Path)
    provider.add_argument("runtime", type=Path)
    provider.add_argument("generated", type=Path)
    validate = commands.add_parser("validate-report")
    validate.add_argument("report", type=Path)
    validate.add_argument("cli", type=Path)
    validate.add_argument("runtime", type=Path)
    validate.add_argument("generated", type=Path)
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    arguments = parse_args(argv)
    try:
        if arguments.command == "provider-free":
            result = provider_free(arguments.cli, arguments.runtime, arguments.generated)
            print(packages.canonical_json(result).decode(), end="")
        else:
            value = json.loads(
                arguments.report.read_text(encoding="utf-8"),
                object_pairs_hook=packages.unique_object,
            )
            if (
                not isinstance(value, dict)
                or packages.canonical_json(value) != arguments.report.read_bytes()
            ):
                raise JourneyError("clean-consumer report is not canonical JSON")
            validate_report(value, arguments.cli, arguments.runtime, arguments.generated)
            print("validated artifact-only clean-consumer report")
    except (
        JourneyError,
        packages.CandidateError,
        cli.CandidateError,
        OSError,
        json.JSONDecodeError,
    ) as error:
        print(f"C artifact journey rejected: {error}", file=os.sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
