#!/usr/bin/env bash
# Preflight or publish one crate without accepting an unknown immutable version.
set -euo pipefail

usage() {
  echo "usage: publish_crate_idempotently.sh [--preflight|--verify-preexisting|--cutoff-state] CRATE" >&2
  exit 2
}

mode="publish"
case "${1:-}" in
  --preflight)
    mode="preflight"
    shift
    ;;
  --verify-preexisting)
    mode="verify-preexisting"
    shift
    ;;
  --cutoff-state)
    mode="cutoff-state"
    shift
    ;;
esac
[[ $# -eq 1 ]] || usage
crate="$1"

cargo_bin="${CARGO_BIN:-cargo}"
curl_bin="${CURL_BIN:-curl}"
python_bin="${PYTHON_BIN:-python3}"
sha256_bin="${SHA256_BIN:-sha256sum}"
sleep_bin="${SLEEP_BIN:-sleep}"
verify_attempts="${CRATES_IO_VERIFY_ATTEMPTS:-12}"
verify_delay="${CRATES_IO_VERIFY_DELAY_SECONDS:-5}"
publish_attempts="${CRATES_IO_PUBLISH_ATTEMPTS:-5}"
publish_backoff="${CRATES_IO_PUBLISH_INITIAL_BACKOFF_SECONDS:-10}"

if [[ ! "$crate" =~ ^[A-Za-z0-9_-]+$ ]]; then
  echo "Invalid crate name: $crate" >&2
  exit 2
fi
if [[ "$mode" == "cutoff-state" && "$crate" != "type-bridge-core-lib" ]]; then
  echo "--cutoff-state is restricted to the type-bridge-core-lib graph witness" >&2
  exit 2
fi
if [[ ! "$verify_attempts" =~ ^[1-9][0-9]*$ ]]; then
  echo "CRATES_IO_VERIFY_ATTEMPTS must be a positive integer" >&2
  exit 2
fi
if [[ ! "$verify_delay" =~ ^[0-9]+$ ]]; then
  echo "CRATES_IO_VERIFY_DELAY_SECONDS must be a non-negative integer" >&2
  exit 2
fi
if [[ ! "$publish_attempts" =~ ^[1-9][0-9]*$ || "$publish_attempts" -gt 10 ]]; then
  echo "CRATES_IO_PUBLISH_ATTEMPTS must be an integer from 1 through 10" >&2
  exit 2
fi
if [[ ! "$publish_backoff" =~ ^[0-9]+$ || "$publish_backoff" -gt 600 ]]; then
  echo "CRATES_IO_PUBLISH_INITIAL_BACKOFF_SECONDS must be an integer from 0 through 600" >&2
  exit 2
fi

pkgid="$($cargo_bin pkgid -p "$crate")"
version="${pkgid##*@}"
if [[ -z "$version" || "$version" == "$pkgid" || ! "$version" =~ ^[0-9A-Za-z.+-]+$ ]]; then
  echo "Could not resolve version for crate: $crate" >&2
  exit 1
fi

# This is intentionally a closed map. Historical b7 packages are consumed as
# immutable registry inputs, so their committed registry checksums are the
# identity authority and the ordinary release workflow can never replace their
# keys. The newly authorized b8 packages follow the ordinary candidate path.
pinned_registry_checksum() {
  case "$1@$2" in
    type-bridge-typedb-protocol-b7@3.7.0)
      printf '%s\n' '030327872cad70433b3c8bde72529d0df6291af08ab3aad82550f8871e409364'
      ;;
    type-bridge-typedb-driver-b7@3.8.1)
      printf '%s\n' '68c5770db7d2bc36c13a24a9fe37e5841e26b2adbeca4d06489a6689685e651d'
      ;;
    *)
      return 4
      ;;
  esac
}

lowercase_crate="$(printf '%s' "$crate" | tr '[:upper:]' '[:lower:]')"
case "${#lowercase_crate}" in
  1)
    sparse_path="1/${lowercase_crate}"
    ;;
  2)
    sparse_path="2/${lowercase_crate}"
    ;;
  3)
    sparse_path="3/${lowercase_crate:0:1}/${lowercase_crate}"
    ;;
  *)
    sparse_path="${lowercase_crate:0:2}/${lowercase_crate:2:2}/${lowercase_crate}"
    ;;
