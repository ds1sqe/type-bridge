"""Release publication must consume artifacts that passed compatibility gates."""

from __future__ import annotations

import json
import re
import tomllib
from pathlib import Path

import pytest
import yaml
from yaml.nodes import MappingNode, Node, ScalarNode, SequenceNode

REPO_ROOT = Path(__file__).resolve().parents[3]
CI_WORKFLOW = REPO_ROOT / ".github/workflows/ci.yml"
RELEASE_WORKFLOW = REPO_ROOT / ".github/workflows/release.yml"
CRATE_PUBLISH_HELPER = REPO_ROOT / "scripts/ci/publish_crate_idempotently.sh"
CRATE_RELEASE_GRAPH = REPO_ROOT / "scripts/ci/release_crates_graph.sh"
FRESH_RUNTIME_PROBE = REPO_ROOT / "scripts/ci/validate_fresh_typedb_runtime_package.sh"
RUST_RELEASE_ARTIFACT_VALIDATOR = REPO_ROOT / "scripts/ci/validate_rust_release_artifacts.py"
RECOVERY_VALIDATOR = REPO_ROOT / "scripts/ci/validate_release_recovery.py"
RECOVERY_PAYLOAD_VALIDATOR = REPO_ROOT / "scripts/ci/validate_release_recovery_payloads.py"
RECOVERY_MANIFEST = REPO_ROOT / ".github/release/v2.0.0-recovery.json"
RECOVERY_MANIFEST_SHA256 = "f8d5b2d04ad01a45694aecdd171846443bfd511a9363ab771e5f182c6bd17d2d"
STABLE_PUBLICATION_GUARD = "if: github.event_name == 'push' && github.ref == 'refs/tags/v2.1.0'"
QEMU_ACTION = "docker/setup-qemu-action@c7c53464625b32c7a7e944ae62b3e17d2b600130"
QEMU_BINFMT_IMAGE = (
    "docker.io/tonistiigi/binfmt@"
    "sha256:400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0"
)
RECOVERY_MUTATING_JOBS = (
    "publish-server-oci",
    "publish-node-npm",
    "publish-core-pypi",
    "publish-python-pypi",
    "github-release",
)
MUTATING_RELEASE_JOBS = ("publish-crates", *RECOVERY_MUTATING_JOBS)
CARGO_PUBLICATION_MARKERS = (
    "publish-crates:",
    "CARGO_REGISTRY_TOKEN",
    "release_crates_graph",
    "cargo_release_candidate.py",
    "publish_cargo_release_candidate.py",
    "cargo-release-candidate",
    '"${cargo_command[@]}" package',
    "patch.crates-io",
    "--verify-preexisting",
    "type-bridge-typedb-protocol-b8",
    "type-bridge-typedb-driver-b8",
)


