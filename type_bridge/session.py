"""Session and transaction management for TypeDB."""

from __future__ import annotations

import logging
import os
import threading
import warnings
from dataclasses import dataclass
from typing import TYPE_CHECKING, Any, overload

import type_bridge.typedb_driver as typedb_driver
import type_bridge.version as version
from type_bridge.proxy import ProxyDatabase, ProxyTransaction, ProxyTransactionContext
from type_bridge.typedb_driver import (
    Credentials,
    TransactionType,
    TypeDB,
    create_driver_options,
)

if TYPE_CHECKING:
    from typedb.driver import Driver
    from typedb.driver import Transaction as TypeDBTransaction

logger = logging.getLogger(__name__)


def _snapshot_tls_root_ca(path: str) -> tuple[Any, str]:
    """Capture one Rust-validated, same-handle CA snapshot."""
    import type_bridge_core

    snapshot = type_bridge_core._CustomRootCaSnapshot(path)
    return snapshot, os.fspath(snapshot.path)


def _resolve_transport_options(
    address: str,
    tls: bool | None,
    tls_root_ca: str | os.PathLike[str] | None,
) -> tuple[str, bool, str | None]:
    """Resolve the released Python address inference into explicit TLS inputs."""
    if tls is not None and type(tls) is not bool:
        raise TypeError("tls must be True, False, or None")

    root: str | None = None
    if tls_root_ca is not None:
        try:
            root_value = os.fspath(tls_root_ca)
        except TypeError as error:
            raise TypeError("tls_root_ca must be a string or path-like object") from error
        if not isinstance(root_value, str):
            raise TypeError("tls_root_ca must resolve to a string path")
        root = root_value
        if tls is False:
            raise ValueError("tls_root_ca contradicts explicit tls=False")
        if tls is None:
            raise ValueError("tls_root_ca requires explicit tls=True")
        if not root:
            raise ValueError("tls_root_ca must not be empty")

    # Preserve the released, case-sensitive inference exactly. Explicit false
    # overrides an https:// prefix; mixed/upper-case spellings do not infer TLS.
    tls_enabled = address.startswith("https://") if tls is None else tls
    normalized_scheme = address.lower()
    if normalized_scheme.startswith("https://"):
        # A mixed-case HTTPS spelling did not enable TLS in the released
        # case-sensitive inference. Preserve it until the preparation boundary
        # can reject it; silently stripping it would turn an upstream scheme
        # mismatch into a plaintext connection.
        plain_address = (
            address[len("https://") :]
            if address.startswith("https://") or tls is not None
            else address
        )
    elif normalized_scheme.startswith("http://"):
        plain_address = address[len("http://") :]
    else:
        plain_address = address
    return plain_address, tls_enabled, root


@dataclass(frozen=True, slots=True)
class _ConnectionIdentity:
    address: str
    database: str
    username: str | None
    password: str | None
    http_port: int
    server_version: str | None
    tls_enabled: bool
    tls_argument: bool | None
    configured_tls_root_ca: str | None


