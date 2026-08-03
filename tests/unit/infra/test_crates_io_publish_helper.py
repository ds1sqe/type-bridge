"""Hermetic adversarial tests for crates.io immutable-version handling."""

from __future__ import annotations

import hashlib
import os
import subprocess
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
PUBLISH_HELPER = REPO_ROOT / "scripts/ci/publish_crate_idempotently.sh"
RELEASE_WORKFLOW = REPO_ROOT / ".github/workflows/release.yml"
CUTOFF_WITNESS = "type-bridge-core-lib"
CANDIDATE_BYTES = b"candidate crate bytes\n"
CANDIDATE_CHECKSUM = hashlib.sha256(CANDIDATE_BYTES).hexdigest()
PINNED_PROTOCOL_B7_CHECKSUM = "030327872cad70433b3c8bde72529d0df6291af08ab3aad82550f8871e409364"
PINNED_DRIVER_B7_CHECKSUM = "68c5770db7d2bc36c13a24a9fe37e5841e26b2adbeca4d06489a6689685e651d"
PINNED_PROTOCOL_B8_CHECKSUM = "a66de9d36b68e726e5a8ebbe1e81edb4e752ff3fbf140a84c3c306386e7169c5"
PINNED_DRIVER_B8_CHECKSUM = "440fa58f99b80028c658f66784c822450c98d30900276d34c8afbcc7b52b4ed4"


def executable(path: Path, source: str) -> Path:
    """Create one private executable test double."""
    path.write_text(source, encoding="utf-8")
    path.chmod(0o755)
    return path


