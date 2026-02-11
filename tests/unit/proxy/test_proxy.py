"""Unit tests for ProxyDatabase adapter."""

from __future__ import annotations

import json
from http.server import BaseHTTPRequestHandler, HTTPServer
from threading import Thread
from typing import Any

import pytest

from type_bridge.proxy import (
    ProxyDatabase,
    ProxyError,
    ProxyTransaction,
    ProxyTransactionContext,
)


class MockProxyHandler(BaseHTTPRequestHandler):
    """Minimal HTTP handler simulating the type-bridge proxy server."""

    def do_GET(self) -> None:
        if self.path == "/health":
            self._respond(200, {"status": "ok", "version": "0.1.0-test", "typedb_connected": True})
        elif self.path == "/schema":
            self._respond(200, {"entities": [], "relations": []})
        else:
            self._respond(
                404, {"status": "error", "error": {"code": "NOT_FOUND", "message": "Not found"}}
            )

    def do_POST(self) -> None:
        content_length = int(self.headers.get("Content-Length", 0))
        body = json.loads(self.rfile.read(content_length)) if content_length > 0 else {}

        if self.path == "/query/raw":
            query = body.get("query", "")
            self._respond(
                200,
                {
                    "status": "ok",
                    "results": [{"stub": True, "compiled_typeql": query}],
                    "metadata": {
                        "request_id": "test-id",
                        "execution_time_ms": 1,
                        "interceptors_applied": [],
                    },
                },
            )
        elif self.path == "/query/validate":
            self._respond(200, {"status": "ok", "is_valid": True, "errors": []})
        else:
            self._respond(
                404, {"status": "error", "error": {"code": "NOT_FOUND", "message": "Not found"}}
            )

    def _respond(self, status: int, body: dict[str, Any]) -> None:
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        data = json.dumps(body).encode()
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, format: str, *args: Any) -> None:
        """Suppress request logging during tests."""


@pytest.fixture()
def mock_server():
    """Start a mock HTTP server on a random port."""
    server = HTTPServer(("127.0.0.1", 0), MockProxyHandler)
    port = server.server_address[1]
    thread = Thread(target=server.serve_forever, daemon=True)
    thread.start()
    yield f"http://127.0.0.1:{port}"
    server.shutdown()


class TestProxyDatabase:
    def test_connect_success(self, mock_server: str) -> None:
        proxy = ProxyDatabase(proxy_url=mock_server, database="testdb")
        proxy.connect()
        assert proxy._connected is True
        proxy.close()
        assert proxy._connected is False

    def test_connect_failure(self) -> None:
        proxy = ProxyDatabase(proxy_url="http://127.0.0.1:1", database="testdb", timeout=1)
        with pytest.raises(ConnectionError):
            proxy.connect()

    def test_context_manager(self, mock_server: str) -> None:
        with ProxyDatabase(proxy_url=mock_server, database="testdb") as proxy:
            assert proxy._connected is True
        assert proxy._connected is False

    def test_execute_query(self, mock_server: str) -> None:
        proxy = ProxyDatabase(proxy_url=mock_server, database="testdb")
        proxy.connect()
        results = proxy.execute_query("match $p isa person; fetch { };", "read")
        assert isinstance(results, list)
        assert len(results) > 0
        assert results[0]["stub"] is True
        proxy.close()

    def test_get_schema(self, mock_server: str) -> None:
        proxy = ProxyDatabase(proxy_url=mock_server, database="testdb")
        proxy.connect()
        schema = proxy.get_schema()
        assert "entities" in schema
        proxy.close()

    def test_transaction_context(self, mock_server: str) -> None:
        proxy = ProxyDatabase(proxy_url=mock_server, database="testdb")
        proxy.connect()
        with proxy.transaction("read") as tx:
            results = tx.execute("match $p isa person; fetch { };")
            assert isinstance(results, list)
        proxy.close()

    def test_transaction_write_autocommit(self, mock_server: str) -> None:
        proxy = ProxyDatabase(proxy_url=mock_server, database="testdb")
        proxy.connect()
        with proxy.transaction("write") as tx:
            tx.execute("insert $p isa person, has name 'Alice';")
            # auto-commit on normal exit
        assert tx.transaction._is_open is False
        proxy.close()

    def test_transaction_type_enum(self, mock_server: str) -> None:
        """Test that TransactionType enum values are handled."""
        proxy = ProxyDatabase(proxy_url=mock_server, database="testdb")
        proxy.connect()
        # Simulate passing a TransactionType-like enum
        from enum import Enum

        class MockTxType(Enum):
            READ = 0
            WRITE = 1

        ctx = proxy.transaction(MockTxType.READ)
        assert ctx._tx_type == "read"
        proxy.close()

    def test_database_name(self, mock_server: str) -> None:
        proxy = ProxyDatabase(proxy_url=mock_server, database="mydb")
        assert proxy.database_name == "mydb"