@dataclass(frozen=True, slots=True)
class _PreparedConnection:
    identity: _ConnectionIdentity
    tls_root_ca_snapshot: str | None


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
    """Extract concept data from a ConceptRow (V1 SELECT query results).

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
        *,
        http_port: int = typedb_driver.DEFAULT_HTTP_PORT,
        server_version: str | None = None,
        tls: bool | None = None,
        tls_root_ca: str | os.PathLike[str] | None = None,
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
            http_port: TypeDB HTTP API port used by the connect-time version
                gate probe (default 8000).
            server_version: Exact TypeDB server version to use for connect-time
                validation instead of probing the HTTP API. Use this for
                gRPC-only deployments with the HTTP API disabled.
            tls: Explicit TLS policy. ``True`` uses native roots, ``False``
                disables TLS, and omission preserves the released exact
                lowercase ``https://`` address-prefix inference.
            tls_root_ca: PEM root-CA path for an explicitly enabled TLS
                connection. A root path never enables TLS implicitly.
        """
        # Establish destructor-safe ownership state before any validation can
        # raise. Python may invoke ``__del__`` for an object whose ``__init__``
        # exited early, including the fail-closed TLS checks below.
        self._driver: Driver | None = driver
        self._owns_driver: bool = driver is None
        self._tls_root_ca_snapshot: Any | None = None
        self._tls_root_ca_snapshot_path: str | None = None
        self._transport_lock = threading.RLock()
        self._prepared_connection: _PreparedConnection | None = None
        self._transport_committed = False
        self.address = address

        # Reject contradictory or ill-typed policy before any connect path can
        # create a native host. The resolved value is recalculated at connect
        # time so mutation of the released public attributes keeps working.
        _, _, normalized_tls_root_ca = _resolve_transport_options(address, tls, tls_root_ca)
        self.database_name = database
        self.username = username
        self.password = password
        self.http_port = http_port
        self.server_version = server_version
        self.tls = tls
        self.tls_root_ca = normalized_tls_root_ca

    def _resolved_transport_options(self) -> tuple[str, bool, str | None]:
        return _resolve_transport_options(self.address, self.tls, self.tls_root_ca)

    def _current_connection_identity(self) -> _ConnectionIdentity:
        address, tls_enabled, tls_root_ca = self._resolved_transport_options()
        if (
            self.tls is None
            and self.address.lower().startswith("https://")
            and not self.address.startswith("https://")
        ):
            raise ValueError(
                "mixed-case HTTPS scheme does not enable TLS; use lowercase https:// "
                "or pass tls=True explicitly"
            )
        return _ConnectionIdentity(
            address=address,
            database=self.database_name,
            username=self.username,
            password=self.password,
            http_port=self.http_port,
            server_version=self.server_version,
            tls_enabled=tls_enabled,
            tls_argument=(
                tls_enabled if self.tls is not None or self.address.startswith("https://") else None
            ),
            configured_tls_root_ca=tls_root_ca,
        )

    def _prepared_connection_options(self) -> _PreparedConnection:
        """Bind and retain one complete connection/transport identity.

        The Rust ORM and the lazily constructed direct Python driver both call
        this boundary while holding ``_transport_lock``. Once either path first
        consumes the configuration, a later consumer must observe the same
        address, credentials, version gate, TLS policy, and custom-root bytes.
        """
        with self._transport_lock:
            identity = self._current_connection_identity()
            prepared = self._prepared_connection
            if prepared is not None:
                if identity != prepared.identity:
                    raise ValueError(
                        "connection settings changed after transport preparation; "
                        "call close() before reconfiguring this Database"
                    )
                return _PreparedConnection(
                    prepared.identity,
                    self._retained_tls_root_ca_path(),
                )

            snapshot_path: str | None = None
            if identity.configured_tls_root_ca is not None:
                snapshot, snapshot_path = _snapshot_tls_root_ca(identity.configured_tls_root_ca)
                self._tls_root_ca_snapshot = snapshot
                self._tls_root_ca_snapshot_path = snapshot_path
            prepared = _PreparedConnection(identity, snapshot_path)
            self._prepared_connection = prepared
            return prepared

    def _retained_tls_root_ca_path(self) -> str | None:
        """Rewind and return the retained core snapshot's driver path."""
        snapshot = self._tls_root_ca_snapshot
        if snapshot is None:
            return None
        path = os.fspath(snapshot.path)
        self._tls_root_ca_snapshot_path = path
        return path

    def _discard_uncommitted_transport(self) -> None:
        """Drop a failed first-attempt snapshot when no backend owns it."""
        with self._transport_lock:
            if self._transport_committed:
                return
            snapshot = self._tls_root_ca_snapshot
            if snapshot is not None:
                snapshot.cleanup()
            self._tls_root_ca_snapshot = None
            self._tls_root_ca_snapshot_path = None
            self._prepared_connection = None

    def connect(self) -> None:
        """Connect to TypeDB server through the Rust runtime.

        If a driver was injected via __init__, this method does nothing
        (the driver is already connected). Otherwise, initializes the cached
        Rust database handle. Direct access to the external Python TypeDB
        driver remains available through the ``driver`` property.
        """
        if self._driver is not None:
            return

        logger.debug(f"Connecting to TypeDB at {self.address} (database: {self.database_name})")
        from type_bridge._backend import selected_backend
        from type_bridge._rust_runtime import rust_database_for

        selected_backend()
        rust_database_for(self)
        logger.info(f"Connected to TypeDB at {self.address}")

    def _connect_python_driver(self) -> None:
        """Connect using the external Python TypeDB driver for direct driver access."""
        with self._transport_lock:
            if self._driver is not None:
                return

            logger.debug(
                f"Connecting Python TypeDB driver at {self.address} "
                f"(database: {self.database_name})"
            )
            try:
                prepared = self._prepared_connection_options()
                identity = prepared.identity
                tls_root_ca = prepared.tls_root_ca_snapshot
                logger.debug(f"TLS enabled: {identity.tls_enabled}")

                detected_driver = typedb_driver.driver_version()
                typedb_driver._ensure_driver_interpreter_supported(detected_driver)
                if identity.server_version is not None:
                    detected_server = identity.server_version
                elif tls_root_ca is None:
                    detected_server = typedb_driver.server_version(
                        identity.address,
                        http_port=identity.http_port,
                        tls=identity.tls_enabled,
                    )
                else:
                    tls_root_ca = self._retained_tls_root_ca_path()
                    assert tls_root_ca is not None
                    detected_server = typedb_driver.server_version(
                        identity.address,
                        http_port=identity.http_port,
                        tls=identity.tls_enabled,
                        tls_root_ca=tls_root_ca,
                    )
                version.ensure_supported(detected_driver, detected_server)
                version.ensure_runtime_supported(detected_server)
                logger.debug(
                    f"Version gate passed: driver={detected_driver}, server={detected_server}"
                )

                if tls_root_ca is None:
                    driver_options = create_driver_options(is_tls_enabled=identity.tls_enabled)
                else:
                    tls_root_ca = self._retained_tls_root_ca_path()
                    assert tls_root_ca is not None
                    driver_options = create_driver_options(
                        is_tls_enabled=identity.tls_enabled,
                        tls_root_ca=tls_root_ca,
                    )
                credentials = (
                    Credentials(identity.username, identity.password)
                    if identity.username and identity.password
                    else None
                )

                if credentials:
                    logger.debug("Using provided credentials for authentication")
                    created_driver = TypeDB.driver(identity.address, credentials, driver_options)
                else:
                    logger.debug("Using default credentials for local connection")
                    created_driver = TypeDB.driver(
                        identity.address,
                        Credentials("admin", "password"),
                        driver_options,
                    )
                self._driver = created_driver
                self._owns_driver = True
                self._transport_committed = True
                logger.info(f"Connected Python TypeDB driver at {self.address}")
            except BaseException as error:
                self._discard_uncommitted_transport()
                if isinstance(error, Exception):
                    logger.error(
                        f"Failed to connect Python TypeDB driver at {self.address}: {error}"
                    )
                raise

    def close(self) -> None:
        """Close connection to TypeDB server.

        If the driver was injected via __init__, this method only clears the
        reference without closing the driver (the caller retains ownership).
        If the driver was created internally, the owned Python driver closes
        first. If that close fails, the complete
        transport remains attached for a released-style retry. After it
        succeeds, embedded-Rust and snapshot cleanup are attempted. A Rust
        close failure is logged and masked to preserve the released Python
        ``Database.close()`` contract; snapshot failures retain their normal
        error behavior.
        """
        first_error: Exception | None = None
        with self._transport_lock:
            # Detach every owned resource before calling external cleanup code.
            # This makes repeated and re-entrant close calls harmless even when
            # one of the cleanup operations fails.
            driver = self._driver
            owns_driver = self._owns_driver
            # The released close path used `if self._driver:` rather than an
            # identity check. Preserve that observable injection seam: a
            # falsey driver double is neither closed nor detached, and an
            # exception from its truth probe occurs before any cleanup state
            # is mutated.
            released_driver_present = bool(driver) if driver is not None else False
            had_rust_database = hasattr(self, "_rust_backend_database")
            rust_database = getattr(self, "_rust_backend_database", None)
            snapshot = self._tls_root_ca_snapshot
            snapshot_path = self._tls_root_ca_snapshot_path
            prepared = self._prepared_connection
            transport_committed = self._transport_committed
            if released_driver_present:
                self._driver = None
            if hasattr(self, "_rust_backend_database"):
                delattr(self, "_rust_backend_database")
            self._tls_root_ca_snapshot = None
            self._tls_root_ca_snapshot_path = None
            self._prepared_connection = None
            self._transport_committed = False

            python_close_failed = False
            if released_driver_present:
                assert driver is not None
                if owns_driver:
                    logger.debug(f"Closing connection to TypeDB at {self.address}")
                    try:
                        driver.close()
                    except Exception as error:
                        first_error = error
                        python_close_failed = True
                    else:
                        logger.info(f"Disconnected from TypeDB at {self.address}")
                else:
                    logger.debug("Clearing driver reference (external driver, not closing)")

            restored_failed_transport = False
            if python_close_failed and self._driver is None:
                # Released close() left the complete owned transport attached
                # when its first (Python-driver) close raised. Restore that
                # ordering and retry state unless re-entrant work installed a
                # replacement while the external close callback was running.
                self._driver = driver
                self._owns_driver = owns_driver
                if had_rust_database:
                    setattr(self, "_rust_backend_database", rust_database)
                self._tls_root_ca_snapshot = snapshot
                self._tls_root_ca_snapshot_path = snapshot_path
                self._prepared_connection = prepared
                self._transport_committed = transport_committed
                restored_failed_transport = True

            if not restored_failed_transport and rust_database is not None:
                try:
                    rust_close = getattr(rust_database, "close", None)
                    # Older test doubles and third-party compatibility shims
                    # may predate the explicit native-close seam. Real Rust
                    # handles always expose it; V1 stand-ins remain safely
                    # releasable.
                    if callable(rust_close):
                        rust_close()
                except Exception:
                    logger.warning("Embedded Rust backend cleanup failed; releasing the handle")

            if not restored_failed_transport and snapshot is not None:
                try:
                    snapshot.cleanup()
                except Exception as error:
                    if first_error is None:
                        first_error = error

        if first_error is not None:
            raise first_error

    def __getstate__(self) -> dict[str, Any]:
        """Preserve released pickling for pristine connection configs."""
        with self._transport_lock:
            if self._prepared_connection is not None:
                raise TypeError("a Database with prepared transport cannot be pickled")
            state = self.__dict__.copy()
            state.pop("_transport_lock", None)
            return state

    def __setstate__(self, state: dict[str, Any]) -> None:
        state.setdefault("tls", None)
        state.setdefault("tls_root_ca", None)
        state.setdefault("_tls_root_ca_snapshot", None)
        state.setdefault("_tls_root_ca_snapshot_path", None)
        state.setdefault("_prepared_connection", None)
        state.setdefault("_transport_committed", False)
        self.__dict__.update(state)
        self._transport_lock = threading.RLock()

    def __enter__(self) -> Database:
        """Context manager entry."""
        self.connect()
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        """Context manager exit."""
        del exc_type, exc_val, exc_tb  # unused
        self.close()

    def __del__(self) -> None:
        """Destructor that warns if driver was not properly closed."""
        lock = getattr(self, "_transport_lock", None)
        if lock is None:
            return
        with lock:
            driver = getattr(self, "_driver", None)
            if driver is not None and getattr(self, "_owns_driver", False):
                warnings.warn(
                    f"Database connection to {getattr(self, 'address', '<unknown>')} was not "
                    "closed. Use 'with Database(...) as db:' or call db.close() explicitly.",
                    ResourceWarning,
                    stacklevel=2,
                )
                # Attempt to close to prevent resource leak
                try:
                    driver.close()
                except Exception:
                    pass  # Ignore errors during cleanup
            snapshot = getattr(self, "_tls_root_ca_snapshot", None)
            if snapshot is not None:
                try:
                    snapshot.cleanup()
                except Exception:
                    pass

    @property
    def driver(self) -> Driver:
        """Get the TypeDB driver, connecting if necessary."""
        with self._transport_lock:
            if self._driver is None:
                self._connect_python_driver()
            assert self._driver is not None, "Driver should be initialized after connect()"
            return self._driver

    def create_database(self) -> None:
        """Create the database if it doesn't exist."""
        if self._driver is not None:
            if not self.driver.databases.contains(self.database_name):
                logger.debug(f"Creating database: {self.database_name}")
                self.driver.databases.create(self.database_name)
                logger.info(f"Database created: {self.database_name}")
            else:
                logger.debug(f"Database already exists: {self.database_name}")
            return

        from type_bridge._rust_runtime import rust_database_for

        rust_db = rust_database_for(self)
        if not rust_db.database_exists():
            logger.debug(f"Creating database: {self.database_name}")
            rust_db.create_database()
            logger.info(f"Database created: {self.database_name}")
        else:
            logger.debug(f"Database already exists: {self.database_name}")

    def delete_database(self) -> None:
        """Delete the database."""
        if self._driver is not None:
            if self.driver.databases.contains(self.database_name):
                logger.debug(f"Deleting database: {self.database_name}")
                self.driver.databases.get(self.database_name).delete()
                logger.info(f"Database deleted: {self.database_name}")
            else:
                logger.debug(f"Database does not exist, skipping delete: {self.database_name}")
            return

        from type_bridge._rust_runtime import rust_database_for

        rust_db = rust_database_for(self)
        if rust_db.database_exists():
            logger.debug(f"Deleting database: {self.database_name}")
            rust_db.delete_database()
            logger.info(f"Database deleted: {self.database_name}")
        else:
            logger.debug(f"Database does not exist, skipping delete: {self.database_name}")

    def database_exists(self) -> bool:
        """Check if database exists."""
        if self._driver is not None:
            exists = self.driver.databases.contains(self.database_name)
        else:
            from type_bridge._rust_runtime import rust_database_for

            exists = rust_database_for(self).database_exists()
        logger.debug(f"Database exists check for '{self.database_name}': {exists}")
        return exists

    @overload
    def transaction(self, transaction_type: TransactionType) -> TransactionContext: ...

    @overload
    def transaction(self, transaction_type: str = "read") -> TransactionContext: ...

    def transaction(self, transaction_type: TransactionType | str = "read") -> TransactionContext:
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
        if transaction_type in ("schema", TransactionType.SCHEMA):
            self.check_schema_annotation_support(query)
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

    def detected_server_version(self) -> str | None:
        """The server version detected by the connect-time version gate.

        Returns the version string (e.g. ``"3.12.1"``) when known. ``None``
        means the negotiated connection path produced no authoritative server
        identity; supply ``server_version=`` at construction when strict
        identity validation is required.
        """
        from type_bridge._rust_runtime import rust_database_for

        return rust_database_for(self).server_version()

    def check_schema_annotation_support(self, typeql: str) -> None:
        """Version-gate schema DDL that uses ``@doc``/``@meta`` annotations.

        Raises the versioned error when the TypeQL uses schema annotations
        (TypeDB 3.12+) and the detected server version predates 3.12. When
        the server version is unknown, the DDL is sent as-is and the server
        decides.
        """
        from type_bridge._rust_runtime import rust_database_for

        rust_database_for(self).check_schema_annotation_support(typeql)

    def supports_given_stage(self) -> bool:
        """Whether ``given`` rows can execute on the active connection.

        This requires both TypeDB 3.12+ syntax support and a negotiated band-9
        provider. It remains ``False`` when the server version is unknown or
        when a 3.12 server stays on the safe band-8 discovery connection after
        a band-9 upgrade failure. Bulk operations consult this before dispatch
        and use their per-row fallback when it is ``False``.
        """
        from type_bridge._rust_runtime import rust_database_for

        return rust_database_for(self).supports_given_stage()

    def execute_with_rows(
        self,
        query: str,
        transaction_type: str,
        variables: list[str],
        column_types: list[str],
        rows: list[list[Any]],
    ) -> list[dict[str, Any]]:
        """Execute a ``given``-stage TypeQL query over input rows.

        One compiled pipeline runs over every input row; the rows travel
        through the driver API instead of being interpolated into the query
        string, so user-supplied values never touch TypeQL text. Requires a
        TypeDB 3.12+ server; on older servers this raises the versioned
        error from the feature gate.

        Args:
            query: TypeQL starting with a ``given`` stage, e.g.
                ``given $n: string; insert $p isa person, has name == $n;``
            transaction_type: "read", "write", or "schema"
            variables: given variable names without the ``$`` sigil,
                in column order
            column_types: TypeQL value type names aligned with ``variables``
                ("string", "integer", "double", "boolean", "date",
                "datetime", "datetime-tz")
            rows: input rows, each a list of primitives in column order
                (temporal values as ISO-8601 strings)

        Returns:
            List of result dictionaries (one per pipeline output row).
        """
        from type_bridge._rust_runtime import rust_database_for

        return rust_database_for(self).execute_with_rows(
            query, transaction_type, variables, column_types, rows
        )

    def get_schema(self) -> str:
        """Get the schema definition for this database."""
        logger.debug(f"Fetching schema for database: {self.database_name}")
        if self._driver is not None:
            db = self.driver.databases.get(self.database_name)
            schema = db.schema()
        else:
            from type_bridge._rust_runtime import schema_text

            schema = schema_text(self)
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

    def __init__(self, context: TransactionContext):
        self._context = context

    @property
    def is_open(self) -> bool:
        return self._context._rust_tx is not None and not self._context._rust_finalized

    @property
    def _tx(self) -> _RustRawTransactionAdapter:
        return _RustRawTransactionAdapter(self._context)

    def execute(self, query: str) -> list[dict[str, Any]]:
        return self._context.execute(query)

    def commit(self) -> None:
        self._context.commit()

    def rollback(self) -> None:
        self._context.rollback()


