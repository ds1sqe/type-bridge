"""Static release contract for the TypeBridge server OCI image."""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import math
import tarfile
import tomllib
from pathlib import Path

import pytest

from scripts.ci import run_pinned_skopeo
from scripts.ci import validate_server_oci_layout as oci_validator

REPO_ROOT = Path(__file__).resolve().parents[3]
CORE = REPO_ROOT / "type-bridge-core"
DOCKERFILE = CORE / "Dockerfile.server"
CONTAINER_CONFIG = CORE / "server.container.toml"
DOCKERIGNORE = CORE / ".dockerignore"
VALIDATOR = REPO_ROOT / "scripts/ci/validate_server_oci_layout.py"
SKOPEO_RUNNER = REPO_ROOT / "scripts/ci/run_pinned_skopeo.py"
TRIVY_VALIDATOR = REPO_ROOT / "scripts/ci/validate_trivy_report.py"
RELEASE_WORKFLOW = REPO_ROOT / ".github/workflows/release.yml"
CREATED = "2026-08-07T06:43:07Z"
REVISION = "641a3a5e0321ca198f052e75470e0552a3b12694"
BUILDKIT_IMAGE = (
    "moby/buildkit@sha256:2f5adac4ecd194d9f8c10b7b5d7bceb5186853db1b26e5abd3a657af0b7e26ec"
)
BINFMT_IMAGE = (
    "docker.io/tonistiigi/binfmt@"
    "sha256:400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0"
)
TYPEDB_SERVICE_IMAGE = (
    "typedb/typedb:3.12.1@sha256:4224951114b044d52e2fe48108be26ae2734726041dae8d63453ecd407fe2422"
)


def test_server_image_build_and_runtime_are_immutable_and_least_privilege() -> None:
    source = DOCKERFILE.read_text(encoding="utf-8")

    assert source.splitlines()[0] == (
        "# syntax=docker/dockerfile:1.7@"
        "sha256:a57df69d0ea827fb7266491f2813635de6f17269be881f696fbfdf2d83dda33e"
    )
    assert (
        "rust:1.94.1-bookworm@"
        "sha256:6ae102bdbf528294bc79ad6e1fae682f6f7c2a6e6621506ba959f9685b308a55"
    ) in source
    assert (
        "debian:bookworm-slim@"
        "sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818"
    ) in source
    assert "cargo build --locked --release -p type-bridge-server --no-default-features" in source
    assert "--features band8,band9,v2-query" in source
    assert "&& chage --lastday 0 typebridge" in source
    assert "&& rm -f /etc/shadow-" in source
    assert "USER 10001:10001" in source
    assert 'ENTRYPOINT ["/usr/local/bin/type-bridge-server"]' in source
    assert 'CMD ["--config", "/etc/type-bridge/server.toml"]' in source
    assert "HEALTHCHECK" not in source
    assert "server.toml /app/server.toml" not in source
    assert "apt-get" not in source


def test_server_image_declares_the_complete_oci_identity() -> None:
    source = DOCKERFILE.read_text(encoding="utf-8")
    for label in (
        "org.opencontainers.image.title",
        "org.opencontainers.image.description",
        "org.opencontainers.image.url",
        "org.opencontainers.image.source",
        "org.opencontainers.image.documentation",
        "org.opencontainers.image.revision",
        "org.opencontainers.image.version",
        "org.opencontainers.image.created",
        "org.opencontainers.image.licenses",
        "io.type-bridge.release-identity",
    ):
        assert source.count(label) == 1


def test_container_default_configuration_contains_no_credentials() -> None:
    source = CONTAINER_CONFIG.read_text(encoding="utf-8")
    config = tomllib.loads(source)

    assert config["server"] == {"host": "0.0.0.0", "port": 8080}
    assert config["typedb"] == {
        "address": "localhost:1729",
        "database": "typedb",
        "http_port": 8000,
    }
    assert config["v2"] == {"enabled": False}
    for marker in ("username", "password", "secret", "token", "private"):
        assert marker not in source.lower()


