#!/usr/bin/env python3
"""Run and validate the Plan 08 clean C consumer journey from candidate archives."""

from __future__ import annotations

import argparse
import base64
import json
import os
import re
import socket
import subprocess
import tempfile
import time
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
SETUP_SOURCE = ROOT / "type-bridge-core/crates/schema-codegen/tests/c_projection_live/setup.rs"
PROVIDER_SCHEMA = ROOT / "tests/contracts/sdk_conformance/workforce-v3/provider-3.12.1-v3.tql"
FULL_MARKER_COUNT = 61
PHASE4_MARKER_COUNT = 8
CODEC_MARKER = "Workforce V5 C live codec direct/remote parity: passed"


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
    """Adapt frozen acceptance source to the independently packaged Workforce V3."""
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
    if path == FULL_CONSUMER:
        # The predecessor query fixture allowed arbitrary nickname strings.
        # Workforce V3 freezes the field to Ada/Dana. Preserve the one
        # optional-field query row and omit the field from all other people;
        # this changes fixture data only, never generated/runtime semantics.
        old_open = "if (!is_query_dana && !is_v5_live) {"
        if source.count(old_open) != 1:
            raise JourneyError("full consumer nickname-open seam drifted")
        source = source.replace(old_open, "if (is_query_ada) {", 1)
        old_check = """if (is_query_dana) {
    CHECK(nickname == NULL);
  } else {
    CHECK(tb_workforcev3_nickname_value(nickname, &text, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(same_text(text, is_query_ada ? "Ada" : expected_identifier));
    CHECK(tb_workforcev3_nickname_close(&nickname) == TYPE_BRIDGE_STATUS_OK);
  }"""
        new_check = """if (!is_query_ada) {
    CHECK(nickname == NULL);
  } else {
    CHECK(tb_workforcev3_nickname_value(nickname, &text, out_diagnostics) ==
          TYPE_BRIDGE_STATUS_OK);
    CHECK(same_text(text, "Ada"));
    CHECK(tb_workforcev3_nickname_close(&nickname) == TYPE_BRIDGE_STATUS_OK);
  }"""
        if source.count(old_check) != 1:
            raise JourneyError("full consumer nickname-check seam drifted")
        source = source.replace(old_check, new_check, 1)
        if source.count('view_of("query route")') != 1:
            raise JourneyError("full consumer relation nickname seam drifted")
        source = source.replace('view_of("query route")', 'view_of("Ada")', 1)
        old_alias_input = "args.field_aliases_chunks = is_v5_live ? NULL : &aliases_chunk;"
        if source.count(old_alias_input) != 1:
            raise JourneyError("full consumer ordered-alias input seam drifted")
        source = source.replace(old_alias_input, "args.field_aliases_chunks = NULL;", 1)
        old_alias_check = """CHECK(tb_workforcev3_person_aliases_count(person, &aliases_count,
                                     out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(aliases_count == 1u);
  CHECK(tb_workforcev3_person_aliases_at(person, 0u, &alias, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(tb_workforcev3_aliases_value(alias, &text, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(same_text(text, expected_alias_value));
  CHECK(tb_workforcev3_aliases_close(&alias) == TYPE_BRIDGE_STATUS_OK);"""
        new_alias_check = """CHECK(tb_workforcev3_person_aliases_count(person, &aliases_count,
                                     out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(aliases_count == 0u);
  CHECK(tb_workforcev3_aliases_close(&alias) == TYPE_BRIDGE_STATUS_OK);
  (void)expected_alias_value;"""
        if source.count(old_alias_check) != 1:
            raise JourneyError("full consumer ordered-alias check seam drifted")
        source = source.replace(old_alias_check, new_alias_check, 1)
        old_alias_observation = """CHECK(tb_workforcev3_person_aliases_count(person, &alias_count, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(alias_count != 0u);
  CHECK(tb_workforcev3_person_aliases_at(person, 0u, &alias, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(tb_workforcev3_aliases_value(alias, &value, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(copy_stable_text(value, observation->first_alias,
                         sizeof(observation->first_alias)));
  CHECK(tb_workforcev3_aliases_close(&alias) == TYPE_BRIDGE_STATUS_OK);"""
        new_alias_observation = """CHECK(tb_workforcev3_person_aliases_count(person, &alias_count, out_diagnostics) ==
        TYPE_BRIDGE_STATUS_OK);
  CHECK(alias_count == 0u);
  CHECK(snprintf(observation->first_alias, sizeof(observation->first_alias),
                 "%s", "ordered-omitted") > 0);
  CHECK(tb_workforcev3_aliases_close(&alias) == TYPE_BRIDGE_STATUS_OK);"""
        if source.count(old_alias_observation) != 1:
            raise JourneyError("full consumer ordered-alias observation seam drifted")
        source = source.replace(old_alias_observation, new_alias_observation, 1)
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