class _RustRawQueryPromise:
    def __init__(self, context: TransactionContext, query: str):
        self._context = context
        self._query = query

    def resolve(self) -> _RustRawQueryResult:
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
    def __init__(self, context: TransactionContext):
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

    def __enter__(self) -> TransactionContext:
        logger.debug(
            f"Opening {_tx_type_name(self.tx_type)} transaction context for database: {self.db.database_name}"
        )
        from type_bridge._backend import selected_backend

        selected_backend()
        from type_bridge._rust_runtime import rust_database_for

        rust_db = rust_database_for(self.db)
        self._rust_tx = rust_db.transaction(_tx_type_wire_name(self.tx_type))
        self._rust_finalized = False
        logger.debug(f"Rust transaction context opened: {_tx_type_name(self.tx_type)}")
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

    def execute_with_rows(
        self,
        query: str,
        variables: list[str],
        column_types: list[str],
        rows: list[list[Any]],
    ) -> list[dict[str, Any]]:
        """Execute a ``given``-stage query with input rows in this transaction.

        See :meth:`Database.execute_with_rows` for the argument contract.
        Requires the Rust backend on a TypeDB 3.12+ connection.
        """
        if self._rust_tx is None:
            raise RuntimeError("execute_with_rows requires an open Rust-backend transaction")
        return self._rust_tx.execute_with_rows(query, variables, column_types, rows)

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
