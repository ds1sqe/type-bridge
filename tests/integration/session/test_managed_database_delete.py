"""A generated delete removes the owned pair and permits a fresh migration."""

from __future__ import annotations

import importlib
import os
import subprocess
import sys
import time
from pathlib import Path

import pytest

from type_bridge import Credentials, Database, TypeDB, create_driver_options

pytestmark = [
    pytest.mark.integration,
    pytest.mark.skipif(
        os.getenv("TYPE_BRIDGE_DELETE_PAIR_LIVE") != "1",
        reason="requires an explicitly selected live database-delete lane",
    ),
]


def test_generated_delete_removes_owned_journal_and_allows_fresh_apply(tmp_path: Path) -> None:
    address = os.environ["TYPEDB_ADDRESS"]
    http_port = int(os.environ["TYPEDB_HTTP_PORT"])
    profile = os.environ["TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE"]
    prefix = os.getenv("TYPE_BRIDGE_SCHEMA_MIGRATION_TYPEDB_DATABASE", "type_bridge_delete_pair")
    database_name = f"{prefix}_delete_pair_{time.time_ns()}"
    journal_name = f"{database_name}__tb_journal"
    username = os.getenv("TYPEDB_USERNAME", "admin")
    password = os.getenv("TYPEDB_PASSWORD", "password")
    cli = Path(os.environ["TYPE_BRIDGE_CLI_BIN"]).resolve()
    (tmp_path / "schema").mkdir()
    (tmp_path / "schema/schema.yaml").write_text(
        "format: typebridge.schema-set/v1\nsources: [model.yaml]\n"
    )
    (tmp_path / "schema/model.yaml").write_text(
        "format: typebridge.schema/v2\nentities: {person: {}}\n"
    )
    (tmp_path / "typebridge.yaml").write_text(
        "format: typebridge.workspace/v1\nschema:\n  root: schema/schema.yaml\n"
        "  ownership: exclusive\n  managed-scope: delete-pair\ncompatibility:\n"
        f"  semantic-profile: {profile}\nmigrations:\n  app-label: deletepair\n"
        "bindings:\n  python: {output: delete_pair_generated}\nenvironments:\n"
        f"  live:\n    database: {database_name}\n    uri: {address}\n"
        f"    http-port: '{http_port}'\n    migrate: 'true'\n    credential:\n"
        "      username: env:TYPEDB_USERNAME\n      password: env:TYPEDB_PASSWORD\n"
    )
    environment = {**os.environ, "TYPEDB_USERNAME": username, "TYPEDB_PASSWORD": password}

    def command(*args: str) -> None:
        subprocess.run([str(cli), *args], cwd=tmp_path, env=environment, check=True)

    command("schema", "generate")
    command("migration", "make", "--name", "initial")
    sys.path.insert(0, str(tmp_path))
    generated = importlib.import_module("delete_pair_generated")
    bare = Database(address=address, database=database_name, http_port=http_port)
    owned = generated.connect(
        generated.DirectConnectionPolicy(
            endpoint=address,
            database=database_name,
            username=username,
            password=password,
            http_port=http_port,
        )
    )
    driver = TypeDB.driver(
        address,
        credentials=Credentials(username=username, password=password),
        driver_options=create_driver_options(is_tls_enabled=False),
    )
    try:
        command("migration", "apply", "--environment", "live")
        for connection in [bare, Database(database=database_name, driver=driver)]:
            with pytest.raises((ValueError, RuntimeError), match=journal_name):
                connection.delete_database()
            assert driver.databases.contains(database_name)
            assert driver.databases.contains(journal_name)
        # Opening the retained Python-driver facade must not lose the
        # generated native connection's verified ownership authority.
        _ = owned.driver
        assert owned.delete_database_outcome() == "deleted"
        assert not driver.databases.contains(database_name)
        assert not driver.databases.contains(journal_name)
        assert owned.delete_database_outcome() == "already_absent"
        command("migration", "apply", "--environment", "live")
        command("migration", "verify", "--environment", "live")
        owned.delete_database()
        assert not driver.databases.contains(database_name)
        assert not driver.databases.contains(journal_name)
    finally:
        bare.close()
        owned.close()
        for name in [journal_name, database_name]:
            if driver.databases.contains(name):
                driver.databases.get(name).delete()
        driver.close()
        sys.path.remove(str(tmp_path))
        for name in list(sys.modules):
            if name == "delete_pair_generated" or name.startswith("delete_pair_generated."):
                del sys.modules[name]