esac
api_url="https://crates.io/api/v1/crates/${crate}/${version}"
sparse_url="https://index.crates.io/${sparse_path}"
api_response_file="$(mktemp)"
sparse_response_file="$(mktemp)"
trap 'rm -f -- "$api_response_file" "$sparse_response_file"' EXIT

# Print the exact-version API checksum. Return 4 when absent and 5 for an
# unavailable or malformed response.
fetch_registry_checksum() {
  local status checksum
  if ! status="$($curl_bin -sS --retry 3 --retry-delay 2 \
    -H "User-Agent: ds1sqe/type-bridge release workflow" \
    -o "$api_response_file" \
    -w "%{http_code}" \
    "$api_url")"; then
    echo "Could not query crates.io API for ${crate}@${version}" >&2
    return 5
  fi

  case "$status" in
    200)
      if ! checksum="$($python_bin - "$api_response_file" "$lowercase_crate" "$version" <<'PY'
import json
import pathlib
import re
import sys

path = pathlib.Path(sys.argv[1])
expected_name = sys.argv[2]
expected_version = sys.argv[3]
try:
    payload = json.loads(path.read_text(encoding="utf-8"))
    version = payload["version"]
    checksum = version["checksum"]
except (OSError, UnicodeDecodeError, json.JSONDecodeError, KeyError, TypeError) as error:
    raise SystemExit(f"Malformed crates.io version response: {error}") from error
name = version.get("crate")
if not isinstance(name, str) or name.lower() != expected_name:
    raise SystemExit("Crates.io API package identity mismatch")
if version.get("num") != expected_version:
    raise SystemExit("Crates.io API package version mismatch")
if version.get("yanked") is not False:
    raise SystemExit("Crates.io API exact version is yanked or lacks yanked=false")
if not isinstance(checksum, str) or re.fullmatch(r"[0-9a-f]{64}", checksum) is None:
    raise SystemExit("Malformed crates.io version checksum")
print(checksum)
PY
      )"; then
        echo "Could not validate crates.io API checksum for ${crate}@${version}" >&2
        return 5
      fi
      printf '%s\n' "$checksum"
      ;;
    404)
      return 4
      ;;
    *)
      echo "Could not confirm crates.io API state for ${crate}@${version} (HTTP ${status})" >&2
      return 5
      ;;
  esac
}

# Print the exact-version sparse-index checksum and require it to be usable.
# Return 4 when the crate/version key is absent and 5 for malformed, yanked, or
# unavailable state.
fetch_registry_index_checksum() {
  local status checksum parse_status
  if ! status="$($curl_bin -sS --retry 3 --retry-delay 2 \
    -H "User-Agent: ds1sqe/type-bridge release workflow" \
    -o "$sparse_response_file" \
    -w "%{http_code}" \
    "$sparse_url")"; then
    echo "Could not query crates.io sparse index for ${crate}@${version}" >&2
    return 5
  fi

  case "$status" in
    200)
      if checksum="$($python_bin - "$sparse_response_file" "$lowercase_crate" "$version" <<'PY'
import json
import pathlib
import re
import sys

path = pathlib.Path(sys.argv[1])
expected_name = sys.argv[2]
expected_version = sys.argv[3]
matches = []
try:
    lines = path.read_text(encoding="utf-8").splitlines()
except (OSError, UnicodeDecodeError) as error:
    raise SystemExit(f"Malformed crates.io sparse-index response: {error}") from error
for line in lines:
    try:
        entry = json.loads(line)
    except json.JSONDecodeError as error:
        raise SystemExit(f"Malformed crates.io sparse-index entry: {error}") from error
    if not isinstance(entry, dict):
        raise SystemExit("Malformed crates.io sparse-index entry")
    if entry.get("vers") == expected_version:
        matches.append(entry)
if not matches:
    raise SystemExit(4)
if len(matches) != 1:
    raise SystemExit("Duplicate exact-version entries in crates.io sparse index")
entry = matches[0]
name = entry.get("name")
checksum = entry.get("cksum")
if not isinstance(name, str) or name.lower() != expected_name:
    raise SystemExit("Crates.io sparse-index package identity mismatch")
if not isinstance(checksum, str) or re.fullmatch(r"[0-9a-f]{64}", checksum) is None:
    raise SystemExit("Malformed crates.io sparse-index checksum")
if entry.get("yanked") is not False:
    raise SystemExit("Crates.io sparse-index exact version is yanked or lacks yanked=false")
