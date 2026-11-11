"""Proxy server health check independent of any SDK query facade."""

import json
from urllib.request import urlopen

import pytest


@pytest.mark.proxy
def test_proxy_health_endpoint(proxy_address: str) -> None:
    """Health endpoint returns 200 with version and connection status."""
    with urlopen(f"{proxy_address.rstrip('/')}/health", timeout=10) as response:
        assert response.status == 200
        health = json.load(response)
    assert health["status"] == "ok"
    assert "version" in health
    assert "typedb_connected" in health
