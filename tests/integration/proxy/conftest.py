"""Pytest fixtures for proxy integration tests."""

from collections.abc import Generator

import pytest

from tests.utils.proxy_lifecycle import (
    PROXY_ADDRESS,
    PROXY_DB_NAME,
    start_proxy_containers,
    stop_proxy_containers,
)
from type_bridge.proxy import ProxyDatabase


@pytest.fixture(scope="session")
def docker_proxy() -> Generator[None]:
    """Start TypeDB + proxy Docker containers for the test session."""
    if start_proxy_containers():
        try:
            yield
        finally:
            stop_proxy_containers()
    else:
        yield


@pytest.fixture(scope="session")
def proxy_db(docker_proxy: None) -> Generator[ProxyDatabase]:
    """Create a ProxyDatabase connection for the test session.

    Yields:
        Connected ProxyDatabase instance
    """
    try:
        proxy = ProxyDatabase(proxy_url=PROXY_ADDRESS, database=PROXY_DB_NAME)
        proxy.connect()
        yield proxy
        proxy.close()
    except Exception as e:
        pytest.skip(f"Proxy server not available at {PROXY_ADDRESS}: {e}")
