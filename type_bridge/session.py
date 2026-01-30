"""Session and transaction management for TypeDB."""

import logging
import os
from contextlib import contextmanager
from typing import Any, overload

from typedb.driver import (
    Credentials,
    Driver,
    DriverOptions,
    TransactionType,
    TypeDB,
)
from typedb.driver import (
    Transaction as TypeDBTransaction,
)

logger = logging.getLogger(__name__)


@contextmanager
def _suppress_stderr():
    """Suppress stderr at the file descriptor level.

    This silences the TypeDB driver's Rust logging initialization warning
    which writes directly to fd 2, bypassing Python's sys.stderr.

    Note: Always use fd 2 directly since Rust code writes to the actual
    stderr file descriptor, not Python's sys.stderr wrapper.
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


def _tx_type_name(tx_type: TransactionType) -> str:
    """Get string name for transaction type (pyright-safe)."""
    names = {
        TransactionType.READ: "READ",
        TransactionType.WRITE: "WRITE",
        TransactionType.SCHEMA: "SCHEMA",
    }
    return names.get(tx_type, "UNKNOWN")


def _extract_values_from_dict(raw_dict: dict[str, Any]) -> dict[str, Any]:
    """Extract actual values from concept objects in a dictionary.

    Args:
        raw_dict: Dictionary from as_dict() with potential concept objects

    Returns:
        Dictionary with concept objects replaced by their values
    """
    result: dict[str, Any] = {}
    for key, concept in raw_dict.items():
        clean_key = key.lstrip("$")
        # Try to extract value from different concept types
        if hasattr(concept, "get_value"):
            # Attribute concept
            try:
                result[clean_key] = {"value": concept.get_value()}
                continue
            except Exception:
                pass
        # _Value concept (from aggregations) - use .get() not .as_value()
        if hasattr(concept, "is_value") and concept.is_value():
            try:
                result[clean_key] = {"value": concept.get()}
                continue
            except Exception:
                pass
        # Fallback: keep as-is (may be a nested structure or primitive)
        result[clean_key] = concept
    return result


def _extract_concept_row(item: Any) -> dict[str, Any]:
    """Extract concept data from a ConceptRow (legacy SELECT query results).

    Note: With TypeQL 3.8.0+, FETCH queries include iid() and label() directly,
    so this function is primarily used for edge cases and backward compatibility.

    Args:
        item: A ConceptRow object from TypeDB driver

    Returns:
        Dictionary with variable names as keys, containing concept data,
        or {"result": str(item)} for aggregation/reduce query results
    """
    result: dict[str, Any] = {}
    has_concept_data = False

    # Try to get column names
    try:
        column_names = list(item.column_names())
    except Exception:
        return {"result": str(item)}

    for var_name in column_names:
        try:
            concept = item.get(var_name)
            concept_data: dict[str, Any] = {}

            # Try to get IID via driver method
            if hasattr(concept, "get_iid"):
                try:
                    iid = concept.get_iid()
                    if iid is not None:
                        concept_data["_iid"] = str(iid)
                        has_concept_data = True
                except Exception:
                    pass

            # Try to get type label via driver method
            if hasattr(concept, "get_type"):
                try:
                    type_obj = concept.get_type()
                    if hasattr(type_obj, "get_label"):
                        label = type_obj.get_label()
                        if isinstance(label, str):
                            concept_data["_type"] = label
                        elif hasattr(label, "name"):
                            concept_data["_type"] = label.name
                        has_concept_data = True
                except Exception:
                    pass

            # Try to get value (for attribute concepts)
            if hasattr(concept, "get_value"):
                try:
                    value = concept.get_value()
                    if value is not None:
                        concept_data["value"] = value
                        has_concept_data = True
                except Exception:
                    pass

            # Try to get value (for _Value concepts from aggregations)
            # Note: _Value.as_value() returns another _Value, use .get() instead
            if hasattr(concept, "is_value") and concept.is_value():
                try:
                    value = concept.get()
                    if value is not None:
                        concept_data["value"] = value
                        has_concept_data = True
                except Exception:
                    pass

            clean_var_name = var_name.lstrip("$")
            result[clean_var_name] = concept_data

        except Exception as e:
            logger.debug(f"Error extracting concept for {var_name}: {e}")
            continue

    # If no concept data was found, fall back to string format
    if not has_concept_data:
        return {"result": str(item)}

    return result


class Database:
    """Main database connection and session manager."""

    def __init__(
        self,
        address: str = "localhost:1729",
        database: str = "typedb",
        username: str | None = None,
        password: str | None = None,
        driver: Driver | None = None,
    ):
        """Initialize database connection.

        Args:
            address: TypeDB server address
            database: Database name
            username: Optional username for authentication
            password: Optional password for authentication
            driver: Optional pre-existing Driver instance to use. If provided,
                the Database will use this driver instead of creating a new one.
                The caller retains ownership and is responsible for closing it.
        """
        self.address = address
        self.database_name = database
        self.username = username
        self.password = password
        self._driver: Driver | None = driver
        self._owns_driver: bool = driver is None  # Track ownership

    def connect(self) -> None:
        """Connect to TypeDB server.

        If a driver was injected via __init__, this method does nothing
        (the driver is already connected). Otherwise, creates a new driver.
        """
        if self._driver is None:
            logger.debug(f"Connecting to TypeDB at {self.address} (database: {self.database_name})")
            # Create credentials if username/password provided
            credentials = (
                Credentials(self.username, self.password)
                if self.username and self.password
                else None
            )

            # Create driver options
            # Disable TLS for local connections (non-HTTPS addresses)
            is_tls_enabled = self.address.startswith("https://")
            driver_options = DriverOptions(is_tls_enabled=is_tls_enabled)
            logger.debug(f"TLS enabled: {is_tls_enabled}")

            # Connect to TypeDB (suppress Rust logging warning)
            try:
                with _suppress_stderr():
                    if credentials:
                        logger.debug("Using provided credentials for authentication")
                        self._driver = TypeDB.driver(self.address, credentials, driver_options)
                    else:
                        # For local TypeDB Core without authentication
                        logger.debug("Using default credentials for local connection")
                        self._driver = TypeDB.driver(
                            self.address, Credentials("admin", "password"), driver_options
                        )
                self._owns_driver = True
                logger.info(f"Connected to TypeDB at {self.address}")
            except Exception as e:
                logger.error(f"Failed to connect to TypeDB at {self.address}: {e}")
                raise

    def close(self) -> None:
        """Close connection to TypeDB server.

        If the driver was injected via __init__, this method only clears the
        reference without closing the driver (the caller retains ownership).
        If the driver was created internally, it will be closed.
        """
        if self._driver:
            if self._owns_driver:
                logger.debug(f"Closing connection to TypeDB at {self.address}")
                self._driver.close()
                logger.info(f"Disconnected from TypeDB at {self.address}")
            else:
                logger.debug("Clearing driver reference (external driver, not closing)")
            self._driver = None

    def __enter__(self) -> "Database":
        """Context manager entry."""
        self.connect()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        """Context manager exit."""
        self.close()

    @property
    def driver(self) -> Driver:
        """Get the TypeDB driver, connecting if necessary."""
        if self._driver is None:
            self.connect()
        assert self._driver is not None, "Driver should be initialized after connect()"
        return self._driver

    def create_database(self) -> None:
        """Create the database if it doesn't exist."""
        if not self.driver.databases.contains(self.database_name):
            logger.debug(f"Creating database: {self.database_name}")
            self.driver.databases.create(self.database_name)
            logger.info(f"Database created: {self.database_name}")
        else:
            logger.debug(f"Database already exists: {self.database_name}")

    def delete_database(self) -> None:
        """Delete the database."""
        if self.driver.databases.contains(self.database_name):
            logger.debug(f"Deleting database: {self.database_name}")
            self.driver.databases.get(self.database_name).delete()
            logger.info(f"Database deleted: {self.database_name}")
        else:
            logger.debug(f"Database does not exist, skipping delete: {self.database_name}")

    def database_exists(self) -> bool:
        """Check if database exists."""
        exists = self.driver.databases.contains(self.database_name)
        logger.debug(f"Database exists check for '{self.database_name}': {exists}")
        return exists

    @overload
    def transaction(self, transaction_type: TransactionType) -> "TransactionContext": ...

    @overload
    def transaction(self, transaction_type: str = "read") -> "TransactionContext": ...

    def transaction(self, transaction_type: TransactionType | str = "read") -> "TransactionContext":
        """Create a transaction context.

        Args:
            transaction_type: TransactionType or string ("read", "write", "schema")

        Returns:
            TransactionContext for use as a context manager
        """
        tx_type_map: dict[str, TransactionType] = {
            "read": TransactionType.READ,
            "write": TransactionType.WRITE,
            "schema": TransactionType.SCHEMA,
        }

        if isinstance(transaction_type, str):
            tx_type = tx_type_map.get(transaction_type, TransactionType.READ)
        else:
            tx_type = transaction_type

        logger.debug(
            f"Creating {_tx_type_name(tx_type)} transaction for database: {self.database_name}"
        )
        return TransactionContext(self, tx_type)

    def execute_query(self, query: str, transaction_type: str = "read") -> list[dict[str, Any]]:
        """Execute a query and return results.

        Args:
            query: TypeQL query string
            transaction_type: Type of transaction ("read", "write", or "schema")

        Returns:
            List of result dictionaries
        """
        logger.debug(f"Executing query (type={transaction_type}, {len(query)} chars)")
        logger.debug(f"Query: {query}")
        with self.transaction(transaction_type) as tx:
            results = tx.execute(query)
            if isinstance(transaction_type, str):
                needs_commit = transaction_type in ("write", "schema")
            else:
                needs_commit = transaction_type in (TransactionType.WRITE, TransactionType.SCHEMA)
            if needs_commit:
                tx.commit()
            logger.debug(f"Query returned {len(results)} results")
            return results

    def get_schema(self) -> str:
        """Get the schema definition for this database."""
        logger.debug(f"Fetching schema for database: {self.database_name}")
        db = self.driver.databases.get(self.database_name)
        schema = db.schema()
        logger.debug(f"Schema fetched ({len(schema)} chars)")
        return schema


