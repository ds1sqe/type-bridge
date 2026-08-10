#!/usr/bin/env python3
"""Run the release-pinned Skopeo image with narrowly scoped host access."""

from __future__ import annotations

import argparse
import os
import subprocess
from collections.abc import Mapping, Sequence
from pathlib import Path

SKOPEO_IMAGE = (
    "quay.io/skopeo/stable@sha256:0e392474a4383b733038b85eff26ade929d2ff10e8deead25a6add3ed79fb362"
)
CONTAINER_AUTH_FILE = "/tmp/type-bridge-skopeo-auth.json"


class RunnerError(RuntimeError):
    """The pinned Skopeo runtime cannot be started safely."""


def _docker_auth_file(environment: Mapping[str, str]) -> Path:
    docker_config = environment.get("DOCKER_CONFIG")
    if docker_config:
        config_directory = Path(docker_config)
    else:
        user_directory = environment.get("HOME")
        if not user_directory:
            raise RunnerError("HOME or DOCKER_CONFIG is required for registry authentication")
        config_directory = Path(user_directory) / ".docker"
    auth_file = config_directory / "config.json"
    if auth_file.is_symlink() or not auth_file.is_file():
        raise RunnerError(f"Docker registry authentication file is missing or unsafe: {auth_file}")
    return auth_file.resolve(strict=True)


def build_command(
    skopeo_arguments: Sequence[str],
    *,
    registry_auth: bool,
    working_directory: Path,
    environment: Mapping[str, str],
    write_directory: Path | None = None,
    user_id: int | None = None,
    group_id: int | None = None,
) -> list[str]:
    """Build the exact Docker invocation without executing it."""
    if not skopeo_arguments:
        raise RunnerError("at least one Skopeo argument is required")
    resolved_working_directory = working_directory.resolve(strict=True)
    if not resolved_working_directory.is_dir():
        raise RunnerError(
            f"Skopeo working directory is not a directory: {resolved_working_directory}"
        )
    resolved_write_directory: Path | None = None
    if write_directory is not None:
        if write_directory.is_symlink() or not write_directory.is_dir():
            raise RunnerError(f"Skopeo write directory is missing or unsafe: {write_directory}")
        resolved_write_directory = write_directory.resolve(strict=True)
    if user_id is None:
        user_id = os.getuid()
    if group_id is None:
        group_id = os.getgid()
    command = [
        "docker",
        "run",
        "--rm",
        "--platform",
        "linux/amd64",
        "--user",
        f"{user_id}:{group_id}",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges:true",
        "--mount",
        (f"type=bind,src={resolved_working_directory},dst={resolved_working_directory},readonly"),
        "--workdir",
        str(resolved_working_directory),
    ]
    if not registry_auth:
        command.extend(["--network", "none"])
    if resolved_write_directory is not None:
        command.extend(
            [
                "--mount",
                f"type=bind,src={resolved_write_directory},dst={resolved_write_directory}",
            ]
        )
    if registry_auth:
        auth_file = _docker_auth_file(environment)
        command.extend(
            [
                "--mount",
                f"type=bind,src={auth_file},dst={CONTAINER_AUTH_FILE},readonly",
                "--env",
                f"REGISTRY_AUTH_FILE={CONTAINER_AUTH_FILE}",
            ]
        )
    command.append(SKOPEO_IMAGE)
    command.extend(skopeo_arguments)
    return command


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--write-directory",
        type=Path,
        help="mount one explicit directory writable for archive output",
    )
    parser.add_argument(
        "--registry-auth",
        action="store_true",
        help="mount the current Docker config as Skopeo's registry auth file",
    )
    parser.add_argument("skopeo_arguments", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.skopeo_arguments[:1] == ["--"]:
        args.skopeo_arguments = args.skopeo_arguments[1:]
    return args


def main() -> int:
    args = parse_args()
    try:
        command = build_command(
            args.skopeo_arguments,
            registry_auth=args.registry_auth,
            working_directory=Path.cwd(),
            environment=os.environ,
            write_directory=args.write_directory,
        )
    except (OSError, RunnerError) as error:
        raise SystemExit(f"pinned Skopeo runner failed: {error}") from error
    return subprocess.run(command, check=False).returncode


if __name__ == "__main__":
    raise SystemExit(main())