print(checksum)
PY
      )"; then
        printf '%s\n' "$checksum"
        return 0
      else
        parse_status=$?
      fi
      if [[ "$parse_status" -eq 4 ]]; then
        return 4
      fi
      echo "Could not validate crates.io sparse-index state for ${crate}@${version}" >&2
      return 5
      ;;
    404)
      return 4
      ;;
    *)
      echo "Could not confirm crates.io sparse-index state for ${crate}@${version} (HTTP ${status})" >&2
      return 5
      ;;
  esac
}

require_matching_registry_checksum() {
  local expected_checksum="$1"
  local attempt registry_checksum fetch_status
  for ((attempt = 1; attempt <= verify_attempts; attempt++)); do
    if registry_checksum="$(fetch_registry_checksum)"; then
      if [[ "$registry_checksum" == "$expected_checksum" ]]; then
        echo "Verified crates.io API checksum for ${crate}@${version}: ${expected_checksum}"
        return 0
      fi
      echo "crates.io API checksum mismatch for ${crate}@${version}: expected=${expected_checksum}, registry=${registry_checksum}" >&2
      return 1
    else
      fetch_status=$?
    fi
    if (( attempt == verify_attempts )); then
      echo "crates.io API did not expose verifiable ${crate}@${version} after ${verify_attempts} attempts (last status ${fetch_status})" >&2
      return 1
    fi
    "$sleep_bin" "$verify_delay"
  done
}

require_matching_registry_index_checksum() {
  local expected_checksum="$1"
  local attempt registry_checksum fetch_status
  for ((attempt = 1; attempt <= verify_attempts; attempt++)); do
    if registry_checksum="$(fetch_registry_index_checksum)"; then
      if [[ "$registry_checksum" == "$expected_checksum" ]]; then
        echo "Verified crates.io sparse-index checksum for ${crate}@${version}: ${expected_checksum}"
        return 0
      fi
      echo "crates.io sparse-index checksum mismatch for ${crate}@${version}: expected=${expected_checksum}, registry=${registry_checksum}" >&2
      return 1
    else
      fetch_status=$?
    fi
    if (( attempt == verify_attempts )); then
      echo "crates.io sparse index did not expose verifiable ${crate}@${version} after ${verify_attempts} attempts (last status ${fetch_status})" >&2
      return 1
    fi
    "$sleep_bin" "$verify_delay"
  done
}

if pinned_checksum="$(pinned_registry_checksum "$crate" "$version")"; then
  require_matching_registry_checksum "$pinned_checksum"
  require_matching_registry_index_checksum "$pinned_checksum"
  echo "Verified pinned pre-existing ${crate}@${version}; no package or publish attempted."
  exit 0
fi
if [[ "$mode" == "verify-preexisting" ]]; then
  echo "No committed checksum authority for pre-existing ${crate}@${version}" >&2
  exit 1
fi

crate_file="target/package/${crate}-${version}.crate"

package_candidate() {
  # Cargo can overwrite a shorter archive without truncating a longer archive
  # left by the patched graph preflight. Remove only this resolved package file
  # first so stale trailing bytes cannot change the immutable checksum.
  if [[ -e "$crate_file" || -L "$crate_file" ]]; then
    if [[ ! -f "$crate_file" || -L "$crate_file" ]]; then
      echo "Refusing to replace non-regular packaged crate: $crate_file" >&2
      return 1
    fi
    rm -f -- "$crate_file"
  fi
  "$cargo_bin" package --locked -p "$crate" >/dev/null
}

if [[ "$mode" == "publish" || "$mode" == "cutoff-state" ]]; then
  # `cargo publish` packages the same clean tree. Materialize that payload first
  # so an existing immutable version can be compared before any upload attempt.
  package_candidate
fi

calculate_candidate_checksum() {
  local checksum_output candidate
  if [[ ! -f "$crate_file" || -L "$crate_file" ]]; then
    echo "Packaged crate is missing or non-regular: $crate_file" >&2
    return 1
  fi
  checksum_output="$($sha256_bin "$crate_file")"
  candidate="${checksum_output%%[[:space:]]*}"
  if [[ ! "$candidate" =~ ^[0-9a-f]{64}$ ]]; then
    echo "Could not calculate a lowercase SHA-256 for $crate_file" >&2
    return 1
  fi
  printf '%s\n' "$candidate"
}

candidate_checksum="$(calculate_candidate_checksum)"

