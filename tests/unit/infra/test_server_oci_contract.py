"""Static release contract for the TypeBridge server OCI image."""

from __future__ import annotations

import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
CORE = REPO_ROOT / "type-bridge-core"
DOCKERFILE = CORE / "Dockerfile.server"
CONTAINER_CONFIG = CORE / "server.container.toml"
DOCKERIGNORE = CORE / ".dockerignore"
VALIDATOR = REPO_ROOT / "scripts/ci/validate_server_oci_layout.py"
TRIVY_VALIDATOR = REPO_ROOT / "scripts/ci/validate_trivy_report.py"
RELEASE_WORKFLOW = REPO_ROOT / ".github/workflows/release.yml"


def test_server_image_build_and_runtime_are_immutable_and_least_privilege() -> None:
    source = DOCKERFILE.read_text(encoding="utf-8")

    assert (
        "rust:1.94.1-bookworm@"
        "sha256:6ae102bdbf528294bc79ad6e1fae682f6f7c2a6e6621506ba959f9685b308a55"
    ) in source
    assert (
        "debian:bookworm-slim@"
        "sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818"
    ) in source
    assert "cargo build --locked --release -p type-bridge-server --features v2-query" in source
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


def test_trivy_report_gate_is_fail_closed() -> None:
    source = TRIVY_VALIDATOR.read_text(encoding="utf-8")
    assert '"Vulnerabilities"' in source
    assert '"Secrets"' in source
    assert '"Misconfigurations"' in source
    assert "accepted server image has" in source


def test_release_builds_accepts_and_publishes_only_exact_oci_bytes() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")

    assert "build-server-oci:" in workflow
    assert "accept-server-oci:" in workflow
    assert "publish-server-oci:" in workflow
    assert "linux/amd64" in workflow
    assert "linux/arm64" in workflow
    assert "type=oci,dest=${archive},oci-mediatypes=false" in workflow
    assert workflow.count("skopeo copy --preserve-digests") == 2
    assert "TYPE_BRIDGE_SERVER_IMAGE" in workflow
    assert "production_binary_serves_v1_health_and_v2_query" in workflow
    assert "docker/setup-qemu-action@" in workflow
    assert "anchore/sbom-action@" in workflow
    assert "aquasecurity/trivy-action@" in workflow
    assert "cosign sign --yes" in workflow
    assert workflow.count("actions/attest-build-provenance@") == 3
    assert workflow.count("actions/attest@e59cbc1ad1ac2d59339667419eb8cdde6eb61e3d") == 3
    assert workflow.count("actions/attest-sbom@") == 2
    assert "Refusing to overwrite conflicting immutable OCI tag." in workflow
    assert "Refusing to move conflicting OCI alias during recovery." in workflow
    assert "|not found|" not in workflow
    assert "Stable OCI index conflicts with accepted platforms" in workflow
    assert "recovery-promotion.json" in workflow
    assert (
        workflow.count("https://github.com/ds1sqe/type-bridge/attestations/release-promotion/v1")
        == 3
    )
    assert "packages: write" in workflow
    assert workflow.count("packages: write") == 1
    assert (
        "body_path: ${{ github.event_name == 'push' && 'docs/guide/v2.0.2-notice.md' "
        "|| 'dist/server-oci-release.md' }}"
    ) in workflow