def test_container_build_context_is_an_explicit_allowlist() -> None:
    lines = DOCKERIGNORE.read_text(encoding="utf-8").splitlines()
    assert lines[0] == "*"
    assert set(lines[1:]) == {
        "!Cargo.toml",
        "!Cargo.lock",
        "!LICENSE",
        "!server.container.toml",
        "!crates/",
        "!crates/**",
        "!vendor/",
        "!vendor/**",
    }


def test_oci_layout_validator_is_fail_closed() -> None:
    source = VALIDATOR.read_text(encoding="utf-8")
    for required in (
        "etc/ssl/certs/ca-certificates.crt",
        "etc/type-bridge/server.toml",
        "usr/local/bin/type-bridge-server",
        "usr/share/licenses/type-bridge/LICENSE",
    ):
        assert required in source
    assert "10001:10001" in source
    assert "must contain exactly one descriptor" in source
    assert "runtime contains forbidden source/secret paths" in source
    assert "created timestamp diverges from the release commit" in source


def _json_bytes(value: object) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def _digest(payload: bytes) -> str:
    return f"sha256:{hashlib.sha256(payload).hexdigest()}"


def _set_tar_mtime(member: tarfile.TarInfo, value: int | float) -> None:
    if isinstance(value, float) and not math.isfinite(value):
        member.mtime = 0
        member.pax_headers = {**member.pax_headers, "mtime": str(value)}
        return
    member.mtime = value


def _layer_bytes(
    *,
    directory_mtime: int | float = 0,
    file_mtime: int | float = 0,
    shadow_backup_entry: str | None = None,
    shadow_entry: str = oci_validator.EXPECTED_RUNTIME_SHADOW_ENTRY,
) -> bytes:
    output = io.BytesIO()
    files = {
        "etc/ssl/certs/ca-certificates.crt": b"test certificate bundle\n",
        "etc/shadow": f"root:*:0:0:99999:7:::\n{shadow_entry}\n".encode(),
        "etc/type-bridge/server.toml": b"[v2]\nenabled = false\n",
        "usr/local/bin/type-bridge-server": b"\x7fELFsynthetic server",
        "usr/share/licenses/type-bridge/LICENSE": b"MIT\n",
    }
    if shadow_backup_entry is not None:
        files["etc/shadow-"] = f"{shadow_backup_entry}\n".encode()
    with tarfile.open(fileobj=output, mode="w") as layer:
        directory = tarfile.TarInfo("etc")
        directory.mode = 0o755
        _set_tar_mtime(directory, directory_mtime)
        directory.type = tarfile.DIRTYPE
        layer.addfile(directory)
        for name, payload in files.items():
            member = tarfile.TarInfo(name)
            member.mode = 0o444
            _set_tar_mtime(member, file_mtime)
            member.size = len(payload)
            layer.addfile(member, io.BytesIO(payload))
    return output.getvalue()


def _entry_layer(*entries: tuple[str, str, bytes | str | None]) -> bytes:
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w") as layer:
        for name, kind, value in entries:
            member = tarfile.TarInfo(name)
            member.mtime = 0
            member.mode = 0o755 if kind == "directory" else 0o444
            payload: bytes | None = None
            if kind == "regular":
                payload = value if isinstance(value, bytes) else b""
                member.size = len(payload)
            elif kind == "directory":
                member.type = tarfile.DIRTYPE
            elif kind == "symlink":
                member.type = tarfile.SYMTYPE
                member.linkname = str(value)
            elif kind == "hardlink":
                member.type = tarfile.LNKTYPE
                member.linkname = str(value)
            else:
                raise AssertionError(f"unsupported synthetic layer entry kind: {kind}")
            layer.addfile(member, None if payload is None else io.BytesIO(payload))
    return output.getvalue()


