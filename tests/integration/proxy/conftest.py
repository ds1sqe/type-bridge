"""Proxy health integration fixture."""

import os
from collections.abc import Generator

import pytest

from tests.utils.proxy_lifecycle import start_proxy_containers, stop_proxy_containers


@pytest.fixture(scope="session")
def proxy_address() -> Generator[str]:
    """Use the configured proxy, starting the isolated stack when requested."""
    started = start_proxy_containers()
    try:
        from tests.utils import proxy_lifecycle

        yield os.getenv("PROXY_ADDRESS", proxy_lifecycle.PROXY_ADDRESS)
    finally:
        if started:
            stop_proxy_containers()