def _tls_source_template() -> bytes:
    return b"""#include <stdio.h>\n#include <stdlib.h>\n#include <string.h>\n#include <tb_workforcev3/tb_workforcev3.h>\n+static type_bridge_byte_view_t view(const char *text) { type_bridge_byte_view_t value = {(const uint8_t *)text, strlen(text)}; return value; }\n+int main(int argc, char **argv) {\n+  FILE *stream; long length; uint8_t *pem = NULL; type_bridge_schema_package_t *package = NULL;\n+  type_bridge_execution_diagnostics_t *diagnostics = NULL; type_bridge_runtime_t *runtime = NULL; type_bridge_database_t *database = NULL;\n+  type_bridge_byte_view_t version = {0}; type_bridge_runtime_config_v1_t runtime_config = {0}; type_bridge_database_config_v2_t config = {0};\n+  type_bridge_query_execution_limits_v1_t limits = TYPE_BRIDGE_QUERY_EXECUTION_LIMITS_V1_DEFAULT;\n+  if (argc != 7) return 10; stream = fopen(argv[6], \"rb\"); if (stream == NULL) return 11;\n+  if (fseek(stream, 0, SEEK_END) != 0 || (length = ftell(stream)) <= 0 || length > TYPE_BRIDGE_DATABASE_CUSTOM_ROOT_CA_BYTES_MAX || fseek(stream, 0, SEEK_SET) != 0) return 12;\n+  pem = (uint8_t *)malloc((size_t)length); if (pem == NULL || fread(pem, 1u, (size_t)length, stream) != (size_t)length || fclose(stream) != 0) return 13;\n+  if (tb_workforcev3_schema_package_open_v2(&package, &diagnostics) != TYPE_BRIDGE_STATUS_OK || diagnostics != NULL) return 14;\n+  runtime_config.struct_size = sizeof(runtime_config); runtime_config.version = TYPE_BRIDGE_RUNTIME_CONFIG_VERSION; runtime_config.worker_threads = TYPE_BRIDGE_RUNTIME_WORKER_THREADS_MIN;\n+  if (type_bridge_runtime_open_v1(&runtime_config, &runtime, &diagnostics) != TYPE_BRIDGE_STATUS_OK) return 15;\n+  config.struct_size = sizeof(config); config.version = TYPE_BRIDGE_DATABASE_CONFIG_V2_VERSION; config.address = view(argv[1]); config.http_port = (uint32_t)strtoul(argv[2], NULL, 10);\n+  config.database = view(argv[3]); config.username = view(argv[4]); config.password = view(argv[5]); config.tls_mode = TYPE_BRIDGE_TLS_CUSTOM_ROOT_CA;\n+  config.custom_root_ca_pem.data = pem; config.custom_root_ca_pem.length = (size_t)length; config.connection_limits = limits; config.answer_limits = limits;\n+  if (type_bridge_database_open_v2(runtime, package, &config, NULL, &database, &diagnostics) != TYPE_BRIDGE_STATUS_OK || diagnostics != NULL) return 16;\n+  memset(pem, 0xa5, (size_t)length); free(pem); pem = NULL;\n+  if (type_bridge_database_server_version(database, &version) != TYPE_BRIDGE_STATUS_OK || version.length != 6u || memcmp(version.data, \"3.12.3\", 6u) != 0) return 17;\n+  if (type_bridge_database_close(&database, &diagnostics) != TYPE_BRIDGE_STATUS_OK || database != NULL) return 18;\n+  if (type_bridge_runtime_close(&runtime, &diagnostics) != TYPE_BRIDGE_STATUS_OK || runtime != NULL) return 19;\n+  if (type_bridge_schema_package_close(&package) != TYPE_BRIDGE_STATUS_OK || package != NULL || diagnostics != NULL) return 20;\n+  puts(\"custom-root TLS direct artifact connection: passed\"); return 0;\n+}\n"""