def _write_oci_archive(
    path: Path,
    *,
    config_created: object = CREATED,
    history_created: object = CREATED,
    index_created: object = CREATED,
    layer_directory_mtime: int | float = 0,
    layer_file_mtime: int | float = 0,
    layer_payloads: list[bytes] | None = None,
    shadow_backup_entry: str | None = None,
    shadow_entry: str = oci_validator.EXPECTED_RUNTIME_SHADOW_ENTRY,
) -> None:
    if layer_payloads is None:
        layer_payloads = [
            _layer_bytes(
                directory_mtime=layer_directory_mtime,
                file_mtime=layer_file_mtime,
                shadow_backup_entry=shadow_backup_entry,
                shadow_entry=shadow_entry,
            )
        ]
    labels = {
        **oci_validator.EXPECTED_LABELS,
        "org.opencontainers.image.revision": REVISION,
        "org.opencontainers.image.version": "2.1.0",
        "org.opencontainers.image.created": CREATED,
        "io.type-bridge.release-identity": f"v2.1.0@{REVISION}",
    }
    config = _json_bytes(
        {
            "architecture": "amd64",
            "config": {
                "Cmd": ["--config", "/etc/type-bridge/server.toml"],
                "Entrypoint": ["/usr/local/bin/type-bridge-server"],
                "Labels": labels,
                "User": "10001:10001",
                "WorkingDir": "/",
            },
            "created": config_created,
            "history": [
                {"created": "2026-07-13T00:00:00Z", "created_by": "pinned base"},
                {"created": history_created, "created_by": "synthetic fixture"},
            ],
            "os": "linux",
            "rootfs": {"diff_ids": [_digest(layer) for layer in layer_payloads], "type": "layers"},
        }
    )
    manifest = _json_bytes(
        {
            "config": {
                "digest": _digest(config),
                "mediaType": "application/vnd.oci.image.config.v1+json",
                "size": len(config),
            },
            "layers": [
                {
                    "digest": _digest(layer),
                    "mediaType": "application/vnd.oci.image.layer.v1.tar",
                    "size": len(layer),
                }
                for layer in layer_payloads
            ],
            "mediaType": oci_validator.OCI_IMAGE_MANIFEST,
            "schemaVersion": 2,
        }
    )
    index = _json_bytes(
        {
            "manifests": [
                {
                    "annotations": {"org.opencontainers.image.created": index_created},
                    "digest": _digest(manifest),
                    "mediaType": oci_validator.OCI_IMAGE_MANIFEST,
                    "platform": {"architecture": "amd64", "os": "linux"},
                    "size": len(manifest),
                }
            ],
            "schemaVersion": 2,
        }
    )
    members = {
        "blobs/sha256/" + _digest(config).split(":", maxsplit=1)[1]: config,
        "blobs/sha256/" + _digest(manifest).split(":", maxsplit=1)[1]: manifest,
        "index.json": index,
        "oci-layout": b'{"imageLayoutVersion":"1.0.0"}',
    }
    members.update(
        {
            "blobs/sha256/" + _digest(layer).split(":", maxsplit=1)[1]: layer
            for layer in layer_payloads
        }
    )
    with tarfile.open(path, mode="w") as archive:
        for name, payload in members.items():
            member = tarfile.TarInfo(name)
            member.mode = 0o444
            member.mtime = 0
            member.size = len(payload)
            archive.addfile(member, io.BytesIO(payload))


def _validator_args(archive: Path) -> argparse.Namespace:
    return argparse.Namespace(
        archive=str(archive),
        created=CREATED,
        platform="linux/amd64",
        release_identity=f"v2.1.0@{REVISION}",
        report=str(archive.with_suffix(".json")),
        revision=REVISION,
        version="2.1.0",
    )


