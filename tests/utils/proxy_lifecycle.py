"""Proxy server lifecycle management for integration tests."""

import os
import subprocess
import time
from pathlib import Path

from tests.utils.typedb_lifecycle import (
    CONTAINER_TOOL,
    _compose_base,
    compose_project,
    discover_port,
)

_REPO_ROOT = Path(__file__).resolve().parents[2]

# Proxy test configuration — resolved after discovery (or explicit env override)
PROXY_PORT = os.getenv("PROXY_PORT", "8081")
PROXY_ADDRESS = os.getenv("PROXY_ADDRESS", f"http://localhost:{PROXY_PORT}")
PROXY_DB_NAME = "type_bridge_proxy_test"


def start_proxy_containers() -> bool:
    """Start TypeDB + proxy Docker containers for testing.

    Returns:
        True if Docker was started, False if USE_DOCKER=false.
    """
    use_docker = os.getenv("USE_DOCKER", "true").lower() != "false"
    if not use_docker:
        return False

    project = compose_project(_REPO_ROOT)
    compose_file = str(_REPO_ROOT / "docker-compose.proxy.yml")
    compose_with_proj = [*_compose_base(), "-p", project, "-f", compose_file]

    # Stop any existing containers for this project
    subprocess.run(
        [*compose_with_proj, "down"],
        cwd=str(_REPO_ROOT),
        capture_output=True,
    )

    # Build and start containers
    subprocess.run(
        [*compose_with_proj, "up", "-d", "--build"],
        cwd=str(_REPO_ROOT),
        check=True,
        capture_output=True,
    )

    # Wait for the proxy to become healthy, resolving its container ID via
    # 'compose ps -q' so we never depend on a hardcoded container name.
    max_retries = 60  # longer timeout for build + startup
    for _ in range(max_retries):
        id_result = subprocess.run(
            [*compose_with_proj, "ps", "-q", "proxy"],
            cwd=str(_REPO_ROOT),
            capture_output=True,
            text=True,
        )
        container_id = id_result.stdout.strip()
        if container_id:
            result = subprocess.run(
                [CONTAINER_TOOL, "inspect", "--format={{.State.Health.Status}}", container_id],
                capture_output=True,
                text=True,
            )
            if result.stdout.strip() == "healthy":
                # Discover ports only when the caller did not set them explicitly.
                global PROXY_PORT, PROXY_ADDRESS
                if not os.getenv("PROXY_PORT"):
                    proxy_port = discover_port(project, "proxy", 8080)
                    PROXY_PORT = str(proxy_port)
                    if not os.getenv("PROXY_ADDRESS"):
                        PROXY_ADDRESS = f"http://localhost:{PROXY_PORT}"
                return True
        time.sleep(1)

    raise RuntimeError("Proxy container failed to become healthy")


def stop_proxy_containers() -> None:
    """Stop TypeDB + proxy Docker containers."""
    project = compose_project(_REPO_ROOT)
    compose_file = str(_REPO_ROOT / "docker-compose.proxy.yml")
    compose_with_proj = [*_compose_base(), "-p", project, "-f", compose_file]

    subprocess.run(
        [*compose_with_proj, "down"],
        cwd=str(_REPO_ROOT),
        capture_output=True,
    )
