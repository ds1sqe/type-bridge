import os
import shutil
import subprocess
import time


def _detect_container_tool() -> str:
    """Auto-detect available container tool (podman or docker).

    Returns the first available tool, preferring podman if both are present.
    Falls back to 'docker' if neither is found (will fail later with clear error).
    """
    # Check environment variable first
    env_tool = os.getenv("CONTAINER_TOOL")
    if env_tool:
        return env_tool

    # Auto-detect: prefer podman, fall back to docker
    for tool in ("podman", "docker"):
        if shutil.which(tool):
            return tool

    # Default to docker (will fail with clear error if not found)
    return "docker"


# Container tool selection (auto-detected or from CONTAINER_TOOL env var)
CONTAINER_TOOL = _detect_container_tool()

# Test database configuration
TEST_DB_NAME = "type_bridge_test"
# Allow overriding port/address via environment (for local conflicts or Podman/Docker remaps)
TEST_DB_ADDRESS = os.getenv("TYPEDB_ADDRESS", "localhost:1730")
# HTTP port for the TypeDB version-probe endpoint; forward when running against a
# remapped port so the gate validates the right server, not a co-located default instance.
TEST_DB_HTTP_PORT = int(os.getenv("TYPEDB_HTTP_PORT", "8000"))


def start_typedb_container():
    """Start TypeDB Docker container for testing."""
    # Build compose commands based on container tool
    compose_base = (
        [CONTAINER_TOOL, "compose"]
        if CONTAINER_TOOL not in ("docker-compose", "podman-compose")
        else [CONTAINER_TOOL]
    )

    # Check if we should use Docker (default: yes, unless USE_DOCKER=false)
    use_docker = os.getenv("USE_DOCKER", "true").lower() != "false"

    if not use_docker:
        # Skip Docker management - assume TypeDB is already running
        return False

    # Get project root directory
    project_root = os.path.dirname(os.path.dirname(os.path.dirname(__file__)))

    # Start Docker container
    # Stop any existing container
    subprocess.run(
        [*compose_base, "down"],
        cwd=project_root,
        capture_output=True,
    )

    # Start container
    subprocess.run(
        [*compose_base, "up", "-d"],
        cwd=project_root,
        check=True,
        capture_output=True,
    )

    # Wait for TypeDB to be healthy
    max_retries = 30
    for i in range(max_retries):
        result = subprocess.run(
            [CONTAINER_TOOL, "inspect", "--format={{.State.Health.Status}}", "typedb_test"],
            capture_output=True,
            text=True,
        )
        if result.stdout.strip() == "healthy":
            break
        time.sleep(1)
    else:
        raise RuntimeError("TypeDB container failed to become healthy")

    return True


def stop_typedb_container():
    """Stop TypeDB Docker container."""
    # Build compose commands based on container tool
    compose_base = (
        [CONTAINER_TOOL, "compose"]
        if CONTAINER_TOOL not in ("docker-compose", "podman-compose")
        else [CONTAINER_TOOL]
    )

    # Get project root directory
    project_root = os.path.dirname(os.path.dirname(os.path.dirname(__file__)))

    subprocess.run(
        [*compose_base, "down"],
        cwd=project_root,
        capture_output=True,
    )
