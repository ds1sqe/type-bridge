"""Release publication must consume artifacts that passed compatibility gates."""

from __future__ import annotations

import io
import json
import re
import subprocess
import tarfile
import tomllib
from pathlib import Path

import pytest
import yaml
from yaml.nodes import MappingNode, Node, ScalarNode, SequenceNode

from tests.integration.parity import cross_language

REPO_ROOT = Path(__file__).resolve().parents[3]
CI_WORKFLOW = REPO_ROOT / ".github/workflows/ci.yml"
RELEASE_WORKFLOW = REPO_ROOT / ".github/workflows/release.yml"
CRATE_PUBLISH_HELPER = REPO_ROOT / "scripts/ci/publish_crate_idempotently.sh"
FRESH_RUNTIME_PROBE = REPO_ROOT / "scripts/ci/validate_fresh_typedb_runtime_package.sh"
RUST_RELEASE_ARTIFACT_VALIDATOR = REPO_ROOT / "scripts/ci/validate_rust_release_artifacts.py"
NPM_ACCESS_VALIDATOR = REPO_ROOT / "scripts/ci/validate_npm_package_access.py"
STABLE_PUBLICATION_GUARD = "if: github.event_name == 'push' && github.ref == 'refs/tags/v2.0.0'"
MUTATING_RELEASE_JOBS = (
    "publish-node-npm",
    "publish-core-pypi",
    "publish-python-pypi",
    "github-release",
)
CARGO_PUBLICATION_MARKERS = (
    "publish-crates:",
    "CARGO_REGISTRY_TOKEN",
    "publish_crate_idempotently",
    "cargo publish",
    "cargo package",
    "patch.crates-io",
    "validate_rust_release_artifacts.py",
    "validate_fresh_typedb_runtime_package.sh",
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
    assert "typing-extensions>=4.12" in root["project"]["dependencies"]

    ci = CI_WORKFLOW.read_text(encoding="utf-8")
    expected_matrix = 'python-version: ["3.12", "3.13.5", "3.14"]'
    assert expected_matrix in job_block(ci, "python-legacy-package-compat")
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
    """Require every external publication job to be unreachable for an RC run."""
    for name in MUTATING_RELEASE_JOBS:
        block = job_block(workflow, name)
        guards = re.findall(r"^    if: (.+)$", block, re.MULTILINE)
        assert guards == [STABLE_PUBLICATION_GUARD.removeprefix("if: ")]

    publication_markers = {
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
    assert "scripts/ci/validate_python_release_artifacts.py" in acceptance
    assert "--core-wheels-dir tmp/release-python-artifacts/core-wheels" in acceptance
    assert "--core-sdist-dir tmp/release-python-artifacts/core-sdist" in acceptance
    assert "--root-dist-dir tmp/release-python-artifacts/root" in acceptance
    assert '--expected-version "$version"' in acceptance
    assert "'auditwheel==6.7.0'" in acceptance
    assert "scripts/ci/audit_manylinux_release_wheels.py" in acceptance
    assert "--manifest tmp/release-python-artifacts/manifest.json" in acceptance
    assert "--core-wheels-dir tmp/release-python-artifacts/core-wheels" in acceptance
    validator_position = acceptance.index("scripts/ci/validate_python_release_artifacts.py")
    auditor_position = acceptance.index("scripts/ci/audit_manylinux_release_wheels.py")
    execution_position = acceptance.index("scripts/ci/run_legacy_python_compat.py")
    assert validator_position < auditor_position < execution_position
    audit_command = acceptance[auditor_position : acceptance.index("\n\n", auditor_position)]
    assert "||" not in audit_command
    assert acceptance.count("scripts/ci/run_legacy_python_compat.py") == 3
    assert "type_bridge_core-*linux*x86_64.whl" in acceptance
    assert "type_bridge_core-*.tar.gz" in acceptance
    assert "type_bridge-*.tar.gz" in acceptance
    assert "scripts/ci/run_typed_python_artifact.py" in acceptance
    assert '"${root_wheels[0]}[typedb-driver]"' in acceptance
    assert '"${core_wheels[0]}"' in acceptance
    assert "tests/compat/typedb_driver_native/probe.py" in acceptance
    assert "for flag in --help -h --version -V" in acceptance
    assert '"$legacy_bin" "$flag"' in acceptance
    assert '"$cli_venv/bin/type-bridge" "$flag"' in acceptance
    assert 'cmp "$parity_dir/direct-$label.stdout"' in acceptance
    assert 'cmp "$parity_dir/direct-$label.stderr"' in acceptance
    assert '"$cli_venv/bin/type-bridge" schema --help' in acceptance
    assert '"$cli_venv/bin/type-bridge" migration --help' in acceptance
    assert '--manifest="$workspace/typebridge.yaml" --version' in acceptance
    assert '--manifest "$workspace/typebridge.yaml" -V' in acceptance
    assert '"$cli_venv/bin/type-bridge" schema export-declared' in acceptance
    assert 'test -s "$workspace/generated/declared-schema.json"' in acceptance
    typed_runner = (REPO_ROOT / "scripts/ci/run_typed_python_artifact.py").read_text(
        encoding="utf-8"
    )
    assert '"pythonVersion": python_version' in typed_runner
    assert '"pythonVersion": "3.13"' not in typed_runner
    assert "uv build" not in acceptance
    assert "actions/upload-artifact" not in acceptance

    core_publish = job_block(workflow, "publish-core-pypi")
    root_publish = job_block(workflow, "publish-python-pypi")
    preflight = job_block(workflow, "channel-preflight")
    assert "accept-python-artifacts" in needs_line(preflight)
    assert "publish-node-npm" in needs_line(core_publish)
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
            "Prepare typed consumer interpreter and Pyright"
        )
    ]
    preinstall = "'type-bridge-core==1.5.11'"
    candidate_install = (
        'uv pip install --python "$resolver_venv/bin/python" \\\n'
        "            --find-links tmp/release-python-artifacts/core-wheels \\\n"
        '            "${root_wheels[0]}"'
    )
    assert resolver.count(preinstall) == 1
    assert resolver.count(candidate_install) == 1
    assert resolver.index(preinstall) < resolver.index(candidate_install)
    assert "type_bridge_core-*.whl" not in resolver
    assert "importlib.metadata.version(distribution)" in resolver
    assert 'for distribution in ("type-bridge", "type-bridge-core")' in resolver
    assert '"$resolver_venv/bin/type-bridge" schema --help' in resolver


