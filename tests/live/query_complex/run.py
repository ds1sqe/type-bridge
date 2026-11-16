"""Run one seeded twelve-binding query in all SDKs on configured TypeDB servers."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
import uuid
from pathlib import Path

from fixture import (
    ORIGINAL_SORTED_RAW_QUERY,
    RAW_QUERY,
    SORTED_RAW_QUERY,
    escape_expected,
    escape_query,
    expected_rows,
    seed_queries,
    width_expected,
    width_query,
)
from typedb.driver import (
    Credentials,
    TransactionOptions,
    TransactionType,
    TypeDB,
)

from type_bridge import create_driver_options

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
CORE = ROOT / "type-bridge-core"
NODE = CORE / "crates/node"


def run(
    command: list[str], cwd: Path, env: dict[str, str], log: Path, *, timeout: int | None = None
) -> str:
    try:
        completed = subprocess.run(
            command, cwd=cwd, env=env, text=True, capture_output=True, timeout=timeout
        )
    except subprocess.TimeoutExpired as error:
        log.write_text(str(error.stdout or "") + str(error.stderr or ""))
        raise
    log.write_text(completed.stdout + completed.stderr)
    if completed.returncode:
        raise RuntimeError(f"command failed ({completed.returncode}); see {log}: {command}")
    return completed.stdout


def prepare_sdk(sdk: str, stage: Path, env: dict[str, str], source: Path = HERE) -> list[str]:
    target = Path(env["CARGO_TARGET_DIR"])
    if sdk == "python":
        config = stage / "pyrightconfig.json"
        config.write_text(
            json.dumps(
                {
                    "pythonVersion": "3.12",
                    "typeCheckingMode": "standard",
                    "venvPath": str(ROOT),
                    "venv": ".venv",
                    "extraPaths": [str(stage / "generated/python")],
                }
            )
        )
        run(
            [str(ROOT / ".venv/bin/pyright"), "--project", str(config), str(source / "python.py")],
            ROOT,
            env,
            stage / "python-typecheck.log",
        )
        return [sys.executable, str(source / "python.py")]
    if sdk == "node":
        generated = stage / "generated/typescript"
        packages = generated / "node_modules/@type-bridge"
        packages.mkdir(parents=True)
        (packages / "node").symlink_to(NODE, target_is_directory=True)
        (generated / "node_modules/@types").symlink_to(
            NODE / "node_modules/@types", target_is_directory=True
        )
        shutil.copy2(source / "node.ts", generated / "src/program.ts")
        run(
            [str(NODE / "node_modules/.bin/tsc"), "-p", str(generated / "tsconfig.json")],
            ROOT,
            env,
            stage / "node-compile.log",
        )
        return ["node", str(generated / "dist/program.js")]
    if sdk == "rust":
        consumer = stage / "rust-consumer"
        (consumer / "src").mkdir(parents=True)
        shutil.copy2(source / "rust.rs", consumer / "src/main.rs")
        ir_manifest = ""
        if (source / "ir.rs").exists():
            shutil.copy2(source / "ir.rs", consumer / "src/ir.rs")
            shutil.copy2(source / "schema.yaml", consumer / "src/schema.yaml")
            ir_manifest = '[[bin]]\nname = "type-bridge-complex-ir"\npath = "src/ir.rs"\n'

        shutil.copy2(CORE / "Cargo.lock", consumer / "Cargo.lock")
        (consumer / "Cargo.toml").write_text(f'''[package]
name = "type-bridge-complex-query"
version = "0.0.0"
edition = "2024"
[dependencies]
models = {{ package = "type-bridge-generated-schema", path = "{stage / "generated/rust"}" }}
type-bridge = {{ path = "{CORE / "crates/rust"}", features = ["typedb"] }}
type-bridge-contract = {{ path = "{CORE / "crates/contract"}" }}
type-bridge-schema = {{ path = "{CORE / "crates/schema"}" }}
type-bridge-orm = {{ path = "{CORE / "crates/orm"}", features = ["typedb"] }}
serde_json = "1"
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}
{ir_manifest}[patch.crates-io]
type-bridge = {{ path = "{CORE / "crates/rust"}" }}
[workspace]
''')
        run(
            [
                "cargo",
                "build",
                "--offline",
                "-j",
                "24",
                "--manifest-path",
                str(consumer / "Cargo.toml"),
            ],
            ROOT,
            env,
            stage / "rust-compile.log",
        )
        return [str(target / "debug/type-bridge-complex-query")]
    model_object = stage / "c-models.o"
    if not model_object.exists():
        run(
            [
                env.get("CC", "cc"),
                "-std=c17",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-pedantic-errors",
                "-I",
                str(CORE / "crates/c/include"),
                "-I",
                str(stage / "generated/c/include"),
                "-c",
                str(stage / "generated/c/src/models.c"),
                "-o",
                str(model_object),
            ],
            ROOT,
            env,
            stage / "c-models-compile.log",
        )
    executable = stage / "c-query"
    run(
        [
            env.get("CC", "cc"),
            "-std=c17",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic-errors",
            "-I",
            str(CORE / "crates/c/include"),
            "-I",
            str(stage / "generated/c/include"),
            str(source / "c.c"),
            str(model_object),
            "-L",
            str(target / "debug"),
            "-ltype_bridge_c",
            f"-Wl,-rpath,{target / 'debug'}",
            "-o",
            str(executable),
        ],
        ROOT,
        env,
        stage / "c-compile.log",
    )
    return [str(executable)]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--server", action="append", required=True, help="VERSION,ADDRESS,HTTP_PORT"
    )
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--size", type=int, default=16)
    parser.add_argument("--database-prefix", default="type_bridge_complex_")
    parser.add_argument("--sdk", action="append", choices=["python", "node", "rust", "c"])
    args = parser.parse_args()
    if args.size < 1 or not args.database_prefix.replace("_", "").isalnum():
        parser.error("size must be positive and database prefix must be alphanumeric/underscore")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    env = dict(os.environ)
    env.setdefault("TMPDIR", str(output / "tmp"))
    env["TMPDIR"] = str(Path(env["TMPDIR"]).expanduser().resolve())
    Path(env["TMPDIR"]).mkdir(parents=True, exist_ok=True)
    env.setdefault("CARGO_TARGET_DIR", str(ROOT / "target"))
    env["CARGO_TARGET_DIR"] = str(Path(env["CARGO_TARGET_DIR"]).expanduser().resolve())
    env["CARGO_BUILD_JOBS"] = "24"
    env["TYPE_BRIDGE_COMPLEX_SIZE"] = str(args.size)
    env.update(TYPEDB_USERNAME="admin", TYPEDB_PASSWORD="password")
    sdks = args.sdk or ["python", "node", "rust", "c"]
    run(
        [
            "cargo",
            "build",
            "--manifest-path",
            str(CORE / "Cargo.toml"),
            "-j",
            "24",
            "-p",
            "type-bridge-cli",
            "-p",
            "type-bridge-c",
        ],
        ROOT,
        env,
        output / "native-build.log",
    )
    if "node" in sdks:
        run(["npm", "run", "build"], NODE, env, output / "node-build.log")
    cli = str(Path(env["CARGO_TARGET_DIR"]) / "debug/type-bridge")
    for server in args.server:
        version, address, port = server.split(",")
        stage = output / version
        stage.mkdir()
        for name in ("typebridge.yaml", "schema.yaml", "schema-set.yaml"):
            shutil.copy2(HERE / name, stage / name)
        profile = "typedb-3.12.1/v1" if version.startswith("3.12.") else "typedb-3.13.6/v1"
        manifest = stage / "typebridge.yaml"
        manifest.write_text(manifest.read_text().replace("typedb-3.13.6/v1", profile))
        run([cli, "schema", "generate"], stage, env, stage / "generate.log")
        run([cli, "migration", "make", "--name", "initial"], stage, env, stage / "migration.log")
        env.update(
            TYPEDB_ADDRESS=address,
            TYPEDB_HTTP_PORT=port,
            TYPE_BRIDGE_TRANSACTION_LIVE_VERSION=version,
        )
        env["PYTHONPATH"] = str(stage / "generated/python")
        env["RUST_LOG"] = "type_bridge=trace"
        env["TYPE_BRIDGE_COMPLEX_TRACE"] = str(stage / "python-statements.jsonl")
        commands = {sdk: prepare_sdk(sdk, stage, env) for sdk in sdks}
        name = args.database_prefix + version.replace(".", "_") + "_" + uuid.uuid4().hex[:12]
        env["TYPE_BRIDGE_COMPLEX_DATABASE"] = name
        with TypeDB.driver(
            address, Credentials("admin", "password"), create_driver_options(is_tls_enabled=False)
        ) as driver:
            manifest.write_text(
                manifest.read_text()
                + f"""
