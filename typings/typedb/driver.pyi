# ruff: noqa: F403
from typing import Any

from typedb.api.answer.concept_document_iterator import *
from typedb.api.answer.concept_row import *
from typedb.api.answer.concept_row_iterator import *
from typedb.api.answer.query_answer import *
from typedb.api.answer.query_type import *
from typedb.api.concept.concept import *
from typedb.api.concept.instance.attribute import *
from typedb.api.concept.instance.entity import *
from typedb.api.concept.instance.instance import *
from typedb.api.concept.instance.relation import *
from typedb.api.concept.type.attribute_type import *
from typedb.api.concept.type.entity_type import *
from typedb.api.concept.type.relation_type import *
from typedb.api.concept.type.role_type import *
from typedb.api.concept.type.type import *
from typedb.api.concept.value import *
from typedb.api.connection.credentials import *
from typedb.api.connection.database import *
from typedb.api.connection.driver import *
from typedb.api.connection.driver_options import *
from typedb.api.connection.query_options import *
from typedb.api.connection.transaction import *
from typedb.api.connection.transaction_options import *
from typedb.api.user.user import *
from typedb.common.datetime import *
from typedb.common.duration import *
from typedb.common.exception import *

# Explicit exports required by type_bridge
class Credentials:
    def __init__(self, username: str, password: str) -> None: ...

class DriverTlsConfig:
    @staticmethod
    def disabled() -> DriverTlsConfig: ...
    @staticmethod
    def enabled_with_native_root_ca() -> DriverTlsConfig: ...
    @staticmethod
    def enabled_with_root_ca(root_ca_path: str) -> DriverTlsConfig: ...

class DriverOptions:
    def __init__(
        self,
        tls_config: DriverTlsConfig,
        *,
        primary_failover_retries: int | None = ...,
        request_timeout_millis: int | None = ...,
    ) -> None: ...

class Driver:
    databases: Any
    def transaction(self, database: str, tx_type: TransactionType) -> Transaction: ...
    def close(self) -> None: ...

class TransactionType:
    READ: TransactionType
    WRITE: TransactionType
    SCHEMA: TransactionType

class Transaction:
    def query(self, query: str) -> Any: ...
    def commit(self) -> None: ...
    def rollback(self) -> None: ...
    def is_open(self) -> bool: ...
    def close(self) -> None: ...

class TypeDB:
    DEFAULT_ADDRESS: str
    @staticmethod
    def driver(
        addresses: str,
        credentials: Credentials | None = ...,
        driver_options: DriverOptions | None = ...,
    ) -> Driver: ...
