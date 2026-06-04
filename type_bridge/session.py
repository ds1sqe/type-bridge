"""Session and transaction management for TypeDB."""

import logging
from typing import Any, overload

from typedb.driver import (
    Credentials,
    Driver,
    TransactionType,
    TypeDB,
)
from typedb.driver import (
    Transaction as TypeDBTransaction,
)

from type_bridge.proxy import ProxyDatabase, ProxyTransaction, ProxyTransactionContext
from type_bridge.typedb_driver import create_driver_options

logger = logging.getLogger(__name__)


def _tx_type_name(tx_type: TransactionType) -> str:
    """Get string name for transaction type (pyright-safe)."""
    names = {
        TransactionType.READ: "READ",
        TransactionType.WRITE: "WRITE",
        TransactionType.SCHEMA: "SCHEMA",
    }
    return names.get(tx_type, "UNKNOWN")


def _tx_type_wire_name(tx_type: TransactionType) -> str:
    """Get Rust/Python backend transaction type name."""
    names = {
        TransactionType.READ: "read",
        TransactionType.WRITE: "write",
        TransactionType.SCHEMA: "schema",
    }
    return names.get(tx_type, "read")


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
            except (AttributeError, TypeError, ValueError) as e:
                logger.debug(f"Failed to extract value via get_value() for {key}: {e}")
        # _Value concept (from aggregations) - use .get() not .as_value()
        if hasattr(concept, "is_value") and concept.is_value():
            try:
                result[clean_key] = {"value": concept.get()}
                continue
            except (AttributeError, TypeError, ValueError) as e:
                logger.debug(f"Failed to extract value via get() for {key}: {e}")
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

    # Try to get column names - if this fails, it's likely an aggregation result
    try:
        column_names = list(item.column_names())
    except (AttributeError, TypeError) as e:
        logger.debug(f"Cannot get column_names, treating as aggregation result: {e}")
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
                except (AttributeError, TypeError) as e:
                    logger.debug(f"Failed to get IID for {var_name}: {e}")

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
                except (AttributeError, TypeError) as e:
                    logger.debug(f"Failed to get type for {var_name}: {e}")

            # Try to get value (for attribute concepts)
            if hasattr(concept, "get_value"):
                try:
                    value = concept.get_value()
                    if value is not None:
                        concept_data["value"] = value
                        has_concept_data = True
                except (AttributeError, TypeError, ValueError) as e:
                    logger.debug(f"Failed to get value for {var_name}: {e}")

            # Try to get value (for _Value concepts from aggregations)
            # Note: _Value.as_value() returns another _Value, use .get() instead
            if hasattr(concept, "is_value") and concept.is_value():
                try:
                    value = concept.get()
                    if value is not None:
                        concept_data["value"] = value
                        has_concept_data = True
                except (AttributeError, TypeError, ValueError) as e:
                    logger.debug(f"Failed to get aggregation value for {var_name}: {e}")

            clean_var_name = var_name.lstrip("$")
            result[clean_var_name] = concept_data

        except (AttributeError, KeyError, TypeError) as e:
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
            driver_options = create_driver_options(is_tls_enabled=is_tls_enabled)
            logger.debug(f"TLS enabled: {is_tls_enabled}")

            # Connect to TypeDB
            try:
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
        del exc_type, exc_val, exc_tb  # unused
        self.close()

    def __del__(self) -> None:
        """Destructor that warns if driver was not properly closed."""
        import warnings

        if self._driver is not None and self._owns_driver:
            warnings.warn(
                f"Database connection to {self.address} was not closed. "
                "Use 'with Database(...) as db:' or call db.close() explicitly.",
                ResourceWarning,
                stacklevel=2,
            )
            # Attempt to close to prevent resource leak
            try:
                self._driver.close()
            except Exception:
                pass  # Ignore errors during cleanup

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


class _RustTransactionView:
    """Small compatibility view for Rust transaction state."""

    def __init__(self, context: "TransactionContext"):
        self._context = context

    @property
    def is_open(self) -> bool:
        return self._context._rust_tx is not None and not self._context._rust_finalized

    @property
    def _tx(self) -> "_RustRawTransactionAdapter":
        return _RustRawTransactionAdapter(self._context)

    def execute(self, query: str) -> list[dict[str, Any]]:
        return self._context.execute(query)

    def commit(self) -> None:
        self._context.commit()

    def rollback(self) -> None:
        self._context.rollback()


class _RustRawQueryPromise:
    def __init__(self, context: "TransactionContext", query: str):
        self._context = context
        self._query = query

    def resolve(self) -> "_RustRawQueryResult":
        return _RustRawQueryResult(self._context.execute(self._query))


class _RustRawQueryResult:
    def __init__(self, rows: list[dict[str, Any]]):
        self._rows = rows

    def __iter__(self):
        return iter(self._rows)

    def __len__(self) -> int:
        return len(self._rows)

    def as_concept_rows(self) -> list[dict[str, Any]]:
        return self._rows

    def as_concept_documents(self) -> list[dict[str, Any]]:
        return self._rows


