"""Compose project identity: sanitization, shell/Python parity, port parsing, precedence.

All tests here are DB-free and daemon-free (no container runtime required).
The one exception is test_compose_config_renders_worktree_unique, which calls
'compose config' for client-side YAML rendering — no daemon needed, skipped
only when no compose binary exists.
"""

from __future__ import annotations

import shutil
import subprocess
from pathlib import Path
from unittest.mock import patch

import pytest

from tests.utils.typedb_lifecycle import PortDiscoveryError, _parse_port_output, compose_project

REPO_ROOT = Path(__file__).resolve().parents[3]
TEST_SH = REPO_ROOT / "test.sh"


# ── Sanitization ──────────────────────────────────────────────────────────────


class TestComposeSanitization:
    """compose_project() derives a valid, collision-free name from a path."""

    def _project(self, basename: str) -> str:
        """Call compose_project with a synthetic path whose basename is *basename*."""
        return compose_project(Path("/fake/worktree") / basename)

    def test_simple_name(self):
        assert self._project("myrepo") == "tb-myrepo"

    def test_uppercase_lowered(self):
        assert self._project("MyRepo") == "tb-myrepo"

    def test_version_tag_dots_to_dashes(self):
        # v1.5.0 → tb-v1-5-0
        assert self._project("v1.5.0") == "tb-v1-5-0"

    def test_leading_digit_kept(self):
        # digits are valid in [a-z0-9]; no special leading-digit rule
        assert self._project("1-feature") == "tb-1-feature"

    def test_consecutive_dots_collapsed(self):
        assert self._project("a..b") == "tb-a-b"

    def test_consecutive_mixed_separators_collapsed(self):
        assert self._project("a._-b") == "tb-a-b"

    def test_leading_dash_stripped(self):
        assert self._project("-leading") == "tb-leading"

    def test_trailing_dash_stripped(self):
        assert self._project("trailing-") == "tb-trailing"

    def test_both_ends_stripped(self):
        assert self._project("-both-") == "tb-both"

    def test_underscore_becomes_dash(self):
        assert self._project("my_repo") == "tb-my-repo"

    def test_feature_branch_style(self):
        assert self._project("feat-123-add-buffer") == "tb-feat-123-add-buffer"

    def test_uses_basename_only(self):
        # Full path → only the last component matters
        result = compose_project(Path("/home/user/projects/type-bridge/v1.5.0"))
        assert result == "tb-v1-5-0"


# ── Shell / Python parity ────────────────────────────────────────────────────


class TestShellPythonParity:
    """The shell compose_project_for() and Python compose_project() agree byte-for-byte."""

    CANONICAL_INPUTS = [
        "v1.5.0",
        "myrepo",
        "MyRepo",
        "feat-123-add-buffer",
        "a._-b",
        "-leading",
        "trailing-",
        "1-feature",
    ]

    @pytest.mark.parametrize("basename", CANONICAL_INPUTS)
    def test_parity(self, basename: str):
        py_result = compose_project(Path("/fake") / basename)

        result = subprocess.run(
            ["bash", str(TEST_SH), "--print-project", f"/fake/{basename}"],
            capture_output=True,
            text=True,
        )
        sh_result = result.stdout.strip()

        assert py_result == sh_result, (
            f"basename={basename!r}: Python={py_result!r} Shell={sh_result!r}"
        )


# ── Port output parsing ──────────────────────────────────────────────────────


class TestParsePortOutput:
    """_parse_port_output handles every shape 'compose port' can emit."""

    def test_ipv4_mapping(self):
        assert _parse_port_output("0.0.0.0:32769\n") == 32769

    def test_ipv6_mapping(self):
        assert _parse_port_output("[::]:32769\n") == 32769

    def test_multiline_last_wins(self):
        # Docker prints IPv4 first, IPv6 second; last line wins.
        text = "0.0.0.0:32769\n[::]:32769\n"
        assert _parse_port_output(text) == 32769

    def test_multiline_different_ports_last_wins(self):
        # Unlikely but the parser must not crash; deterministic: last line.
        text = "0.0.0.0:11111\n[::]:22222\n"
        assert _parse_port_output(text) == 22222

    def test_empty_raises(self):
        with pytest.raises(PortDiscoveryError):
            _parse_port_output("")

    def test_whitespace_only_raises(self):
        with pytest.raises(PortDiscoveryError):
            _parse_port_output("   \n  \n")

    def test_garbage_raises(self):
        with pytest.raises(PortDiscoveryError):
            _parse_port_output("no port info here\n")

    def test_missing_colon_raises(self):
        with pytest.raises(PortDiscoveryError):
            _parse_port_output("32769\n")


# ── Env-override precedence ──────────────────────────────────────────────────