class Transaction:
    """Wrapper around TypeDB transaction."""

    def __init__(self, tx: TypeDBTransaction):
        """Initialize transaction wrapper.

        Args:
            tx: TypeDB transaction
        """
        self._tx = tx

    def execute(self, query: str) -> list[dict[str, Any]]:
        """Execute a query.

        Args:
            query: TypeQL query string

        Returns:
            List of result dictionaries
        """
        logger.debug(f"Transaction.execute: query ({len(query)} chars)")
        logger.debug(f"Query: {query}")
        # Execute query - returns a Promise[QueryAnswer]
        promise = self._tx.query(query)
        answer = promise.resolve()

        # Process based on answer type
        results = []

        # Check if the answer has an iterator (for fetch/get queries)
        if hasattr(answer, "__iter__"):
            for item in answer:
                if hasattr(item, "as_dict"):
                    # ConceptRow with as_dict method - extract values from concepts
                    raw_dict = dict(item.as_dict())
                    results.append(_extract_values_from_dict(raw_dict))
                elif hasattr(item, "as_json"):
                    # Document with as_json method
                    results.append(item.as_json())
                elif hasattr(item, "column_names") and hasattr(item, "get"):
                    # ConceptRow - extract IID and concept info
                    result = _extract_concept_row(item)
                    results.append(result)
                else:
                    # Try to convert to dict
                    results.append(
                        dict(item) if hasattr(item, "__iter__") else {"result": str(item)}
                    )

        logger.debug(f"Query executed, {len(results)} results returned")
        return results

    def commit(self) -> None:
        """Commit the transaction."""
        logger.debug("Committing transaction")
        self._tx.commit()
        logger.info("Transaction committed")

    def rollback(self) -> None:
        """Rollback the transaction."""
        logger.debug("Rolling back transaction")
        self._tx.rollback()
        logger.info("Transaction rolled back")

    @property
    def is_open(self) -> bool:
        """Check if transaction is open."""
        return self._tx.is_open()

    def close(self) -> None:
        """Close the transaction if open."""
        if self._tx.is_open():
            logger.debug("Closing transaction")
            self._tx.close()


