"""Build and run the shop walkthrough against an existing local TypeDB."""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import time
import uuid
from pathlib import Path

from typedb.driver import TransactionOptions, TransactionType

from type_bridge import Database
from type_bridge.session import _resolve_transport_options
from type_bridge.typedb_driver import Credentials, TypeDB, create_driver_options

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
NODE = ROOT / "type-bridge-core/crates/node"


def run(
    command: list[str], *, cwd: Path = ROOT, env: dict[str, str], output: Path | None = None
) -> None:
    if output is None:
        subprocess.run(command, cwd=cwd, env=env, check=True)
    else:
        with output.open("w") as stream, output.with_suffix(".log").open("w") as errors:
            subprocess.run(command, cwd=cwd, env=env, stdout=stream, stderr=errors, check=True)


def stage_workspace(workspace: Path) -> None:
    workspace.mkdir()
    for name in ("typebridge.yaml", "schema.yaml", "documents.yaml"):
        shutil.copy2(HERE / name, workspace / name)
    shutil.copy2(
        ROOT / "tests/contracts/sdk_conformance/writes/shop/schema/fragments/shop.yaml",
        workspace / "shop.yaml",
    )


def probe_database(db: Database, name: str) -> None:
    address, tls, root_ca = _resolve_transport_options(db.address, db.tls, db.tls_root_ca)
    options = create_driver_options(is_tls_enabled=tls, tls_root_ca=root_ca)
    options.request_timeout_millis = 1_000
    options.primary_failover_retries = 0
    with TypeDB.driver(
        address,
        Credentials(
            db.username if db.username is not None else "admin",
            db.password if db.password is not None else "password",
        ),
        options,
    ) as driver:
        with driver.transaction(
            name,
            TransactionType.READ,
            TransactionOptions(
                transaction_timeout_millis=1_000, schema_lock_acquire_timeout_millis=1_000
            ),
        ):
            pass