def tls_source() -> bytes:
    return _tls_source_template().replace(b"\n+", b"\n")


def passed_markers(path: Path, *, exclude_codec: bool = False) -> list[str]:
    markers = [
        match.group(1)
        for match in re.finditer(
            r'^\s*puts\("([^"]+: passed)"\);\s*$', path.read_text(), re.MULTILINE
        )
    ]
    if exclude_codec:
        markers = [
            marker for marker in markers if not marker.startswith("Workforce V5 C live codec")
        ]
    if len(markers) != len(set(markers)):
        raise JourneyError(f"duplicate connected marker authority: {path.name}")
    return markers


def require_markers(stdout: str, markers: Sequence[str], label: str) -> None:
    missing = [marker for marker in markers if marker not in stdout]
    if missing:
        raise JourneyError(f"{label} omitted connected markers: {missing}")


def tls_probe(
    runtime: Path,
    generated: Path,
    *,
    address: str,
    http_port: str,
    database: str,
    username: str,
    password: str,
    root_ca: Path,
    root: Path,
) -> dict[str, Any]:
    if not root_ca.is_file() or root_ca.is_symlink():
        raise JourneyError("TLS root CA must be a regular non-symlink file")
    source = root / "tls-consumer.c"
    body = tls_source()
    body = body.replace(
        b'if (argc != 7) return 10; stream = fopen(argv[6], "rb");',
        b'if (argc != 7) return 10;\n  stream = fopen(argv[6], "rb");',
    )
    source.write_bytes(body)
    executable = root / "tls-consumer"
    _compile(
        "cc",
        "-std=c17",
        [generated / "src/tb_workforcev3.c", source],
        executable,
        runtime,
        generated,
    )
    marker = "custom-root TLS direct artifact connection: passed"
    stdout = run(
        [
            str(executable),
            address,
            http_port,
            database,
            username,
            password,
            str(root_ca),
        ],
        env={**os.environ, "LD_LIBRARY_PATH": str(runtime / "lib")},
    )
    require_markers(stdout, [marker], "custom-root TLS consumer")
    return {
        "captured-root-buffer-overwritten-after-open": True,
        "direct": True,
        "marker": marker,
        "mode": "custom-root",
    }