def test_published_v1_root_is_exercised_against_candidate_core_offline() -> None:
    acceptance = job_block(
        RELEASE_WORKFLOW.read_text(encoding="utf-8"),
        "accept-python-artifacts",
    )
    reverse = acceptance[
        acceptance.index(
            "Prove published V1 root compatibility with candidate core"
        ) : acceptance.index("Run isolated Python artifact acceptance")
    ]

    assert "PyPI has no 1.5.7" in acceptance
    assert reverse.count("python -m pip download") == 2
    assert "--no-deps \\\n            --only-binary=:all:" in reverse
    assert reverse.count("--index-url https://pypi.org/simple") == 2
    assert "'type-bridge==1.5.11'" in reverse
    assert "type_bridge-1.5.11-py3-none-any.whl" in reverse
    assert "scripts/ci/validate_released_python_root.py" in reverse
    assert "--verify-pypi-authority" in reverse
    assert "type_bridge_core-*linux*x86_64.whl" in reverse
    assert "Dependency wheelhouse unexpectedly contains type-bridge-core" in reverse
    assert 'uv pip install --python "$compat_venv/bin/python" \\\n            --no-index' in reverse
    pair_install = reverse[reverse.rindex("uv pip install") :]
    assert "--no-deps" in pair_install
    assert '"${released_root_wheels[0]}"' in pair_install
    assert '"${candidate_core_wheels[0]}"' in pair_install
    assert "scripts/ci/run_legacy_python_compat.py" in pair_install
    assert '--python "$compat_venv/bin/python"' in pair_install
    assert "--expected-root-version 1.5.11" in pair_install
    assert '--expected-core-version "$version"' in pair_install
    assert "type-bridge-core==1.5.11" not in reverse


def test_release_identity_gate_receives_all_lockstep_authorities() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    identity = job_block(workflow, "validate-release-identity")

    assert "--artifact-contract python-npm-only" in identity
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
        "      - 'v2.0.0'\n"
        "  workflow_dispatch:\n"
        "    inputs:\n"
        "      release_channel:\n"
        "        description: Validate the fixed RC or final 2.0.0 artifacts "
        "without publishing\n"
        "        required: true\n"
        "        type: choice\n"
        "        default: candidate\n"
        "        options:\n"
        "          - candidate\n"
        "          - stable\n"
    ) in preamble
    assert "'v*'" not in preamble
    assert (
        "RELEASE_TAG: ${{ github.event_name == 'workflow_dispatch' && "
        "inputs.release_channel == 'candidate' && 'v2.0.0-rc.0' || 'v2.0.0' }}"
    ) in preamble
    assert (
        "PYTHON_RELEASE_VERSION: ${{ github.event_name == 'workflow_dispatch' && "
        "inputs.release_channel == 'candidate' && '2.0.0rc0' || '2.0.0' }}"
    ) in preamble
    assert (
        "RELEASE_CHANNEL: ${{ github.event_name == 'workflow_dispatch' "
        "&& inputs.release_channel || 'stable' }}"
    ) in preamble
    assert workflow.count("RELEASE_TAG:") == 1
    assert workflow.count("PYTHON_RELEASE_VERSION:") == 1
    assert workflow.count("RELEASE_CHANNEL:") == 1
    assert workflow.count("inputs.release_channel") == 3
    assert "GITHUB_REF_NAME" not in workflow
    assert "github.ref_name" not in workflow
    assert "RELEASE_TAG#v" not in workflow
    assert workflow.count('version="$PYTHON_RELEASE_VERSION"') == 8

    pack = job_block(workflow, "pack-node-package")
    publish = job_block(workflow, "publish-node-npm")
    assert "validator_args+=(--allow-prerelease)" in pack
    assert "--allow-prerelease" not in publish


