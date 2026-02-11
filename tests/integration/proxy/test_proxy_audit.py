"""Proxy server audit log integration tests.

Verifies that the audit-log interceptor is active and producing entries.
Since the server's audit log writes to stdout by default, we verify
the interceptor is listed in the response metadata.
"""

import pytest

from type_bridge.proxy import ProxyDatabase


@pytest.mark.proxy
def test_audit_interceptor_applied(proxy_db: ProxyDatabase) -> None:
    """The audit-log interceptor is listed in response metadata."""
    resp = proxy_db._http_post(
        "/query/raw",
        {
            "database": proxy_db.database_name,
            "transaction_type": "read",
            "query": "match $p isa person; fetch { };",
            "metadata": {},
        },
    )
    assert resp["status"] == "ok"
    interceptors = resp["metadata"]["interceptors_applied"]
    assert "audit-log" in interceptors


@pytest.mark.proxy
def test_audit_interceptor_on_write(proxy_db: ProxyDatabase) -> None:
    """Audit log captures write operations."""
    resp = proxy_db._http_post(
        "/query/raw",
        {
            "database": proxy_db.database_name,
            "transaction_type": "write",
            "query": "insert $p isa person, has name 'AuditTest';",
            "metadata": {},
        },
    )
    assert resp["status"] == "ok"
    interceptors = resp["metadata"]["interceptors_applied"]
    assert "audit-log" in interceptors


@pytest.mark.proxy
def test_multiple_queries_all_audited(proxy_db: ProxyDatabase) -> None:
    """Each query request goes through the audit interceptor."""
    for i in range(3):
        resp = proxy_db._http_post(
            "/query/raw",
            {
                "database": proxy_db.database_name,
                "transaction_type": "read",
                "query": f"match $p isa person, has name 'test_{i}'; fetch {{ }};",
                "metadata": {"test_index": i},
            },
        )
        assert resp["status"] == "ok"
        assert "audit-log" in resp["metadata"]["interceptors_applied"]