def connected(
    runtime_archive: Path,
    generated_archive: Path,
    *,
    address: str,
    http_port: str,
    database: str,
    username: str,
    password: str,
    remote_port: str,
    tls_address: str | None = None,
    tls_http_port: str | None = None,
    tls_root_ca: Path | None = None,
) -> dict[str, Any]:
    runtime_manifest = packages.validate_runtime(runtime_archive)
    generated_manifest = packages.validate_generated(generated_archive)
    runtime_files = packages.safe_archive_files(runtime_archive, packages.RUNTIME_MEMBERS)
    generated_files = packages.safe_archive_files(generated_archive, packages.PACKAGE_MEMBERS)
    full_markers = passed_markers(FULL_CONSUMER, exclude_codec=True)
    phase4_markers = passed_markers(PHASE4_CONSUMER)
    if len(full_markers) != FULL_MARKER_COUNT or len(phase4_markers) != PHASE4_MARKER_COUNT:
        raise JourneyError("connected marker inventory drifted")
    environment = {
        **os.environ,
        "TYPEDB_ADDRESS": address,
        "TYPEDB_HTTP_PORT": http_port,
        "TYPE_BRIDGE_C_PROJECTION_INTG_DATABASE": database,
        "TYPEDB_USERNAME": username,
        "TYPEDB_PASSWORD": password,
        "TYPE_BRIDGE_C_REMOTE_PORT": remote_port,
    }
    with tempfile.TemporaryDirectory(prefix="type-bridge-c-artifact-connected-") as temporary:
        root = Path(temporary)
        runtime = root / "runtime prefix"
        generated = root / "generated prefix"
        runtime.mkdir()
        generated.mkdir()
        _write(runtime_files, runtime)
        _write(generated_files, generated)
        environment["LD_LIBRARY_PATH"] = str(runtime / "lib")
        generated_source = generated / "src/tb_workforcev3.c"
        tls_values = (tls_address, tls_http_port, tls_root_ca)
        if any(value is not None for value in tls_values) and not all(
            value is not None for value in tls_values
        ):
            raise JourneyError("TLS connected lane requires address, HTTP port, and root CA")
        tls = (
            tls_probe(
                runtime,
                generated,
                address=tls_address,
                http_port=tls_http_port,
                database=database,
                username=username,
                password=password,
                root_ca=tls_root_ca,
                root=root,
            )
            if tls_address is not None and tls_http_port is not None and tls_root_ca is not None
            else None
        )

        full_source = root / "full-consumer.c"
        full_source.write_bytes(adapted_fixture(FULL_CONSUMER))
        full_executable = root / "full-consumer"
        _compile(
            "cc",
            "-std=c17",
            [generated_source, full_source],
            full_executable,
            runtime,
            generated,
        )
        full_stdout = run([str(full_executable)], env=environment)
        require_markers(full_stdout, full_markers, "full C17 consumer")

        codec_executable = root / "codec-consumer"
        _compile(
            "cc",
            "-std=c17",
            [generated_source, full_source],
            codec_executable,
            runtime,
            generated,
            extra=("-DTYPE_BRIDGE_WORKFORCE_V5_C_CODEC",),
        )
        codec_evidence = root / "codec-evidence"
        codec_evidence.mkdir()
        codec_stdout = run(
            [str(codec_executable)],
            env={
                **environment,
                "TYPE_BRIDGE_WORKFORCE_V5_C_EVIDENCE_DIR": str(codec_evidence),
            },
        )
        require_markers(codec_stdout, [CODEC_MARKER], "Workforce V5 codec consumer")
        codec_files = {}
        for name in ("entity.bin", "relation.bin"):
            body = packages.read_regular(codec_evidence / name)
            if not body:
                raise JourneyError(f"Workforce V5 codec evidence is empty: {name}")
            codec_files[name] = {"sha256": packages.sha256(body), "size": len(body)}

        package_source = root / "phase4-package.c"
        package_source.write_bytes(adapted_flat_package())
        package_object = root / "phase4-package.o"
        run(
            [
                "cc",
                "-std=c17",
                "-O1",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic-errors",
                f"-I{runtime / 'include'}",
                f"-I{generated / 'include'}",
                "-I",
                str(generated),
                "-c",
                str(package_source),
                "-o",
                str(package_object),
            ]
        )
        phase4_source = adapted_fixture(PHASE4_CONSUMER)
        phase4_outputs: dict[str, str] = {}
        for compiler, standard, language in (
            ("cc", "-std=c17", "c17"),
            ("c++", "-std=c++17", "cpp17"),
        ):
            source = root / f"phase4-{language}.c"
            source.write_bytes(phase4_source)
            executable = root / f"phase4-{language}"
            _compile(
                compiler,
                standard,
                [source, package_object],
                executable,
                runtime,
                generated,
            )
            stdout = run([str(executable)], env=environment)
            require_markers(stdout, phase4_markers, f"Phase4 {language} consumer")
            phase4_outputs[language] = packages.sha256(stdout.encode())

    return {
        "artifacts": {
            "generated-package": generated_manifest["candidate-id"],
            "runtime": runtime_manifest["candidate-id"],
        },
        "format": "typebridge.c-artifact-connected-observation/v1",
        "full-c17-marker-count": len(full_markers),
        "model-codec": {
            "direct-remote-equal": True,
            "files": codec_files,
            "marker": CODEC_MARKER,
        },
        "phase4": {
            "c17-marker-count": len(phase4_markers),
            "cpp17-marker-count": len(phase4_markers),
            "stdout-sha256": phase4_outputs,
        },
        "plaintext-direct": True,
        "remote": {"caller-transport": True},
        "tls": tls,
    }