@pytest.mark.parametrize("job", MUTATING_RELEASE_JOBS)
def test_candidate_guard_gate_rejects_an_unguarded_mutation_job(job: str) -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    block = job_block(workflow, job)
    guarded_line = f"    {STABLE_PUBLICATION_GUARD}\n"
    assert guarded_line in block
    hostile_block = block.replace(guarded_line, "", 1)
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
def test_python_npm_contract_rejects_restored_cargo_release_paths(marker: str) -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")

    with pytest.raises(AssertionError):
        assert_no_cargo_publication_path(f"{workflow}\n# hostile: {marker}\n")


def test_python_npm_publication_is_serial_after_global_candidate_gates() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    preflight = job_block(workflow, "channel-preflight")
    node_publish = job_block(workflow, "publish-node-npm")
    core_publish = job_block(workflow, "publish-core-pypi")
    root_publish = job_block(workflow, "publish-python-pypi")
    github_release = job_block(workflow, "github-release")
    python_acceptance = job_block(workflow, "accept-python-artifacts")
    node_acceptance = job_block(workflow, "accept-node-package")

    assert needs_line(preflight) == (
        "    needs: [validate-release-identity, accept-python-artifacts, "
        "accept-node-package, accept-live-artifact-parity]"
    )
    assert needs_line(node_publish) == "    needs: channel-preflight"
    assert needs_line(core_publish) == (
        "    needs: [build-core-wheels, build-core-sdist, publish-node-npm]"
    )
    assert needs_line(root_publish) == ("    needs: [build-python, publish-core-pypi]")
    assert needs_line(github_release) == (
        "    needs: [publish-node-npm, publish-core-pypi, publish-python-pypi]"
    )
    assert "NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}" in preflight
    assert "NPM_TOKEN is required for an atomic cross-registry release." in preflight
    assert f"        {STABLE_PUBLICATION_GUARD}" in preflight
    assert "environment: release" in node_publish
    assert_stable_only_release_mutations(workflow)
    assert_no_cargo_publication_path(workflow)
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

    assert "Validate identity and legacy TypeDB package provenance" in identity
    command = "python scripts/ci/validate_release_identity.py"
    assert identity.count(command) == 1
    notice = "generate_native_dependency_notice.py --check"
    historical = "validate_historical_band9_registry.py"
    official = "validate_latest_typedb_driver_pin.py"
    assert notice in identity
    assert historical in identity
    assert "--committed-cutoff" in identity
    assert "owner-frozen official TypeDB 3.12.1" in identity
    assert identity.index(notice) < identity.index(historical) < identity.index(command)
    assert identity.index(command) < identity.index(official)
    assert "||" not in identity[identity.index(command) :]
    assert "validate-release-identity" in needs_line(preflight)


def test_npm_preflight_authenticates_package_write_access_without_registry_mutation() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    preflight = job_block(workflow, "channel-preflight")

    setup = preflight.index("actions/setup-node@49933ea5288caeca8642d1e84afbd3f7d6820020 # v4")
    pinned_cli = preflight.index("npm install --global --ignore-scripts npm@11.18.0")
    identity = preflight.index("npm whoami --registry=https://registry.npmjs.org")
    write_access = preflight.index('npm access list packages "$authenticated_user"')
    validation = preflight.index("python scripts/ci/validate_npm_package_access.py")

    assert setup < pinned_cli < identity < write_access < validation
    assert f"        {STABLE_PUBLICATION_GUARD}" in preflight
    assert 'test "$(npm --version)" = "11.18.0"' in preflight
    assert "node -p \"require('./type-bridge-core/crates/node/package.json').name\"" in preflight
    assert "--registry=https://registry.npmjs.org" in preflight
    assert 'mktemp "$RUNNER_TEMP/npm-package-access.XXXXXX.json"' in preflight
    assert '--access-json "$access_json"' in preflight
    assert '--package "$package_name"' in preflight
    assert "npm trust list" not in preflight
    assert "npm publish" not in preflight
    assert "npm access set" not in preflight
    assert "npm trust revoke" not in preflight

    validator = NPM_ACCESS_VALIDATOR.read_text(encoding="utf-8")
    assert "import json" in validator
    assert "subprocess" not in validator
    assert "urllib" not in validator
    assert 'permission != "read-write"' in validator
    assert "duplicate key" in validator


