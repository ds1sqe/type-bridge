"""Proxy server CRUD integration tests.

These tests verify that queries can flow through the proxy server.
Since the TypeDB driver is not yet integrated (stub execution),
these tests verify the full HTTP pipeline: parse → validate → intercept → compile → respond.
"""

import pytest

from type_bridge.proxy import ProxyDatabase, ProxyError


@pytest.mark.proxy
def test_raw_query_passthrough(proxy_db: ProxyDatabase) -> None:
    """Raw TypeQL query is parsed, compiled, and executed."""
    results = proxy_db.execute_query(
        'match $p isa person; fetch { "person": { $p.* } };',
        transaction_type="read",
    )
    assert isinstance(results, list)


@pytest.mark.proxy
def test_raw_query_with_write_transaction(proxy_db: ProxyDatabase) -> None:
    """Write queries flow through the proxy pipeline."""
    results = proxy_db.execute_query(
        'insert $p isa person; $p has name "Alice";',
        transaction_type="write",
    )
    assert isinstance(results, list)


@pytest.mark.proxy
def test_transaction_context_execute(proxy_db: ProxyDatabase) -> None:
    """Queries work through transaction context."""
    with proxy_db.transaction("read") as tx:
        results = tx.execute('match $p isa person; fetch { "person": { $p.* } };')
        assert isinstance(results, list)


@pytest.mark.proxy
def test_transaction_context_write(proxy_db: ProxyDatabase) -> None:
    """Write transaction context auto-commits on exit."""
    with proxy_db.transaction("write") as tx:
        results = tx.execute('insert $p isa person; $p has name "Bob";')
        assert isinstance(results, list)
    # Transaction should be closed after context exit
    assert tx.transaction.is_open is False


@pytest.mark.proxy
def test_query_response_metadata(proxy_db: ProxyDatabase) -> None:
    """Response includes metadata (request_id, execution_time, interceptors)."""
    resp = proxy_db._http_post(
        "/query/raw",
        {
            "database": proxy_db.database_name,
            "transaction_type": "read",
            "query": 'match $p isa person; fetch { "person": { $p.* } };',
            "metadata": {},
        },
    )
    assert resp["status"] == "ok"
    assert "metadata" in resp
    metadata = resp["metadata"]
    assert "request_id" in metadata
    assert "execution_time_ms" in metadata
    assert "interceptors_applied" in metadata


@pytest.mark.proxy
def test_invalid_query_returns_error(proxy_db: ProxyDatabase) -> None:
    """Invalid TypeQL returns a parse error."""
    with pytest.raises(ProxyError) as exc_info:
        proxy_db.execute_query("this is not valid typeql!!!", "read")
    assert "PARSE" in exc_info.value.code or "parse" in str(exc_info.value).lower()