class TransactionContext:
    """Context manager for sharing a TypeDB transaction across operations."""

    def __init__(self, db: Database, tx_type: TransactionType):
        self.db = db
        self.tx_type = tx_type
        self._tx: Transaction | None = None

    def __enter__(self) -> "TransactionContext":
        logger.debug(
            f"Opening {_tx_type_name(self.tx_type)} transaction context for database: {self.db.database_name}"
        )
        self.db.connect()
        raw_tx = self.db.driver.transaction(self.db.database_name, self.tx_type)
        self._tx = Transaction(raw_tx)
        logger.debug(f"Transaction context opened: {_tx_type_name(self.tx_type)}")
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        if self._tx is None:
            return

        if self._tx.is_open:
            if exc_type is None:
                if self.tx_type in (TransactionType.WRITE, TransactionType.SCHEMA):
                    logger.debug("Transaction context exiting normally, committing")
                    self._tx.commit()
            else:
                # Only rollback WRITE/SCHEMA transactions - READ can't be rolled back
                if self.tx_type in (TransactionType.WRITE, TransactionType.SCHEMA):
                    logger.warning(
                        f"Transaction context exiting with exception, rolling back: {exc_type.__name__}"
                    )
                    self._tx.rollback()

        self._tx.close()
        logger.debug("Transaction context closed")

    @property
    def transaction(self) -> Transaction:
        """Underlying transaction wrapper."""
        if self._tx is None:
            raise RuntimeError("TransactionContext not entered")
        return self._tx

    @property
    def database(self) -> Database:
        """Database backing this transaction."""
        return self.db

    def execute(self, query: str) -> list[dict[str, Any]]:
        """Execute a query within the active transaction."""
        return self.transaction.execute(query)

    def commit(self) -> None:
        """Commit the active transaction."""
        self.transaction.commit()

    def rollback(self) -> None:
        """Rollback the active transaction."""
        self.transaction.rollback()

    def manager(self, model_cls: Any):
        """Get an Entity/Relation manager bound to this transaction."""
        from type_bridge.crud import EntityManager, RelationManager
        from type_bridge.models import Entity, Relation

        if issubclass(model_cls, Entity):
            return EntityManager(self.transaction, model_cls)
        if issubclass(model_cls, Relation):
            return RelationManager(self.transaction, model_cls)

        raise TypeError("manager() expects an Entity or Relation subclass")