def test_legacy_crates_helper_requires_identical_registry_bytes() -> None:
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
    assert "030327872cad70433b3c8bde72529d0df6291af08ab3aad82550f8871e409364" in helper
    assert "a66de9d36b68e726e5a8ebbe1e81edb4e752ff3fbf140a84c3c306386e7169c5" in helper
    assert "440fa58f99b80028c658f66784c822450c98d30900276d34c8afbcc7b52b4ed4" in helper
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
    assert "type-bridge-typedb-driver-b7-${driver_b7_pin}.crate" not in probe
    assert "type-bridge-typedb-driver-b8-${driver_b8_pin}.crate" in probe
    assert "type-bridge-typedb-protocol-b8-${protocol_b8_pin}.crate" in probe
    for package in (
        "type-bridge-typedb-driver-b7",
        "type-bridge-typedb-protocol-b7",
        "type-bridge-typedb-driver-b8",
        "type-bridge-typedb-protocol-b8",
        "typedb-driver",
        "typedb-protocol",
    ):
        assert f'"{package}"' in probe
    assert 'dependency.get("name") == "typedb-protocol"' in probe
    assert 're.fullmatch(r"=([0-9]+\\.[0-9]+\\.[0-9]+)"' in probe
    assert "Official typedb-driver typedb-protocol requirement is not exact" in probe
    assert "Unexpected downstream band-9 fork" in probe
    assert 'not source.startswith("registry+")' in probe
    assert (
        '"type-bridge-typedb-driver-b7",\n    expected["type-bridge-typedb-driver-b7"],\n    registry=True'
        in probe
    )
    assert '{"default", "band7", "band8", "band9"}' in probe
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
        assert source.index("npm run typecheck:query-contract") < source.index(
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
        "verify_never_creates_databases_live",
        "adopt_legacy_history_then_evolve_live",
        "shipped_python_converter_to_native_adoption_live",
    )
    rust_integration = job_block(ci, "rust-integration")
    for test in tests:
        assert f"          {test}\n          -- --ignored --exact --nocapture" in rust_integration

    loop = re.search(
        r"for cli_live_test in \\\n(?P<tests>.*?); do\n(?P<body>.*?)\n    done",
        test_script,
        re.DOTALL,
    )
    assert loop is not None
    selected_tests = set(re.findall(r"^        ([a-z0-9_]+)(?: \\)?$", loop["tests"], re.MULTILINE))
    assert selected_tests == set(tests)
    assert '"$cli_live_test" -- --ignored --exact --nocapture' in loop["body"]
    assert 'TYPE_BRIDGE_TEST_PYTHON="$ROOT/.venv/bin/python"' in loop["body"]
    assert "timeout-minutes: 30" in rust_integration
    assert "uv sync --no-dev" in rust_integration
    assert "TYPE_BRIDGE_TEST_PYTHON: ${{ github.workspace }}/.venv/bin/python" in rust_integration
    assert test_script.count("timeout --foreground") >= 6


def test_remote_model_parity_is_explicitly_required_in_the_tls_lane() -> None:
    """Both public bindings must exercise the new remote terminal over verified TLS."""
    test_script = (REPO_ROOT / "test.sh").read_text(encoding="utf-8")
    start = test_script.index("run_tls_transport_steps() {")
    end = test_script.index('\n}\n\nif [[ "$tls" == 1 ]]', start)
    tls_lane = test_script[start:end]

    assert (
        "tests/integration/queries/test_remote_query_session_parity.py"
        "::test_public_remote_query_session_matches_direct_subtype_hydration"
    ) in tls_lane
    assert '"$NODE_DIR/tests/integration/queries/typed-remote-query-parity.test.ts"' in tls_lane
    for variable in (
        "TYPEDB_TLS_ADDRESS",
        "TYPEDB_TLS_HTTP_PORT",
        "TYPEDB_TLS_ROOT_CA",
        "SMOKE_TLS_CERT",
        "SMOKE_TLS_KEY",
    ):
        assert tls_lane.count(variable) >= 2
    assert 'NODE_EXTRA_CA_CERTS="$fixture_root_ca"' in tls_lane

    packed_reader = (REPO_ROOT / "tests/integration/parity/node_v2_authoring_reader.cjs").read_text(
        encoding="utf-8"
    )
    assert "TYPE_BRIDGE_V2_TYPEDB_TLS_ENABLED" in packed_reader
    assert "TYPE_BRIDGE_V2_TYPEDB_TLS_ROOT_CA" in packed_reader
    assert "tlsEnabled: true" in packed_reader
    assert "NODE_EXTRA_CA_CERTS" in packed_reader
    assert "NODE_TLS_REJECT_UNAUTHORIZED" not in packed_reader