environments:
  live:
    database: {name}
    uri: {address}
    http-port: '{port}'
    tls: 'false'
    migrate: 'true'
    credential:
      username: env:TYPEDB_USERNAME
      password: env:TYPEDB_PASSWORD
"""
            )
            try:
                run(
                    [cli, "migration", "apply", "--environment", "live"],
                    stage,
                    env,
                    stage / "schema-apply.log",
                )
                with driver.transaction(name, TransactionType.WRITE) as tx:
                    for query in seed_queries(args.size):
                        tx.query(query).resolve()
                    tx.commit()
                (stage / "seed.typeql").write_text("\n".join(seed_queries(args.size)))
                (stage / "raw-fetch.typeql").write_text(RAW_QUERY)
                (stage / "raw-sorted.typeql").write_text(SORTED_RAW_QUERY)
                (stage / "raw-original.typeql").write_text(ORIGINAL_SORTED_RAW_QUERY)
                oracles = {}
                rows = []
                for oracle in ("original", "sorted", "fetch"):
                    started = time.monotonic()
                    try:
                        result = run(
                            [sys.executable, str(HERE / "raw_probe.py"), "--oracle", oracle],
                            ROOT,
                            env,
                            stage / f"raw-{oracle}.log",
                            timeout=20,
                        )
                        found = json.loads(result)
                        assert found == expected_rows(args.size), (version, oracle, found)
                        oracles[oracle] = {
                            "status": "passed",
                            "seconds": time.monotonic() - started,
                        }
                        if oracle == "fetch":
                            rows = found
                    except subprocess.TimeoutExpired:
                        oracles[oracle] = {
                            "status": "client_timeout",
                            "seconds": time.monotonic() - started,
                        }
                        (stage / "oracles.json").write_text(json.dumps(oracles, indent=2))
                        time.sleep(1)
                        with driver.transaction(
                            name,
                            TransactionType.READ,
                            TransactionOptions(transaction_timeout_millis=3000),
                        ):
                            pass
                        if oracle == "fetch":
                            raise
                    (stage / "oracles.json").write_text(json.dumps(oracles, indent=2))
                expected = dict(
                    rows=rows,
                    page_id=rows[1][0] if len(rows) > 1 else None,
                    groups=[[row[0], 1] for row in rows],
                    first=rows[0] if rows else None,
                    count=len(rows),
                    exists=bool(rows),
                    abstract_count=args.size,
                    tagged_count=args.size // 2,
                )
                (stage / "raw.json").write_text(json.dumps(expected, indent=2) + "\n")
                print(f"{version} raw oracle: {len(rows)} rows", flush=True)
                for sdk, command in commands.items():
                    actual = json.loads(run(command, ROOT, env, stage / f"{sdk}.log", timeout=60))
                    actual["groups"].sort()
                    assert actual == expected, (version, sdk, actual, expected)
                    print(
                        f"{version} {sdk}: {len(rows)} exact rows, first/count/exists/abstract/multivalue passed",
                        flush=True,
                    )
                for width in (2, 4, 6, 8, 10):
                    env["TYPE_BRIDGE_COMPLEX_WIDTH"] = str(width)
                    (stage / f"raw-{width}.typeql").write_text(width_query(width))
                    rows = json.loads(
                        run(
                            [sys.executable, str(HERE / "raw_probe.py"), "--oracle", "fetch"],
                            ROOT,
                            env,
                            stage / f"raw-{width}.log",
                            timeout=20,
                        )
                    )
                    assert rows == width_expected(args.size, width), (version, width, rows)
                    expected.update(
                        page_id=rows[1][0] if len(rows) > 1 else None,
                        groups=[[row[0], 1] for row in rows],
                        rows=rows,
                        first=rows[0] if rows else None,
                        count=len(rows),
                        exists=bool(rows),
                    )
                    for sdk, command in commands.items():
                        actual = json.loads(
                            run(command, ROOT, env, stage / f"{sdk}-{width}.log", timeout=60)
                        )
                        actual["groups"].sort()
                        assert actual == expected, (version, sdk, width, actual, expected)
                        print(f"{version} {sdk}: {width} bindings passed", flush=True)
                for width in (4, 10):
                    for list_membership in (False, True):
                        kind = "list" if list_membership else "optional"
                        text = escape_query(width, list_membership=list_membership)
                        env["TYPE_BRIDGE_COMPLEX_RAW_QUERY"] = text
                        (stage / f"escape-{kind}-{width}.typeql").write_text(text)
                        for sdk, command in commands.items():
                            log = stage / f"escape-{kind}-{sdk}-{width}.log"
                            try:
                                envelope = json.loads(run(command, ROOT, env, log, timeout=60))
                            except RuntimeError:
                                if list_membership and "REP254" in log.read_text():
                                    print(
                                        f"{version} {sdk}: list at {width} bindings skipped: TypeDB REP254 Lists not implemented",
                                        flush=True,
                                    )
                                    continue
                                raise
                            answers = envelope["answers"]
                            found = sorted(
                                [row["identifier"], row["note"], row.get("nickname")]
                                for row in answers
                            )
                            assert found == escape_expected(args.size, width), (
                                version,
                                width,
                                sdk,
                                kind,
                                found,
                            )
                            print(
                                f"{version} {sdk}: raw {kind} at {width} bindings passed",
                                flush=True,
                            )
                if "rust" in sdks:
                    for width in (4, 10):
                        env["TYPE_BRIDGE_COMPLEX_WIDTH"] = str(width)
                        result = run(
                            [str(Path(env["CARGO_TARGET_DIR"]) / "debug/type-bridge-complex-ir")],
                            ROOT,
                            env,
                            stage / f"ir-{width}.log",
                            timeout=60,
                        )
                        (stage / f"ir-{width}.json").write_text(result)
                        values = sorted(json.loads(result))
                        assert values == escape_expected(args.size, width), (version, width, values)
                        print(
                            f"{version} IR optional branch at {width} bindings passed", flush=True
                        )
                env.pop("TYPE_BRIDGE_COMPLEX_RAW_QUERY", None)
                env.pop("TYPE_BRIDGE_COMPLEX_WIDTH", None)
            finally:
                # A terminated client may still have a server-side query in flight.
                # Confirm this database accepts and closes a fresh transaction before deletion.
                if driver.databases.contains(name):
                    time.sleep(1)
                    try:
                        with driver.transaction(
                            name,
                            TransactionType.READ,
                            TransactionOptions(transaction_timeout_millis=3000),
                        ):
                            pass
                    except Exception as error:
                        raise RuntimeError(
                            f"Cleanup probe failed; retained {name} and {name}__tb_journal"
                        ) from error
                for owned in (name, name + "__tb_journal"):
                    if driver.databases.contains(owned):
                        driver.databases.get(owned).delete()
                    assert not driver.databases.contains(owned)
    print(f"Evidence: {output}")


if __name__ == "__main__":
    main()