def test_oci_layout_validator_accepts_commit_normalized_timestamps(tmp_path: Path) -> None:
    archive = tmp_path / "accepted.oci.tar"
    _write_oci_archive(archive)

    report = oci_validator.validate(_validator_args(archive))

    assert report["config_created"] == CREATED
    assert report["history_entry_count"] == 2
    assert report["index_created"] == CREATED
    assert report["latest_layer_mtime"] == 0
    assert report["source_date_epoch"] == 1_786_084_987


@pytest.mark.parametrize(
    ("field", "value", "message"),
    (
        ("config", "2026-08-07T06:43:08Z", "OCI image config created timestamp diverges"),
        (
            "config",
            "2026-08-07T08:43:07+02:00",
            "OCI image config created timestamp diverges",
        ),
        (
            "index",
            "2026-08-07T06:43:08Z",
            "OCI index descriptor created timestamp diverges",
        ),
        (
            "index",
            "2026-08-07T06:43:07.000Z",
            "OCI index descriptor created timestamp diverges",
        ),
        ("config", "not-a-timestamp", "OCI image config created timestamp is not RFC 3339"),
        ("index", None, "OCI index descriptor created timestamp is missing"),
    ),
)
def test_oci_layout_validator_rejects_unbound_creation_times(
    tmp_path: Path,
    field: str,
    value: object,
    message: str,
) -> None:
    archive = tmp_path / f"rejected-{field}.oci.tar"
    if field == "config":
        _write_oci_archive(archive, config_created=value)
    elif field == "index":
        _write_oci_archive(archive, index_created=value)
    else:
        raise AssertionError(f"unsupported synthetic creation-time field: {field}")

    with pytest.raises(oci_validator.ValidationError, match=message):
        oci_validator.validate(_validator_args(archive))


def test_oci_layout_validator_rejects_history_after_release_commit(tmp_path: Path) -> None:
    archive = tmp_path / "future-history.oci.tar"
    _write_oci_archive(archive, history_created="2026-08-07T06:43:08Z")

    with pytest.raises(oci_validator.ValidationError, match="history entry 1 was created after"):
        oci_validator.validate(_validator_args(archive))


@pytest.mark.parametrize(
    ("field", "mtime"),
    (("directory", 1_786_084_988), ("file", 1_786_084_988), ("file", float("nan"))),
)
def test_oci_layout_validator_rejects_layer_members_after_release_commit(
    tmp_path: Path,
    field: str,
    mtime: int | float,
) -> None:
    archive = tmp_path / f"future-layer-{field}.oci.tar"
    if field == "directory":
        _write_oci_archive(archive, layer_directory_mtime=mtime)
    elif field == "file":
        _write_oci_archive(archive, layer_file_mtime=mtime)
    else:
        raise AssertionError(f"unsupported synthetic layer field: {field}")

    with pytest.raises(oci_validator.ValidationError, match="post-release layer member timestamps"):
        oci_validator.validate(_validator_args(archive))


@pytest.mark.parametrize(
    "shadow_entry",
    (
        "typebridge:!:20672:0:99999:7:::",
        "typebridge:*:0:0:99999:7:::",
        "typebridge::0:0:99999:7:::",
        "typebridge:!:0:0:99999:7:::\ntypebridge:!:0:0:99999:7:::",
        "root:*:0:0:99999:7:::",
    ),
)
def test_oci_layout_validator_rejects_nondeterministic_or_unlocked_service_account(
    tmp_path: Path,
    shadow_entry: str,
) -> None:
    archive = tmp_path / "unsafe-shadow.oci.tar"
    _write_oci_archive(archive, shadow_entry=shadow_entry)

    with pytest.raises(
        oci_validator.ValidationError,
        match="deterministic locked typebridge shadow entry",
    ):
        oci_validator.validate(_validator_args(archive))


def test_oci_layout_validator_rejects_shadow_backup_with_wall_clock_state(
    tmp_path: Path,
) -> None:
    archive = tmp_path / "shadow-backup.oci.tar"
    _write_oci_archive(
        archive,
        shadow_backup_entry="typebridge:!:20672:0:99999:7:::",
    )

    with pytest.raises(oci_validator.ValidationError, match="must not retain.*shadow backup"):
        oci_validator.validate(_validator_args(archive))