def test_live_release_parity_consumes_exact_artifacts_before_every_publish() -> None:
    """The live F8 and V2 gates must execute uploaded candidates without rebuilding."""
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    acceptance = job_block(workflow, "accept-live-artifact-parity")
    smoke_server = job_block(workflow, "build-v2-smoke-server")

    assert needs_line(acceptance) == (
        "    needs: [build-core-wheels, build-python, pack-node-package, build-v2-smoke-server]"
    )
    assert re.findall(
        r'^          - python-version: "([^"]+)"\n'
        r'            typedb-server: "([^"]+)"\n'
        r'            expect-given: "([01])"\n'
        r'            expect-legacy-warning: "([01])"$',
        acceptance,
        re.MULTILINE,
    ) == [
        ("3.12", "typedb/typedb:3.8.3", "0", "1"),
        ("3.13.5", "typedb/typedb:3.11.5", "0", "0"),
        ("3.14", "typedb/typedb:3.12.1", "1", "0"),
    ]
    assert "image: ${{ matrix.typedb-server }}" in acceptance
    assert "python-version: ${{ matrix.python-version }}" in acceptance
    assert "- 1729:1729" in acceptance
    assert "- 8000:8000" in acceptance
    assert "curl --fail --silent http://localhost:8000/v1/version" in acceptance

    for artifact in (
        "core-wheels-linux-x86_64",
        "python-dist",
        "node-package",
        "v2-smoke-server",
    ):
        assert f"name: {artifact}" in acceptance
    assert "type_bridge_core-*linux*x86_64.whl" in acceptance
    assert "TYPE_BRIDGE_PARITY_CORE_WHEEL" in acceptance
    assert "TYPE_BRIDGE_PARITY_ROOT_WHEEL" in acceptance
    assert "TYPE_BRIDGE_PARITY_NODE_PACKAGE" in acceptance
    assert "TYPE_BRIDGE_V2_SMOKE_SERVER" in acceptance
    assert 'TYPE_BRIDGE_PARITY_STRICT: "1"' in acceptance
    assert "TYPE_BRIDGE_PARITY_EXPECT_GIVEN: ${{ matrix.expect-given }}" in acceptance
    assert (
        "TYPE_BRIDGE_PARITY_EXPECT_LEGACY_WARNING: ${{ matrix.expect-legacy-warning }}"
    ) in acceptance
    assert 'USE_DOCKER: "false"' in acceptance

    assert "uv venv" in acceptance
    assert "uv pip install" in acceptance
    assert 'for module_name in ("type_bridge", "type_bridge_core")' in acceptance
    assert "leaked to the source checkout" in acceptance
    assert "escaped the exact-wheel environment" in acceptance
    assert "test_live_typed_query_summary_and_f8_contract_match_built_artifacts" in acceptance
    assert "test_public_remote_query_session_matches_direct_subtype_hydration" in acceptance
    assert "--import-mode=importlib" in acceptance
    assert "release-live-parity.xml" in acceptance
    assert "release-v2-artifact-parity.xml" in acceptance
    assert "matrix.typedb-server == 'typedb/typedb:3.12.1'" in acceptance
    assert '"tests": 1' in acceptance
    assert '"failures": 0' in acceptance
    assert '"errors": 0' in acceptance
    assert '"skipped": 0' in acceptance

    for forbidden in (
        "uv sync",
        "uv build",
        "maturin",
        "npm ci",
        "npm pack",
        "npm run build:native",
        "npm run build:types",
        "cargo build",
        "actions/upload-artifact",
    ):
        assert forbidden not in acceptance

    assert needs_line(smoke_server) == "    needs: validate-release-identity"
    assert "toolchain: 1.94.1" in smoke_server
    assert (
        "cargo build --manifest-path type-bridge-core/Cargo.toml "
        "--release -p type-bridge-server --features v2-query "
        "--example v2_smoke_server"
    ) in " ".join(smoke_server.split())
    assert "name: v2-smoke-server" in smoke_server
    assert "path: type-bridge-core/target/release/examples/v2_smoke_server" in smoke_server
    assert "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02" in smoke_server
    assert "npm publish" not in smoke_server
    assert "cargo publish" not in smoke_server

    helper = (REPO_ROOT / "tests/integration/parity/cross_language.py").read_text(encoding="utf-8")
    assert 'PARITY_NODE_PACKAGE_ENV = "TYPE_BRIDGE_PARITY_NODE_PACKAGE"' in helper
    assert "if supplied_package is None:" in helper
    assert '"npm",\n                    "pack"' in helper

    source_reader = (
        REPO_ROOT / "tests/integration/parity/test_typed_query_live_parity.py"
    ).read_text(encoding="utf-8")
    wheel_reader = (REPO_ROOT / "tests/compat/typed_python/live.py").read_text(encoding="utf-8")
    node_reader = (REPO_ROOT / "tests/integration/parity/node_typed_query_reader.cjs").read_text(
        encoding="utf-8"
    )
    assert "TYPE_BRIDGE_PARITY_EXPECT_GIVEN" in source_reader
    assert "TYPE_BRIDGE_PARITY_EXPECT_GIVEN" in wheel_reader
    assert "TYPE_BRIDGE_PARITY_EXPECT_LEGACY_WARNING" in source_reader
    assert '"legacy_notices": legacy_notices' in wheel_reader
    assert "legacy_notices: legacyNotices" in node_reader
    assert "session.var(ParityQueryEnvelope)" in source_reader
    assert "QuerySession(connection).var(Envelope)" in wheel_reader
    assert "cannot materialize nested relation role" in source_reader
    assert "cannot materialize nested relation role" in wheel_reader
    assert ".eq(new EnvelopeCode(expected.relation_player.envelope_code))" in node_reader

    preflight = job_block(workflow, "channel-preflight")
    assert "accept-live-artifact-parity" in needs_line(preflight)
    assert needs_line(job_block(workflow, "publish-node-npm")) == ("    needs: channel-preflight")