def test_root_python_artifacts_exclude_the_native_workspace() -> None:
    """The MIT facade must not redistribute native compatibility source trees."""
    pyproject = tomllib.loads((REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    targets = pyproject["tool"]["hatch"]["build"]["targets"]

    assert targets["wheel"]["packages"] == ["type_bridge"]
    assert targets["sdist"]["only-include"] == ["type_bridge"]


def test_core_build_excludes_generated_python_bytecode() -> None:
    """A dirty developer checkout must produce the same clean core artifacts as CI."""
    pyproject = tomllib.loads(
        (REPO_ROOT / "type-bridge-core/pyproject.toml").read_text(encoding="utf-8")
    )
    excluded = set(pyproject["tool"]["maturin"]["exclude"])
    assert {
        "python/**/__pycache__/**",
        "python/**/*.pyc",
        "python/**/*.pyo",
    } <= excluded


def test_core_sdist_includes_optional_rust_path_dependency() -> None:
    """The core sdist must remain loadable by Cargo's metadata resolver."""
    pyproject = tomllib.loads(
        (REPO_ROOT / "type-bridge-core/pyproject.toml").read_text(encoding="utf-8")
    )
    included = pyproject["tool"]["maturin"]["include"]

    assert {
        "path": "crates/orm-derive/**/*",
        "format": "sdist",
    } in included
    assert {"path": "LICENSE", "format": "sdist"} in included


def test_core_artifact_builders_pin_the_validated_maturin_contract() -> None:
    """All release builders must retain identical sdist transformation semantics."""
    pyproject = tomllib.loads(
        (REPO_ROOT / "type-bridge-core/pyproject.toml").read_text(encoding="utf-8")
    )
    ci = CI_WORKFLOW.read_text(encoding="utf-8")
    release = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    action = "PyO3/maturin-action@e83996d129638aa358a18fbd1dfb82f0b0fb5d3b"
    version = "maturin-version: v1.14.1"
    rust_toolchain = "rust-toolchain: 1.94.1"

    assert pyproject["build-system"]["requires"] == ["maturin==1.14.1"]
    assert ci.count(action) == 1
    assert ci.count(version) == 1
    assert 'RUST_STABLE_TOOLCHAIN: "1.94.1"' in ci
    ci_step = ci.split(action, maxsplit=1)[1].split("\n      - name:", maxsplit=1)[0]
    assert "rust-toolchain: ${{ env.RUST_STABLE_TOOLCHAIN }}" in ci_step
    assert release.count(action) == 2
    assert release.count(version) == 2
    assert release.count(rust_toolchain) == 2
    release_steps = [
        suffix.split("\n      - name:", maxsplit=1)[0] for suffix in release.split(action)[1:]
    ]
    assert all(version in step and rust_toolchain in step for step in release_steps)


def test_qemu_actions_pin_the_exact_binfmt_runtime() -> None:
    """Cross-platform build and acceptance jobs must not inherit a mutable image."""
    for workflow_path in (CI_WORKFLOW, RELEASE_WORKFLOW):
        workflow = workflow_path.read_text(encoding="utf-8")
        assert workflow.count(QEMU_ACTION) == 1
        qemu_step = workflow.split(QEMU_ACTION, maxsplit=1)[1].split("\n      - name:", maxsplit=1)[
            0
        ]
        assert f"image: {QEMU_BINFMT_IMAGE}" in qemu_step
        assert "tonistiigi/binfmt:latest" not in workflow


def test_core_metadata_advertises_only_the_supported_python_implementation() -> None:
    pyproject = tomllib.loads(
        (REPO_ROOT / "type-bridge-core/pyproject.toml").read_text(encoding="utf-8")
    )
    classifiers = set(pyproject["project"]["classifiers"])

    assert "Programming Language :: Python :: Implementation :: CPython" in classifiers
    assert "Programming Language :: Python :: Implementation :: PyPy" not in classifiers


def test_supported_python_range_is_declared_and_exercised() -> None:
    root = tomllib.loads((REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    core = tomllib.loads(
        (REPO_ROOT / "type-bridge-core/pyproject.toml").read_text(encoding="utf-8")
    )
    expected_versions = {
        "Programming Language :: Python :: 3.12",
        "Programming Language :: Python :: 3.13",
        "Programming Language :: Python :: 3.14",
    }

    assert root["project"]["requires-python"] == ">=3.12,<3.15"
    assert core["project"]["requires-python"] == ">=3.12,<3.15"
    assert expected_versions <= set(root["project"]["classifiers"])
    assert expected_versions <= set(core["project"]["classifiers"])
    assert "Development Status :: 5 - Production/Stable" in set(root["project"]["classifiers"])
    assert "Development Status :: 3 - Alpha" not in set(root["project"]["classifiers"])
    assert "typing-extensions>=4.12" in root["project"]["dependencies"]

    ci = CI_WORKFLOW.read_text(encoding="utf-8")
    expected_matrix = 'python-version: ["3.12", "3.13.5", "3.14"]'
    assert expected_matrix in job_block(ci, "python-generated-package-compat")
    assert expected_matrix in job_block(ci, "test-unit")

    release = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    assert expected_matrix in job_block(release, "accept-python-artifacts")


def test_pyo3_extension_link_mode_is_enabled_only_for_wheel_builds() -> None:
    """Rust test binaries link libpython while maturin still builds an extension."""
    cargo = tomllib.loads(
        (REPO_ROOT / "type-bridge-core/crates/python/Cargo.toml").read_text(encoding="utf-8")
    )
    pyproject = tomllib.loads(
        (REPO_ROOT / "type-bridge-core/pyproject.toml").read_text(encoding="utf-8")
    )

    assert "extension-module" not in cargo["dependencies"]["pyo3"]["features"]
    assert "abi3-py312" in cargo["dependencies"]["pyo3"]["features"]
    assert "pyo3/extension-module" in pyproject["tool"]["maturin"]["features"]


def job_block(workflow: str, name: str) -> str:
    """Return one top-level release job without requiring a YAML dependency."""
    jobs = workflow.split("\njobs:\n", maxsplit=1)[1]
    header = re.search(rf"^  {re.escape(name)}:\n", jobs, re.MULTILINE)
    assert header is not None, f"release.yml job {name!r} is missing"
    next_header = re.search(r"^  [a-z][a-z0-9-]*:\n", jobs[header.end() :], re.MULTILINE)
    end = header.end() + next_header.start() if next_header is not None else len(jobs)
    return jobs[header.start() : end]


def needs_line(block: str) -> str:
    """Return the dependency declaration for a release job."""
    match = re.search(r"^    needs: .+$", block, re.MULTILINE)
    assert match is not None, "release job has no needs declaration"
    return match.group()


def assert_stable_only_release_mutations(workflow: str) -> None:
    """Require every publication to use the exact tag or pinned recovery path."""
    for name in RECOVERY_MUTATING_JOBS:
        block = job_block(workflow, name)
        assert block.count("    if: >-\n") == 1
        assert "      always() &&\n      !cancelled() &&\n" in block
        assert "github.event_name == 'push'" in block
        assert "github.ref == 'refs/tags/v2.1.0'" in block
        assert "github.event_name == 'workflow_dispatch'" in block
        assert "github.ref == 'refs/heads/master'" in block
        assert "inputs.release_channel == 'recovery'" in block
        assert "inputs.recovery_mode == 'publish'" in block
        assert "inputs.recovery_run_id == '30612912483'" in block
        assert "needs.recovery-preflight.result == 'success'" in block
        assert "inputs.release_channel == 'candidate'" not in block

    cargo = job_block(workflow, "publish-crates")
    assert cargo.count("    if: >-\n") == 1
    assert "      always() &&\n      !cancelled() &&\n" in cargo
    assert "github.event_name == 'push'" in cargo
    assert "github.ref == 'refs/tags/v2.1.0'" in cargo
    assert "github.event_name == 'workflow_dispatch'" not in cargo
    assert "needs.release-tag-preflight.result == 'success'" in cargo
    assert "needs.publish-node-npm.result == 'success'" in cargo

    for name in MUTATING_RELEASE_JOBS:
        block = job_block(workflow, name)
        assert "release-tag-preflight" in needs_line(block)
        assert "EXPECTED_RELEASE_TAG_OBJECT" in block
        assert 'test "$tag_object" = "$EXPECTED_RELEASE_TAG_OBJECT"' in block

    publication_markers = {
        "publish_cargo_release_candidate.py publish": "publish-crates",
        "npm publish": "publish-node-npm",
        "pypa/gh-action-pypi-publish": (
            "publish-core-pypi",
            "publish-python-pypi",
        ),
        "softprops/action-gh-release": "github-release",
    }
    for marker, owners in publication_markers.items():
        expected_owners = (owners,) if isinstance(owners, str) else owners
        containing_jobs = tuple(
            name for name in MUTATING_RELEASE_JOBS if marker in job_block(workflow, name)
        )
        assert containing_jobs == expected_owners
        owned_count = sum(job_block(workflow, name).count(marker) for name in expected_owners)
        assert workflow.count(marker) == owned_count


def assert_no_cargo_publication_path(workflow: str) -> None:
    """The ordinary 2.0 release contract publishes only Python and npm."""
    for marker in CARGO_PUBLICATION_MARKERS:
        assert marker not in workflow


def workflow_files(root: Path) -> list[Path]:
    """Return every supported GitHub workflow filename deterministically."""
    workflow_dir = root / ".github/workflows"
    return sorted((*workflow_dir.glob("*.yml"), *workflow_dir.glob("*.yaml")))


def official_action_references(paths: list[Path]) -> list[tuple[str, str]]:
    """Decode every YAML uses node before checking official commit pins."""
    official_action = re.compile(
        r"(?P<action>(?i:actions/[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*))@"
        r"(?P<reference>[0-9a-f]{40})"
    )
    references: list[tuple[str, str]] = []

    for path in paths:
        root = yaml.compose(path.read_text(encoding="utf-8"), Loader=yaml.SafeLoader)
        assert root is not None, f"{path}: workflow must not be empty"
        visited: set[int] = set()

        def visit(node: Node) -> None:
            node_id = id(node)
            if node_id in visited:
                return
            visited.add(node_id)

            if isinstance(node, MappingNode):
                for key, value in node.value:
                    if isinstance(key, ScalarNode) and key.value == "uses":
                        assert isinstance(value, ScalarNode), (
                            f"{path}:{value.start_mark.line + 1}: uses must be a scalar"
                        )
                        owner, separator, _ = value.value.partition("/")
                        if separator and owner.casefold() == "actions":
                            action_match = official_action.fullmatch(value.value)
                            assert action_match is not None, (
                                f"{path}:{value.start_mark.line + 1}: official actions must "
                                "use a 40-character lowercase commit SHA"
                            )
                            references.append(
                                (
                                    action_match.group("action"),
                                    action_match.group("reference"),
                                )
                            )
                    visit(key)
                    visit(value)
            elif isinstance(node, SequenceNode):
                for value in node.value:
                    visit(value)

        visit(root)

    return references


def test_official_action_steps_are_commit_pinned() -> None:
    action_references = official_action_references(workflow_files(REPO_ROOT))
    assert action_references

    ci = CI_WORKFLOW.read_text(encoding="utf-8")
    rust_integration = job_block(ci, "rust-integration")
    tls_matrix = job_block(ci, "tls-transport-matrix")
    release = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    preflight = job_block(release, "channel-preflight")

    assert "actions/setup-python@a26af69be951a213d495a4c3e4e4022e16d87065 # v5" in rust_integration
    assert "actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4" in tls_matrix
    assert "actions/cache@0057852bfaa89a56745cba8c7296529d2fc39830 # v4" in tls_matrix
    assert "actions/checkout@v4" not in tls_matrix
    assert "actions/cache@v4" not in tls_matrix

    assert "actions/checkout@11d5960a326750d5838078e36cf38b85af677262 # v4" in preflight
    assert "actions/checkout@v4" not in preflight
    assert "actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020 # v4" in preflight
    assert "actions/setup-node@v4" not in preflight


@pytest.mark.parametrize(
    ("filename", "source"),
    [
        (
            "quoted.yml",
            'jobs:\n  test:\n    steps:\n      - uses: "actions/checkout@v4"\n',
        ),
        (
            "escaped.yaml",
            'jobs:\n  test:\n    steps:\n      - "u\\u0073es": "actions\\u002fcheckout@v4"\n',
        ),
        (
            "block.yaml",
            "jobs:\n  test:\n    steps:\n      - uses: >\n          actions/checkout@v4\n",
        ),
        (
            "case.yml",
            "jobs:\n  test:\n    steps:\n      - uses: Actions/checkout@v4\n",
        ),
        (
            "alias.yaml",
            "mutable: &mutable actions/checkout@v4\n"
            "jobs:\n  test:\n    steps:\n      - uses: *mutable\n",
        ),
    ],
)
def test_official_action_pin_gate_rejects_yaml_scalar_evasions(
    tmp_path: Path,
    filename: str,
    source: str,
) -> None:
    workflow_dir = tmp_path / ".github/workflows"
    workflow_dir.mkdir(parents=True)
    (workflow_dir / filename).write_text(source, encoding="utf-8")

    paths = workflow_files(tmp_path)
    assert [path.name for path in paths] == [filename]
    with pytest.raises(AssertionError, match="40-character lowercase commit SHA"):
        official_action_references(paths)


def test_python_builds_upload_only_the_exact_distribution_files() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")

    core_wheels = job_block(workflow, "build-core-wheels")
    assert "name: ${{ matrix.artifact }}" in core_wheels
    assert "path: type-bridge-core/dist/*.whl" in core_wheels
    assert "if-no-files-found: error" in core_wheels

    core_sdist = job_block(workflow, "build-core-sdist")
    assert "name: core-sdist" in core_sdist
    assert "path: type-bridge-core/dist/*.tar.gz" in core_sdist
    assert "if-no-files-found: error" in core_sdist

    root = job_block(workflow, "build-python")
    assert "name: python-dist" in root
    assert "dist/*.whl" in root
    assert "dist/*.tar.gz" in root
    assert "path: dist/" not in root
    assert "if-no-files-found: error" in root


def test_python_publication_depends_on_exact_artifact_acceptance() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    acceptance = job_block(workflow, "accept-python-artifacts")
    pyproject = tomllib.loads((REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8"))

    assert pyproject["project"]["requires-python"] == ">=3.12,<3.15"
    assert "needs: [build-core-wheels, build-core-sdist, build-python]" in acceptance
    assert 'python-version: ["3.12", "3.13.5", "3.14"]' in acceptance
    assert "PYO3_USE_ABI3_FORWARD_COMPATIBILITY" in acceptance
    assert "pattern: core-wheels-*" in acceptance
    assert "merge-multiple: true" in acceptance
    assert "name: core-sdist" in acceptance
    assert "name: python-dist" in acceptance
    assert "name: generated-python-live-fixture" in acceptance
    assert "scripts/ci/validate_python_release_artifacts.py" in acceptance
    assert "--core-wheels-dir tmp/release-python-artifacts/core-wheels" in acceptance
    assert "--core-sdist-dir tmp/release-python-artifacts/core-sdist" in acceptance
    assert "--root-dist-dir tmp/release-python-artifacts/root" in acceptance
    assert '--expected-version "$version"' in acceptance
    assert "'auditwheel==6.7.0'" in acceptance
    assert "scripts/ci/audit_manylinux_release_wheels.py" in acceptance
    assert "--manifest tmp/release-python-artifacts/manifest.json" in acceptance

    validator_position = acceptance.index("scripts/ci/validate_python_release_artifacts.py")
    auditor_position = acceptance.index("scripts/ci/audit_manylinux_release_wheels.py")
    execution_position = acceptance.index("scripts/ci/run_generated_python_artifact.py")
    assert validator_position < auditor_position < execution_position
    assert acceptance.count("scripts/ci/run_generated_python_artifact.py") == 1
    assert "--generated-stage tmp/release-python-artifacts/generated" in acceptance
    assert "runtime_check.py" in acceptance
    assert "release-generated-sdist" in acceptance
    assert "run_legacy_python_compat.py" not in acceptance
    assert "run_typed_python_artifact.py" not in acceptance
    assert "type_bridge_core-*linux*x86_64.whl" in acceptance
    assert "type_bridge_core-*.tar.gz" in acceptance
    assert "type_bridge-*.tar.gz" in acceptance
    assert '"${root_wheels[0]}[typedb-driver]"' in acceptance
    assert "tests/compat/typedb_driver_native/probe.py" in acceptance

    assert "Run generated-only CLI wheel acceptance" in acceptance
    assert '"$cli_venv/bin/type-bridge" schema --help' in acceptance
    assert '"$cli_venv/bin/type-bridge" migration --help' in acceptance
    assert '"$cli_venv/bin/type-bridge" plan --help' not in acceptance
    assert '"$cli_venv/bin/type-bridge" makemigrations --help' not in acceptance
    assert "type-bridge-migration" not in acceptance
    assert '--manifest="$workspace/typebridge.yaml" --version' in acceptance
    assert '--manifest "$workspace/typebridge.yaml" -V' in acceptance
    assert "artifacts:" in acceptance
    assert "schema-authority:" in acceptance
    assert "output: generated/schema-authority.json" in acceptance
    assert 'test -s "$workspace/generated/schema-authority.json"' in acceptance
    assert '"$cli_venv/bin/type-bridge" schema export-declared' not in acceptance
    assert "declared-schema.json" not in acceptance

    generated_runner = (REPO_ROOT / "scripts/ci/run_generated_python_artifact.py").read_text(
        encoding="utf-8"
    )
    assert '"pythonVersion": python_version' in generated_runner
    assert '"pythonVersion": "3.13"' not in generated_runner
    assert 'generated_root / "schema-authority.json"' in generated_runner
    assert "declared-schema.json" not in generated_runner
    assert "uv build" not in acceptance
    assert "actions/upload-artifact" not in acceptance

    core_publish = job_block(workflow, "publish-core-pypi")
    root_publish = job_block(workflow, "publish-python-pypi")
    preflight = job_block(workflow, "channel-preflight")
    assert "accept-python-artifacts" in needs_line(preflight)
    assert "publish-server-oci" in needs_line(core_publish)
    assert "publish-core-pypi" in needs_line(root_publish)
    assert "pattern: core-wheels-*" in core_publish
    assert "merge-multiple: true" in core_publish
    assert "name: core-sdist" in core_publish
    assert "name: python-dist" in root_publish

    github_release = job_block(workflow, "github-release")
    assert "pattern: core-wheels-*" in github_release
    assert "name: core-sdist" in github_release
    assert "name: python-dist" in github_release


def test_python_facade_and_core_release_in_exact_resolver_lockstep() -> None:
    pyproject = tomllib.loads((REPO_ROOT / "pyproject.toml").read_text(encoding="utf-8"))
    version = pyproject["project"]["version"]
    core_requirements = [
        requirement
        for requirement in pyproject["project"]["dependencies"]
        if re.match(r"(?i)^type[-_.]bridge[-_.]core", requirement)
    ]
    assert core_requirements == [f"type-bridge-core=={version}"]

    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    acceptance = job_block(workflow, "accept-python-artifacts")
    resolver = acceptance[
        acceptance.index("Prove Python facade/core resolver lockstep") : acceptance.index(
            "Prepare generated consumer interpreter and Pyright"
        )
    ]
    candidate_install = (
        'uv pip install --python "$resolver_venv/bin/python" \\\n'
        "            --find-links tmp/release-python-artifacts/core-wheels \\\n"
        '            "${root_wheels[0]}"'
    )
    assert resolver.count(candidate_install) == 1
    assert "type-bridge-core==1.5.11" not in resolver
    assert "type_bridge_core-*.whl" not in resolver
    assert "importlib.metadata.version(distribution)" in resolver
    assert 'for distribution in ("type-bridge", "type-bridge-core")' in resolver
    assert '"$resolver_venv/bin/type-bridge" schema --help' in resolver


def test_cutover_artifacts_do_not_pair_a_published_handwritten_root_with_candidate_core() -> None:
    acceptance = job_block(
        RELEASE_WORKFLOW.read_text(encoding="utf-8"),
        "accept-python-artifacts",
    )

    for removed_probe in (
        "Prove published V1 root compatibility with candidate core",
        "type-bridge==1.5.11",
        "validate_released_python_root.py",
        "run_legacy_python_compat.py",
        "released-root-candidate-core",
    ):
        assert removed_probe not in acceptance
    assert "run_generated_python_artifact.py" in acceptance
    assert "--generated-stage tmp/release-python-artifacts/generated" in acceptance


def test_release_identity_gate_receives_all_lockstep_authorities() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    identity = job_block(workflow, "validate-release-identity")

    assert "--artifact-contract cargo-inclusive" in identity
    assert '--release-channel "$RELEASE_CHANNEL"' in identity
    assert '--tag "$RELEASE_TAG"' in identity
    assert "--root-python pyproject.toml" in identity
    assert "--root-python-init type_bridge/__init__.py" in identity
    assert "--node-package type-bridge-core/crates/node/package.json" in identity
    assert "--node-package-lock type-bridge-core/crates/node/package-lock.json" in identity


def test_artifact_builds_cannot_start_before_release_identity_passes() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")

    core_wheels = job_block(workflow, "build-core-wheels")
    assert needs_line(core_wheels) == "    needs: [validate-release-identity, build-python]"
    for job in ("build-core-sdist", "build-python", "build-node-native"):
        assert needs_line(job_block(workflow, job)) == "    needs: validate-release-identity"


def test_each_core_wheel_executes_v2_authoring_on_its_native_target() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    build = job_block(workflow, "build-core-wheels")
    legs = re.findall(
        r"^          - target: (.+)\n"
        r"^            runner: (.+)\n"
        r"^            artifact: (.+)$",
        build,
        re.MULTILINE,
    )

    assert legs == [
        (
            "x86_64-unknown-linux-gnu",
            "ubuntu-latest",
            "core-wheels-linux-x86_64",
        ),
        (
            "aarch64-unknown-linux-gnu",
            "ubuntu-24.04-arm",
            "core-wheels-linux-aarch64",
        ),
        (
            "x86_64-apple-darwin",
            "macos-15-intel",
            "core-wheels-macos-x86_64",
        ),
        (
            "aarch64-apple-darwin",
            "macos-14",
            "core-wheels-macos-aarch64",
        ),
        (
            "x86_64-pc-windows-msvc",
            "windows-latest",
            "core-wheels-windows-x86_64",
        ),
    ]
    assert "docker/setup-qemu-action" not in build
    assert "if: runner.os != 'Linux'" not in build
    assert "name: python-dist" in build
    assert "path: tmp/python-v2-platform-root" in build
    runner = "scripts/ci/run_python_v2_platform_artifact.py"
    assert runner in build
    for argument in (
        "--root-dist-dir tmp/python-v2-platform-root",
        "--core-dist-dir type-bridge-core/dist",
        '--work-dir "${{ runner.temp }}/type-bridge-v2-platform-artifact"',
        '--expected-version "${{ env.PYTHON_RELEASE_VERSION }}"',
        '--source-root "${{ github.workspace }}"',
        "tests/fixtures/query-v2-model-remote-parity-declared.json",
    ):
        assert argument in build
    assert build.index("name: Build wheel") < build.index(runner)
    assert build.index(runner) < build.index("name: Upload wheel artifact")


def test_release_channels_have_fixed_non_attacker_controlled_identities() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    preamble = workflow.split("\njobs:\n", maxsplit=1)[0]

    assert (
        "on:\n"
        "  push:\n"
        "    tags:\n"
        "      - 'v2.1.0'\n"
        "  workflow_dispatch:\n"
        "    inputs:\n"
        "      release_channel:\n"
        "        description: Validate the 2.1.0 release identity or recover "
        "the accepted v2.0.0 tag run\n"
        "        required: true\n"
        "        type: choice\n"
        "        default: candidate\n"
        "        options:\n"
        "          - candidate\n"
        "          - stable\n"
        "          - recovery\n"
        "      recovery_run_id:\n"
        "        description: Exact failed v2.0.0 tag run; required only for recovery\n"
        "        required: false\n"
        "        type: string\n"
        "        default: ''\n"
        "      recovery_mode:\n"
        "        description: Verify recovery inputs without mutation before "
        "explicitly publishing\n"
        "        required: false\n"
        "        type: choice\n"
        "        default: verify\n"
        "        options:\n"
        "          - verify\n"
        "          - publish\n"
    ) in preamble
    assert "'v*'" not in preamble
    assert (
        "RELEASE_TAG: ${{ github.event_name == 'workflow_dispatch' && "
        "inputs.release_channel == 'recovery' && 'v2.0.0' || 'v2.1.0' }}"
    ) in preamble
    assert (
        "RELEASE_VERSION: ${{ github.event_name == 'workflow_dispatch' && "
        "inputs.release_channel == 'recovery' && '2.0.0' || '2.1.0' }}"
    ) in preamble
    assert (
        "PYTHON_RELEASE_VERSION: ${{ github.event_name == 'workflow_dispatch' && "
        "inputs.release_channel == 'recovery' && '2.0.0' || '2.1.0' }}"
    ) in preamble
    assert (
        "SERVER_OCI_MINOR_ALIAS: ${{ github.event_name == 'workflow_dispatch' && "
        "inputs.release_channel == 'recovery' && '2.0' || '2.1' }}"
    ) in preamble
    assert (
        "RELEASE_CHANNEL: ${{ github.event_name == 'workflow_dispatch' "
        "&& inputs.release_channel || 'stable' }}"
    ) in preamble
    assert (
        "RELEASE_REVISION: ${{ github.event_name == 'workflow_dispatch' && "
        "inputs.release_channel == 'recovery' && "
        "'aacf4d16486a3a3bae47c3b10c1d526c587dd7a7' || github.sha }}"
    ) in preamble
    assert (
        "RELEASE_ARTIFACT_RUN_ID: ${{ github.event_name == 'workflow_dispatch' && "
        "inputs.release_channel == 'recovery' && inputs.recovery_run_id || github.run_id }}"
    ) in preamble
    assert f"RELEASE_RECOVERY_MANIFEST_SHA256: {RECOVERY_MANIFEST_SHA256}" in preamble
    assert workflow.count("RELEASE_TAG:") == 1
    assert workflow.count("\n  RELEASE_VERSION:") == 1
    assert workflow.count("PYTHON_RELEASE_VERSION:") == 1
    assert workflow.count("RELEASE_CHANNEL:") == 1
    assert preamble.count("inputs.release_channel") == 7
    assert "GITHUB_REF_NAME" not in workflow
    assert "github.ref_name" not in workflow
    assert "RELEASE_TAG#v" not in workflow
    assert workflow.count('version="$PYTHON_RELEASE_VERSION"') == 7

    pack = job_block(workflow, "pack-node-package")
    publish = job_block(workflow, "publish-node-npm")
    assert "validator_args+=(--allow-prerelease)" in pack
    assert "--allow-prerelease" not in publish


def test_recovery_preflight_is_pinned_to_the_failed_exact_tag_run() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    recovery = job_block(workflow, "recovery-preflight")
    test_job = job_block(workflow, "test")

    assert (
        "if: github.event_name != 'workflow_dispatch' || inputs.release_channel != 'recovery'"
    ) in test_job
    assert "github.event_name == 'workflow_dispatch'" in recovery
    assert "github.ref == 'refs/heads/master'" in recovery
    assert "inputs.release_channel == 'recovery'" in recovery
    assert "inputs.recovery_mode == 'publish'" not in recovery
    assert "ref: refs/tags/v2.0.0" in recovery
    assert "fetch-depth: 0" in recovery
    assert "path: tmp/release-source" in recovery
    assert 'test "$RECOVERY_RUN_ID" = "30612912483"' in recovery
    assert "cat-file -t refs/tags/v2.0.0" in recovery
    assert "a4cec6478ad4e764f039e51eabcbb68d45efd45a" in recovery
    assert "refs/tags/v2.0.0^{}" in recovery
    assert "aacf4d16486a3a3bae47c3b10c1d526c587dd7a7" in recovery
    for endpoint in (
        'actions/runs/${RECOVERY_RUN_ID}"',
        'actions/runs/${RECOVERY_RUN_ID}/jobs?per_page=100"',
        'actions/runs/${RECOVERY_RUN_ID}/artifacts?per_page=100"',
    ):
        assert endpoint in recovery
    assert "pattern: '*'" in recovery
    assert "merge-multiple: false" in recovery
    assert "github-token: ${{ secrets.GITHUB_TOKEN }}" in recovery
    assert "run-id: ${{ inputs.recovery_run_id }}" in recovery
    assert "python scripts/ci/validate_release_recovery.py" in recovery
    assert "--manifest .github/release/v2.0.0-recovery.json" in recovery
    assert '--expected-manifest-sha256 "$RELEASE_RECOVERY_MANIFEST_SHA256"' in recovery
    assert "--artifact-root tmp/recovery-artifacts" in recovery
    assert "name: release-recovery-evidence" in recovery
    assert "path: tmp/recovery-release-evidence" in recovery
    assert "validation-summary.json" in recovery
    assert "source-run.json" in recovery
    assert "source-jobs.json" in recovery
    assert "source-artifacts.json" in recovery
    assert 'test "$authenticated_user" = "ds1sqe"' in recovery
    assert "npm access list" not in recovery
    assert "npm publish" not in recovery
    assert "skopeo copy" not in recovery
    assert "gh-action-pypi-publish" not in recovery
    assert RECOVERY_VALIDATOR.is_file()
    assert RECOVERY_PAYLOAD_VALIDATOR.is_file()
    assert RECOVERY_MANIFEST.is_file()


def test_recovery_publishers_reuse_only_source_run_artifacts() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")

    for job in RECOVERY_MUTATING_JOBS:
        publish = job_block(workflow, job)
        assert "ref: ${{ env.RELEASE_REVISION }}" in publish
        assert "persist-credentials: false" in publish
        assert "name: Checkout recovery payload controls" in publish
        assert "ref: ${{ github.sha }}" in publish
        assert "path: tmp/recovery-controls" in publish
        assert "validate_release_recovery_payloads.py" in publish
        assert "tmp/recovery-controls/.github/release/v2.0.0-recovery.json" in publish
        assert '--expected-manifest-sha256 "$RELEASE_RECOVERY_MANIFEST_SHA256"' in publish
        assert publish.count("name: Revalidate immutable release tag") == 1
        assert 'gh api "repos/${GITHUB_REPOSITORY}/git/ref/tags/${RELEASE_TAG}"' in publish
        assert 'test "$(jq -r \'.object.type\' <<<"$tag_ref_json")" = tag' in publish
        assert 'gh api "repos/${GITHUB_REPOSITORY}/git/tags/${tag_object}"' in publish
        assert 'test "$(jq -r \'.object.type\' <<<"$tag_json")" = commit' in publish
        assert 'test "$(jq -r \'.object.sha\' <<<"$tag_json")" = "$RELEASE_REVISION"' in publish
        assert "EXPECTED_RELEASE_TAG_OBJECT" in publish
        assert 'test "$tag_object" = "$EXPECTED_RELEASE_TAG_OBJECT"' in publish
        assert "actions: read" in publish
        assert "github.event_name == 'workflow_dispatch'" in publish
        assert "github.ref == 'refs/heads/master'" in publish
        assert "inputs.release_channel == 'recovery'" in publish
        assert "inputs.recovery_mode == 'publish'" in publish
        assert "inputs.recovery_run_id == '30612912483'" in publish
        assert "needs.recovery-preflight.result == 'success'" in publish
        assert "RELEASE_ARTIFACT_RUN_ID" in publish

    recovery = job_block(workflow, "recovery-preflight")
    assert "a4cec6478ad4e764f039e51eabcbb68d45efd45a" in recovery

    frozen = job_block(workflow, "release-tag-preflight")
    assert needs_line(frozen) == "    needs: [channel-preflight, recovery-preflight]"
    assert "tag_object: ${{ steps.freeze-tag.outputs.tag_object }}" in frozen
    assert "a4cec6478ad4e764f039e51eabcbb68d45efd45a" in frozen
    assert "inputs.recovery_mode == 'publish'" in frozen

    node = job_block(workflow, "publish-node-npm")
    server = job_block(workflow, "publish-server-oci")
    core = job_block(workflow, "publish-core-pypi")
    root = job_block(workflow, "publish-python-pypi")
    release = job_block(workflow, "github-release")
    assert needs_line(node) == (
        "    needs: [channel-preflight, recovery-preflight, release-tag-preflight]"
    )
    assert "publish-node-npm" in needs_line(server)
    assert "publish-server-oci" in needs_line(core)
    assert "publish-core-pypi" in needs_line(root)
    assert "publish-python-pypi" in needs_line(release)
    assert "run-id: ${{ env.RELEASE_ARTIFACT_RUN_ID }}" in node
    assert server.count("run-id: ${{ env.RELEASE_ARTIFACT_RUN_ID }}") == 2
    assert core.count("run-id: ${{ env.RELEASE_ARTIFACT_RUN_ID }}") == 2
    assert root.count("run-id: ${{ env.RELEASE_ARTIFACT_RUN_ID }}") == 1
    assert release.count("run-id: ${{ env.RELEASE_ARTIFACT_RUN_ID }}") == 4

    expected_payloads = {
        "publish-server-oci": {
            "server-oci-accepted-amd64",
            "server-oci-accepted-arm64",
        },
        "publish-node-npm": {"node-package"},
        "publish-core-pypi": {
            "core-wheels-linux-aarch64",
            "core-wheels-linux-x86_64",
            "core-wheels-macos-aarch64",
            "core-wheels-macos-x86_64",
            "core-wheels-windows-x86_64",
            "core-sdist",
        },
        "publish-python-pypi": {"python-dist"},
        "github-release": {
            "core-wheels-linux-aarch64",
            "core-wheels-linux-x86_64",
            "core-wheels-macos-aarch64",
            "core-wheels-macos-x86_64",
            "core-wheels-windows-x86_64",
            "core-sdist",
            "python-dist",
            "node-package",
        },
    }
    mutation_markers = {
        "publish-server-oci": "- name: Publish exact accepted platform manifests",
        "publish-node-npm": "- name: Publish to npm registry",
        "publish-core-pypi": "- name: Publish to PyPI",
        "publish-python-pypi": "- name: Publish to PyPI",
        "github-release": "- name: Create draft release",
    }
    for job, expected in expected_payloads.items():
        publish = job_block(workflow, job)
        selected = set(
            re.findall(
                r"^\s+--artifact ([A-Za-z0-9._-]+)(?: \\)?$",
                publish,
                re.MULTILINE,
            )
        )
        assert selected == expected
        payload_position = publish.index("validate_release_recovery_payloads.py")
        tag_position = publish.index("name: Revalidate immutable release tag")
        mutation_position = publish.index(mutation_markers[job])
        assert payload_position < tag_position < mutation_position


def test_recovery_metadata_preserves_release_source_and_exact_tag() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    server = job_block(workflow, "publish-server-oci")
    release = job_block(workflow, "github-release")

    assert '"revision": os.environ["RELEASE_REVISION"]' in server
    assert "release.yml@refs/heads/master$" in server
    assert "release.yml@refs/tags/v2[.]1[.]0$" in server
    assert "release.yml@refs/tags/v2[.]0[.]0$" not in server
    for name in (
        "Attest amd64 build provenance",
        "Attest arm64 build provenance",
        "Attest multi-platform provenance",
    ):
        suffix = server.split(f"- name: {name}\n", maxsplit=1)[1]
        step = suffix.split("\n      - name:", maxsplit=1)[0]
        assert "if: github.event_name == 'push'" in step
        assert "actions/attest-build-provenance@" in step
    assert server.count("actions/attest@e59cbc1ad1ac2d59339667419eb8cdde6eb61e3d") == 3
    assert server.count("if: github.event_name == 'workflow_dispatch'") >= 5
    assert (
        server.count("https://github.com/ds1sqe/type-bridge/attestations/release-promotion/v1") == 3
    )
    assert '"source_revision": os.environ["RELEASE_REVISION"]' in server
    assert '"source_run_id": manifest["run"]["id"]' in server
    assert '"artifact_ledger_sha256": ledger_sha256' in server
    assert "RELEASE_RECOVERY_MANIFEST_SHA256" in server
    assert '"recovery_promotion_amd64"' in server
    assert '"build_provenance_amd64"' in server
    assert "tag_name: ${{ env.RELEASE_TAG }}" in release
    assert "target_commitish: ${{ env.RELEASE_REVISION }}" in release
    assert "name: TypeBridge ${{ env.RELEASE_VERSION }}" in release
    assert "draft: true" in release
    assert "name: release-recovery-evidence" in release


@pytest.mark.parametrize("job", RECOVERY_MUTATING_JOBS)
def test_candidate_guard_gate_rejects_an_unguarded_mutation_job(job: str) -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    block = job_block(workflow, job)
    guarded_term = "          inputs.recovery_mode == 'publish' &&\n"
    assert guarded_term in block
    hostile_block = block.replace(guarded_term, "", 1)
    hostile_workflow = workflow.replace(block, hostile_block, 1)

    with pytest.raises(AssertionError):
        assert_stable_only_release_mutations(hostile_workflow)


def test_cargo_publication_rejects_a_broadened_stable_tag_guard() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    block = job_block(workflow, "publish-crates")
    guarded = "      github.ref == 'refs/tags/v2.1.0' &&\n"
    assert guarded in block
    hostile_workflow = workflow.replace(block, block.replace(guarded, "", 1), 1)

    with pytest.raises(AssertionError):
        assert_stable_only_release_mutations(hostile_workflow)


@pytest.mark.parametrize("job", MUTATING_RELEASE_JOBS)
def test_release_mutation_gate_remains_cancellable(job: str) -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    block = job_block(workflow, job)
    guarded_term = "      !cancelled() &&\n"
    assert guarded_term in block
    hostile_block = block.replace(guarded_term, "", 1)
    hostile_workflow = workflow.replace(block, hostile_block, 1)

    with pytest.raises(AssertionError):
        assert_stable_only_release_mutations(hostile_workflow)


def test_candidate_guard_gate_rejects_hidden_preflight_publication() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    preflight = job_block(workflow, "channel-preflight")
    hostile_preflight = preflight.replace(
        "    steps:\n",
        "    steps:\n      - run: npm publish attacker-controlled.tgz\n",
        1,
    )
    hostile_workflow = workflow.replace(preflight, hostile_preflight, 1)

    with pytest.raises(AssertionError):
        assert_stable_only_release_mutations(hostile_workflow)


@pytest.mark.parametrize("marker", CARGO_PUBLICATION_MARKERS)
def test_cargo_inclusive_release_contains_each_required_path(marker: str) -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    graph = CRATE_RELEASE_GRAPH.read_text(encoding="utf-8")

    assert marker in workflow or marker in graph


def test_python_npm_publication_is_serial_after_global_candidate_gates() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    preflight = job_block(workflow, "channel-preflight")
    node_publish = job_block(workflow, "publish-node-npm")
    core_publish = job_block(workflow, "publish-core-pypi")
    root_publish = job_block(workflow, "publish-python-pypi")
    cargo_publish = job_block(workflow, "publish-crates")
    github_release = job_block(workflow, "github-release")
    python_acceptance = job_block(workflow, "accept-python-artifacts")
    node_acceptance = job_block(workflow, "accept-node-package")

    assert needs_line(preflight) == (
        "    needs: [validate-release-identity, accept-python-artifacts, "
        "accept-node-package, accept-live-artifact-parity, accept-server-oci]"
    )
    assert needs_line(node_publish) == (
        "    needs: [channel-preflight, recovery-preflight, release-tag-preflight]"
    )
    assert needs_line(job_block(workflow, "publish-server-oci")) == (
        "    needs: [channel-preflight, recovery-preflight, release-tag-preflight, "
        "accept-server-oci, publish-node-npm]"
    )
    assert needs_line(core_publish) == (
        "    needs: [build-core-wheels, build-core-sdist, recovery-preflight, "
        "release-tag-preflight, publish-server-oci]"
    )
    assert needs_line(root_publish) == (
        "    needs: [build-python, recovery-preflight, release-tag-preflight, publish-core-pypi]"
    )
    assert needs_line(github_release) == (
        "    needs: [recovery-preflight, release-tag-preflight, publish-server-oci, "
        "publish-node-npm, publish-core-pypi, publish-python-pypi, publish-crates]"
    )
    assert needs_line(cargo_publish) == (
        "    needs: [release-tag-preflight, publish-node-npm, validate-release-identity]"
    )
    assert "github.ref == 'refs/tags/v2.1.0'" in cargo_publish
    assert "needs.publish-node-npm.result == 'success'" in cargo_publish
    assert "CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}" in cargo_publish
    assert "NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}" in preflight
    assert "NPM_TOKEN is required for an atomic cross-registry release." in preflight
    assert f"        {STABLE_PUBLICATION_GUARD}" in preflight
    assert "environment: release" in node_publish
    assert_stable_only_release_mutations(workflow)
    assert "publish-" not in needs_line(python_acceptance)
    assert "publish-" not in needs_line(node_acceptance)


def test_rust_release_payload_gate_is_stdlib_only_and_fail_closed() -> None:
    source = RUST_RELEASE_ARTIFACT_VALIDATOR.read_text(encoding="utf-8")

    assert "import tarfile" in source
    assert "import tomllib" in source
    assert 'mode="r|gz"' in source
    assert "extractall" not in source
    assert "subprocess" not in source
    assert "urllib" not in source
    assert "FIRST_PARTY_PACKAGES" in source
    assert "VENDORED_PACKAGES" in source
    assert "CANONICAL_LICENSE_DIGESTS" in source
    assert 'package.get("license-file") != "LICENSE"' in source
    assert 'license_documents != ["LICENSE"]' in source


def test_read_only_notices_and_driver_provenance_precede_publication() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    identity = job_block(workflow, "validate-release-identity")
    preflight = job_block(workflow, "channel-preflight")

    assert "Validate identity and retained TypeDB package provenance" in identity
    command = "python scripts/ci/validate_release_identity.py"
    rust_artifacts = "python scripts/ci/validate_rust_release_artifacts.py"
    cargo_graph = "python scripts/ci/cargo_release_candidate.py build"
    assert identity.count(command) == 1
    assert identity.count(rust_artifacts) == 1
    notice = "generate_native_dependency_notice.py --check"
    historical = "validate_historical_band9_registry.py"
    official = "validate_latest_typedb_driver_pin.py"
    assert notice in identity
    assert historical in identity
    assert "--committed-cutoff" in identity
    assert "owner-frozen official TypeDB 3.12.1" in identity
    assert (
        identity.index(notice)
        < identity.index(historical)
        < identity.index(cargo_graph)
        < identity.index(rust_artifacts)
        < identity.index(command)
    )
    assert "--candidate-bundle type-bridge-core/target/cargo-release-candidate" in identity
    assert "--expected-manifest-sha256" in identity
    assert '--expected-release-version "$RELEASE_VERSION"' in identity
    assert identity.index(command) < identity.index(official)
    assert "||" not in identity[identity.index(command) :]
    assert "validate-release-identity" in needs_line(preflight)


def test_npm_preflight_authenticates_without_an_owner_wide_acl_probe() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    preflight = job_block(workflow, "channel-preflight")

    setup = preflight.index("actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020 # v4")
    pinned_cli = preflight.index("npm install --global --ignore-scripts npm@11.18.0")
    identity = preflight.index("npm whoami --registry=https://registry.npmjs.org")

    assert setup < pinned_cli < identity
    assert f"        {STABLE_PUBLICATION_GUARD}" in preflight
    assert 'test "$(npm --version)" = "11.18.0"' in preflight
    assert "--registry=https://registry.npmjs.org" in preflight
    assert "npm access list packages" not in workflow
    assert "npm access list collaborators" not in workflow
    assert "validate_npm_package_access.py" not in workflow
    assert "npm trust list" not in preflight
    assert "npm publish" not in preflight
    assert "npm access set" not in preflight
    assert "npm trust revoke" not in preflight

    node_publish = job_block(workflow, "publish-node-npm")
    server_publish = job_block(workflow, "publish-server-oci")
    assert "publish-server-oci" not in needs_line(node_publish)
    assert "publish-node-npm" in needs_line(server_publish)


def test_stable_preflight_rejects_a_missing_or_untrimmed_cargo_token_before_npm() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    preflight = job_block(workflow, "channel-preflight")

    cargo_shape = preflight.index("Validate Cargo publication credential shape")
    npm_auth = preflight.index("Authenticate npm publication credential")
    assert cargo_shape < npm_auth
    assert f"        {STABLE_PUBLICATION_GUARD}" in preflight[cargo_shape:npm_auth]
    assert (
        "CARGO_REGISTRY_TOKEN: ${{ secrets.CARGO_REGISTRY_TOKEN }}"
        in preflight[cargo_shape:npm_auth]
    )
    assert "if not token or token != token.strip():" in preflight[cargo_shape:npm_auth]
    assert "cargo owner" not in preflight
    assert "CARGO_REGISTRY_TOKEN is required and must not have surrounding whitespace." in preflight


def test_crates_helper_requires_identical_registry_bytes() -> None:
    helper = CRATE_PUBLISH_HELPER.read_text(encoding="utf-8")

    assert 'cargo_bin" package --locked -p "$crate"' in helper
    assert "candidate_checksum" in helper
    assert 'version = payload["version"]' in helper
    assert 'checksum = version["checksum"]' in helper
    assert "https://index.crates.io/${sparse_path}" in helper
    assert 'entry.get("vers") == expected_version' in helper
    assert "crates.io API checksum mismatch" in helper
    assert "crates.io sparse-index checksum mismatch" in helper
    assert helper.count("require_matching_registry_checksum") >= 4
    assert helper.count("require_matching_registry_index_checksum") >= 4
    assert "already exists on crates.io index" in helper
    assert "CRATES_IO_VERIFY_ATTEMPTS" in helper
    assert "--verify-preexisting" in helper
    assert "--preflight" in helper
    # Closed Band 7 checksums remain recovery evidence only; retained Band 8
    # checksums protect the immutable inputs to the current release graph.
    assert "030327872cad70433b3c8bde72529d0df6291af08ab3aad82550f8871e409364" in helper
    assert "e181af88e3742a13e35225c439f8a98968f014417b1814b18736743f6d799b16" in helper
    assert "a2c4fe7da8c6c8d6a075bb667c916f8fceda416bbb844d0396f987cd48204d2e" in helper
    assert "68c5770db7d2bc36c13a24a9fe37e5841e26b2adbeca4d06489a6689685e651d" in helper


def test_release_rust_toolchain_is_exactly_pinned() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")

    assert "toolchain: stable" not in workflow
    assert re.findall(r"^          toolchain: (.+)$", workflow, re.MULTILINE)
    assert set(re.findall(r"^          toolchain: (.+)$", workflow, re.MULTILINE)) == {"1.94.1"}


def test_fresh_runtime_probe_cannot_reuse_the_workspace_lock() -> None:
    probe = FRESH_RUNTIME_PROBE.read_text(encoding="utf-8")

    assert "type-bridge-typedb-runtime-${release_version}.crate" in probe
    assert "Fresh consumer unexpectedly started with a Cargo.lock" in probe
    assert '"$cargo_bin" metadata' in probe
    assert "--format-version 1" in probe
    assert "Fresh consumer resolution did not create an independent Cargo.lock" in probe
    assert "type-bridge-typedb-driver-b8-${driver_b8_pin}.crate" in probe
    assert "type-bridge-typedb-protocol-b8-${protocol_b8_pin}.crate" in probe
    for package in (
        "type-bridge-typedb-driver-b8",
        "type-bridge-typedb-protocol-b8",
        "typedb-driver",
        "typedb-protocol",
    ):
        assert f'"{package}"' in probe
    for retired in (
        "type-bridge-typedb-driver-b7",
        "type-bridge-typedb-protocol-b7",
    ):
        assert f'"{retired}"' not in probe
    assert 'dependency.get("name") == "typedb-protocol"' in probe
    assert 're.fullmatch(r"=([0-9]+\\.[0-9]+\\.[0-9]+)"' in probe
    assert "Official typedb-driver typedb-protocol requirement is not exact" in probe
    assert "Unexpected downstream band-9 fork" in probe
    assert 'not source.startswith("registry+")' in probe
    assert '{"default", "band8", "band9"}' in probe
    assert 'if "band7" in features:' in probe
    assert "Fresh downstream resolution escaped {name} pin" in probe
    assert 'env RUSTFLAGS="-Dwarnings" "$cargo_bin" check' in probe
    assert "--quiet" not in probe
    assert '"$cargo_bin" check' in probe
    assert "--locked" in probe


def test_historical_band9_forks_are_inert_and_unpublishable() -> None:
    workspace = tomllib.loads(
        (REPO_ROOT / "type-bridge-core/Cargo.toml").read_text(encoding="utf-8")
    )
    members = set(workspace["workspace"]["members"])
    runtime = (REPO_ROOT / "type-bridge-core/crates/typedb-runtime/Cargo.toml").read_text(
        encoding="utf-8"
    )

    for crate in ("typedb-driver-b9", "typedb-protocol-b9"):
        assert f"vendor/{crate}" not in members
        manifest = tomllib.loads(
            (REPO_ROOT / f"type-bridge-core/vendor/{crate}/Cargo.toml").read_text(encoding="utf-8")
        )
        assert manifest["package"]["publish"] is False
        assert f"type-bridge-{crate}" not in runtime


def test_node_native_crate_configures_napi_platform_linking() -> None:
    """Direct Cargo builds must retain napi-rs's platform linker setup."""
    crate_root = REPO_ROOT / "type-bridge-core/crates/node"
    cargo = tomllib.loads((crate_root / "Cargo.toml").read_text(encoding="utf-8"))

    assert cargo["build-dependencies"]["napi-build"] == "2"
    assert (crate_root / "build.rs").read_text(encoding="utf-8") == (
        "fn main() {\n    napi_build::setup();\n}\n"
    )


def test_real_node_declaration_parity_gate_runs_in_every_acceptance_entrypoint() -> None:
    ci = CI_WORKFLOW.read_text(encoding="utf-8")
    release = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    test_script = (REPO_ROOT / "test.sh").read_text(encoding="utf-8")
    check_script = (REPO_ROOT / "scripts/check.sh").read_text(encoding="utf-8")

    for source in (ci, release, test_script, check_script):
        assert "npm run test:dts" in source
        assert "npm run dts:parity" in source
        assert source.index("npm run test:dts") < source.index("npm run dts:parity")


def test_generated_examples_are_generated_before_typechecking_and_compile_all_targets() -> None:
    ci = CI_WORKFLOW.read_text(encoding="utf-8")
    check_script = (REPO_ROOT / "scripts/check.sh").read_text(encoding="utf-8")
    validator = (REPO_ROOT / "scripts/ci/validate_generated_examples.sh").read_text(
        encoding="utf-8"
    )

    assert "if: matrix.target == 'examples'" in ci
    assert "type-bridge --manifest examples/typebridge.yaml schema generate" in ci
    assert ci.index("Generate example Python package") < ci.index(
        "Run pyright on ${{ matrix.target }}"
    )
    assert "scripts/ci/validate_generated_examples.sh" in check_script
    for command in ("schema check", "schema generate", "uv run pyright", "tsc", "cargo check"):
        assert command in validator


def test_node_scope_probe_runs_in_every_acceptance_entrypoint() -> None:
    package = json.loads(
        (REPO_ROOT / "type-bridge-core/crates/node/package.json").read_text(encoding="utf-8")
    )
    assert package["scripts"]["scope:probe"] == (
        "node --test tests/scope-probe.test.cjs && node tests/scope-probe.cjs"
    )

    ci_node = job_block(CI_WORKFLOW.read_text(encoding="utf-8"), "node-check")
    release_pack = job_block(
        RELEASE_WORKFLOW.read_text(encoding="utf-8"),
        "pack-node-package",
    )
    test_script = (REPO_ROOT / "test.sh").read_text(encoding="utf-8")
    check_script = (REPO_ROOT / "scripts/check.sh").read_text(encoding="utf-8")
    for source in (ci_node, release_pack):
        assert source.count("npm run scope:probe") == 1
    for source in (test_script, check_script):
        assert source.count("npm run scope:probe") == 2
    for source in (ci_node, release_pack, test_script, check_script):
        assert source.index("npm run build") < source.index("npm run scope:probe")

    for source in (ci_node, release_pack, check_script):
        assert source.index("npm run typecheck:projection-integration") < source.index(
            "npm run scope:probe"
        )
    for source in (ci_node, release_pack, test_script, check_script):
        assert source.index("npm run scope:probe") < source.index("npm run test:unit")


def test_standalone_released_rule_wire_runs_in_every_acceptance_entrypoint() -> None:
    """The no-feature-unification probe is outside the parent Cargo workspace."""
    fixture = tomllib.loads(
        (
            REPO_ROOT
            / "type-bridge-core/crates/core/tests/fixtures/rule-wire-standalone/Cargo.toml"
        ).read_text(encoding="utf-8")
    )
    assert fixture["package"]["publish"] is False
    assert fixture["workspace"] == {}

    sources = (
        CI_WORKFLOW.read_text(encoding="utf-8"),
        RELEASE_WORKFLOW.read_text(encoding="utf-8"),
        (REPO_ROOT / "test.sh").read_text(encoding="utf-8"),
        (REPO_ROOT / "scripts/check.sh").read_text(encoding="utf-8"),
    )
    for source in sources:
        assert "rule-wire-standalone/Cargo.toml" in source
        assert "cargo test --locked" in source
        assert "CARGO_TARGET_DIR" in source
        assert "target/rule-wire-standalone" in source


def test_live_cli_workspace_state_machine_is_required_locally_and_in_ci() -> None:
    """Ignored CLI lifecycle probes must have explicit release-blocking entrypoints."""
    ci = CI_WORKFLOW.read_text(encoding="utf-8")
    test_script = (REPO_ROOT / "test.sh").read_text(encoding="utf-8")
    tests = (
        "empty_workspace_to_replayed_history_live",
        "documented_examples_initial_constraints_apply_and_verify_live",
        "verify_never_creates_databases_live",
        "adopt_legacy_history_then_evolve_live",
        "shipped_python_converter_to_native_adoption_live",
    )
    rust_integration = job_block(ci, "rust-integration")
    for test in tests:
        assert f"          {test}\n          --manifest-path" in rust_integration
    assert rust_integration.count("scripts/ci/run_exact_ignored_rust_test.sh") == len(tests) + 2
    assert "unsupported_server_apply_creates_neither_database_live" in rust_integration

    loop = re.search(
        r"for cli_live_test in \\\n(?P<tests>.*?); do\n(?P<body>.*?)\n    done",
        test_script,
        re.DOTALL,
    )
    assert loop is not None
    selected_tests = set(re.findall(r"^        ([a-z0-9_]+)(?: \\)?$", loop["tests"], re.MULTILINE))
    assert selected_tests == set(tests)
    assert 'run_exact_ignored_rust_test.sh "$cli_live_test"' in loop["body"]
    assert 'TYPE_BRIDGE_TEST_PYTHON="$ROOT/.venv/bin/python"' in loop["body"]
    assert "timeout-minutes: 30" in rust_integration
    assert "uv sync --no-dev" in rust_integration
    assert "TYPE_BRIDGE_TEST_PYTHON: ${{ github.workspace }}/.venv/bin/python" in rust_integration
    assert test_script.count("timeout --foreground") >= 6


def test_generated_and_low_level_queries_are_required_in_the_tls_lane() -> None:
    """TLS must cover generated applications as well as the retained low-level facade."""
    test_script = (REPO_ROOT / "test.sh").read_text(encoding="utf-8")
    start = test_script.index("run_tls_transport_steps() {")
    end = test_script.index('\n}\n\nif [[ "$tls" == 1 ]]', start)
    tls_lane = test_script[start:end]

    assert (
        "tests/integration/schema/test_generated_projection_live.py"
        "::test_generated_package_preserves_application_operation_outcomes_live"
    ) in tls_lane
    assert 'npm --prefix "$NODE_DIR" run test:projection-integration' in tls_lane
    assert (
        "tests/integration/queries/test_query_v2_binding_smoke.py"
        "::test_prepared_plan_executes_locally_and_remotely"
    ) in tls_lane
    assert '"$NODE_DIR/tests/integration/queries/query-v2-smoke.test.ts"' in tls_lane
    assert "test_remote_query_session_parity.py" not in tls_lane
    assert "typed-remote-query-parity.test.ts" not in tls_lane
    assert tls_lane.count("TYPEDB_TLS_ROOT_CA") >= 4
    assert 'NODE_EXTRA_CA_CERTS="$fixture_root_ca"' in tls_lane
    assert "NODE_TLS_REJECT_UNAUTHORIZED" not in tls_lane


def test_live_release_parity_consumes_exact_artifacts_before_every_publish() -> None:
    """Generated live gates must execute uploaded candidates without rebuilding."""
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    acceptance = job_block(workflow, "accept-live-artifact-parity")
    build_python = job_block(workflow, "build-python")
    pack_node = job_block(workflow, "pack-node-package")
    smoke_server = job_block(workflow, "build-v2-smoke-server")

    assert needs_line(acceptance) == (
        "    needs: [build-core-wheels, build-python, pack-node-package, build-v2-smoke-server]"
    )
    assert "image: typedb/typedb:3.12.1" in acceptance
    assert 'python-version: "3.13.5"' in acceptance
    assert "- 1729:1729" in acceptance
    assert "- 8000:8000" in acceptance
    assert "curl --fail --silent http://localhost:8000/v1/version" in acceptance

    for artifact in (
        "core-wheels-linux-x86_64",
        "python-dist",
        "node-package",
        "generated-python-live-fixture",
        "generated-node-live-fixture",
        "v2-smoke-server",
    ):
        assert f"name: {artifact}" in acceptance
    for variable in (
        "TYPE_BRIDGE_CORE_WHEEL",
        "TYPE_BRIDGE_ROOT_WHEEL",
        "TYPE_BRIDGE_NODE_PACKAGE",
        "TYPE_BRIDGE_NODE_PACKAGE_ROOT",
        "TYPE_BRIDGE_GENERATED_PYTHON_STAGE",
        "TYPE_BRIDGE_GENERATED_NODE_STAGE",
        "TYPE_BRIDGE_V2_SMOKE_SERVER",
    ):
        assert variable in acceptance

    assert "uv venv" in acceptance
    assert "uv pip install" in acceptance
    assert "npm install --ignore-scripts" in acceptance
    assert 'for module_name in ("type_bridge", "type_bridge_core")' in acceptance
    assert "leaked to the source checkout" in acceptance
    assert "escaped the exact-wheel environment" in acceptance
    assert "test_generated_projection_live.py" in acceptance
    assert "test_generated_package_preserves_application_operation_outcomes_live" in acceptance
    assert "test_generated_projection_round_trips_live_models" in acceptance
    assert "generated-package-live.test.js" in acceptance
    assert "--import-mode=importlib" in acceptance
    assert "release-generated-parity.xml" in acceptance
    assert '"tests": 2' in acceptance
    assert '"failures": 0' in acceptance
    assert '"errors": 0' in acceptance
    assert '"skipped": 0' in acceptance
    assert "test_typed_query_live_parity.py" not in acceptance
    assert "test_remote_query_session_parity.py" not in acceptance

    for forbidden in (
        "uv sync",
        "uv build",
        "maturin",
        "npm ci",
        "npm pack --",
        "npm run build:native",
        "npm run build:types",
        "cargo build",
        "cargo run",
        "actions/upload-artifact",
    ):
        assert forbidden not in acceptance

    fixture_script = "scripts/ci/prepare_generated_live_fixture.sh"
    assert f"{fixture_script} python" in build_python
    assert "name: generated-python-live-fixture" in build_python
    assert 'prepare_generated_live_fixture.sh" node' in pack_node
    assert "name: generated-node-live-fixture" in pack_node
    fixture_source = (REPO_ROOT / fixture_script).read_text(encoding="utf-8")
    assert "format: typebridge.workspace/v1" in fixture_source
    assert "format: typebridge.schema-set/v1" in fixture_source
    assert "artifacts:" in fixture_source
    assert "schema-authority:" in fixture_source
    assert "-p type-bridge-cli --bin type-bridge" in fixture_source
    assert "schema generate" in fixture_source
    assert "type-bridge-schema-codegen" not in fixture_source
    assert "emit_python_acceptance" not in fixture_source
    assert "emit_typescript_acceptance" not in fixture_source
    assert "declared-schema.json" not in fixture_source

    python_live = (
        REPO_ROOT / "tests/integration/schema/test_generated_projection_live.py"
    ).read_text(encoding="utf-8")
    node_live = (
        REPO_ROOT
        / "type-bridge-core/crates/node/tests/projection-integration/generated-package-live.test.ts"
    ).read_text(encoding="utf-8")
    for consumer in (python_live, node_live):
        assert "schema-authority.json" in consumer
        assert "SMOKE_AUTHORITY_B64" in consumer
        assert "QueryV2Authority" not in consumer
        assert "declared-schema.json" not in consumer
        assert "SMOKE_DECLARED_B64" not in consumer
        assert "SMOKE_SCOPE" not in consumer
        assert "SMOKE_PROFILE" not in consumer

    assert needs_line(smoke_server) == "    needs: validate-release-identity"
    assert "toolchain: 1.94.1" in smoke_server
    assert (
        "cargo build --manifest-path type-bridge-core/Cargo.toml "
        "--release -p type-bridge-server --no-default-features "
        "--features band9,v2-query "
        "--example v2_smoke_server"
    ) in " ".join(smoke_server.split())
    assert "name: v2-smoke-server" in smoke_server
    assert "path: type-bridge-core/target/release/examples/v2_smoke_server" in smoke_server
    assert "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02" in smoke_server
    assert "npm publish" not in smoke_server
    assert "cargo publish" not in smoke_server

    preflight = job_block(workflow, "channel-preflight")
    assert "accept-live-artifact-parity" in needs_line(preflight)
    assert needs_line(job_block(workflow, "publish-node-npm")) == (
        "    needs: [channel-preflight, recovery-preflight, release-tag-preflight]"
    )


def test_low_level_query_smokes_use_compiled_server_authority() -> None:
    fixture = REPO_ROOT / "tests/fixtures/query-v2-binding-smoke"
    manifest = (fixture / "typebridge.yaml").read_text(encoding="utf-8")
    schema_set = (fixture / "schema/schema.yaml").read_text(encoding="utf-8")
    assert "format: typebridge.workspace/v1" in manifest
    assert "managed-scope: binding-smoke" in manifest
    assert "schema-authority:" in manifest
    assert "output: generated/schema-authority.json" in manifest
    assert "format: typebridge.schema-set/v1" in schema_set

    consumers = (
        REPO_ROOT / "tests/integration/queries/test_query_v2_binding_smoke.py",
        REPO_ROOT / "type-bridge-core/crates/node/tests/integration/queries/query-v2-smoke.test.ts",
    )
    for path in consumers:
        source = path.read_text(encoding="utf-8")
        assert "QueryV2Authority" in source
        assert "query-v2-binding-smoke" in source
        assert "type-bridge-cli" in source
        assert "schema-authority.json" in source
        assert "SMOKE_AUTHORITY_B64" in source
        assert "SMOKE_DECLARED_B64" not in source
        assert "SMOKE_SCOPE" not in source
        assert "SMOKE_PROFILE" not in source


def test_npm_publication_uses_the_accepted_tarball() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    build = job_block(workflow, "build-node-native")
    pack = job_block(workflow, "pack-node-package")
    acceptance = job_block(workflow, "accept-node-package")
    publish = job_block(workflow, "publish-node-npm")
    package = json.loads(
        (REPO_ROOT / "type-bridge-core/crates/node/package.json").read_text(encoding="utf-8")
    )
    package_smoke = (REPO_ROOT / "type-bridge-core/crates/node/tests/package-smoke.cjs").read_text(
        encoding="utf-8"
    )
    assert package["scripts"]["clean:types"] == "node scripts/clean-types.js"
    assert package["scripts"]["build:types"] == ("npm run clean:types && tsc -p tsconfig.json")
    assert 'require("../dist/native.js").loadNative()' in package_smoke
    assert "readdirSync" not in package_smoke
    assert ".sort()[0]" not in package_smoke

    native_legs = re.findall(
        r"^          - target: (.+)\n"
        r"^            runner: (.+)\n"
        r"^            artifact: (.+)\n"
        r"^            filename: (.+)$",
        build,
        re.MULTILINE,
    )
    assert native_legs == [
        (
            "linux-x64-gnu",
            "ubuntu-latest",
            "node-native-linux-x64-gnu",
            "type_bridge_node.linux-x64-gnu.node",
        ),
        (
            "linux-arm64-gnu",
            "ubuntu-24.04-arm",
            "node-native-linux-arm64-gnu",
            "type_bridge_node.linux-arm64-gnu.node",
        ),
        (
            "darwin-x64",
            "macos-15-intel",
            "node-native-darwin-x64",
            "type_bridge_node.darwin-x64.node",
        ),
        (
            "darwin-arm64",
            "macos-14",
            "node-native-darwin-arm64",
            "type_bridge_node.darwin-arm64.node",
        ),
        (
            "win32-x64-msvc",
            "windows-latest",
            "node-native-win32-x64-msvc",
            "type_bridge_node.win32-x64-msvc.node",
        ),
        (
            "win32-arm64-msvc",
            "windows-11-arm",
            "node-native-win32-arm64-msvc",
            "type_bridge_node.win32-arm64-msvc.node",
        ),
    ]
    assert build.count("          - target: ") == 6
    assert "needs: validate-release-identity" in build
    assert "runs-on: ${{ matrix.runner }}" in build
    assert "npm run build:native" in build
    assert "name: ${{ matrix.artifact }}" in build
    assert "path: type-bridge-core/crates/node/${{ matrix.filename }}" in build
    assert "if-no-files-found: error" in build
    assert "npm pack" not in build
    assert "npm publish" not in build

    assert "needs: build-node-native" in pack
    assert "pattern: node-native-*" in pack
    assert "path: type-bridge-core/crates/node" in pack
    assert "merge-multiple: true" in pack
    assert pack.count("scripts/ci/validate_node_release_package.py") == 2
    ordered_pack_gates = (
        "pattern: node-native-*",
        "--native-directory .",
        "npm run build:types",
        "npm run typecheck",
        "npm run typecheck:projection-integration",
        "npm run scope:probe",
        "npm run test:unit",
        "npm run test:dts",
        "npm run dts:parity",
        "npm run smoke:package",
        'npm pack --ignore-scripts --pack-destination "$package_dir"',
        '--artifact "${packages[0]}"',
        "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02 # v4",
    )
    positions = [pack.index(gate) for gate in ordered_pack_gates]
    assert positions == sorted(positions)
    assert "--repository-package package.json" in pack
    assert '--tag "$RELEASE_TAG"' in pack
    assert 'if [[ "$RELEASE_CHANNEL" == "candidate" ]]' in pack
    assert "validator_args+=(--allow-prerelease)" in pack
    assert "npm run build:native" not in pack

    assert "needs: pack-node-package" in acceptance
    assert package["engines"]["node"] == ">=18"
    acceptance_legs = re.findall(
        r"^          - target: (.+)\n"
        r"^            runner: (.+)\n"
        r"^            node-version: (.+)$",
        acceptance,
        re.MULTILINE,
    )
    assert acceptance_legs == [
        ("linux-x64-gnu", "ubuntu-latest", '"18"'),
        ("linux-x64-gnu", "ubuntu-latest", '"20"'),
        ("linux-arm64-gnu", "ubuntu-24.04-arm", '"20"'),
        ("darwin-x64", "macos-15-intel", '"20"'),
        ("darwin-arm64", "macos-14", '"20"'),
        ("win32-x64-msvc", "windows-latest", '"20"'),
        ("win32-arm64-msvc", "windows-11-arm", '"20"'),
    ]
    assert acceptance.count("          - target: ") == 7
    assert "name: node-package" in acceptance
    assert "path: tmp/release-node-package" in acceptance
    assert "npm run smoke:packed-package -- --artifact-directory" in acceptance
    assert "npm run build:native" not in acceptance
    assert "npm run build:types" not in acceptance
    assert "npm pack" not in acceptance
    assert "actions/upload-artifact" not in acceptance

    assert needs_line(publish) == (
        "    needs: [channel-preflight, recovery-preflight, release-tag-preflight]"
    )
    assert "name: node-package" in publish
    assert publish.count("scripts/ci/validate_node_release_package.py") == 2
    assert "--repository-package type-bridge-core/crates/node/package.json" in publish
    assert publish.count('--tag "$RELEASE_TAG"') == 2
    assert "--allow-prerelease" not in publish
    assert "environment: release" in publish
    assert "github.ref == 'refs/tags/v2.1.0'" in publish
    assert "inputs.release_channel == 'recovery'" in publish
    assert "inputs.recovery_mode == 'publish'" in publish
    assert "name: Install pinned npm publisher" in publish
    assert "npm install --global --ignore-scripts npm@11.18.0" in publish
    assert 'test "$(npm --version)" = "11.18.0"' in publish
    identity_position = publish.index("scripts/ci/validate_node_release_package.py")
    publish_position = publish.index('npm publish "${packages[0]}" --access public')
    assert identity_position < publish_position
    assert "dist.integrity" in publish
    assert '--registry-integrity "$registry_integrity"' in publish
    assert "npm-view-error.log" in publish
    assert "grep -Eq 'E404|404 Not Found'" in publish
    assert "lookup failed without an authoritative 404" in publish
    assert publish.count('npm publish "${packages[0]}" --access public') == 1
    assert "Detect npm token" not in publish
    assert "steps.npm_token.outputs.present" not in publish
    assert "skipping the npm registry publish" not in publish

    github_release = job_block(workflow, "github-release")
    assert "publish-node-npm" in needs_line(github_release)
    assert "name: node-package" in github_release


def test_pypi_skip_existing_is_guarded_before_and_after_publish() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    for job, project in (
        ("publish-core-pypi", "type-bridge-core"),
        ("publish-python-pypi", "type-bridge"),
    ):
        publish = job_block(workflow, job)
        assert publish.count("scripts/ci/verify_pypi_release_hashes.py") == 2
        assert publish.count("--dist-dir tmp/pypi-candidate") == 2
        snapshot = publish.index("cp -a dist tmp/pypi-candidate")
        preflight = publish.index("scripts/ci/verify_pypi_release_hashes.py")
        publisher = publish.index("pypa/gh-action-pypi-publish")
        post_publish = publish.rindex("scripts/ci/verify_pypi_release_hashes.py")
        assert snapshot < preflight < publisher < post_publish
        assert f"--project {project}" in publish
        assert '--version "$version"' in publish
        assert "skip-existing: true" in publish
        assert "--require-existing" in publish[post_publish:]
        assert "--attempts 6" in publish[post_publish:]