def embedded_resource(source: bytes, label: str) -> bytes:
    pattern = re.compile(
        rf"static const uint8_t {packages.PACKAGE_NAME}_{label}_chunk_\d+\[\] = \{{\n(.*?)\n\}};",
        re.DOTALL,
    )
    chunks = pattern.findall(source.decode())
    if not chunks:
        raise JourneyError(f"generated package omits embedded {label}")
    return bytes(
        int(value, 16) for chunk in chunks for value in re.findall(r"0x([0-9a-fA-F]{2})u", chunk)
    )


def free_port() -> int:
    with socket.socket() as candidate:
        candidate.bind(("127.0.0.1", 0))
        return int(candidate.getsockname()[1])


def setup_workspace(root: Path) -> Path:
    source = SETUP_SOURCE.read_text(encoding="utf-8")
    old_provider = 'include_str!("../acceptance/provider-3.12.1.tql")'
    if source.count(old_provider) != 1:
        raise JourneyError("connected setup provider seam drifted")
    source = source.replace(old_provider, f'include_str!("{PROVIDER_SCHEMA}")', 1)
    setup = root / "setup.rs"
    setup.write_text(source, encoding="utf-8")
    manifest = root / "Cargo.toml"
    manifest.write_text(
        '[package]\nname = "type-bridge-c-artifact-live-setup"\n'
        'version = "0.0.0"\nedition = "2024"\npublish = false\n\n'
        '[[bin]]\nname = "setup"\npath = "setup.rs"\n\n'
        f'[dependencies]\ntype-bridge-orm = {{ path = "{ROOT / "type-bridge-core/crates/orm"}" }}\n'
        'tokio = { version = "1", features = ["macros", "rt-multi-thread"] }\n\n'
        "[workspace]\n",
        encoding="utf-8",
    )
    return manifest


def fixture_environment(
    *, address: str, http_port: str, database: str, username: str, password: str
) -> dict[str, str]:
    return {
        **os.environ,
        "TYPEDB_ADDRESS": address,
        "TYPEDB_HTTP_PORT": http_port,
        "TYPE_BRIDGE_C_PROJECTION_INTG_DATABASE": database,
        "TYPEDB_USERNAME": username,
        "TYPEDB_PASSWORD": password,
        "TYPE_BRIDGE_C_EXPECTED_PROVIDER_PATCH": "3",
    }


def run_setup(manifest: Path, target: Path, environment: Mapping[str, str], mode: str) -> str:
    return run(
        ["cargo", "run", "--quiet", "--manifest-path", str(manifest), "--", mode],
        env={**environment, "CARGO_TARGET_DIR": str(target)},
    )


def wait_for_server(process: subprocess.Popen[str], port: int) -> None:
    for _ in range(100):
        if process.poll() is not None:
            stdout, stderr = process.communicate()
            raise JourneyError(f"remote fixture exited early:\n{stdout}\n{stderr}")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                return
        except OSError:
            time.sleep(0.1)
    raise JourneyError("remote fixture did not become ready")


