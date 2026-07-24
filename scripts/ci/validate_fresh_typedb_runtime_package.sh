#!/usr/bin/env bash
# Resolve and compile the extracted default multiband runtime from a fresh lock.
set -euo pipefail

package_dir="${1:?usage: validate_fresh_typedb_runtime_package.sh PACKAGE_DIR RELEASE_VERSION}"
release_version="${2:?usage: validate_fresh_typedb_runtime_package.sh PACKAGE_DIR RELEASE_VERSION}"
cargo_bin="${CARGO_BIN:-cargo}"
python_bin="${PYTHON_BIN:-python3}"
tar_bin="${TAR_BIN:-tar}"

if [[ ! "$release_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Invalid release version: $release_version" >&2
  exit 2
fi

runtime_archive="$package_dir/type-bridge-typedb-runtime-${release_version}.crate"
core_archive="$package_dir/type-bridge-core-lib-${release_version}.crate"
contract_archive="$package_dir/type-bridge-contract-${release_version}.crate"
for archive in "$runtime_archive" "$core_archive" "$contract_archive"; do
  if [[ ! -f "$archive" || -L "$archive" ]]; then
    echo "Expected one regular packaged crate: $archive" >&2
    exit 1
  fi
done

probe_root="$(mktemp -d)"
trap 'rm -rf -- "$probe_root"' EXIT
extract_root="$probe_root/extracted"
consumer_root="$probe_root/consumer"
mkdir -p "$extract_root" "$consumer_root/src"
for archive in "$runtime_archive" "$core_archive" "$contract_archive"; do
  "$tar_bin" -xzf "$archive" -C "$extract_root"
done

runtime_root="$extract_root/type-bridge-typedb-runtime-${release_version}"
core_root="$extract_root/type-bridge-core-lib-${release_version}"
contract_root="$extract_root/type-bridge-contract-${release_version}"

readarray -t driver_pins < <("$python_bin" - "$runtime_root" <<'PY'
import pathlib
import re
import sys
import tomllib

runtime_root = pathlib.Path(sys.argv[1])
runtime_source = runtime_root / "src/lib.rs"
runtime_manifest = runtime_root / "Cargo.toml"
for path in (runtime_source, runtime_manifest):
    if not path.is_file() or path.is_symlink():
        raise SystemExit(f"Extracted runtime member is missing or non-regular: {path}")

source = runtime_source.read_text(encoding="utf-8")
payload = tomllib.loads(runtime_manifest.read_text(encoding="utf-8"))
dependencies = payload.get("dependencies", {})
authorities = (
    (
        "type-bridge-typedb-driver-b7",
        "PINNED_DRIVER_VERSION_B7",
    ),
    (
        "type-bridge-typedb-driver-b8",
        "PINNED_DRIVER_VERSION",
    ),
    (
        "typedb-driver",
        "PINNED_DRIVER_VERSION_B9",
    ),
)
for dependency, constant in authorities:
    matches = re.findall(
        rf'^pub const {constant}: &str = "([^"]+)";$',
        source,
        re.MULTILINE,
    )
    if len(matches) != 1:
        raise SystemExit(f"Extracted runtime must define {constant} exactly once")
    pin = matches[0]
    if re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", pin) is None:
        raise SystemExit(f"Extracted runtime has an invalid {constant}: {pin!r}")
    specification = dependencies.get(dependency, {})
    requirement = specification.get("version")
    package = specification.get("package", dependency)
    if requirement != f"={pin}":
        raise SystemExit(
            f"Extracted runtime {dependency} requirement disagrees with {constant}: "
            f"actual={requirement!r}, expected='={pin}'"
        )
    if package != dependency:
        raise SystemExit(
            f"Extracted runtime {dependency} has the wrong package identity: "
            f"actual={package!r}"
        )
    print(pin)
PY
)
if [[ ${#driver_pins[@]} -ne 3 ]]; then
  echo "Extracted runtime did not yield exactly three driver pins" >&2
  exit 1
fi
driver_b7_pin="${driver_pins[0]}"
driver_b8_pin="${driver_pins[1]}"
driver_b9_pin="${driver_pins[2]}"

driver_b8_archive="$package_dir/type-bridge-typedb-driver-b8-${driver_b8_pin}.crate"
if [[ ! -f "$driver_b8_archive" || -L "$driver_b8_archive" ]]; then
  echo "Expected one regular packaged crate: $driver_b8_archive" >&2
  exit 1
fi
"$tar_bin" -xzf "$driver_b8_archive" -C "$extract_root"
driver_b8_root="$extract_root/type-bridge-typedb-driver-b8-${driver_b8_pin}"

readarray -t protocol_pins < <("$python_bin" - \
  "$driver_b8_root" <<'PY'
import pathlib
import re
import sys
import tomllib

authorities = ((pathlib.Path(sys.argv[1]), "type-bridge-typedb-protocol-b8"),)
for driver_root, expected_package in authorities:
    manifest = driver_root / "Cargo.toml"
    if not manifest.is_file() or manifest.is_symlink():
        raise SystemExit(f"Extracted driver manifest is missing or non-regular: {manifest}")
    payload = tomllib.loads(manifest.read_text(encoding="utf-8"))
    specification = payload.get("dependencies", {}).get("typedb-protocol", {})
    package = specification.get("package")
    requirement = specification.get("version")
    if package != expected_package:
        raise SystemExit(
            f"Extracted driver does not name {expected_package}: actual={package!r}"
        )
    if not isinstance(requirement, str) or re.fullmatch(
        r"=[0-9]+\.[0-9]+\.[0-9]+", requirement
    ) is None:
        raise SystemExit(
            f"Extracted driver protocol requirement is not exact: actual={requirement!r}"
        )
    print(requirement[1:])
PY
)
if [[ ${#protocol_pins[@]} -ne 1 ]]; then
  echo "Extracted b8 driver did not yield exactly one compatibility protocol pin" >&2
  exit 1
fi
protocol_b7_pin="3.7.0"
protocol_b8_pin="${protocol_pins[0]}"

# Both b7 packages are immutable pre-existing crates.io packages and are never
# repackaged. The expected-new b8 packages are validated from candidate archives.
protocol_b8_archive="$package_dir/type-bridge-typedb-protocol-b8-${protocol_b8_pin}.crate"
if [[ ! -f "$protocol_b8_archive" || -L "$protocol_b8_archive" ]]; then
  echo "Expected one regular packaged crate: $protocol_b8_archive" >&2
  exit 1
fi
"$tar_bin" -xzf "$protocol_b8_archive" -C "$extract_root"
protocol_b8_root="$extract_root/type-bridge-typedb-protocol-b8-${protocol_b8_pin}"

"$python_bin" - \
  "$runtime_root" \
  "$core_root" \
  "$contract_root" \
  "$driver_b8_root" \
  "$protocol_b8_root" \
  "$driver_b7_pin" \
  "$driver_b8_pin" \
  "$driver_b9_pin" \
  "$protocol_b7_pin" \
  "$protocol_b8_pin" \
  "$consumer_root/Cargo.toml" \
  "$consumer_root/expected-versions.json" <<'PY'
import json
import pathlib
import sys
import tomllib

(
    runtime_root,
    core_root,
    contract_root,
    driver_b8_root,
    protocol_b8_root,
) = map(pathlib.Path, sys.argv[1:6])
driver_b7_pin, driver_b8_pin, driver_b9_pin = sys.argv[6:9]
protocol_b7_pin, protocol_b8_pin = sys.argv[9:11]
consumer_manifest = pathlib.Path(sys.argv[11])
expected_versions = pathlib.Path(sys.argv[12])

expected_packages = (
    (runtime_root, "type-bridge-typedb-runtime", None),
    (core_root, "type-bridge-core-lib", None),
    (contract_root, "type-bridge-contract", None),
    (driver_b8_root, "type-bridge-typedb-driver-b8", driver_b8_pin),
    (protocol_b8_root, "type-bridge-typedb-protocol-b8", protocol_b8_pin),
)
for root, expected_name, expected_version in expected_packages:
    manifest = root / "Cargo.toml"
    if not manifest.is_file() or manifest.is_symlink():
        raise SystemExit(f"Extracted crate manifest is missing or non-regular: {manifest}")
    package = tomllib.loads(manifest.read_text(encoding="utf-8")).get("package", {})
    if package.get("name") != expected_name:
        raise SystemExit(
            f"Extracted package has the wrong name: actual={package.get('name')!r}, "
            f"expected={expected_name!r}"
        )
    if expected_version is not None and package.get("version") != expected_version:
        raise SystemExit(
            f"Extracted {expected_name} has the wrong version: "
            f"actual={package.get('version')!r}, expected={expected_version!r}"
        )


def toml_string(path: pathlib.Path) -> str:
    return json.dumps(str(path.resolve()))


consumer_manifest.write_text(
    "\n".join(
        (
            "[workspace]",
            "",
            "[package]",
            'name = "type-bridge-fresh-runtime-consumer"',
            'version = "0.0.0"',
            'edition = "2024"',
            "publish = false",
            "",
            "[dependencies]",
            f"type-bridge-typedb-runtime = {{ path = {toml_string(runtime_root)} }}",
            "",
            "[patch.crates-io]",
            f"type-bridge-core-lib = {{ path = {toml_string(core_root)} }}",
            f"type-bridge-contract = {{ path = {toml_string(contract_root)} }}",
            f"type-bridge-typedb-driver-b8 = {{ path = {toml_string(driver_b8_root)} }}",
            f"type-bridge-typedb-protocol-b8 = {{ path = {toml_string(protocol_b8_root)} }}",
            "",
        )
    ),
    encoding="utf-8",
)
(consumer_manifest.parent / "src/main.rs").write_text("fn main() {}\n", encoding="utf-8")
expected_versions.write_text(
    json.dumps(
        {
            "type-bridge-typedb-driver-b7": driver_b7_pin,
            "type-bridge-typedb-protocol-b7": protocol_b7_pin,
            "type-bridge-typedb-driver-b8": driver_b8_pin,
            "type-bridge-typedb-protocol-b8": protocol_b8_pin,
            "typedb-driver": driver_b9_pin,
        },
        sort_keys=True,
    ),
    encoding="utf-8",
)
PY

if [[ -e "$consumer_root/Cargo.lock" ]]; then
  echo "Fresh consumer unexpectedly started with a Cargo.lock" >&2
  exit 1
fi
"$cargo_bin" metadata \
  --manifest-path "$consumer_root/Cargo.toml" \
  --format-version 1 > "$consumer_root/metadata.json"
if [[ ! -f "$consumer_root/Cargo.lock" ]]; then
  echo "Fresh consumer resolution did not create an independent Cargo.lock" >&2
  exit 1
fi

"$python_bin" - \
  "$consumer_root/metadata.json" \
  "$consumer_root/expected-versions.json" <<'PY'
import json
import pathlib
import re
import sys

metadata = json.loads(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
expected = json.loads(pathlib.Path(sys.argv[2]).read_text(encoding="utf-8"))
packages = metadata.get("packages", [])


def require_package(
    name: str, expected_version: str, *, registry: bool
) -> dict[str, object]:
    matches = [package for package in packages if package.get("name") == name]
    if len(matches) != 1:
        raise SystemExit(f"Expected exactly one {name}, found: {matches!r}")
    package = matches[0]
    actual = package.get("version")
    if actual != expected_version:
        raise SystemExit(
            f"Fresh downstream resolution escaped {name} pin: "
            f"actual={actual!r}, expected={expected_version!r}"
        )
    source = package.get("source")
    if registry and (not isinstance(source, str) or not source.startswith("registry+")):
        raise SystemExit(f"Fresh runtime did not resolve registry {name}: {package!r}")
    if not registry and source is not None:
        raise SystemExit(f"Fresh runtime did not use candidate path for {name}: {package!r}")
    return package


require_package(
    "type-bridge-typedb-driver-b7",
    expected["type-bridge-typedb-driver-b7"],
    registry=True,
)
require_package(
    "type-bridge-typedb-protocol-b7",
    expected["type-bridge-typedb-protocol-b7"],
    registry=True,
)
require_package(
    "type-bridge-typedb-driver-b8",
    expected["type-bridge-typedb-driver-b8"],
    registry=False,
)
require_package(
    "type-bridge-typedb-protocol-b8",
    expected["type-bridge-typedb-protocol-b8"],
    registry=False,
)
official_driver = require_package(
    "typedb-driver", expected["typedb-driver"], registry=True
)
protocol_dependencies = [
    dependency
    for dependency in official_driver.get("dependencies", [])
    if dependency.get("name") == "typedb-protocol"
    and dependency.get("kind") is None
]
if len(protocol_dependencies) != 1:
    raise SystemExit(
        "Official typedb-driver must declare exactly one normal typedb-protocol "
        f"dependency: {protocol_dependencies!r}"
    )
protocol_dependency = protocol_dependencies[0]
protocol_requirement = protocol_dependency.get("req")
protocol_source = protocol_dependency.get("source")
protocol_match = (
    re.fullmatch(r"=([0-9]+\.[0-9]+\.[0-9]+)", protocol_requirement)
    if isinstance(protocol_requirement, str)
    else None
)
if protocol_match is None:
    raise SystemExit(
        "Official typedb-driver typedb-protocol requirement is not exact: "
        f"{protocol_requirement!r}"
    )
if not isinstance(protocol_source, str) or not protocol_source.startswith("registry+"):
    raise SystemExit(
        "Official typedb-driver typedb-protocol dependency is not from a registry: "
        f"{protocol_dependency!r}"
    )
official_protocol_pin = protocol_match.group(1)
require_package("typedb-protocol", official_protocol_pin, registry=True)

for forbidden in (
    "type-bridge-typedb-driver-b9",
    "type-bridge-typedb-protocol-b9",
):
    matches = [package for package in packages if package.get("name") == forbidden]
    if matches:
        raise SystemExit(f"Unexpected downstream band-9 fork in fresh graph: {matches!r}")

resolve = metadata.get("resolve", {})
runtime_nodes = [
    node
    for node in resolve.get("nodes", [])
    if node.get("id", "").split("#")[-1].startswith("type-bridge-typedb-runtime@")
]
if len(runtime_nodes) != 1:
    raise SystemExit(f"Expected one runtime resolve node, found: {runtime_nodes!r}")
features = set(runtime_nodes[0].get("features", []))
if not {"default", "band7", "band8", "band9"} <= features:
    raise SystemExit(f"Fresh runtime did not activate its default multiband graph: {features!r}")

print(
    "fresh extracted default runtime resolved "
    f"b7 {expected['type-bridge-typedb-driver-b7']}/"
    f"{expected['type-bridge-typedb-protocol-b7']}, "
    f"b8 {expected['type-bridge-typedb-driver-b8']}/"
    f"{expected['type-bridge-typedb-protocol-b8']}, and "
    f"official b9 {expected['typedb-driver']}/{official_protocol_pin}"
)
PY

env RUSTFLAGS="-Dwarnings" "$cargo_bin" check \
  --manifest-path "$consumer_root/Cargo.toml" \
  --locked
