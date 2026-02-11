"""Proxy server health check integration tests."""

import pytest

from type_bridge.proxy import ProxyDatabase


@pytest.mark.proxy
def test_proxy_health_endpoint(proxy_db: ProxyDatabase) -> None:
    """Health endpoint returns 200 with version and status."""
    health = proxy_db._http_get("/health")
    assert health["status"] == "ok"
    assert "version" in health
    assert "typedb_connected" in health


@pytest.mark.proxy
def test_proxy_schema_endpoint(proxy_db: ProxyDatabase) -> None:
    """Schema endpoint returns schema or appropriate error."""
    # Without a schema file configured, this should return a schema error
    # The server returns 500 with "No schema loaded" when no schema is configured
    from type_bridge.proxy import ProxyError

    try:
        schema = proxy_db.get_schema()
        # If schema is loaded, it should be valid JSON
        assert isinstance(schema, str)
    except ProxyError as e:
        # Expected when no schema file is configured
        assert "schema" in e.code.lower() or "SCHEMA" in e.code
