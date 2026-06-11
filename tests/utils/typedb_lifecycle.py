import os
import re
import shutil
import subprocess
import time
from pathlib import Path


class PortDiscoveryError(RuntimeError):
    """Raised when the host port for a container port cannot be determined."""


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

# Repository root — two levels up from tests/utils/
_REPO_ROOT = Path(__file__).resolve().parents[2]


def compose_project(path: str | Path) -> str:
    """Derive the compose project name for the given worktree directory.

    Rule (must stay byte-identical with compose_project_for() in test.sh):
      1. Take the basename of *path*.
      2. Lowercase it.
      3. Replace every maximal run of characters outside [a-z0-9] with '-'.
      4. Strip any leading or trailing '-'.
      5. Prefix with "tb-".
    """
    base = Path(path).name.lower()
    base = re.sub(r"[^a-z0-9]+", "-", base)
    base = base.strip("-")
    return f"tb-{base}"


def _parse_port_output(text: str) -> int:
    """Parse the host port from 'compose port' output.

    Compose may emit one line per address family, e.g.::

        0.0.0.0:32769
        [::]:32769

    Taking the last non-empty line is deterministic: when both families are
    present, the IPv6 line is always last, and both carry the same port number.
    Raises PortDiscoveryError on empty or malformed input.
    """
    lines = [line.strip() for line in text.splitlines() if line.strip()]
    if not lines:
        raise PortDiscoveryError("compose port returned empty output")
    last = lines[-1]
    # Match "host:port" — host may be an IPv4 address or an IPv6 bracket form.
    m = re.search(r":(\d+)$", last)
    if not m:
        raise PortDiscoveryError(f"could not parse port from compose port output: {last!r}")
    return int(m.group(1))


def discover_port(project: str, service: str, container_port: int) -> int:
    """Return the host port mapped to *container_port* for *service* in *project*.

    Retries up to 3 times with a 1-second pause because 'compose port' can
    return empty immediately after 'up -d' while the port mapping propagates.
    Raises PortDiscoveryError if the port cannot be determined after retries.
    """
    compose_base = _compose_base()
    for _ in range(3):
        result = subprocess.run(
            [*compose_base, "-p", project, "port", service, str(container_port)],
            cwd=str(_REPO_ROOT),
            capture_output=True,
            text=True,
        )
        if result.stdout.strip():
            return _parse_port_output(result.stdout)
        time.sleep(1)
    raise PortDiscoveryError(
        f"compose port returned empty for project={project!r} "
        f"service={service!r} container_port={container_port}"
    )


def _compose_base() -> list[str]:
    """Build compose command prefix."""
    if CONTAINER_TOOL in ("docker-compose", "podman-compose"):
        return [CONTAINER_TOOL]
    return [CONTAINER_TOOL, "compose"]


def start_typedb_container():
    """Start TypeDB Docker container for testing."""
    # Check if we should use Docker (default: yes, unless USE_DOCKER=false)
    use_docker = os.getenv("USE_DOCKER", "true").lower() != "false"

    if not use_docker:
        # Skip Docker management - assume TypeDB is already running
        return False

    compose_base = _compose_base()
    project = compose_project(_REPO_ROOT)
    compose_with_proj = [*compose_base, "-p", project, "-f", str(_REPO_ROOT / "docker-compose.yml")]

    # Stop any existing container for this project
    subprocess.run(
        [*compose_with_proj, "down"],
        cwd=str(_REPO_ROOT),
        capture_output=True,
    )

    # Start container
    subprocess.run(
        [*compose_with_proj, "up", "-d"],
        cwd=str(_REPO_ROOT),
        check=True,
        capture_output=True,
    )

    # Discover the container ID via 'compose ps -q' so we never depend on a
    # hardcoded container name (which varies per project scope).
    max_retries = 30
    for _ in range(max_retries):
        id_result = subprocess.run(
            [*compose_with_proj, "ps", "-q", "typedb"],
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
                break
        time.sleep(1)
    else:
        raise RuntimeError("TypeDB container failed to become healthy")

    # Derive address from discovery only when the caller did not set them explicitly.
    global TEST_DB_ADDRESS, TEST_DB_HTTP_PORT
    if not os.getenv("TYPEDB_ADDRESS"):
        port = discover_port(project, "typedb", 1729)
        TEST_DB_ADDRESS = f"localhost:{port}"
    if not os.getenv("TYPEDB_HTTP_PORT"):
        http_port = discover_port(project, "typedb", 8000)
        TEST_DB_HTTP_PORT = http_port

    return True


def stop_typedb_container():
    """Stop TypeDB Docker container."""
    compose_base = _compose_base()
    project = compose_project(_REPO_ROOT)
    compose_with_proj = [*compose_base, "-p", project, "-f", str(_REPO_ROOT / "docker-compose.yml")]

    subprocess.run(
        [*compose_with_proj, "down"],
        cwd=str(_REPO_ROOT),
        capture_output=True,
    )