@pytest.mark.parametrize("kind", ("symlink", "hardlink", "directory"))
def test_oci_layout_validator_requires_final_regular_runtime_files(
    tmp_path: Path,
    kind: str,
) -> None:
    archive = tmp_path / f"non-regular-server-{kind}.oci.tar"
    replacement: bytes | str | None = None
    if kind in ("symlink", "hardlink"):
        replacement = "/etc/passwd"
    _write_oci_archive(
        archive,
        layer_payloads=[
            _layer_bytes(),
            _entry_layer(("usr/local/bin/type-bridge-server", kind, replacement)),
        ],
    )

    with pytest.raises(oci_validator.ValidationError, match="not final regular files"):
        oci_validator.validate(_validator_args(archive))


def test_oci_layout_validator_applies_opaque_whiteouts_to_lower_entries(
    tmp_path: Path,
) -> None:
    archive = tmp_path / "opaque-server-directory.oci.tar"
    _write_oci_archive(
        archive,
        layer_payloads=[
            _layer_bytes(),
            _entry_layer(("usr/local/bin/.wh..wh..opq", "regular", b"")),
        ],
    )

    with pytest.raises(oci_validator.ValidationError, match="not final regular files"):
        oci_validator.validate(_validator_args(archive))


def test_oci_layout_validator_preserves_same_layer_entries_after_opaque_whiteout(
    tmp_path: Path,
) -> None:
    archive = tmp_path / "opaque-with-upper-server.oci.tar"
    _write_oci_archive(
        archive,
        layer_payloads=[
            _layer_bytes(),
            _entry_layer(
                ("usr/local/bin/type-bridge-server", "regular", b"\x7fELFupper server"),
                ("usr/local/bin/.wh..wh..opq", "regular", b""),
            ),
        ],
    )

    report = oci_validator.validate(_validator_args(archive))

    assert report["rootfs_diff_ids"] == [
        _digest(_layer_bytes()),
        _digest(
            _entry_layer(
                ("usr/local/bin/type-bridge-server", "regular", b"\x7fELFupper server"),
                ("usr/local/bin/.wh..wh..opq", "regular", b""),
            )
        ),
    ]


def test_oci_layout_validator_rejects_forbidden_non_regular_paths(tmp_path: Path) -> None:
    archive = tmp_path / "forbidden-symlink.oci.tar"
    _write_oci_archive(
        archive,
        layer_payloads=[
            _layer_bytes(),
            _entry_layer(("opt/runtime/Cargo.toml", "symlink", "/etc/passwd")),
        ],
    )

    with pytest.raises(oci_validator.ValidationError, match="forbidden source/secret paths"):
        oci_validator.validate(_validator_args(archive))


