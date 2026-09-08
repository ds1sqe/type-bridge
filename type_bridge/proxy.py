"""Proxy database adapter for type-bridge server.

Routes queries through a type-bridge proxy server instead of connecting
directly to TypeDB. Drop-in replacement for Database.

Usage:
    proxy = ProxyDatabase("http://localhost:8080", database="mydb")
    proxy.connect()

    manager = Person.manager(proxy)
    manager.insert(alice)
    results = manager.all()

    proxy.close()
"""

from __future__ import annotations

import json
import logging
import urllib.error
import urllib.request
from typing import Any

logger = logging.getLogger(__name__)


class ProxyError(Exception):
    """Error returned by the proxy server."""

    def __init__(self, message: str, code: str = "UNKNOWN", details: Any = None):
        super().__init__(message)
        self.code = code
        self.details = details


class ProxyTransaction:
    """Transaction-like object that executes queries via the proxy server.

    For MVP, each query is an independent HTTP request (stateless).
    """

    def __init__(self, proxy: ProxyDatabase, tx_type: str):
        self._proxy = proxy
        self._tx_type = tx_type
        self._is_open = True

    def execute(self, query: str) -> list[dict[str, Any]]:
        """Execute a query via the proxy server."""
        if not self._is_open:
            raise RuntimeError("ProxyTransaction is closed")
        return self._proxy._send_raw_query(query, self._tx_type)

    def commit(self) -> None:
        """No-op for stateless MVP — each query is auto-committed by the server."""
        logger.debug("ProxyTransaction.commit() (stateless, no-op)")
        self._is_open = False

    def rollback(self) -> None:
        """No-op for stateless MVP."""
        logger.debug("ProxyTransaction.rollback() (stateless, no-op)")
        self._is_open = False

    @property
    def is_open(self) -> bool:
        """Check if this proxy transaction is still open."""
        return self._is_open

    def close(self) -> None:
        """Close this proxy transaction."""
        self._is_open = False


class ProxyTransactionContext:
    """Context manager mimicking TransactionContext but routing through the proxy."""

    def __init__(self, proxy: ProxyDatabase, tx_type: str):
        self._proxy = proxy
        self._tx_type = tx_type
        self._tx: ProxyTransaction | None = None

    def __enter__(self) -> ProxyTransactionContext:
        self._tx = ProxyTransaction(self._proxy, self._tx_type)
        return self

    def __exit__(self, exc_type: type[BaseException] | None, exc_val: Any, exc_tb: Any) -> None:
        if self._tx is None or not self._tx.is_open:
            return
        if exc_type is None:
            if self._tx_type in ("write", "schema"):
                self._tx.commit()
        else:
            if self._tx_type in ("write", "schema"):
                self._tx.rollback()
        self._tx.close()

    @property
    def transaction(self) -> ProxyTransaction:
        """Underlying proxy transaction."""
        if self._tx is None:
            raise RuntimeError("ProxyTransactionContext not entered")
        return self._tx

    @property
    def database(self) -> ProxyDatabase:
        """Proxy database backing this context."""
        return self._proxy

    def execute(self, query: str) -> list[dict[str, Any]]:
        """Execute a query within this context."""
        return self.transaction.execute(query)

    def commit(self) -> None:
        """Commit the active transaction."""
        self.transaction.commit()

    def rollback(self) -> None:
        """Rollback the active transaction."""
        self.transaction.rollback()