class _RustRawTransactionAdapter:
    def __init__(self, context: "TransactionContext"):
        self._context = context

    def query(self, query: str) -> _RustRawQueryPromise:
        return _RustRawQueryPromise(self._context, query)

    def commit(self) -> None:
        self._context.commit()

    def rollback(self) -> None:
        self._context.rollback()

    def is_open(self) -> bool:
        return self._context._rust_tx is not None and not self._context._rust_finalized


class TransactionContext:
    """Context manager for sharing a TypeDB transaction across operations."""

    def __init__(self, db: Database, tx_type: TransactionType):
        self.db = db
        self.tx_type = tx_type
        self._tx: Transaction | None = None
        self._rust_tx: Any | None = None
        self._rust_finalized = False

    def __enter__(self) -> "TransactionContext":
        logger.debug(
            f"Opening {_tx_type_name(self.tx_type)} transaction context for database: {self.db.database_name}"
        )
        from type_bridge._backend import RUST_BACKEND, selected_backend

        if selected_backend() == RUST_BACKEND:
            from type_bridge._rust_runtime import rust_database_for

            rust_db = rust_database_for(self.db)
            self._rust_tx = rust_db.transaction(_tx_type_wire_name(self.tx_type))
            self._rust_finalized = False
            logger.debug(f"Rust transaction context opened: {_tx_type_name(self.tx_type)}")
            return self

        self.db.connect()
        raw_tx = self.db.driver.transaction(self.db.database_name, self.tx_type)
        self._tx = Transaction(raw_tx)
        logger.debug(f"Transaction context opened: {_tx_type_name(self.tx_type)}")
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        if self._rust_tx is not None:
            try:
                if not self._rust_finalized:
                    if exc_type is None:
                        if self.tx_type in (TransactionType.WRITE, TransactionType.SCHEMA):
                            logger.debug("Rust transaction context exiting normally, committing")
                            self._rust_tx.commit()
                            self._rust_finalized = True
                    elif self.tx_type in (TransactionType.WRITE, TransactionType.SCHEMA):
                        logger.warning(
                            f"Rust transaction context exiting with exception, rolling back: {exc_type.__name__}"
                        )
                        self._rust_tx.rollback()
                        self._rust_finalized = True
            finally:
                self._rust_tx.close()
                self._rust_tx = None
                logger.debug("Rust transaction context closed")
            return

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
    def transaction(self) -> Transaction | _RustTransactionView:
        """Underlying transaction wrapper."""
        if self._rust_tx is not None:
            return _RustTransactionView(self)
        if self._tx is None:
            raise RuntimeError("TransactionContext not entered")
        return self._tx

    @property
    def database(self) -> Database:
        """Database backing this transaction."""
        return self.db

    def execute(self, query: str) -> list[dict[str, Any]]:
        """Execute a query within the active transaction."""
        if self._rust_tx is not None:
            return self._rust_tx.execute(query)
        return self.transaction.execute(query)

    def commit(self) -> None:
        """Commit the active transaction."""
        if self._rust_tx is not None:
            self._rust_tx.commit()
            self._rust_finalized = True
            return
        self.transaction.commit()

    def rollback(self) -> None:
        """Rollback the active transaction."""
        if self._rust_tx is not None:
            self._rust_tx.rollback()
            self._rust_finalized = True
            return
        self.transaction.rollback()

    def manager(self, model_cls: Any):
        """Get a TypeDBManager bound to this transaction."""
        from type_bridge.models import Entity, Relation

        if issubclass(model_cls, (Entity, Relation)):
            return model_cls.manager(self)

        raise TypeError("manager() expects an Entity or Relation subclass")


# Type alias for unified connection type (includes proxy equivalents)
Connection = (
    Database
    | Transaction
    | TransactionContext
    | ProxyDatabase
    | ProxyTransaction
    | ProxyTransactionContext
)


class ConnectionExecutor:
    """Delegate that handles query execution across connection types.

    This class encapsulates the logic for executing queries against different
    connection types (Database, Transaction, TransactionContext, or proxy equivalents),
    providing a unified interface for CRUD operations.
    """

    def __init__(self, connection: Connection):
        """Initialize the executor with a connection.

        Args:
            connection: Database, Transaction, TransactionContext, or proxy equivalent
        """
        if isinstance(connection, (TransactionContext, ProxyTransactionContext)):
            logger.debug("ConnectionExecutor initialized with TransactionContext")
            self._transaction: Transaction | ProxyTransaction | _RustTransactionView | None = (
                connection.transaction
            )
            self._database: Database | ProxyDatabase | None = None
        elif isinstance(connection, (Transaction, ProxyTransaction)):
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
    def database(self) -> Database | ProxyDatabase | None:
        """Get database if available (for creating new transactions)."""
        return self._database

    @property
    def transaction(self) -> Transaction | ProxyTransaction | _RustTransactionView | None:
        """Get transaction if available."""
        return self._transaction