def run_helper(
    tmp_path: Path,
    *,
    api_sequence: str,
    index_sequence: str,
    publish_mode: str = "unexpected",
    mode: str = "publish",
    crate: str = "demo-crate",
    version: str = "1.2.3",
    registry_checksum: str = CANDIDATE_CHECKSUM,
) -> tuple[subprocess.CompletedProcess[str], list[str], list[str]]:
    """Run the helper against deterministic cargo and crates.io doubles."""
    workspace = tmp_path / "workspace"
    tools = tmp_path / "tools"
    workspace.mkdir()
    tools.mkdir()
    command_log = tmp_path / "cargo-commands"
    curl_log = tmp_path / "curl-urls"
    api_state = tmp_path / "api-state"
    index_state = tmp_path / "index-state"

    if mode == "preflight":
        archive = workspace / f"target/package/{crate}-{version}.crate"
        archive.parent.mkdir(parents=True)
        archive.write_bytes(CANDIDATE_BYTES)

    cargo = executable(
        tools / "cargo",
        """#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >> "$COMMAND_LOG"
case "${1:-}" in
  pkgid)
    printf 'path+file:///fixture#%s@%s\\n' "$CRATE_NAME" "$CRATE_VERSION"
    ;;
  package)
    mkdir -p target/package
    printf '%s\\n' 'candidate crate bytes' > \
      "target/package/${CRATE_NAME}-${CRATE_VERSION}.crate"
    ;;
  publish)
    case "$CARGO_PUBLISH_MODE" in
      success)
        printf 'published %s\\n' "$CRATE_NAME"
        ;;
      already)
        printf 'crate %s@%s already exists on crates.io index\\n' \
          "$CRATE_NAME" "$CRATE_VERSION" >&2
        exit 101
        ;;
      *)
        printf '%s\\n' 'unexpected cargo publish invocation' >&2
        exit 42
        ;;
    esac
    ;;
  *)
    printf 'unexpected cargo command: %s\\n' "$*" >&2
    exit 43
    ;;
esac
""",
    )
    curl = executable(
        tools / "curl",
        """#!/usr/bin/env bash
set -euo pipefail
output=''
url="${!#}"
printf '%s\\n' "$url" >> "$CURL_LOG"
while (( $# > 0 )); do
  case "$1" in
    -o)
      output="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
if [[ -z "$output" ]]; then
  echo 'curl double received no response output path' >&2
  exit 2
fi
if [[ "$url" == https://crates.io/api/* ]]; then
  sequence="$API_SEQUENCE"
  state_file="$API_STATE_FILE"
  authority='api'
else
  sequence="$INDEX_SEQUENCE"
  state_file="$INDEX_STATE_FILE"
  authority='index'
fi
count=0
if [[ -f "$state_file" ]]; then
  count="$(<"$state_file")"
fi
IFS=',' read -r -a states <<< "$sequence"
index="$count"
if (( index >= ${#states[@]} )); then
  index=$((${#states[@]} - 1))
fi
state="${states[$index]}"
printf '%s\\n' "$((count + 1))" > "$state_file"

if [[ "$authority" == api ]]; then
  case "$state" in
    matching)
      printf '{"version":{"crate":"%s","num":"%s","yanked":false,"checksum":"%s"}}\\n' \\
        "$CRATE_NAME" "$CRATE_VERSION" "$REGISTRY_CHECKSUM" > "$output"
      printf '200'
      ;;
    mismatch)
      printf '{"version":{"crate":"%s","num":"%s","yanked":false,"checksum":"%064d"}}\\n' \\
        "$CRATE_NAME" "$CRATE_VERSION" 0 > "$output"
      printf '200'
      ;;
    yanked)
      printf '{"version":{"crate":"%s","num":"%s","yanked":true,"checksum":"%s"}}\\n' \\
        "$CRATE_NAME" "$CRATE_VERSION" "$REGISTRY_CHECKSUM" > "$output"
      printf '200'
      ;;
    missing)
      printf '{}\\n' > "$output"
      printf '404'
      ;;
    malformed)
      printf '{"version":{}}\\n' > "$output"
      printf '200'
      ;;
    *)
      echo "unknown API state: $state" >&2
      exit 3
      ;;
  esac
else
  case "$state" in
    matching)
      printf '{"name":"%s","vers":"%s","cksum":"%s","yanked":false}\\n' \
        "$CRATE_NAME" "$CRATE_VERSION" "$REGISTRY_CHECKSUM" > "$output"
      printf '200'
      ;;
    mismatch)
      printf '{"name":"%s","vers":"%s","cksum":"%064d","yanked":false}\\n' \
        "$CRATE_NAME" "$CRATE_VERSION" 0 > "$output"
      printf '200'
      ;;
    missing)
      printf '{}\\n' > "$output"
      printf '404'
      ;;
    absent-version)
      printf '{"name":"%s","vers":"0.0.1","cksum":"%064d","yanked":false}\\n' \
        "$CRATE_NAME" 0 > "$output"
      printf '200'
      ;;
    yanked)
      printf '{"name":"%s","vers":"%s","cksum":"%s","yanked":true}\\n' \
        "$CRATE_NAME" "$CRATE_VERSION" "$REGISTRY_CHECKSUM" > "$output"
      printf '200'
      ;;
    malformed)
      printf 'not-json\\n' > "$output"
      printf '200'
      ;;
    *)
      echo "unknown sparse-index state: $state" >&2
      exit 3
      ;;
  esac
fi
""",
    )

    env = os.environ.copy()
    env.update(
        {
            "API_SEQUENCE": api_sequence,
            "API_STATE_FILE": str(api_state),
            "CARGO_BIN": str(cargo),
            "CARGO_PUBLISH_MODE": publish_mode,
            "COMMAND_LOG": str(command_log),
            "CRATE_NAME": crate,
            "CRATE_VERSION": version,
            "CRATES_IO_VERIFY_ATTEMPTS": "3",
            "CRATES_IO_VERIFY_DELAY_SECONDS": "0",
            "CURL_BIN": str(curl),
            "CURL_LOG": str(curl_log),
            "INDEX_SEQUENCE": index_sequence,
            "INDEX_STATE_FILE": str(index_state),
            "REGISTRY_CHECKSUM": registry_checksum,
        }
    )
    arguments = ["bash", str(PUBLISH_HELPER)]
    if mode == "preflight":
        arguments.append("--preflight")
    elif mode == "verify-preexisting":
        arguments.append("--verify-preexisting")
    elif mode == "cutoff-state":
        arguments.append("--cutoff-state")
    elif mode != "publish":
        raise AssertionError(f"unknown helper mode: {mode}")
    arguments.append(crate)
    result = subprocess.run(
        arguments,
        cwd=workspace,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )
    commands = command_log.read_text(encoding="utf-8").splitlines() if command_log.exists() else []
    urls = curl_log.read_text(encoding="utf-8").splitlines() if curl_log.exists() else []
    return result, commands, urls