class TestProxyTransaction:
    def test_execute(self, mock_server: str) -> None:
        proxy = ProxyDatabase(proxy_url=mock_server, database="testdb")
        proxy.connect()
        tx = ProxyTransaction(proxy, "read")
        results = tx.execute("match $p isa person; fetch { };")
        assert isinstance(results, list)
        proxy.close()

    def test_execute_after_close(self, mock_server: str) -> None:
        proxy = ProxyDatabase(proxy_url=mock_server, database="testdb")
        proxy.connect()
        tx = ProxyTransaction(proxy, "read")
        tx.close()
        with pytest.raises(RuntimeError, match="closed"):
            tx.execute("match $p isa person; fetch { };")
        proxy.close()

    def test_commit_closes(self, mock_server: str) -> None:
        proxy = ProxyDatabase(proxy_url=mock_server, database="testdb")
        proxy.connect()
        tx = ProxyTransaction(proxy, "write")
        assert tx.is_open is True
        tx.commit()
        assert tx.is_open is False
        proxy.close()


class TestProxyTransactionContext:
    def test_context_manager(self, mock_server: str) -> None:
        proxy = ProxyDatabase(proxy_url=mock_server, database="testdb")
        proxy.connect()
        ctx = ProxyTransactionContext(proxy, "read")
        with ctx:
            assert ctx.transaction.is_open is True
            ctx.execute("match $p isa person; fetch { };")
        proxy.close()

    def test_database_property(self, mock_server: str) -> None:
        proxy = ProxyDatabase(proxy_url=mock_server, database="testdb")
        ctx = ProxyTransactionContext(proxy, "read")
        assert ctx.database is proxy

    def test_transaction_before_enter(self) -> None:
        proxy = ProxyDatabase(proxy_url="http://localhost:8080", database="testdb")
        ctx = ProxyTransactionContext(proxy, "read")
        with pytest.raises(RuntimeError, match="not entered"):
            _ = ctx.transaction


class TestProxyError:
    def test_error_attributes(self) -> None:
        err = ProxyError("test error", code="TEST_CODE", details={"key": "val"})
        assert str(err) == "test error"
        assert err.code == "TEST_CODE"
        assert err.details == {"key": "val"}

    def test_http_error_handling(self) -> None:
        """Test that HTTP errors from unreachable servers raise ConnectionError."""
        proxy = ProxyDatabase(proxy_url="http://127.0.0.1:1", database="testdb", timeout=1)
        with pytest.raises(ConnectionError):
            proxy._send_raw_query("match $p isa person;", "read")


class TestConnectionExecutorIntegration:
    """Test that ProxyDatabase works with ConnectionExecutor."""

    def test_proxy_database_as_connection(self, mock_server: str) -> None:
        from type_bridge.session import ConnectionExecutor

        proxy = ProxyDatabase(proxy_url=mock_server, database="testdb")
        proxy.connect()
        executor = ConnectionExecutor(proxy)
        assert executor.has_transaction is False
        assert executor.database is proxy
        proxy.close()

    def test_proxy_transaction_as_connection(self, mock_server: str) -> None:
        from type_bridge.session import ConnectionExecutor

        proxy = ProxyDatabase(proxy_url=mock_server, database="testdb")
        proxy.connect()
        tx = ProxyTransaction(proxy, "read")
        executor = ConnectionExecutor(tx)
        assert executor.has_transaction is True
        assert executor.transaction is tx
        proxy.close()

    def test_proxy_transaction_context_as_connection(self, mock_server: str) -> None:
        from type_bridge.session import ConnectionExecutor

        proxy = ProxyDatabase(proxy_url=mock_server, database="testdb")
        proxy.connect()
        with proxy.transaction("read") as ctx:
            executor = ConnectionExecutor(ctx)
            assert executor.has_transaction is True
            assert executor.transaction is ctx.transaction
        proxy.close()