def test_pinned_skopeo_runner_scopes_offline_output_and_registry_credentials(
    tmp_path: Path,
) -> None:
    working_directory = tmp_path / "workspace"
    working_directory.mkdir()
    write_directory = tmp_path / "output"
    write_directory.mkdir()
    docker_config = tmp_path / "docker-config"
    docker_config.mkdir()
    auth_file = docker_config / "config.json"
    auth_file.write_text('{"auths": {}}\n', encoding="utf-8")
    offline_command = run_pinned_skopeo.build_command(
        ["copy", "oci-archive:server.oci.tar", "docker-archive:server.docker.tar"],
        registry_auth=False,
        working_directory=working_directory,
        environment={"DOCKER_CONFIG": str(docker_config)},
        write_directory=write_directory,
        user_id=1234,
        group_id=5678,
    )
    registry_command = run_pinned_skopeo.build_command(
        ["inspect", "docker://ghcr.io/ds1sqe/type-bridge-server:2.1.0"],
        registry_auth=True,
        working_directory=working_directory,
        environment={"DOCKER_CONFIG": str(docker_config)},
        user_id=1234,
        group_id=5678,
    )

    assert offline_command[0:3] == ["docker", "run", "--rm"]
    assert run_pinned_skopeo.SKOPEO_IMAGE in offline_command
    assert offline_command[offline_command.index("--network") :][:2] == ["--network", "none"]
    assert offline_command[offline_command.index("--user") :][:2] == ["--user", "1234:5678"]
    assert any(
        argument == f"type=bind,src={write_directory},dst={write_directory}"
        for argument in offline_command
    )
    assert not any(str(auth_file) in argument for argument in offline_command)
    assert not any("docker.sock" in argument for argument in offline_command)
    assert offline_command[-3:] == [
        "copy",
        "oci-archive:server.oci.tar",
        "docker-archive:server.docker.tar",
    ]
    assert run_pinned_skopeo.SKOPEO_IMAGE in registry_command
    assert "--network" not in registry_command
    assert registry_command[registry_command.index("--user") :][:2] == ["--user", "1234:5678"]
    assert f"REGISTRY_AUTH_FILE={run_pinned_skopeo.CONTAINER_AUTH_FILE}" in registry_command
    assert not any("docker.sock" in argument for argument in registry_command)
    assert any(str(auth_file) in argument for argument in registry_command)
    assert registry_command[-2:] == [
        "inspect",
        "docker://ghcr.io/ds1sqe/type-bridge-server:2.1.0",
    ]


def test_pinned_skopeo_runner_rejects_symlinked_registry_auth(tmp_path: Path) -> None:
    working_directory = tmp_path / "workspace"
    working_directory.mkdir()
    docker_config = tmp_path / "docker-config"
    docker_config.mkdir()
    real_auth = tmp_path / "real-auth.json"
    real_auth.write_text('{"auths": {}}\n', encoding="utf-8")
    (docker_config / "config.json").symlink_to(real_auth)

    with pytest.raises(run_pinned_skopeo.RunnerError, match="missing or unsafe"):
        run_pinned_skopeo.build_command(
            ["inspect", "docker://ghcr.io/ds1sqe/type-bridge-server:2.1.0"],
            registry_auth=True,
            working_directory=working_directory,
            environment={"DOCKER_CONFIG": str(docker_config)},
        )


def test_pinned_skopeo_runner_rejects_symlinked_write_directory(
    tmp_path: Path,
) -> None:
    working_directory = tmp_path / "workspace"
    working_directory.mkdir()
    real_output = tmp_path / "real-output"
    real_output.mkdir()
    linked_output = tmp_path / "linked-output"
    linked_output.symlink_to(real_output, target_is_directory=True)

    with pytest.raises(run_pinned_skopeo.RunnerError, match="write directory.*unsafe"):
        run_pinned_skopeo.build_command(
            ["copy", "oci-archive:server.oci.tar", "docker-archive:server.docker.tar"],
            registry_auth=False,
            working_directory=working_directory,
            environment={},
            write_directory=linked_output,
        )


def test_trivy_report_gate_is_fail_closed() -> None:
    source = TRIVY_VALIDATOR.read_text(encoding="utf-8")
    assert '"Vulnerabilities"' in source
    assert '"Secrets"' in source
    assert '"Misconfigurations"' in source
    assert "accepted server image has" in source