def cleanup_database(db: Database, name: str, failed_child: BaseException | None) -> None:
    if failed_child is not None:
        print(
            f"Walkthrough child failed: {type(failed_child).__name__}: {failed_child}; database: {name}",
            file=sys.stderr,
            flush=True,
        )
        for attempt in range(3):
            time.sleep(1)
            try:
                # A crashed child may leave server transactions closing asynchronously.
                probe_database(db, name)
            except Exception as error:
                if attempt == 2:
                    raise RuntimeError(
                        f"Leaving walkthrough database {name}: fresh read did not close: {error}"
                    ) from error
            else:
                break
    db.delete_database()
    if db.database_exists():
        raise RuntimeError(f"walkthrough database still exists after deletion: {name}")
    print(f"Deleted walkthrough database: {name}", flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", action="append", choices=["python", "typescript", "rust", "c"])
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--python-models",
        type=Path,
        help="only generate walkthrough_models into this directory with the installed CLI",
    )
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    print(f"Walkthrough output: {output}", flush=True)
    workspace = output / "workspace"
    stage_workspace(workspace)
    if args.python_models is not None:
        # Static checkers resolve program.py's generated import from here.
        run(["type-bridge", "schema", "generate"], cwd=workspace, env=dict(os.environ))
        shutil.copytree(workspace / "generated/python", args.python_models)
        return
    environment = dict(os.environ)
    environment.setdefault("TMPDIR", str(Path.home() / ".cache/type-bridge-walkthrough"))
    Path(environment["TMPDIR"]).mkdir(parents=True, exist_ok=True)
    environment.setdefault("CARGO_TARGET_DIR", str(ROOT / "target"))
    environment.setdefault("CARGO_BUILD_JOBS", "48")
    environment["RUST_LOG"] = "type_bridge=trace"
    target = Path(environment["CARGO_TARGET_DIR"]).resolve()
    run(
        [
            "cargo",
            "build",
            "--locked",
            "--manifest-path",
            str(ROOT / "type-bridge-core/Cargo.toml"),
            "-p",
            "type-bridge-cli",
        ],
        env=environment,
    )
    cli = str(target / "debug/type-bridge")
    run([cli, "migration", "make", "--name", "initial"], cwd=workspace, env=environment)
    run([cli, "schema", "generate"], cwd=workspace, env=environment)
    schema = (workspace / "migrations/0001_initial.typeql").read_text()
    python_path = output / "python"
    python_path.mkdir()
    shutil.copytree(workspace / "generated/python", python_path / "walkthrough_models")
    environment["PYTHONPATH"] = str(python_path)
    transcripts = []
    for sdk in args.sdk or ["python", "typescript", "rust", "c"]:
        if sdk == "python":
            command = [sys.executable, str(HERE / "python/program.py")]
        elif sdk == "typescript":
            run(["npm", "run", "build"], cwd=NODE, env=environment)
            generated = workspace / "generated/typescript"
            packages = generated / "node_modules/@type-bridge"
            packages.mkdir(parents=True)
            (packages / "node").symlink_to(NODE, target_is_directory=True)
            shutil.copy2(HERE / "typescript/program.ts", generated / "src/program.ts")
            run(
                [
                    "node",
                    str(NODE / "node_modules/typescript/bin/tsc"),
                    "-p",
                    str(generated / "tsconfig.json"),
                ],
                env=environment,
            )
            command = ["node", str(generated / "dist/program.js")]
        elif sdk == "rust":
            consumer = workspace / "rust-consumer"
            (consumer / "src").mkdir(parents=True)
            shutil.copy2(HERE / "rust/main.rs", consumer / "src/main.rs")
            shutil.copy2(ROOT / "type-bridge-core/Cargo.lock", consumer / "Cargo.lock")
            (consumer / "Cargo.toml").write_text(f'''[package]
name = "type-bridge-walkthrough"
version = "0.0.0"
edition = "2024"
[dependencies]
models = {{ package = "type-bridge-generated-schema", path = "{workspace / "generated/rust"}" }}
type-bridge = {{ path = "{ROOT / "type-bridge-core/crates/rust"}", features = ["typedb"] }}
serde = "1"
serde_json = "1"
tokio = {{ version = "1", features = ["macros", "rt-multi-thread"] }}
tracing = "0.1"
tracing-subscriber = {{ version = "0.3", features = ["registry"] }}
[patch.crates-io]
type-bridge = {{ path = "{ROOT / "type-bridge-core/crates/rust"}" }}
[workspace]
''')
            run(
                ["cargo", "build", "--offline", "--manifest-path", str(consumer / "Cargo.toml")],
                env=environment,
            )
            command = [str(target / "debug/type-bridge-walkthrough")]
        else:
            run(
                [
                    "cargo",
                    "build",
                    "--locked",
                    "--manifest-path",
                    str(ROOT / "type-bridge-core/Cargo.toml"),
                    "-p",
                    "type-bridge-c",
                    "--lib",
                ],
                env=environment,
            )
            executable = output / "c-walkthrough"
            run(
                [
                    environment.get("CC", "cc"),
                    "-std=c17",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-pedantic-errors",
                    "-I",
                    str(ROOT / "type-bridge-core/crates/c/include"),
                    "-I",
                    str(workspace / "generated/c/include"),
                    str(HERE / "c/program.c"),
                    str(workspace / "generated/c/src/models.c"),
                    "-L",
                    str(target / "debug"),
                    "-ltype_bridge_c",
                    f"-Wl,-rpath,{target / 'debug'}",
                    "-o",
                    str(executable),
                ],
                env=environment,
            )
            command = [str(executable)]
        name = (
            environment.get("TYPE_BRIDGE_WALKTHROUGH_DATABASE_PREFIX", "type_bridge_walkthrough_")
            + sdk
            + "_"
            + uuid.uuid4().hex
        )
        environment["TYPE_BRIDGE_WALKTHROUGH_DATABASE"] = name
        db = Database(
            address=environment["TYPEDB_ADDRESS"],
            database=name,
            username=environment.get("TYPEDB_USERNAME", "admin"),
            password=environment.get("TYPEDB_PASSWORD", "password"),
            http_port=int(environment["TYPEDB_HTTP_PORT"]),
        )
        created = False
        failed_child = None
        try:
            if db.create_database_outcome() != "created":
                raise RuntimeError(f"refusing to reuse existing database {name}")
            created = True
            with db.raw_transaction("schema") as tx:
                for statement in schema.split("\n\n"):
                    if statement.strip():
                        tx.execute_query(statement)
            transcript = output / f"{sdk}.jsonl"
            run(command, env=environment, output=transcript)
            transcripts.append(transcript)
        except BaseException as error:
            failed_child = error
            raise
        finally:
            try:
                if created:
                    cleanup_database(db, name, failed_child)
            finally:
                db.close()
    run(
        [
            sys.executable,
            str(HERE / "compare.py"),
            "--page",
            str(ROOT / "docs/guide/worked-examples.md"),
            *map(str, transcripts),
        ],
        env=environment,
    )


if __name__ == "__main__":
    main()