# A candidate key is safe only if both crates.io authorities agree that it is
# absent, or both expose the exact candidate checksum as a non-yanked version.
preflight_candidate_key() {
  local api_checksum api_status index_checksum index_status
  if api_checksum="$(fetch_registry_checksum)"; then
    api_status=0
  else
    api_status=$?
  fi
  if index_checksum="$(fetch_registry_index_checksum)"; then
    index_status=0
  else
    index_status=$?
  fi

  # The graph preflight uses local patches so every unpublished dependent crate
  # can be packaged before registry mutation. Those patches add metadata to the
  # archive's generated Cargo.lock. Once a key exists, rebuild it through the
  # exact unpatched publish path before comparing immutable bytes.
  if [[ "$mode" == "preflight" && ("$api_status" -eq 0 || "$index_status" -eq 0) ]]; then
    package_candidate
    candidate_checksum="$(calculate_candidate_checksum)"
  fi

  if [[ "$api_status" -eq 4 && "$index_status" -eq 4 ]]; then
    echo "Verified crates.io key is absent for ${crate}@${version}."
    return 4
  fi
  if [[ "$api_status" -eq 5 || "$index_status" -eq 5 ]]; then
    echo "crates.io API and sparse index disagree or are unavailable for ${crate}@${version}: api_status=${api_status}, index_status=${index_status}" >&2
    return 1
  fi
  if [[ "$api_status" -eq 0 && "$api_checksum" != "$candidate_checksum" ]]; then
    echo "crates.io API checksum mismatch for ${crate}@${version}: candidate=${candidate_checksum}, registry=${api_checksum}" >&2
    return 1
  fi
  if [[ "$index_status" -eq 0 && "$index_checksum" != "$candidate_checksum" ]]; then
    echo "crates.io sparse-index checksum mismatch for ${crate}@${version}: candidate=${candidate_checksum}, registry=${index_checksum}" >&2
    return 1
  fi
  if [[ "$api_status" -eq 4 ]]; then
    require_matching_registry_checksum "$candidate_checksum" || return 1
  fi
  if [[ "$index_status" -eq 4 ]]; then
    require_matching_registry_index_checksum "$candidate_checksum" || return 1
  fi
  echo "${crate}@${version} already published with identical non-yanked bytes."
  return 0
}

if preflight_candidate_key; then
  exit 0
else
  preflight_status=$?
fi
if [[ "$preflight_status" -ne 4 ]]; then
  exit 1
fi
if [[ "$mode" == "cutoff-state" ]]; then
  # Exit 4 is the closed signal that the immutable graph witness does not yet
  # exist at either crates.io authority. Any partial or mismatched state has
  # already failed closed above.
  exit 4
fi
if [[ "$mode" == "preflight" ]]; then
  exit 0
fi

for ((publish_attempt = 1; publish_attempt <= publish_attempts; publish_attempt++)); do
  if publish_output="$($cargo_bin publish --locked --registry crates-io -p "$crate" 2>&1)"; then
    printf '%s\n' "$publish_output"
    require_matching_registry_checksum "$candidate_checksum"
    require_matching_registry_index_checksum "$candidate_checksum"
    exit 0
  fi
  if ! grep -qiE "status 429|429 Too Many Requests" <<<"$publish_output"; then
    break
  fi
  printf '%s\n' "$publish_output" >&2
  if (( publish_attempt == publish_attempts )); then
    echo "${crate}@${version} remained rate-limited after ${publish_attempts} publish attempts" >&2
    exit 1
  fi
  echo "${crate}@${version} was rate-limited; retrying publish in ${publish_backoff}s (attempt $((publish_attempt + 1))/${publish_attempts})." >&2
  "$sleep_bin" "$publish_backoff"
  publish_backoff=$((publish_backoff * 2))
done

printf '%s\n' "$publish_output" >&2
if ! grep -qiE \
  "already (uploaded|exists|published)|crate .* already exists on crates.io index" \
  <<<"$publish_output"; then
  echo "${crate}@${version} publish failed (not already-published)" >&2
  exit 1
fi

# Another publisher may have won the race between the initial absence check and
# upload. Treat that as success only after both immutable authorities match.
require_matching_registry_checksum "$candidate_checksum"
require_matching_registry_index_checksum "$candidate_checksum"
echo "${crate}@${version} already published with identical non-yanked bytes; skipping."