# Type alias for unified connection type
Connection = Database | Transaction | TransactionContext


class ConnectionExecutor:
    """Delegate that handles query execution across connection types.

    This class encapsulates the logic for executing queries against different
    connection types (Database, Transaction, or TransactionContext), providing
    a unified interface for CRUD operations.
    """

    def __init__(self, connection: Connection):
        """Initialize the executor with a connection.

        Args:
            connection: Database, Transaction, or TransactionContext
        """
        if isinstance(connection, TransactionContext):
            logger.debug("ConnectionExecutor initialized with TransactionContext")
            self._transaction: Transaction | None = connection.transaction
            self._database: Database | None = None
        elif isinstance(connection, Transaction):
            logger.debug("ConnectionExecutor initialized with Transaction")
            self._transaction = connection
            self._database = None
        else:
            logger.debug("ConnectionExecutor initialized with Database")
            self._transaction = None
            self._database = connection

    def execute(self, query: str, tx_type: TransactionType) -> list[dict[str, Any]]:
        """Execute query, using existing transaction or creating a new one.

        Args:
            query: TypeQL query string
            tx_type: Transaction type (used only when creating new transaction)

        Returns:
            List of result dictionaries
        """
        if self._transaction:
            logger.debug("ConnectionExecutor: using existing transaction")
            return self._transaction.execute(query)
        assert self._database is not None
        logger.debug(f"ConnectionExecutor: creating new {_tx_type_name(tx_type)} transaction")
        with self._database.transaction(tx_type) as tx:
            return tx.execute(query)

    @property
    def has_transaction(self) -> bool:
        """Check if using an existing transaction."""
        return self._transaction is not None

    @property
    def database(self) -> Database | None:
        """Get database if available (for creating new transactions)."""
        return self._database

    @property
    def transaction(self) -> Transaction | None:
        """Get transaction if available."""
        return self._transaction