def live_journey(
    cli_archive: Path,
    runtime_archive: Path,
    generated_archive: Path,
    *,
    address: str,
    http_port: str,
    database: str,
    migration_database: str,
    username: str,
    password: str,
    infra_target: Path,
    tls_address: str | None = None,
    tls_http_port: str | None = None,
    tls_root_ca: Path | None = None,
) -> dict[str, Any]:
    for value in (database, migration_database):
        if re.fullmatch(r"[a-z][a-z0-9_]{2,62}", value) is None:
            raise JourneyError(f"unsafe isolated database name: {value}")
    if database == migration_database:
        raise JourneyError("data and migration fixtures must use different databases")
    generated_files = packages.safe_archive_files(generated_archive, packages.PACKAGE_MEMBERS)
    authority = embedded_resource(
        generated_files[PurePosixPath(f"src/{packages.PACKAGE_NAME}.c")],
        "schema_authority_json",
    )
    with tempfile.TemporaryDirectory(prefix="type-bridge-c-artifact-live-infra-") as temporary:
        root = Path(temporary)
        manifest = setup_workspace(root)
        data_environment = fixture_environment(
            address=address,
            http_port=http_port,
            database=database,
            username=username,
            password=password,
        )
        migration_environment = fixture_environment(
            address=address,
            http_port=http_port,
            database=migration_database,
            username=username,
            password=password,
        )
        server: subprocess.Popen[str] | None = None
        data_touched = False
        migration_touched = False
        observation: dict[str, Any] | None = None
        migration: dict[str, Any] | None = None
        try:
            data_touched = True
            run_setup(manifest, infra_target, data_environment, "setup")
            run(
                [
                    "cargo",
                    "build",
                    "--locked",
                    "--manifest-path",
                    str(ROOT / "type-bridge-core/Cargo.toml"),
                    "-p",
                    "type-bridge-server",
                    "--features",
                    "v2-query",
                    "--example",
                    "v2_smoke_server",
                ],
                env={**os.environ, "CARGO_TARGET_DIR": str(infra_target)},
            )
            port = free_port()
            server_environment = {
                **os.environ,
                "SMOKE_TYPEDB_ADDRESS": address,
                "SMOKE_TYPEDB_USERNAME": username,
                "SMOKE_TYPEDB_PASSWORD": password,
                "SMOKE_TYPEDB_HTTP_PORT": http_port,
                "SMOKE_DATABASE": database,
                "SMOKE_AUTHORITY_B64": base64.b64encode(authority).decode(),
                "SMOKE_PORT": str(port),
            }
            server = subprocess.Popen(
                [str(infra_target / "debug/examples/v2_smoke_server")],
                env=server_environment,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            wait_for_server(server, port)
            observation = connected(
                runtime_archive,
                generated_archive,
                address=address,
                http_port=http_port,
                database=database,
                username=username,
                password=password,
                remote_port=str(port),
                tls_address=tls_address,
                tls_http_port=tls_http_port,
                tls_root_ca=tls_root_ca,
            )
            migration_touched = True
            migration = cli.connected_smoke(
                cli_archive,
                address=address,
                http_port=http_port,
                database=migration_database,
                username=username,
                password=password,
            )
            if migration.pop("database", None) != migration_database:
                raise JourneyError("CLI migration observation database drifted")
        finally:
            if server is not None:
                server.terminate()
                try:
                    server.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait(timeout=10)
            if data_touched:
                run_setup(manifest, infra_target, data_environment, "cleanup")
            if migration_touched:
                run_setup(manifest, infra_target, migration_environment, "cleanup")
        if observation is None or migration is None:
            raise JourneyError("connected artifact journey did not publish observations")
        report = {
            "artifacts": {
                "cli": {
                    "candidate-id": cli.validate(cli_archive)["candidate-id"],
                    "sha256": cli.sha256(cli_archive.read_bytes()),
                },
                "generated-package": {
                    "candidate-id": packages.validate_generated(generated_archive)["candidate-id"],
                    "sha256": packages.sha256(generated_archive.read_bytes()),
                },
                "runtime": {
                    "candidate-id": packages.validate_runtime(runtime_archive)["candidate-id"],
                    "sha256": packages.sha256(runtime_archive.read_bytes()),
                },
            },
            "cleanup": {"data-database": "removed", "migration-database": "removed"},
            "data-query-remote": observation,
            "format": "typebridge.c-artifact-live-journey/v1",
            "migration": migration,
            "test-infrastructure": {
                "cargo-confined-to-fixture-processes": True,
                "consumer-cargo-dependency": False,
                "consumer-python-dependency": False,
                "consumer-source-library-fallback": False,
            },
        }
        validate_live_report(report, cli_archive, runtime_archive, generated_archive)
        return report


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


def validate_live_report(
    report: dict[str, Any],
    cli_archive: Path,
    runtime_archive: Path,
    generated_archive: Path,
) -> None:
    if set(report) != {
        "artifacts",
        "cleanup",
        "data-query-remote",
        "format",
        "migration",
        "test-infrastructure",
    }:
        raise JourneyError("live artifact report member set drifted")
    if report["format"] != "typebridge.c-artifact-live-journey/v1":
        raise JourneyError("live artifact report identity drifted")
    expected_artifacts = {
        "cli": {
            "candidate-id": cli.validate(cli_archive)["candidate-id"],
            "sha256": cli.sha256(cli_archive.read_bytes()),
        },
        "generated-package": {
            "candidate-id": packages.validate_generated(generated_archive)["candidate-id"],
            "sha256": packages.sha256(generated_archive.read_bytes()),
        },
        "runtime": {
            "candidate-id": packages.validate_runtime(runtime_archive)["candidate-id"],
            "sha256": packages.sha256(runtime_archive.read_bytes()),
        },
    }
    if report["artifacts"] != expected_artifacts:
        raise JourneyError("live artifact report archive binding drifted")
    if report["cleanup"] != {
        "data-database": "removed",
        "migration-database": "removed",
    }:
        raise JourneyError("live artifact report cleanup is incomplete")
    connected_value = report["data-query-remote"]
    if not isinstance(connected_value, dict):
        raise JourneyError("live artifact connected observation is missing")
    if connected_value.get("format") != "typebridge.c-artifact-connected-observation/v1":
        raise JourneyError("live artifact connected identity drifted")
    if connected_value.get("full-c17-marker-count") != FULL_MARKER_COUNT:
        raise JourneyError("live artifact full-consumer markers are incomplete")
    codec = connected_value.get("model-codec")
    if not isinstance(codec, dict) or codec.get("marker") != CODEC_MARKER:
        raise JourneyError("live artifact model-codec marker is missing")
    if codec.get("direct-remote-equal") is not True:
        raise JourneyError("live artifact model-codec lanes disagree")
    codec_files = codec.get("files")
    if not isinstance(codec_files, dict) or set(codec_files) != {"entity.bin", "relation.bin"}:
        raise JourneyError("live artifact model-codec evidence inventory drifted")
    for value in codec_files.values():
        if (
            not isinstance(value, dict)
            or not isinstance(value.get("size"), int)
            or value["size"] <= 0
            or re.fullmatch(r"[0-9a-f]{64}", value.get("sha256", "")) is None
        ):
            raise JourneyError("live artifact model-codec evidence identity drifted")
    if connected_value.get("plaintext-direct") is not True or connected_value.get("remote") != {
        "caller-transport": True
    }:
        raise JourneyError("live artifact direct/remote lanes are incomplete")
    if connected_value.get("tls") != {
        "captured-root-buffer-overwritten-after-open": True,
        "direct": True,
        "marker": "custom-root TLS direct artifact connection: passed",
        "mode": "custom-root",
    }:
        raise JourneyError("live artifact TLS lane is incomplete")
    phase4 = connected_value.get("phase4")
    if not isinstance(phase4, dict) or phase4.get("c17-marker-count") != PHASE4_MARKER_COUNT:
        raise JourneyError("live artifact C17 successor markers are incomplete")
    if phase4.get("cpp17-marker-count") != PHASE4_MARKER_COUNT:
        raise JourneyError("live artifact C++17 successor markers are incomplete")
    hashes = phase4.get("stdout-sha256")
    if not isinstance(hashes, dict) or set(hashes) != {"c17", "cpp17"}:
        raise JourneyError("live artifact successor output identities are incomplete")
    if hashes["c17"] != hashes["cpp17"] or re.fullmatch(r"[0-9a-f]{64}", hashes["c17"]) is None:
        raise JourneyError("live artifact C/C++ successor outcomes disagree")
    if connected_value.get("artifacts") != {
        "generated-package": expected_artifacts["generated-package"]["candidate-id"],
        "runtime": expected_artifacts["runtime"]["candidate-id"],
    }:
        raise JourneyError("live connected observation artifact identity drifted")
    migration = report.get("migration")
    if not isinstance(migration, dict) or migration != {
        "candidate-id": expected_artifacts["cli"]["candidate-id"],
        "explicit-credentials": True,
        "history-length": 4,
        "operations": ["apply", "verify", "rollback", "reapply", "verify"],
        "uninstall": "clean",
    }:
        raise JourneyError("live artifact migration journey is incomplete")
    if report["test-infrastructure"] != {
        "cargo-confined-to-fixture-processes": True,
        "consumer-cargo-dependency": False,
        "consumer-python-dependency": False,
        "consumer-source-library-fallback": False,
    }:
        raise JourneyError("live artifact consumer dependency boundary drifted")


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    provider = commands.add_parser("provider-free")
    provider.add_argument("cli", type=Path)
    provider.add_argument("runtime", type=Path)
    provider.add_argument("generated", type=Path)
    connected_parser = commands.add_parser("connected")
    connected_parser.add_argument("runtime", type=Path)
    connected_parser.add_argument("generated", type=Path)
    connected_parser.add_argument("--address", default="127.0.0.1:1729")
    connected_parser.add_argument("--http-port", default="8000")
    connected_parser.add_argument("--database", required=True)
    connected_parser.add_argument("--username", default="admin")
    connected_parser.add_argument("--password", default="password")
    connected_parser.add_argument("--remote-port", required=True)
    live = commands.add_parser("live")
    live.add_argument("cli", type=Path)
    live.add_argument("runtime", type=Path)
    live.add_argument("generated", type=Path)
    live.add_argument("--address", default="127.0.0.1:1729")
    live.add_argument("--http-port", default="8000")
    live.add_argument("--database", required=True)
    live.add_argument("--migration-database", required=True)
    live.add_argument("--username", default="admin")
    live.add_argument("--password", default="password")
    live.add_argument("--infra-target", type=Path, required=True)
    live.add_argument("--tls-address", required=True)
    live.add_argument("--tls-http-port", required=True)
    live.add_argument("--tls-root-ca", type=Path, required=True)
    validate = commands.add_parser("validate-report")
    validate.add_argument("report", type=Path)
    validate.add_argument("cli", type=Path)
    validate.add_argument("runtime", type=Path)
    validate.add_argument("generated", type=Path)
    validate_live = commands.add_parser("validate-live")
    validate_live.add_argument("report", type=Path)
    validate_live.add_argument("cli", type=Path)
    validate_live.add_argument("runtime", type=Path)
    validate_live.add_argument("generated", type=Path)
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    arguments = parse_args(argv)
    try:
        if arguments.command == "provider-free":
            result = provider_free(arguments.cli, arguments.runtime, arguments.generated)
            print(packages.canonical_json(result).decode(), end="")
        elif arguments.command == "connected":
            result = connected(
                arguments.runtime,
                arguments.generated,
                address=arguments.address,
                http_port=arguments.http_port,
                database=arguments.database,
                username=arguments.username,
                password=arguments.password,
                remote_port=arguments.remote_port,
            )
            print(packages.canonical_json(result).decode(), end="")
        elif arguments.command == "live":
            result = live_journey(
                arguments.cli,
                arguments.runtime,
                arguments.generated,
                address=arguments.address,
                http_port=arguments.http_port,
                database=arguments.database,
                migration_database=arguments.migration_database,
                username=arguments.username,
                password=arguments.password,
                infra_target=arguments.infra_target,
                tls_address=arguments.tls_address,
                tls_http_port=arguments.tls_http_port,
                tls_root_ca=arguments.tls_root_ca,
            )
            print(packages.canonical_json(result).decode(), end="")
        elif arguments.command in {"validate-report", "validate-live"}:
            value = json.loads(
                arguments.report.read_text(encoding="utf-8"),
                object_pairs_hook=packages.unique_object,
            )
            if (
                not isinstance(value, dict)
                or packages.canonical_json(value) != arguments.report.read_bytes()
            ):
                raise JourneyError("artifact journey report is not canonical JSON")
            if arguments.command == "validate-report":
                validate_report(value, arguments.cli, arguments.runtime, arguments.generated)
                print("validated artifact-only clean-consumer report")
            else:
                validate_live_report(value, arguments.cli, arguments.runtime, arguments.generated)
                print("validated artifact-only live journey report")
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
