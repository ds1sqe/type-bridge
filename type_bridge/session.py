"""Session and transaction management for TypeDB."""

from __future__ import annotations

import logging
import os
import threading
import warnings
from dataclasses import dataclass
from enum import Enum
from typing import TYPE_CHECKING, Any, Literal, cast, overload

import type_bridge.typedb_driver as typedb_driver
import type_bridge.version as version
from type_bridge.typedb_driver import (
    Credentials,
    TransactionType,
    TypeDB,
    create_driver_options,
)

if TYPE_CHECKING:
    from type_bridge_core import QueryLimits
    from type_bridge_core import Transaction as StagedTransaction
    from typedb.driver import Driver

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
    query_limits: QueryLimits | None = None


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
    """Get the native wire name for a Python transaction type."""
    names = {
        TransactionType.READ: "read",
        TransactionType.WRITE: "write",
        TransactionType.SCHEMA: "schema",
    }
    return names.get(tx_type, "read")


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
        query_limits: QueryLimits | None = None,
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
            query_limits: Independent Rust query policy inherited by managers
                and transactions. Omission keeps the native defaults.
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
        self.query_limits = query_limits

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
            query_limits=self.query_limits,
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
        """Drop a failed first-attempt snapshot when no transport owns it."""
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
        from type_bridge._native import core_database_for

        core_database_for(self)
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
            had_native_database = hasattr(self, "_native_database")
            native_database = getattr(self, "_native_database", None)
            snapshot = self._tls_root_ca_snapshot
            snapshot_path = self._tls_root_ca_snapshot_path
            prepared = self._prepared_connection
            transport_committed = self._transport_committed
            if released_driver_present:
                self._driver = None
            if hasattr(self, "_native_database"):
                delattr(self, "_native_database")
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
                if had_native_database:
                    setattr(self, "_native_database", native_database)
                self._tls_root_ca_snapshot = snapshot
                self._tls_root_ca_snapshot_path = snapshot_path
                self._prepared_connection = prepared
                self._transport_committed = transport_committed
                restored_failed_transport = True

            if not restored_failed_transport and native_database is not None:
                try:
                    native_close = getattr(native_database, "close", None)
                    # Older test doubles and third-party compatibility shims
                    # may predate the explicit native-close seam. Real Rust
                    # handles always expose it; V1 stand-ins remain safely
                    # releasable.
                    if callable(native_close):
                        native_close()
                except Exception:
                    logger.warning("Native database cleanup failed; releasing the handle")

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
            if self.query_limits is not None:
                state["query_limits"] = {
                    name: self.query_limits.get(name) for name in self.query_limits.names()
                }
            state.pop("_transport_lock", None)
            return state

    def __setstate__(self, state: dict[str, Any]) -> None:
        limits = state.setdefault("query_limits", None)
        if isinstance(limits, dict):
            from type_bridge_core import QueryLimits

            state["query_limits"] = QueryLimits(**limits)
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
        self.create_database_outcome()

    def create_database_outcome(self) -> Literal["created", "already_exists"]:
        """Create the bound database and return a race-normalized outcome."""
        if self._driver is not None:
            if self.driver.databases.contains(self.database_name):
                logger.debug(f"Database already exists: {self.database_name}")
                return "already_exists"
            logger.debug(f"Creating database: {self.database_name}")
            try:
                self.driver.databases.create(self.database_name)
            except Exception:
                if self.driver.databases.contains(self.database_name):
                    return "already_exists"
                raise
            logger.info(f"Database created: {self.database_name}")
            return "created"

        from type_bridge._native import core_database_for

        outcome = cast(
            Literal["created", "already_exists"],
            core_database_for(self).create_database_outcome(),
        )
        logger.debug(f"Database create outcome for '{self.database_name}': {outcome}")
        return outcome

    def delete_database(self) -> None:
        """Delete the database."""
        self.delete_database_outcome()

    def delete_database_outcome(self) -> Literal["deleted", "already_absent"]:
        """Delete the bound database and return a race-normalized outcome."""
        if self._driver is not None:
            if not self.driver.databases.contains(self.database_name):
                logger.debug(f"Database does not exist, skipping delete: {self.database_name}")
                return "already_absent"
            logger.debug(f"Deleting database: {self.database_name}")
            try:
                self.driver.databases.get(self.database_name).delete()
            except Exception:
                if not self.driver.databases.contains(self.database_name):
                    return "deleted"
                raise
            logger.info(f"Database deleted: {self.database_name}")
            return "deleted"

        from type_bridge._native import core_database_for

        outcome = cast(
            Literal["deleted", "already_absent"],
            core_database_for(self).delete_database_outcome(),
        )
        logger.debug(f"Database delete outcome for '{self.database_name}': {outcome}")
        return outcome

    def database_exists(self) -> bool:
        """Check if database exists."""
        if self._driver is not None:
            exists = self.driver.databases.contains(self.database_name)
        else:
            from type_bridge._native import core_database_for

            exists = core_database_for(self).database_exists()
        logger.debug(f"Database exists check for '{self.database_name}': {exists}")
        return exists

    def inspect_database_pair(
        self,
    ) -> Literal["absent", "standalone_managed", "owned_pair", "owned_journal_orphan"]:
        """Inspect the managed database and its package-owned journal as one pair."""
        from type_bridge._native import core_database_for

        return cast(
            Literal["absent", "standalone_managed", "owned_pair", "owned_journal_orphan"],
            core_database_for(self).inspect_database_pair(),
        )

    def inspect_database_pair_controlled(
        self,
        *,
        timeout_milliseconds: int | None = None,
        cancellation: Any | None = None,
    ) -> Literal["absent", "standalone_managed", "owned_pair", "owned_journal_orphan"]:
        """Inspect the managed database pair under explicit execution controls."""
        from type_bridge._native import core_database_for

        return cast(
            Literal["absent", "standalone_managed", "owned_pair", "owned_journal_orphan"],
            core_database_for(self).inspect_database_pair_controlled(
                timeout_milliseconds=timeout_milliseconds,
                cancellation=cancellation,
            ),
        )

    def plan_database_delete(self) -> Any:
        """Create a single-use, pair-aware managed database deletion plan."""
        from type_bridge._native import core_database_for

        return core_database_for(self).plan_database_delete()

    def plan_database_delete_controlled(
        self,
        *,
        timeout_milliseconds: int | None = None,
        cancellation: Any | None = None,
    ) -> Any:
        """Create a managed deletion plan under explicit execution controls."""
        from type_bridge._native import core_database_for

        return core_database_for(self).plan_database_delete_controlled(
            timeout_milliseconds=timeout_milliseconds,
            cancellation=cancellation,
        )

    @overload
    def transaction(self, transaction_type: Enum) -> TransactionContext: ...

    @overload
    def transaction(self, transaction_type: str = "read") -> TransactionContext: ...

    def transaction(self, transaction_type: Enum | str = "read") -> TransactionContext:
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
            tx_type = cast(TransactionType, transaction_type)

        logger.debug(
            f"Creating {_tx_type_name(tx_type)} transaction for database: {self.database_name}"
        )
        return TransactionContext(self, tx_type)

    def staged_transaction(
        self,
        kind: Literal["read", "write", "schema"] | None = None,
        *,
        commit_on_exit: bool = True,
        timeout_milliseconds: int | None = None,
        collect_phase_timestamps: bool = False,
    ) -> StagedTransaction:
        """Create a staged transaction for typed plans.

        Plans staged with ``tx.stage(plan)`` reach TypeDB only at
        ``tx.commit()``, as one transaction; each ``Pending.result()`` then
        answers ``Rows``, ``Documents``, ``Count``, ``Exists`` or ``Matched``. As a context
        manager it commits on a clean exit (unless ``commit_on_exit=False``),
        discards on an exception, and closes; a commit whose outcome is
        unknown raises, and the caller reads the database. An omitted
        ``kind`` is inferred from the staged plans. ``transaction()`` keeps
        its eager context. The timeout defaults to 60000 ms; zero or negative
        disables it. Phase timestamp collection is off unless requested.
        """
        from type_bridge._native import core_database_for

        return core_database_for(self).staged_transaction(
            kind,
            commit_on_exit=commit_on_exit,
            timeout_milliseconds=timeout_milliseconds,
            collect_phase_timestamps=collect_phase_timestamps,
        )

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

        Returns the version string (e.g. ``"3.12.3"``) when known. ``None``
        means the negotiated connection path produced no authoritative server
        identity; supply ``server_version=`` at construction when strict
        identity validation is required.
        """
        from type_bridge._native import core_database_for

        return core_database_for(self).server_version()

    def check_schema_annotation_support(self, typeql: str) -> None:
        """Version-gate schema DDL that uses ``@doc``/``@meta`` annotations.

        Raises the versioned error when the TypeQL uses schema annotations
        (TypeDB 3.12+) and the detected server version predates 3.12. When
        the server version is unknown, the DDL is sent as-is and the server
        decides.
        """
        from type_bridge._native import core_database_for

        core_database_for(self).check_schema_annotation_support(typeql)

    def supports_given_stage(self) -> bool:
        """Whether ``given`` rows can execute on the active connection.

        This requires both TypeDB 3.12+ syntax support and the retained band-9
        provider. Bulk operations consult this before dispatch and use their
        per-row fallback when it is ``False``.
        """
        from type_bridge._native import core_database_for

        return core_database_for(self).supports_given_stage()

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
        from type_bridge._native import core_database_for

        return core_database_for(self).execute_with_rows(
            query, transaction_type, variables, column_types, rows
        )

    def get_schema(self) -> str:
        """Get the schema definition for this database."""
        logger.debug(f"Fetching schema for database: {self.database_name}")
        if self._driver is not None:
            db = self.driver.databases.get(self.database_name)
            schema = db.schema()
        else:
            from type_bridge._native import schema_text

            schema = schema_text(self)
        logger.debug(f"Schema fetched ({len(schema)} chars)")
        return schema


class _NativeTransactionView:
    """Small view for native transaction state."""

    def __init__(self, context: TransactionContext):
        self._context = context

    @property
    def is_open(self) -> bool:
        return self._context._native_tx is not None and not self._context._native_finalized

    def execute(self, query: str) -> list[dict[str, Any]]:
        return self._context.execute(query)

    def commit(self) -> None:
        self._context.commit()

    def rollback(self) -> None:
        self._context.rollback()


class TransactionContext:
    """Context manager for sharing a TypeDB transaction across operations."""

    def __init__(self, db: Database, tx_type: TransactionType):
        self.db = db
        self.tx_type = tx_type
        self._native_tx: Any | None = None
        self._native_finalized = False

    def __enter__(self) -> TransactionContext:
        logger.debug(
            f"Opening {_tx_type_name(self.tx_type)} transaction context for database: {self.db.database_name}"
        )

        from type_bridge._native import core_database_for

        native_db = core_database_for(self.db)
        self._native_tx = native_db.transaction(_tx_type_wire_name(self.tx_type))
        self._native_finalized = False
        logger.debug(f"Native transaction context opened: {_tx_type_name(self.tx_type)}")
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        if self._native_tx is not None:
            try:
                if not self._native_finalized:
                    if exc_type is None:
                        if self.tx_type in (TransactionType.WRITE, TransactionType.SCHEMA):
                            logger.debug("Native transaction context exiting normally, committing")
                            self._native_tx.commit()
                            self._native_finalized = True
                    elif self.tx_type in (TransactionType.WRITE, TransactionType.SCHEMA):
                        logger.warning(
                            f"Native transaction context exiting with exception, rolling back: {exc_type.__name__}"
                        )
                        self._native_tx.rollback()
                        self._native_finalized = True
            finally:
                self._native_tx.close()
                self._native_tx = None
                logger.debug("Native transaction context closed")
            return

    @property
    def transaction(self) -> _NativeTransactionView:
        """View of the active native transaction."""
        if self._native_tx is None:
            raise RuntimeError("TransactionContext not entered")
        return _NativeTransactionView(self)

    @property
    def database(self) -> Database:
        """Database backing this transaction."""
        return self.db

    def execute(self, query: str) -> list[dict[str, Any]]:
        """Execute a query within the active transaction."""
        if self._native_tx is not None:
            return self._native_tx.execute(query)
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
        Requires a TypeDB 3.12+ connection.
        """
        if self._native_tx is None:
            raise RuntimeError("execute_with_rows requires an open native transaction")
        return self._native_tx.execute_with_rows(query, variables, column_types, rows)

    def commit(self) -> None:
        """Commit the active transaction."""
        if self._native_tx is not None:
            self._native_tx.commit()
            self._native_finalized = True
            return
        self.transaction.commit()

    def rollback(self) -> None:
        """Roll back writes and keep the active transaction usable."""
        if self._native_tx is not None:
            self._native_tx.rollback()
            self._native_finalized = False
            return
        self.transaction.rollback()


# Direct facade connection types. Generated proxy packages own their connection type.
Connection = Database | TransactionContext
