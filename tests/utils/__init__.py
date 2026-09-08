"""Generated and retained-runtime integration-test utilities."""

from tests.utils.data_builders import (
    make_email,
    make_isbn,
    make_name,
    unique_suffix,
)
from tests.utils.typedb_lifecycle import (
    CONTAINER_TOOL,
    TEST_DB_ADDRESS,
    TEST_DB_HTTP_PORT,
    TEST_DB_NAME,
    start_typedb_container,
    stop_typedb_container,
)

__all__ = [
    # Data builders
    "make_email",
    "make_isbn",
    "make_name",
    "unique_suffix",
    # TypeDB lifecycle
    "CONTAINER_TOOL",
    "TEST_DB_ADDRESS",
    "TEST_DB_HTTP_PORT",
    "TEST_DB_NAME",
    "start_typedb_container",
    "stop_typedb_container",
]
