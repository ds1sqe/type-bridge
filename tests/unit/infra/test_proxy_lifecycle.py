"""The strict proxy Compose template receives disposable fixture credentials."""

import subprocess

import pytest

from tests.utils import proxy_lifecycle as lifecycle


@pytest.mark.parametrize("credentials", [None, ("fixture-user", "fixture-password")])
def test_proxy_compose_supplies_fixture_credentials(monkeypatch: pytest.MonkeyPatch, credentials):
    monkeypatch.setenv("USE_DOCKER", "true")
    for name in ("TYPEDB_USERNAME", "TYPEDB_PASSWORD"):
        monkeypatch.delenv(name, raising=False)
    if credentials is not None:
        monkeypatch.setenv("TYPEDB_USERNAME", credentials[0])
        monkeypatch.setenv("TYPEDB_PASSWORD", credentials[1])
    expected = credentials or ("admin", "password")
    monkeypatch.setattr(lifecycle, "_compose_base", lambda: ["fixture-compose"])
    monkeypatch.setattr(lifecycle, "compose_project", lambda _: "fixture-project")
    monkeypatch.setattr(lifecycle, "discover_port", lambda *_: 8081)
    calls = []

    def run(command, **kwargs):
        if command[0] == "fixture-compose":
            environment = kwargs["env"]
            assert (environment["TYPEDB_USERNAME"], environment["TYPEDB_PASSWORD"]) == expected
            calls.append(command)
        output = "fixture-container" if "ps" in command else "healthy"
        return subprocess.CompletedProcess(command, 0, stdout=output)

    monkeypatch.setattr(lifecycle.subprocess, "run", run)
    assert lifecycle.start_proxy_containers() is True
    lifecycle.stop_proxy_containers()
    assert len(calls) == 4  # down, up, ps, final down all parse the strict template.
