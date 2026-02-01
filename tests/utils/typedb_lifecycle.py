import os
import shutil
import subprocess
import time
from contextlib import contextmanager


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


@contextmanager
def suppress_stderr():
    """Suppress stderr at the file descriptor level.

    This is needed to silence the TypeDB driver's Rust logging initialization
    warning which writes directly to fd 2, bypassing Python's sys.stderr.
    """
    # Always use fd 2 directly (actual stderr) since Rust writes there
    stderr_fd = 2
    try:
        saved_stderr = os.dup(stderr_fd)
    except OSError:
        # In some environments, fd 2 might not be available for dup
        yield
        return

    devnull = os.open(os.devnull, os.O_WRONLY)
    os.dup2(devnull, stderr_fd)
    os.close(devnull)
    try:
        yield
    finally:
        os.dup2(saved_stderr, stderr_fd)
        os.close(saved_stderr)


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