class TestEnvOverridePrecedence:
    """Explicit env pins skip port discovery; absent pins trigger it."""

    def _run_start(self, monkeypatch, *, typedb_address: str | None):
        """Drive start_typedb_container with compose subprocess calls faked.

        The fake makes 'ps -q' return a container ID and 'inspect' report
        healthy, so the function reaches the discovery branch without a
        container runtime.  Returns the list of discover_port calls.
        """
        import tests.utils.typedb_lifecycle as lc

        monkeypatch.setenv("USE_DOCKER", "true")
        if typedb_address is None:
            monkeypatch.delenv("TYPEDB_ADDRESS", raising=False)
        else:
            monkeypatch.setenv("TYPEDB_ADDRESS", typedb_address)
        monkeypatch.delenv("TYPEDB_HTTP_PORT", raising=False)

        def _fake_run(cmd, **kwargs):
            class _Result:
                returncode = 0
                stdout = ""
                stderr = ""

            result = _Result()
            if "ps" in cmd and "-q" in cmd:
                result.stdout = "abc123\n"
            elif "inspect" in cmd[0:2] or any("inspect" == part for part in cmd):
                result.stdout = "healthy\n"
            return result

        discovered: list[tuple[str, str, int]] = []

        def _fake_discover(project, service, container_port):
            discovered.append((project, service, container_port))
            return 32769

        with (
            patch.object(lc.subprocess, "run", _fake_run),
            patch.object(lc.time, "sleep", lambda *_: None),
            patch.object(lc, "discover_port", _fake_discover),
        ):
            assert lc.start_typedb_container() is True
        return discovered

    def test_explicit_typedb_address_skips_discovery(self, monkeypatch):
        """An explicit TYPEDB_ADDRESS pin must short-circuit gRPC-port discovery."""
        discovered = self._run_start(monkeypatch, typedb_address="localhost:19999")
        grpc_calls = [c for c in discovered if c[2] == 1729]
        assert grpc_calls == [], "explicit TYPEDB_ADDRESS must skip discovery"

    def test_absent_pins_trigger_discovery_for_both_ports(self, monkeypatch):
        """Without explicit pins, both the gRPC and HTTP ports are discovered."""
        discovered = self._run_start(monkeypatch, typedb_address=None)
        ports = sorted(c[2] for c in discovered)
        assert ports == [1729, 8000], f"expected discovery of both ports, got {discovered}"


# ── Compose config render ────────────────────────────────────────────────────


def _compose_binary() -> str | None:
    """Return the first available compose binary path, or None."""
    tool = None
    for candidate in ("podman", "docker"):
        if shutil.which(candidate):
            tool = candidate
            break
    if tool is None:
        return None
    # Verify that '<tool> compose' subcommand is available
    result = subprocess.run(
        [tool, "compose", "version"],
        capture_output=True,
    )
    if result.returncode == 0:
        return tool
    return None


_NO_COMPOSE = _compose_binary() is None


@pytest.mark.skipif(_NO_COMPOSE, reason="no compose binary available")
class TestComposeConfigRendersWorktreeUnique:
    """'compose config' renders disjoint project names and no container_name pins."""

    def _render_config(self, project_name: str) -> str:
        tool = _compose_binary()
        assert tool is not None  # guarded by the class-level skipif
        result = subprocess.run(
            [
                tool,
                "compose",
                "-f",
                str(REPO_ROOT / "docker-compose.yml"),
                "-p",
                project_name,
                "config",
            ],
            capture_output=True,
            text=True,
            cwd=str(REPO_ROOT),
        )
        assert result.returncode == 0, (
            f"compose config failed for project {project_name!r}:\n{result.stderr}"
        )
        return result.stdout

    def test_disjoint_project_names(self):
        """Two synthetic worktree paths yield distinct rendered project names."""
        proj1 = compose_project(Path("/worktrees/type-bridge/v1.5.0"))
        proj2 = compose_project(Path("/worktrees/type-bridge/feat-148-isolation"))
        assert proj1 != proj2

        cfg1 = self._render_config(proj1)
        cfg2 = self._render_config(proj2)

        # Rendered YAML includes 'name: <project>' at the top level.
        assert f"name: {proj1}" in cfg1
        assert f"name: {proj2}" in cfg2
        assert f"name: {proj1}" not in cfg2
        assert f"name: {proj2}" not in cfg1

    def test_no_container_name_in_rendered_config(self):
        """The rendered compose config must not contain any container_name key."""
        proj = compose_project(Path("/worktrees/type-bridge/v1.5.0"))
        cfg = self._render_config(proj)
        assert "container_name" not in cfg, (
            "container_name must not appear in rendered compose config; "
            "container naming is project-scoped by compose itself"
        )

    def test_no_fixed_host_port_in_defaults(self):
        """Port entries must not pin a fixed non-zero host port in the defaults.

        Parsed textually: compose renders long-form port entries with a
        `published: <port>` line, so any published value other than "0"
        is a fixed pin.  Textual parsing avoids a yaml dependency for one
        assertion.
        """
        proj = compose_project(Path("/worktrees/type-bridge/v1.5.0"))
        cfg = self._render_config(proj)

        for line in cfg.splitlines():
            stripped = line.strip()
            if not stripped.startswith("published:"):
                continue
            published = stripped.removeprefix("published:").strip().strip("\"'")
            assert published == "0", (
                f"fixed host port {published!r} in the rendered config; "
                "port default should be 0 (engine-assigned)"
            )