def test_existing_identical_crate_skips_without_publish(tmp_path: Path) -> None:
    result, commands, urls = run_helper(
        tmp_path,
        api_sequence="matching",
        index_sequence="matching",
    )

    assert result.returncode == 0, result.stderr
    assert "already published with identical non-yanked bytes" in result.stdout
    assert commands == ["pkgid -p demo-crate", "package --locked -p demo-crate"]
    assert urls == [
        "https://crates.io/api/v1/crates/demo-crate/1.2.3",
        "https://index.crates.io/de/mo/demo-crate",
    ]


@pytest.mark.parametrize(
    ("api_state", "index_state"),
    [("mismatch", "matching"), ("malformed", "matching"), ("matching", "yanked")],
)
def test_existing_unverifiable_crate_hard_fails_before_publish(
    tmp_path: Path,
    api_state: str,
    index_state: str,
) -> None:
    result, commands, _ = run_helper(
        tmp_path,
        api_sequence=api_state,
        index_sequence=index_state,
    )

    assert result.returncode != 0
    assert commands == ["pkgid -p demo-crate", "package --locked -p demo-crate"]
    assert "publish --locked" not in "\n".join(commands)


def test_already_exists_race_is_accepted_only_after_both_checksums_match(
    tmp_path: Path,
) -> None:
    result, commands, _ = run_helper(
        tmp_path,
        api_sequence="missing,matching",
        index_sequence="missing,matching",
        publish_mode="already",
    )

    assert result.returncode == 0, result.stderr
    assert commands[-1] == "publish --locked --registry crates-io -p demo-crate"
    assert "Verified crates.io API checksum" in result.stdout
    assert "Verified crates.io sparse-index checksum" in result.stdout


def test_already_exists_race_with_different_bytes_hard_fails(tmp_path: Path) -> None:
    result, commands, _ = run_helper(
        tmp_path,
        api_sequence="missing,mismatch",
        index_sequence="missing,matching",
        publish_mode="already",
    )

    assert result.returncode != 0
    assert commands[-1] == "publish --locked --registry crates-io -p demo-crate"
    assert "crates.io API checksum mismatch" in result.stderr


def test_successful_publish_polls_exact_sparse_index_version(tmp_path: Path) -> None:
    result, commands, _ = run_helper(
        tmp_path,
        api_sequence="missing,matching",
        index_sequence="missing,absent-version,matching",
        publish_mode="success",
    )

    assert result.returncode == 0, result.stderr
    assert commands[-1] == "publish --locked --registry crates-io -p demo-crate"
    assert "Verified crates.io sparse-index checksum" in result.stdout


def test_preflight_absent_key_hashes_existing_archive_without_publish(tmp_path: Path) -> None:
    result, commands, _ = run_helper(
        tmp_path,
        api_sequence="missing",
        index_sequence="absent-version",
        mode="preflight",
    )

    assert result.returncode == 0, result.stderr
    assert commands == ["pkgid -p demo-crate"]
    assert "Verified crates.io key is absent" in result.stdout


def test_preflight_rejects_inconsistent_api_and_index_visibility(tmp_path: Path) -> None:
    result, commands, _ = run_helper(
        tmp_path,
        api_sequence="missing",
        index_sequence="matching",
        mode="preflight",
    )

    assert result.returncode != 0
    assert commands == ["pkgid -p demo-crate"]
    assert "API did not expose verifiable" in result.stderr


@pytest.mark.parametrize(
    ("crate", "version", "checksum"),
    [
        ("type-bridge-typedb-protocol-b7", "3.7.0", PINNED_PROTOCOL_B7_CHECKSUM),
        ("type-bridge-typedb-driver-b7", "3.8.1", PINNED_DRIVER_B7_CHECKSUM),
    ],
)
def test_pinned_preexisting_crate_uses_committed_checksum_without_packaging(
    tmp_path: Path,
    crate: str,
    version: str,
    checksum: str,
) -> None:
    result, commands, _ = run_helper(
        tmp_path,
        api_sequence="matching",
        index_sequence="matching",
        mode="verify-preexisting",
        crate=crate,
        version=version,
        registry_checksum=checksum,
    )

    assert result.returncode == 0, result.stderr
    assert commands == [f"pkgid -p {crate}"]
    assert "no package or publish attempted" in result.stdout