class ProxyDatabase:
    """Drop-in replacement for Database that routes queries through a type-bridge proxy server.

    Instead of connecting directly to TypeDB, all queries are sent as HTTP
    requests to the proxy server's REST API. The proxy handles validation,
    interceptors (audit log, etc.), and forwarding to TypeDB.
    """

    def __init__(
        self,
        proxy_url: str = "http://localhost:8080",
        database: str = "typedb",
        timeout: int = 30,
    ):
        self.proxy_url = proxy_url.rstrip("/")
        self.database_name = database
        self.timeout = timeout
        self._connected = False

    def connect(self) -> None:
        """Verify the proxy server is reachable via health check."""
        try:
            health = self._http_get("/health")
            self._connected = True
            logger.info(
                "Connected to proxy at %s (version: %s)",
                self.proxy_url,
                health.get("version", "unknown"),
            )
        except Exception as e:
            raise ConnectionError(f"Failed to connect to proxy at {self.proxy_url}: {e}") from e

    def close(self) -> None:
        """Close the proxy connection (clears connected state)."""
        self._connected = False
        logger.debug("Proxy connection closed: %s", self.proxy_url)

    def __enter__(self) -> ProxyDatabase:
        self.connect()
        return self

    def __exit__(self, exc_type: type[BaseException] | None, exc_val: Any, exc_tb: Any) -> None:
        del exc_type, exc_val, exc_tb
        self.close()

    def transaction(self, transaction_type: Any = "read") -> ProxyTransactionContext:
        """Create a proxy transaction context.

        Args:
            transaction_type: Transaction type string ("read", "write", "schema")
                or TransactionType enum value.
        """
        if isinstance(transaction_type, str):
            tx_type = transaction_type
        else:
            # Handle TransactionType enum from typedb.driver
            name = getattr(transaction_type, "name", str(transaction_type))
            tx_type = name.lower()
        return ProxyTransactionContext(self, tx_type)

    def execute_query(self, query: str, transaction_type: str = "read") -> list[dict[str, Any]]:
        """Execute a query through the proxy and return results."""
        logger.debug("Executing query via proxy (type=%s, %d chars)", transaction_type, len(query))
        results = self._send_raw_query(query, transaction_type)
        return results if isinstance(results, list) else [results]

    def get_schema(self) -> str:
        """Fetch the loaded schema from the proxy server."""
        resp = self._http_get("/schema")
        return json.dumps(resp) if isinstance(resp, dict) else str(resp)

    def _send_raw_query(self, query: str, tx_type: str) -> list[dict[str, Any]]:
        """Send a raw TypeQL query to the proxy server."""
        payload = {
            "database": self.database_name,
            "transaction_type": tx_type,
            "query": query,
            "metadata": {},
        }
        resp = self._http_post("/query/raw", payload)
        if resp.get("status") == "error":
            error = resp.get("error", {})
            raise ProxyError(
                error.get("message", "Unknown error"),
                code=error.get("code", "UNKNOWN"),
                details=error.get("details"),
            )
        return resp.get("results", [])

    def _http_get(self, path: str) -> dict[str, Any]:
        """Send an HTTP GET request to the proxy."""
        url = f"{self.proxy_url}{path}"
        req = urllib.request.Request(url, headers={"Accept": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                return json.loads(resp.read())
        except urllib.error.HTTPError as e:
            body = e.read().decode("utf-8", errors="replace")
            try:
                error_data = json.loads(body)
                raise ProxyError(
                    error_data.get("error", {}).get("message", body),
                    code=error_data.get("error", {}).get("code", f"HTTP_{e.code}"),
                ) from e
            except json.JSONDecodeError:
                raise ProxyError(body, code=f"HTTP_{e.code}") from e
        except urllib.error.URLError as e:
            raise ConnectionError(f"Cannot reach proxy at {url}: {e}") from e

    def _http_post(self, path: str, data: dict[str, Any]) -> dict[str, Any]:
        """Send an HTTP POST request to the proxy."""
        url = f"{self.proxy_url}{path}"
        payload = json.dumps(data).encode("utf-8")
        req = urllib.request.Request(
            url,
            data=payload,
            headers={"Content-Type": "application/json", "Accept": "application/json"},
        )
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                return json.loads(resp.read())
        except urllib.error.HTTPError as e:
            body = e.read().decode("utf-8", errors="replace")
            try:
                error_data = json.loads(body)
                raise ProxyError(
                    error_data.get("error", {}).get("message", body),
                    code=error_data.get("error", {}).get("code", f"HTTP_{e.code}"),
                ) from e
            except json.JSONDecodeError:
                raise ProxyError(body, code=f"HTTP_{e.code}") from e
        except urllib.error.URLError as e:
            raise ConnectionError(f"Cannot reach proxy at {url}: {e}") from e
