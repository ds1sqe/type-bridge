"""Release publication must consume artifacts that passed compatibility gates."""

from __future__ import annotations

import json
import re
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
CI_WORKFLOW = REPO_ROOT / ".github/workflows/ci.yml"
RELEASE_WORKFLOW = REPO_ROOT / ".github/workflows/release.yml"


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
    assert acceptance.count("scripts/ci/run_legacy_python_compat.py") == 2
    assert "type_bridge_core-*linux*x86_64.whl" in acceptance
    assert "type_bridge_core-*.tar.gz" in acceptance
    assert "type_bridge-*.tar.gz" in acceptance
    assert "scripts/ci/run_typed_python_artifact.py" in acceptance
    assert '"${root_wheels[0]}[typedb-driver]"' in acceptance
    assert '"${core_wheels[0]}"' in acceptance
    assert "tests/compat/typedb_driver_native/probe.py" in acceptance
    typed_runner = (REPO_ROOT / "scripts/ci/run_typed_python_artifact.py").read_text(
        encoding="utf-8"
    )
    assert '"pythonVersion": python_version' in typed_runner
    assert '"pythonVersion": "3.13"' not in typed_runner
    assert "uv build" not in acceptance
    assert "actions/upload-artifact" not in acceptance

    core_publish = job_block(workflow, "publish-core-pypi")
    root_publish = job_block(workflow, "publish-python-pypi")
    assert "accept-python-artifacts" in needs_line(core_publish)
    assert "accept-python-artifacts" in needs_line(root_publish)
    assert "pattern: core-wheels-*" in core_publish
    assert "merge-multiple: true" in core_publish
    assert "name: core-sdist" in core_publish
    assert "name: python-dist" in root_publish

    github_release = job_block(workflow, "github-release")
    assert "pattern: core-wheels-*" in github_release
    assert "name: core-sdist" in github_release
    assert "name: python-dist" in github_release


def test_crates_publication_waits_for_both_release_candidate_acceptance_gates() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")
    publish = job_block(workflow, "publish-crates")
    python_acceptance = job_block(workflow, "accept-python-artifacts")
    node_acceptance = job_block(workflow, "accept-node-package")

    assert needs_line(publish) == (
        "    needs: [validate-release-identity, accept-python-artifacts, accept-node-package]"
    )
    assert "publish-crates" not in needs_line(python_acceptance)
    assert "publish-crates" not in needs_line(node_acceptance)


def test_node_native_crate_configures_napi_platform_linking() -> None:
    """Direct Cargo builds must retain napi-rs's platform linker setup."""
    crate_root = REPO_ROOT / "type-bridge-core/crates/node"
    cargo = tomllib.loads((crate_root / "Cargo.toml").read_text(encoding="utf-8"))

    assert cargo["build-dependencies"]["napi-build"] == "2"
    assert (crate_root / "build.rs").read_text(encoding="utf-8") == (
        "fn main() {\n    napi_build::setup();\n}\n"
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
    assert "needs: test" in build
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
        "npm run test:unit",
        "npm run test:dts",
        "npm run smoke:package",
        'npm pack --ignore-scripts --pack-destination "$package_dir"',
        '--artifact "${packages[0]}"',
        "actions/upload-artifact@v4",
    )
    positions = [pack.index(gate) for gate in ordered_pack_gates]
    assert positions == sorted(positions)
    assert "--repository-package package.json" in pack
    assert '--tag "$GITHUB_REF_NAME"' in pack
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

    assert "needs: accept-node-package" in publish
    assert "name: node-package" in publish
    assert publish.count("scripts/ci/validate_node_release_package.py") == 2
    assert "--repository-package type-bridge-core/crates/node/package.json" in publish
    assert '--tag "$GITHUB_REF_NAME"' in publish
    identity_position = publish.index("scripts/ci/validate_node_release_package.py")
    publish_position = publish.index('npm publish "${packages[0]}" --access public')
    assert identity_position < publish_position
    assert "dist.integrity" in publish
    assert '--registry-integrity "$registry_integrity"' in publish
    assert 'npm publish "${packages[0]}" --access public' in publish

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
        preflight = publish.index("scripts/ci/verify_pypi_release_hashes.py")
        publisher = publish.index("pypa/gh-action-pypi-publish")
        post_publish = publish.rindex("scripts/ci/verify_pypi_release_hashes.py")
        assert preflight < publisher < post_publish
        assert f"--project {project}" in publish
        assert '--version "$version"' in publish
        assert "skip-existing: true" in publish
        assert "--require-existing" in publish[post_publish:]
        assert "--attempts 6" in publish[post_publish:]
