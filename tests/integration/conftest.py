"""Pytest fixtures for integration tests."""

import os

import pytest

from tests.utils.typedb_lifecycle import (
    TEST_DB_ADDRESS,
    TEST_DB_HTTP_PORT,
    TEST_DB_NAME,
    start_typedb_container,
    stop_typedb_container,
)
from type_bridge import Credentials, Database, TypeDB, create_driver_options


def _database(database: str) -> Database:
    tls_root_ca = os.getenv("TYPEDB_TLS_ROOT_CA")
    return Database(
        address=TEST_DB_ADDRESS,
        database=database,
        http_port=TEST_DB_HTTP_PORT,
        tls=True if tls_root_ca is not None else None,
        tls_root_ca=tls_root_ca,
    )


@pytest.fixture(scope="session")
def docker_typedb():
    """Start TypeDB Docker container for the test session.

    Yields:
        None (container runs in background)
    """
    if start_typedb_container():
        try:
            yield
        finally:
            stop_typedb_container()
    else:
        yield


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
        # Address passed positionally: the band-8 driver renamed the keyword
        # (address -> addresses); the positional form works on every band.
        driver = TypeDB.driver(
            TEST_DB_ADDRESS,
            credentials=Credentials(username="admin", password="password"),
            driver_options=create_driver_options(is_tls_enabled=False),
        )
        yield driver
        driver.close()
    except Exception as e:
        pytest.skip(f"TypeDB server not available at {TEST_DB_ADDRESS}: {e}")


@pytest.fixture(scope="session")
def test_database(docker_typedb):
    """Create a test database for the session and clean it up after.

    Args:
        docker_typedb: Fixture that ensures Docker container is running

    Yields:
        Database name (str)
    """
    database = _database(TEST_DB_NAME)
    try:
        database.connect()

        if database.database_exists():
            database.delete_database()
        database.create_database()

        yield TEST_DB_NAME

        if database.database_exists():
            database.delete_database()
    except Exception as e:
        pytest.skip(f"TypeDB server not available at {TEST_DB_ADDRESS}: {e}")
    finally:
        database.close()


@pytest.fixture(scope="function")
def db(test_database):
    """Create a Database instance for each test function.

    Args:
        test_database: Test database name fixture

    Yields:
        Database instance
    """
    database = _database(test_database)
    database.connect()
    yield database
    database.close()


@pytest.fixture(scope="function")
def clean_db(docker_typedb, test_database):
    """Provide a clean database for each test by wiping all data.

    This fixture ensures each test starts with an empty database by:
    1. Deleting the existing test database
    2. Recreating it fresh

    Args:
        docker_typedb: Fixture that ensures Docker container is running
        test_database: Test database name

    Yields:
        Database instance with clean state
    """
    database = _database(test_database)
    try:
        database.connect()
        if database.database_exists():
            database.delete_database()
        database.create_database()
        yield database
    finally:
        database.close()
