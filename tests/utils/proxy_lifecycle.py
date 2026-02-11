"""Proxy server lifecycle management for integration tests."""

import os
import shutil
import subprocess
import time


def _detect_container_tool() -> str:
    """Auto-detect available container tool (podman or docker)."""
    env_tool = os.getenv("CONTAINER_TOOL")
    if env_tool:
        return env_tool
    for tool in ("podman", "docker"):
        if shutil.which(tool):
            return tool
    return "docker"


CONTAINER_TOOL = _detect_container_tool()

# Proxy test configuration
PROXY_PORT = os.getenv("PROXY_PORT", "8081")
PROXY_ADDRESS = os.getenv("PROXY_ADDRESS", f"http://localhost:{PROXY_PORT}")
PROXY_DB_NAME = "type_bridge_proxy_test"


def _compose_base() -> list[str]:
    """Build compose command base."""
    if CONTAINER_TOOL in ("docker-compose", "podman-compose"):
        return [CONTAINER_TOOL]
    return [CONTAINER_TOOL, "compose"]


def start_proxy_containers() -> bool:
    """Start TypeDB + proxy Docker containers for testing.

    Returns:
        True if Docker was started, False if USE_DOCKER=false.
    """
    use_docker = os.getenv("USE_DOCKER", "true").lower() != "false"
    if not use_docker:
        return False

    project_root = os.path.dirname(os.path.dirname(os.path.dirname(__file__)))
    compose_file = os.path.join(project_root, "docker-compose.proxy.yml")
    compose_base = [*_compose_base(), "-f", compose_file]

    # Stop any existing containers
    subprocess.run(
        [*compose_base, "down"],
        cwd=project_root,
        capture_output=True,
    )

    # Build and start containers
    subprocess.run(
        [*compose_base, "up", "-d", "--build"],
        cwd=project_root,
        check=True,
        capture_output=True,
    )

    # Wait for proxy to be healthy
    max_retries = 60  # longer timeout for build + startup
    for _ in range(max_retries):
        result = subprocess.run(
            [
                CONTAINER_TOOL,
                "inspect",
                "--format={{.State.Health.Status}}",
                "type_bridge_proxy_test",
            ],
            capture_output=True,
            text=True,
        )
        if result.stdout.strip() == "healthy":
            return True
        time.sleep(1)

    raise RuntimeError("Proxy container failed to become healthy")


def stop_proxy_containers() -> None:
    """Stop TypeDB + proxy Docker containers."""
    project_root = os.path.dirname(os.path.dirname(os.path.dirname(__file__)))
    compose_file = os.path.join(project_root, "docker-compose.proxy.yml")
    compose_base = [*_compose_base(), "-f", compose_file]

    subprocess.run(
        [*compose_base, "down"],
        cwd=project_root,
        capture_output=True,
    )