def test_authorized_band8_absence_uses_the_ordinary_publish_path(tmp_path: Path) -> None:
    result, commands, _ = run_helper(
        tmp_path,
        api_sequence="missing,matching",
        index_sequence="missing,matching",
        publish_mode="success",
        crate="type-bridge-typedb-protocol-b8",
        version="3.11.0",
        registry_checksum=CANDIDATE_CHECKSUM,
    )

    assert result.returncode == 0, result.stderr
    assert commands == [
        "pkgid -p type-bridge-typedb-protocol-b8",
        "package --locked -p type-bridge-typedb-protocol-b8",
        "publish --locked --registry crates-io -p type-bridge-typedb-protocol-b8",
    ]


def test_verify_preexisting_rejects_unmapped_crate(tmp_path: Path) -> None:
    result, commands, _ = run_helper(
        tmp_path,
        api_sequence="matching",
        index_sequence="matching",
        mode="verify-preexisting",
    )

    assert result.returncode != 0
    assert commands == ["pkgid -p demo-crate"]
    assert "No committed checksum authority" in result.stderr


def test_cutoff_state_reports_a_matching_first_graph_crate_as_committed(
    tmp_path: Path,
) -> None:
    result, commands, urls = run_helper(
        tmp_path,
        api_sequence="matching",
        index_sequence="matching",
        mode="cutoff-state",
        crate=CUTOFF_WITNESS,
    )

    assert result.returncode == 0, result.stderr
    assert commands == [
        f"pkgid -p {CUTOFF_WITNESS}",
        f"package --locked -p {CUTOFF_WITNESS}",
    ]
    assert "publish --locked" not in "\n".join(commands)
    assert urls == [
        f"https://crates.io/api/v1/crates/{CUTOFF_WITNESS}/1.2.3",
        f"https://index.crates.io/ty/pe/{CUTOFF_WITNESS}",
    ]


def test_cutoff_state_reports_an_absent_first_graph_crate_with_exit_four(
    tmp_path: Path,
) -> None:
    result, commands, _ = run_helper(
        tmp_path,
        api_sequence="missing",
        index_sequence="missing",
        mode="cutoff-state",
        crate=CUTOFF_WITNESS,
    )

    assert result.returncode == 4, result.stderr
    assert commands == [
        f"pkgid -p {CUTOFF_WITNESS}",
        f"package --locked -p {CUTOFF_WITNESS}",
    ]
    assert "Verified crates.io key is absent" in result.stdout
    assert "publish --locked" not in "\n".join(commands)


@pytest.mark.parametrize(
    ("api_state", "index_state"),
    [
        ("matching", "mismatch"),
        ("missing", "matching"),
        ("matching", "yanked"),
        ("yanked", "matching"),
    ],
)
def test_cutoff_state_fails_closed_on_nonmatching_or_partial_registry_state(
    tmp_path: Path,
    api_state: str,
    index_state: str,
) -> None:
    result, commands, _ = run_helper(
        tmp_path,
        api_sequence=api_state,
        index_sequence=index_state,
        mode="cutoff-state",
        crate=CUTOFF_WITNESS,
    )

    assert result.returncode not in {0, 4}
    assert "publish --locked" not in "\n".join(commands)


def test_cutoff_state_is_closed_to_the_first_graph_crate(tmp_path: Path) -> None:
    result, commands, urls = run_helper(
        tmp_path,
        api_sequence="matching",
        index_sequence="matching",
        mode="cutoff-state",
    )

    assert result.returncode == 2
    assert commands == []
    assert urls == []
    assert f"restricted to the {CUTOFF_WITNESS} graph witness" in result.stderr


def test_cargo_inclusive_release_workflow_invokes_the_cargo_helper() -> None:
    workflow = RELEASE_WORKFLOW.read_text(encoding="utf-8")

    assert "--artifact-contract cargo-inclusive" in workflow
    assert PUBLISH_HELPER.name in workflow
    assert "--cutoff-state" not in workflow
    assert "publish-crates:" in workflow
