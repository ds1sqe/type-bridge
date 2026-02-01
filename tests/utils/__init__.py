"""Test utilities for type-bridge integration tests."""

from tests.utils.assertions import (
    assert_entity_count,
    assert_entity_exists,
    assert_relation_count,
    assert_relation_exists,
)
from tests.utils.data_builders import (
    make_email,
    make_isbn,
    make_name,
    unique_suffix,
)
from tests.utils.typedb_lifecycle import (
    CONTAINER_TOOL,
    TEST_DB_ADDRESS,
    TEST_DB_NAME,
    start_typedb_container,
    stop_typedb_container,
    suppress_stderr,
)

__all__ = [
    # Assertions
    "assert_entity_count",
    "assert_entity_exists",
    "assert_relation_count",
    "assert_relation_exists",
    # Data builders
    "make_email",
    "make_isbn",
    "make_name",
    "unique_suffix",
    # TypeDB lifecycle
    "CONTAINER_TOOL",
    "TEST_DB_ADDRESS",
    "TEST_DB_NAME",
    "start_typedb_container",
    "stop_typedb_container",
    "suppress_stderr",
]
