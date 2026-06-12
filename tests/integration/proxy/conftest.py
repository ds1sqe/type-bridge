"""Pytest fixtures for proxy integration tests."""

import os
from collections.abc import Generator
from pathlib import Path

import pytest

from tests.utils.proxy_lifecycle import (
    PROXY_DB_NAME,
    start_proxy_containers,
    stop_proxy_containers,
)
from tests.utils.typedb_lifecycle import (
    PortDiscoveryError,
    compose_project,
    discover_port,
)
from type_bridge.proxy import ProxyDatabase

_REPO_ROOT = Path(__file__).resolve().parents[3]


def _prepare_proxy_database() -> None:
    """Create the proxy test database and a minimal schema for raw query tests."""
    from type_bridge import (
        AttributeFlags,
        Database,
        Entity,
        Flag,
        Key,
        SchemaManager,
        String,
        TypeFlags,
    )

    # When running isolated, the TypeDB port exposed for the proxy stack is
    # engine-assigned.  Discover it the same way proxy_lifecycle does for the
    # proxy port; fall back to the legacy default if not isolated.
    typedb_address = os.getenv("PROXY_TYPEDB_ADDRESS")
    if not typedb_address:
        project = compose_project(_REPO_ROOT)
        try:
            port = discover_port(project, "typedb", 1729)
            typedb_address = f"localhost:{port}"
        except PortDiscoveryError:
            # Conventional pre-discovery default; reached only when the stack
            # was brought up outside this session with a pinned port.
            typedb_address = "localhost:1731"

    class Name(String):
        flags = AttributeFlags(name="name")

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)

    database = Database(address=typedb_address, database=PROXY_DB_NAME)
    database.connect()
    try:
        database.delete_database()
        database.create_database()

        schema_manager = SchemaManager(database)
        schema_manager.register(Person)
        schema_manager.sync_schema(force=True)
    finally:
        database.close()


@pytest.fixture(scope="session")
def docker_proxy() -> Generator[None]:
    """Start TypeDB + proxy Docker containers for the test session."""
    if start_proxy_containers():
        try:
            yield
        finally:
            stop_proxy_containers()
    else:
        yield


@pytest.fixture(scope="session")
def proxy_db(docker_proxy: None) -> Generator[ProxyDatabase]:
    """Create a ProxyDatabase connection for the test session.

    Yields:
        Connected ProxyDatabase instance
    """
    # Import here so the module-level PROXY_ADDRESS reflects any post-start
    # discovery that start_proxy_containers() may have performed.
    from tests.utils import proxy_lifecycle

    proxy_address = os.getenv("PROXY_ADDRESS", proxy_lifecycle.PROXY_ADDRESS)
    try:
        _prepare_proxy_database()
        proxy = ProxyDatabase(proxy_url=proxy_address, database=PROXY_DB_NAME)
        proxy.connect()
        yield proxy
        proxy.close()
    except Exception as e:
        pytest.skip(f"Proxy server not available at {proxy_address}: {e}")
