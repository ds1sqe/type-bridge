"""Pytest fixtures for integration tests."""

import os
import subprocess
import time
from contextlib import contextmanager

import pytest
from typedb.driver import DriverOptions

from type_bridge import Credentials, Database, TypeDB


@contextmanager
def suppress_stderr():
    """Suppress stderr at the file descriptor level.

    This is needed to silence the TypeDB driver's Rust logging initialization
    warning which writes directly to fd 2, bypassing Python's sys.stderr.
    """
    # Always use fd 2 directly (actual stderr) since Rust writes there
    stderr_fd = 2
    saved_stderr = os.dup(stderr_fd)
    devnull = os.open(os.devnull, os.O_WRONLY)
    os.dup2(devnull, stderr_fd)
    os.close(devnull)
    try:
        yield
    finally:
        os.dup2(saved_stderr, stderr_fd)
        os.close(saved_stderr)


# Container tool selection (docker|podman or explicit binary)
CONTAINER_TOOL = os.getenv("CONTAINER_TOOL", "docker")

# Test database configuration
TEST_DB_NAME = "type_bridge_test"
# Allow overriding port/address via environment (for local conflicts or Podman/Docker remaps)
TEST_DB_ADDRESS = os.getenv("TYPEDB_ADDRESS", "localhost:1730")


@pytest.fixture(scope="session")
def docker_typedb():
    """Start TypeDB Docker container for the test session.

    Yields:
        None (container runs in background)
    """
    # Build compose commands based on container tool
    compose_base = (
        [CONTAINER_TOOL, "compose"]
        if CONTAINER_TOOL not in ("docker-compose", "podman-compose")
        else [CONTAINER_TOOL]
    )

    # Check if we should use Docker (default: yes, unless USE_DOCKER=false)
    use_docker = os.getenv("USE_DOCKER", "true").lower() != "false"

    if not use_docker:
        # Skip Docker management - assume TypeDB is already running
        yield
        return

    # Get project root directory
    project_root = os.path.dirname(os.path.dirname(os.path.dirname(__file__)))

    # Start Docker container
    try:
        # Stop any existing container
        subprocess.run(
            [*compose_base, "down"],
            cwd=project_root,
            capture_output=True,
        )

        # Start container
        subprocess.run(
            [*compose_base, "up", "-d"],
            cwd=project_root,
            check=True,
            capture_output=True,
        )

        # Wait for TypeDB to be healthy
        max_retries = 30
        for i in range(max_retries):
            result = subprocess.run(
                [CONTAINER_TOOL, "inspect", "--format={{.State.Health.Status}}", "typedb_test"],
                capture_output=True,
                text=True,
            )
            if result.stdout.strip() == "healthy":
                break
            time.sleep(1)
        else:
            raise RuntimeError("TypeDB container failed to become healthy")

        yield

    finally:
        # Stop Docker container
        subprocess.run(
            [*compose_base, "down"],
            cwd=project_root,
            capture_output=True,
        )


@pytest.fixture(scope="session")
def typedb_driver(docker_typedb):
    """Create a TypeDB driver connection for the test session.

    Args:
        docker_typedb: Fixture that ensures Docker container is running

    Yields:
        TypeDB driver instance

    Raises:
        ConnectionError: If TypeDB server is not running
    """
    try:
        with suppress_stderr():
            driver = TypeDB.driver(
                address=TEST_DB_ADDRESS,
                credentials=Credentials(username="admin", password="password"),
                driver_options=DriverOptions(is_tls_enabled=False),
            )
        yield driver
        driver.close()
    except Exception as e:
        pytest.skip(f"TypeDB server not available at {TEST_DB_ADDRESS}: {e}")


@pytest.fixture(scope="session")
def test_database(typedb_driver):
    """Create a test database for the session and clean it up after.

    Args:
        typedb_driver: TypeDB driver fixture

    Yields:
        Database name (str)
    """
    # Create database if it doesn't exist
    if typedb_driver.databases.contains(TEST_DB_NAME):
        typedb_driver.databases.get(TEST_DB_NAME).delete()

    typedb_driver.databases.create(TEST_DB_NAME)

    yield TEST_DB_NAME

    # Cleanup: Delete test database after all tests
    if typedb_driver.databases.contains(TEST_DB_NAME):
        typedb_driver.databases.get(TEST_DB_NAME).delete()


@pytest.fixture(scope="function")
def db(test_database):
    """Create a Database instance for each test function.

    Args:
        test_database: Test database name fixture

    Yields:
        Database instance
    """
    database = Database(address=TEST_DB_ADDRESS, database=test_database)
    database.connect()
    yield database
    database.close()


@pytest.fixture(scope="function")
def clean_db(typedb_driver, test_database):
    """Provide a clean database for each test by wiping all data.

    This fixture ensures each test starts with an empty database by:
    1. Deleting the existing test database
    2. Recreating it fresh

    Args:
        typedb_driver: TypeDB driver fixture
        test_database: Test database name

    Yields:
        Database instance with clean state
    """
    # Delete and recreate database for clean state
    if typedb_driver.databases.contains(test_database):
        typedb_driver.databases.get(test_database).delete()
    typedb_driver.databases.create(test_database)

    database = Database(address=TEST_DB_ADDRESS, database=test_database)
    database.connect()
    yield database
    database.close()


@pytest.fixture(scope="function")
def db_with_schema(clean_db):
    """Provide a database with a basic schema already defined.

    This fixture is useful for tests that need a schema but don't test schema creation.

    Args:
        clean_db: Clean database fixture

    Yields:
        Database instance with basic schema
    """
    from type_bridge import (
        Entity,
        Flag,
        Integer,
        Key,
        Relation,
        Role,
        SchemaManager,
        String,
        TypeFlags,
    )

    # Define basic test schema
    class Name(String):
        pass

    class Age(Integer):
        pass

    class Person(Entity):
        flags = TypeFlags(name="person")
        name: Name = Flag(Key)
        age: Age | None

    class Company(Entity):
        flags = TypeFlags(name="company")
        name: Name = Flag(Key)

    class Position(String):
        pass

    class Employment(Relation):
        flags = TypeFlags(name="employment")
        employee: Role[Person] = Role("employee", Person)
        employer: Role[Company] = Role("employer", Company)
        position: Position

    # Create schema
    schema_manager = SchemaManager(clean_db)
    schema_manager.register(Person, Company, Employment)
    schema_manager.sync_schema(force=True)

    yield clean_db