def test_live_node_reader_consumes_supplied_tarball_without_npm_pack(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """The release path extracts its supplied tarball and never invokes npm."""
    artifact = tmp_path / "type-bridge-node-test.tgz"
    members = {
        "package/dist/index.js": b"module.exports = {};\n",
        "package/dist/typed/index.js": b"module.exports = {};\n",
        "package/dist/typed/index.d.ts": b"export {};\n",
        "package/type_bridge_node.linux-x64-gnu.node": b"native\n",
    }
    with tarfile.open(artifact, "w:gz") as archive:
        for name, content in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(content)
            archive.addfile(info, io.BytesIO(content))

    monkeypatch.setenv(cross_language.PARITY_NODE_PACKAGE_ENV, str(artifact))
    monkeypatch.setenv("NODE_OPTIONS", "--throw-deprecation")

    def fake_which(name: str) -> str | None:
        assert name == "node", "the supplied-artifact path must not inspect npm"
        return "/usr/bin/node"

    def fake_run(
        command: list[str],
        *,
        check: bool,
        cwd: Path,
        env: dict[str, str],
        capture_output: bool,
        text: bool,
    ) -> subprocess.CompletedProcess[str]:
        assert command == ["node", str(cross_language.PACKED_TYPED_QUERY_READER)]
        assert check is False
        assert capture_output is True
        assert text is True
        installed = cwd / "node_modules" / "@type-bridge" / "node"
        assert (installed / "dist/index.js").is_file()
        assert (installed / "dist/typed/index.js").is_file()
        assert (installed / "dist/typed/index.d.ts").is_file()
        assert (installed / "type_bridge_node.linux-x64-gnu.node").is_file()
        assert env["TYPE_BRIDGE_PACKED_CONSUMER_ROOT"] == str(cwd)
        assert env["TYPEDB_ADDRESS"] == "localhost:1729"
        assert env["TYPEDB_HTTP_PORT"] == "8000"
        assert env["TYPE_BRIDGE_PARITY_DATABASE"] == "artifact-parity"
        assert "TYPE_BRIDGE_NODE_NATIVE_PATH" not in env
        assert "NODE_OPTIONS" not in env
        payload = {
            "artifact": "packed",
            "legacy_notices": [],
            "summary": {"relation_player": "shallow"},
        }
        return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")

    monkeypatch.setattr(cross_language.shutil, "which", fake_which)
    monkeypatch.setattr(cross_language.subprocess, "run", fake_run)

    assert cross_language.read_typed_query_with_packed_node(
        "localhost:1729",
        "artifact-parity",
        http_port=8000,
    ) == {
        "artifact": "packed",
        "legacy_notices": [],
        "summary": {"relation_player": "shallow"},
    }


@pytest.mark.parametrize("tls_enabled", (False, True))
def test_live_v2_node_reader_consumes_query_v2_subpath_from_supplied_tarball(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    tls_enabled: bool,
) -> None:
    """The V2 release path resolves every facade from the immutable tarball."""
    artifact = tmp_path / "type-bridge-node-v2-test.tgz"
    members = {
        "package/dist/index.js": b"module.exports = {};\n",
        "package/dist/query-v2.js": b"module.exports = {};\n",
        "package/dist/query-v2.d.ts": b"export {};\n",
        "package/dist/typed/index.js": b"module.exports = {};\n",
        "package/dist/typed/index.d.ts": b"export {};\n",
        "package/type_bridge_node.linux-x64-gnu.node": b"native\n",
    }
    with tarfile.open(artifact, "w:gz") as archive:
        for name, content in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(content)
            archive.addfile(info, io.BytesIO(content))
    declared = tmp_path / "declared.json"
    declared.write_text("{}\n", encoding="utf-8")
    typedb_root = tmp_path / "typedb-root.pem"
    remote_root = tmp_path / "remote-root.pem"
    typedb_root.write_text("typedb root\n", encoding="utf-8")
    remote_root.write_text("remote root\n", encoding="utf-8")
    server_url = "https://127.0.0.1:18080" if tls_enabled else "http://127.0.0.1:18080"

    monkeypatch.setenv(cross_language.PARITY_NODE_PACKAGE_ENV, str(artifact))
    monkeypatch.setenv("NODE_EXTRA_CA_CERTS", "/ambient/must-not-leak.pem")

    def fake_which(name: str) -> str | None:
        assert name == "node", "the supplied-artifact path must not inspect npm"
        return "/usr/bin/node"

    def fake_run(
        command: list[str],
        *,
        check: bool,
        cwd: Path,
        env: dict[str, str],
        capture_output: bool,
        text: bool,
    ) -> subprocess.CompletedProcess[str]:
        assert command == ["node", str(cross_language.PACKED_V2_AUTHORING_READER)]
        assert check is False
        assert capture_output is True
        assert text is True
        installed = cwd / "node_modules" / "@type-bridge" / "node"
        for member in (
            "dist/index.js",
            "dist/query-v2.js",
            "dist/query-v2.d.ts",
            "dist/typed/index.js",
            "dist/typed/index.d.ts",
            "type_bridge_node.linux-x64-gnu.node",
        ):
            assert (installed / member).is_file()
        assert env["TYPE_BRIDGE_V2_DECLARED_FIXTURE"] == str(declared)
        assert env["TYPE_BRIDGE_V2_SERVER_URL"] == server_url
        assert "TYPE_BRIDGE_NODE_NATIVE_PATH" not in env
        assert env["TYPE_BRIDGE_V2_TYPEDB_TLS_ENABLED"] == ("1" if tls_enabled else "0")
        if tls_enabled:
            assert env["TYPE_BRIDGE_V2_TYPEDB_TLS_ROOT_CA"] == str(typedb_root.resolve())
            assert env["NODE_EXTRA_CA_CERTS"] == str(remote_root.resolve())
        else:
            assert "TYPE_BRIDGE_V2_TYPEDB_TLS_ROOT_CA" not in env
            assert "NODE_EXTRA_CA_CERTS" not in env
        payload = {
            "advanced": {"exchanges": 1},
            "artifact": "packed-v2",
            "model": {"exchanges": 1},
        }
        return subprocess.CompletedProcess(command, 0, json.dumps(payload), "")

    monkeypatch.setattr(cross_language.shutil, "which", fake_which)
    monkeypatch.setattr(cross_language.subprocess, "run", fake_run)

    assert cross_language.read_v2_authoring_with_packed_node(
        "localhost:1729",
        "artifact-parity",
        http_port=8000,
        declared_fixture=declared,
        server_url=server_url,
        typedb_tls_root_ca=typedb_root if tls_enabled else None,
        remote_tls_root_ca=remote_root if tls_enabled else None,
    ) == {
        "advanced": {"exchanges": 1},
        "artifact": "packed-v2",
        "model": {"exchanges": 1},
    }


def test_live_v2_node_reader_requires_both_tls_trust_roots(
    tmp_path: Path,
) -> None:
    """TypeDB TLS and remote HTTPS trust cannot be configured independently."""
    root = tmp_path / "root.pem"
    root.write_text("root\n", encoding="utf-8")

    with pytest.raises(AssertionError, match="requires both"):
        cross_language.read_v2_authoring_with_packed_node(
            "localhost:1729",
            "artifact-parity",
            http_port=8000,
            declared_fixture=tmp_path / "unused.json",
            server_url="https://127.0.0.1:18080",
            typedb_tls_root_ca=root,
        )


def test_strict_live_v2_node_reader_rejects_source_pack_fallback(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("TYPE_BRIDGE_PARITY_STRICT", "1")
    monkeypatch.delenv(cross_language.PARITY_NODE_PACKAGE_ENV, raising=False)

    with pytest.raises(
        AssertionError,
        match="strict V2 artifact parity requires TYPE_BRIDGE_PARITY_NODE_PACKAGE",
    ):
        cross_language.read_v2_authoring_with_packed_node(
            "localhost:1729",
            "artifact-parity",
            http_port=8000,
            declared_fixture=tmp_path / "unused.json",
            server_url="http://127.0.0.1:18080",
        )


@pytest.mark.parametrize(
    ("member_type", "link_target"),
    (
        (tarfile.SYMTYPE, "index.js"),
        (tarfile.LNKTYPE, "package/dist/index.js"),
    ),
)
def test_live_v2_node_reader_rejects_link_members_before_node(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    member_type: bytes,
    link_target: str,
) -> None:
    """A required packed entry cannot alias another archive member."""
    artifact = tmp_path / "type-bridge-node-v2-link.tgz"
    members = {
        "package/dist/index.js": b"module.exports = {};\n",
        "package/dist/query-v2.d.ts": b"export {};\n",
        "package/dist/typed/index.js": b"module.exports = {};\n",
        "package/dist/typed/index.d.ts": b"export {};\n",
        "package/type_bridge_node.linux-x64-gnu.node": b"native\n",
    }
    with tarfile.open(artifact, "w:gz") as archive:
        for name, content in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(content)
            archive.addfile(info, io.BytesIO(content))
        linked = tarfile.TarInfo("package/dist/query-v2.js")
        linked.type = member_type
        linked.linkname = link_target
        archive.addfile(linked)
    declared = tmp_path / "declared.json"
    declared.write_text("{}\n", encoding="utf-8")

    monkeypatch.setenv(cross_language.PARITY_NODE_PACKAGE_ENV, str(artifact))
    monkeypatch.setattr(cross_language.shutil, "which", lambda name: f"/usr/bin/{name}")

    def unexpected_run(*args: object, **kwargs: object) -> None:
        raise AssertionError("Node must not execute for a linked package member")

    monkeypatch.setattr(cross_language.subprocess, "run", unexpected_run)

    with pytest.raises(AssertionError, match="non-regular member"):
        cross_language.read_v2_authoring_with_packed_node(
            "localhost:1729",
            "artifact-parity",
            http_port=8000,
            declared_fixture=declared,
            server_url="http://127.0.0.1:18080",
        )


def test_npm_publication_uses_the_accepted_tarball() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    build = job_block(workflow, "build-node-native")
    pack = job_block(workflow, "pack-node-package")
    acceptance = job_block(workflow, "accept-node-package")
    publish = job_block(workflow, "publish-node-npm")
    package = json.loads(
        (REPO_ROOT / "type-bridge-core/crates/node/package.json").read_text(encoding="utf-8")
    )
    assert package["scripts"]["clean:types"] == "node scripts/clean-types.js"
    assert package["scripts"]["build:types"] == ("npm run clean:types && tsc -p tsconfig.json")

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
        "npm run typecheck:query-contract",
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
    assert "npm run smoke:legacy-package -- --artifact-directory" in acceptance
    assert "npm run build:native" not in acceptance
    assert "npm run build:types" not in acceptance
    assert "npm pack" not in acceptance
    assert "actions/upload-artifact" not in acceptance

    assert needs_line(publish) == "    needs: channel-preflight"
    assert "name: node-package" in publish
    assert publish.count("scripts/ci/validate_node_release_package.py") == 2
    assert "--repository-package type-bridge-core/crates/node/package.json" in publish
    assert publish.count('--tag "$RELEASE_TAG"') == 2
    assert "--allow-prerelease" not in publish
    assert "environment: release" in publish
    assert f"    {STABLE_PUBLICATION_GUARD}" in publish
    identity_position = publish.index("scripts/ci/validate_node_release_package.py")
    publish_position = publish.index('npm publish "${packages[0]}" --access public')
    assert identity_position < publish_position
    assert "dist.integrity" in publish
    assert '--registry-integrity "$registry_integrity"' in publish
    assert 'npm publish "${packages[0]}" --access public' in publish
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