def test_release_builds_accepts_and_publishes_only_exact_oci_bytes() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    skopeo_runner = SKOPEO_RUNNER.read_text(encoding="utf-8")
    accept_job = workflow.split("\n  accept-server-oci:", maxsplit=1)[1].split(
        "\n  accept-live-artifact-parity:", maxsplit=1
    )[0]

    assert "build-server-oci:" in workflow
    assert "accept-server-oci:" in workflow
    assert "publish-server-oci:" in workflow
    assert "persist-credentials: false" in accept_job
    assert "linux/amd64" in workflow
    assert "linux/arm64" in workflow
    assert "type=oci,dest=${archive},oci-mediatypes=false" in workflow
    assert workflow.count("copy --preserve-digests") == 2
    assert workflow.count("scripts/ci/run_pinned_skopeo.py") == 3
    assert workflow.count('python "$TYPE_BRIDGE_SKOPEO_RUNNER" --registry-auth --') == 8
    assert "tmp/recovery-controls/scripts/ci/run_pinned_skopeo.py" in workflow
    assert "--daemon --" not in workflow
    assert workflow.count('--write-directory "$conversion_dir"') == 1
    assert workflow.count("--registry-auth --") == 8
    assert "apt-get install --yes --no-install-recommends skopeo" not in workflow
    assert run_pinned_skopeo.SKOPEO_IMAGE in skopeo_runner
    assert "/var/run/docker.sock" not in skopeo_runner
    assert '"docker-archive:${docker_archive}"' in accept_job
    assert 'docker load --input "$docker_archive"' in accept_job
    assert 'docker tag "$expected_config" "$image"' in accept_job
    assert "json .RootFS.Layers" in accept_job
    assert workflow.count(f"image={BUILDKIT_IMAGE}") == 2
    assert workflow.count(f"image: {TYPEDB_SERVICE_IMAGE}") == 2
    assert (
        workflow.count("sha256:f683eed7f07c2c519caa795afbc1cd4df4f83c6422ad2cc07406f28f7537423a")
        == 2
    )
    assert workflow.count('source_date_epoch="$(git show --no-patch --format=%ct') == 1
    assert workflow.count('date --utc --date="@${source_date_epoch}"') == 1
    assert workflow.count("'+%Y-%m-%dT%H:%M:%SZ'") == 1
    assert "%cI" not in workflow
    assert workflow.count('export SOURCE_DATE_EPOCH="$source_date_epoch"') == 1
    assert workflow.count('--build-arg "SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH}"') == 1
    assert "TYPE_BRIDGE_SERVER_IMAGE" in workflow
    assert "production_binary_serves_v1_health_and_v2_query" in workflow
    assert "docker/setup-qemu-action@" in workflow
    assert workflow.count(f"image: {BINFMT_IMAGE}") == 1
    assert "tonistiigi/binfmt:latest" not in workflow
    assert "anchore/sbom-action@" in workflow
    assert "aquasecurity/trivy-action@" in workflow
    assert "cosign sign --yes" in workflow
    assert workflow.count("actions/attest-build-provenance@") == 3
    assert workflow.count("actions/attest@e59cbc1ad1ac2d59339667419eb8cdde6eb61e3d") == 3
    assert workflow.count("actions/attest-sbom@") == 2
    assert "Refusing to overwrite conflicting immutable OCI tag." in workflow
    assert "Refusing to move conflicting OCI alias during recovery." in workflow
    assert (
        "SERVER_OCI_MINOR_ALIAS: ${{ github.event_name == 'workflow_dispatch' && "
        "inputs.release_channel == 'recovery' && '2.0' || '2.1' }}"
    ) in workflow
    assert 'for alias in "$SERVER_OCI_MINOR_ALIAS" 2 latest; do' in workflow
    assert '"aliases": [os.environ["SERVER_OCI_MINOR_ALIAS"], "2", "latest"],' in workflow
    assert "release.yml@refs/tags/v2[.]1[.]0$'" in workflow
    assert "release.yml@refs/tags/v2[.]0[.]0$'" not in workflow
    assert "for alias in 2.0 2 latest; do" not in workflow
    assert "|not found|" not in workflow
    assert "Stable OCI index conflicts with accepted platforms" in workflow
    assert "recovery-promotion.json" in workflow
    assert (
        workflow.count("https://github.com/ds1sqe/type-bridge/attestations/release-promotion/v1")
        == 3
    )
    assert "packages: write" in workflow
    assert workflow.count("packages: write") == 1
    assert "body_path: dist/server-oci-release.md" in workflow
