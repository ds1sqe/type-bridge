"""Cross-language parity helpers for Python writer and Node reader tests."""

from __future__ import annotations

import difflib
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
from datetime import datetime
from pathlib import Path, PurePosixPath
from typing import Any

import pytest

from tests.integration.parity.canonical import canonical_json, load_fixture_contract
from tests.integration.parity.models import (
    ParityActive,
    ParityAge,
    ParityBalance,
    ParityBirthDate,
    ParityCompany,
    ParityConfidence,
    ParityEmail,
    ParityEmailMessage,
    ParityId,
    ParityKind,
    ParityLoginAt,
    ParityMembership,
    ParityName,
    ParityNote,
    ParityPerson,
    ParityScore,
    ParitySeenAt,
    ParitySessionLength,
    ParitySince,
    ParityTag,
    ParityTokenOrigin,
)

REPO_ROOT = Path(__file__).resolve().parents[3]
NODE_READER = Path(__file__).with_name("node_reader.cjs")
NODE_PACKAGE_DIR = REPO_ROOT / "type-bridge-core" / "crates" / "node"
WRITE_DATA = Path(__file__).with_name("fixtures") / "write-data.json"
# The typed reader is compiled by `npm run build:parity-reader` (tsconfig.parity.json)
# into tmp/node-parity. It is a sibling to NODE_READER that reads the typed
# Entity()/Relation() surface and serializes through toDict(). See
# canonicalize_typed_reader_output for the descriptor-gate vs value-gate split.
TYPED_NODE_READER = REPO_ROOT / "tmp" / "node-parity" / "tests" / "parity" / "typed-reader.js"
PACKED_TYPED_QUERY_READER = Path(__file__).with_name("node_typed_query_reader.cjs")
PACKED_V2_AUTHORING_READER = Path(__file__).with_name("node_v2_authoring_reader.cjs")
TYPED_QUERY_FIXTURE = Path(__file__).with_name("fixtures") / "typed-query" / "contract.json"
TYPED_PYTHON_ARTIFACT_RUNNER = REPO_ROOT / "scripts" / "ci" / "run_typed_python_artifact.py"
PARITY_ROOT_WHEEL_ENV = "TYPE_BRIDGE_PARITY_ROOT_WHEEL"
PARITY_CORE_WHEEL_ENV = "TYPE_BRIDGE_PARITY_CORE_WHEEL"
PARITY_NODE_PACKAGE_ENV = "TYPE_BRIDGE_PARITY_NODE_PACKAGE"

ENTITY_CLASSES = {
    "parity-person": ParityPerson,
    "parity-company": ParityCompany,
    "parity-email-message": ParityEmailMessage,
}

ATTRIBUTE_CLASSES = {
    "id": ParityId,
    "name": ParityName,
    "email": ParityEmail,
    "age": ParityAge,
    "score": ParityScore,
    "active": ParityActive,
    "birth_date": ParityBirthDate,
    "login_at": ParityLoginAt,
    "seen_at": ParitySeenAt,
    "balance": ParityBalance,
    "session_length": ParitySessionLength,
    "tags": ParityTag,
    "note": ParityNote,
    "since": ParitySince,
    "confidence": ParityConfidence,
    "kind": ParityKind,
}


def parse_npm_pack_manifest(stdout: str) -> dict[str, Any]:
    """Accept the one-artifact JSON shapes emitted across supported npm lines."""
    payload = json.loads(stdout)
    if isinstance(payload, list):
        if len(payload) != 1:
            raise ValueError("npm pack did not return exactly one artifact")
        manifest = payload[0]
    elif isinstance(payload, dict):
        if len(payload) != 1:
            raise ValueError("npm pack did not return exactly one artifact")
        manifest = next(iter(payload.values()))
    else:
        raise TypeError("npm pack returned neither an array nor an object")
    if not isinstance(manifest, dict):
        raise TypeError("npm pack artifact manifest is not an object")
    return manifest


NODE_VALUE_TYPES = {
    "String": "string",
    "Long": "long",
    "Double": "double",
    "Boolean": "boolean",
    "Date": "date",
    "DateTime": "datetime",
    "DateTimeTZ": "datetime-tz",
    "Decimal": "decimal",
    "Duration": "duration",
}


def load_parity_schema(db: Any) -> None:
    """Load the shared parity TypeQL schema into a fresh TypeDB database."""
    db.execute_query(load_fixture_contract()["schema"], transaction_type="schema")


def write_fixture_with_python(db: Any) -> None:
    """Write the shared fixture through public Python model manager APIs."""
    contract = load_fixture_contract()
    entities_by_id: dict[str, Any] = {}

    for row in contract["write_data"]["entities"]:
        entity = _build_entity(row)
        ENTITY_CLASSES[row["type"]].manager(db).insert(entity)
        entities_by_id[row["stable_id"]] = entity

    for row in contract["write_data"]["relations"]:
        relation = _build_relation(row, entities_by_id)
        relation.__class__.manager(db).insert(relation)


def read_with_node(address: str, database: str) -> dict[str, Any]:
    """Read fixture rows through the public Node package dynamic manager surface."""
    if shutil.which("node") is None:
        pytest.skip("node executable is not installed")

    env = dict(os.environ)
    env["TYPEDB_ADDRESS"] = address
    env["TYPE_BRIDGE_PARITY_DATABASE"] = database
    # Never auto-select an opaque binary from tmp/: it can outlive the source
    # build and turn strict parity into a stale-artifact false green. The
    # package loader resolves the native module beside package.json; callers
    # may still provide an explicit override when that is the artifact under test.

    completed = subprocess.run(
        ["node", str(NODE_READER)],
        check=False,
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise AssertionError(
            f"Node parity reader failed\nstdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    return json.loads(completed.stdout)


def assert_node_output_matches_expected(raw_output: dict[str, Any]) -> None:
    contract = load_fixture_contract()
    actual = canonicalize_node_reader_output(raw_output, contract)
    expected = contract["expected"]
    actual_json = canonical_json(actual)
    expected_json = canonical_json(expected)
    if actual_json != expected_json:
        diff = "\n".join(
            difflib.unified_diff(
                expected_json.splitlines(),
                actual_json.splitlines(),
                fromfile="expected-canonical.json",
                tofile="node-reader",
                lineterm="",
            )
        )
        raise AssertionError(f"Node reader canonical output drifted:\n{diff}")


def canonicalize_node_reader_output(
    raw_output: dict[str, Any],
    contract: dict[str, Any],
) -> dict[str, Any]:
    descriptors = _descriptor_maps(contract["descriptors"])
    entities = [
        _canonical_entity(row, descriptors[section["type_name"]])
        for section in raw_output["entities"]
        for row in section["rows"]
    ]
    relations = [
        _canonical_relation(row, descriptors[section["type_name"]], contract)
        for section in raw_output["relations"]
        for row in _merge_relation_rows(section["rows"])
    ]
    return {
        "fixture_id": contract["expected"]["fixture_id"],
        "version": contract["expected"]["version"],
        "entities": sorted(entities, key=lambda row: row["stable_id"]),
        "relations": sorted(relations, key=lambda row: row["stable_id"]),
    }


def read_with_typed_node(
    address: str | None = None,
    database: str | None = None,
    *,
    offline: bool = False,
) -> dict[str, Any]:
    """Read fixture rows through the typed Node surface and toDict().

    Sibling to :func:`read_with_node`. The typed reader is compiled by
    ``npm run build:parity-reader``; this skips cleanly if it is not built or
    if node is unavailable. In ``offline`` mode the reader builds instances from
    ``write-data.json`` with no database; otherwise it reads live via the typed
    manager ``.all()``.
    """
    if shutil.which("node") is None:
        pytest.skip("node executable is not installed")
    if not TYPED_NODE_READER.exists():
        pytest.skip(
            f"typed parity reader not built ({TYPED_NODE_READER}); "
            "run `npm run build:parity-reader` in type-bridge-core/crates/node"
        )

    env = dict(os.environ)
    env["TYPE_BRIDGE_NODE_PACKAGE_DIR"] = str(NODE_PACKAGE_DIR)
    env["TYPE_BRIDGE_PARITY_WRITE_DATA"] = str(WRITE_DATA)
    if address is not None:
        env["TYPEDB_ADDRESS"] = address
    if database is not None:
        env["TYPE_BRIDGE_PARITY_DATABASE"] = database

    cmd = ["node", str(TYPED_NODE_READER)]
    if offline:
        cmd.append("--offline")
    completed = subprocess.run(
        cmd,
        check=False,
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise AssertionError(
            f"Typed parity reader failed\nstdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    return json.loads(completed.stdout)


def read_typed_query_with_packed_node(
    address: str,
    database: str,
    *,
    http_port: int,
) -> dict[str, Any]:
    """Run the live typed-query reader from an isolated packed Node artifact.

    This deliberately performs no dependency installation. Release acceptance
    supplies the exact immutable tarball through
    ``TYPE_BRIDGE_PARITY_NODE_PACKAGE``; local/source parity retains the
    existing lifecycle-disabled ``npm pack`` fallback. Either artifact is
    extracted into a fresh ignored consumer and resolved only by its public
    package subpaths.
    """
    return _read_with_packed_node(
        address,
        database,
        http_port=http_port,
        reader=PACKED_TYPED_QUERY_READER,
        required_paths={
            "dist/index.js",
            "dist/typed/index.js",
            "dist/typed/index.d.ts",
        },
        extra_environment={
            "TYPE_BRIDGE_PARITY_TYPED_QUERY_FIXTURE": str(TYPED_QUERY_FIXTURE),
        },
        temp_prefix="node-typed-query-parity-",
        description="typed-query parity",
    )


def read_v2_authoring_with_packed_node(
    address: str,
    database: str,
    *,
    http_port: int,
    declared_fixture: Path,
    server_url: str,
    typedb_tls_root_ca: Path | None = None,
    remote_tls_root_ca: Path | None = None,
) -> dict[str, Any] | None:
    """Run advanced V2 authoring and model parity from one packed Node artifact."""
    if (typedb_tls_root_ca is None) != (remote_tls_root_ca is None):
        raise AssertionError("packed Node V2 TLS requires both TypeDB and remote-server root CAs")
    tls_environment = {"TYPE_BRIDGE_V2_TYPEDB_TLS_ENABLED": "0"}
    if typedb_tls_root_ca is not None and remote_tls_root_ca is not None:
        typedb_root = typedb_tls_root_ca.expanduser().resolve()
        remote_root = remote_tls_root_ca.expanduser().resolve()
        for label, root in (
            ("TypeDB", typedb_root),
            ("remote-server", remote_root),
        ):
            if not root.is_file():
                raise AssertionError(
                    f"packed Node V2 {label} TLS root must be a regular file: {root}"
                )
        tls_environment = {
            "TYPE_BRIDGE_V2_TYPEDB_TLS_ENABLED": "1",
            "TYPE_BRIDGE_V2_TYPEDB_TLS_ROOT_CA": str(typedb_root),
            # Node reads this trust extension only when the child process starts.
            "NODE_EXTRA_CA_CERTS": str(remote_root),
        }
    strict = os.environ.get("TYPE_BRIDGE_PARITY_STRICT") == "1"
    supplied_package = os.environ.get(PARITY_NODE_PACKAGE_ENV)
    if strict and supplied_package is None:
        raise AssertionError(f"strict V2 artifact parity requires {PARITY_NODE_PACKAGE_ENV}")
    unavailable = shutil.which("node") is None or (
        supplied_package is None
        and (
            shutil.which("npm") is None
            or not (NODE_PACKAGE_DIR / "dist" / "index.js").is_file()
            or not (NODE_PACKAGE_DIR / "dist" / "query-v2.js").is_file()
            or not (NODE_PACKAGE_DIR / "dist" / "typed" / "index.js").is_file()
            or not any(NODE_PACKAGE_DIR.glob("*.node"))
        )
    )
    if unavailable:
        if strict:
            raise AssertionError(
                "strict V2 artifact parity requires Node plus one complete packed artifact"
            )
        return None
    return _read_with_packed_node(
        address,
        database,
        http_port=http_port,
        reader=PACKED_V2_AUTHORING_READER,
        required_paths={
            "dist/index.js",
            "dist/query-v2.js",
            "dist/query-v2.d.ts",
            "dist/typed/index.js",
            "dist/typed/index.d.ts",
        },
        extra_environment={
            "TYPE_BRIDGE_V2_DECLARED_FIXTURE": str(declared_fixture),
            "TYPE_BRIDGE_V2_SERVER_URL": server_url,
            **tls_environment,
        },
        temp_prefix="node-v2-authoring-parity-",
        description="V2 authoring parity",
    )


def _read_with_packed_node(
    address: str,
    database: str,
    *,
    http_port: int,
    reader: Path,
    required_paths: set[str],
    extra_environment: dict[str, str],
    temp_prefix: str,
    description: str,
) -> dict[str, Any]:
    """Extract one exact tarball and invoke a source-owned artifact reader."""
    if shutil.which("node") is None:
        pytest.skip("node executable is not installed")

    supplied_package_raw = os.environ.get(PARITY_NODE_PACKAGE_ENV)
    supplied_package: Path | None = None
    if supplied_package_raw is not None:
        supplied_package = Path(supplied_package_raw).expanduser().resolve()
        if not supplied_package.is_file() or supplied_package.suffix != ".tgz":
            raise AssertionError(
                f"{PARITY_NODE_PACKAGE_ENV} must name one prebuilt .tgz file: {supplied_package}"
            )
    else:
        if shutil.which("npm") is None:
            pytest.skip("npm executable is not installed")
        if (
            not (NODE_PACKAGE_DIR / "dist" / "index.js").exists()
            or not (NODE_PACKAGE_DIR / "dist" / "typed" / "index.js").exists()
        ):
            pytest.skip(
                "compiled Node package not built; run `npm run build:types` in the node crate"
            )
        if not any(NODE_PACKAGE_DIR.glob("*.node")):
            pytest.skip(
                "native node module not built; run `npm run build:native` in the node crate"
            )

    repo_tmp = REPO_ROOT / "tmp"
    repo_tmp.mkdir(exist_ok=True)
    temp_root = Path(tempfile.mkdtemp(prefix=temp_prefix, dir=repo_tmp))
    try:
        pack_root = temp_root / "pack"
        unpack_root = temp_root / "unpack"
        consumer_root = temp_root / "consumer"
        pack_root.mkdir()
        unpack_root.mkdir()
        (consumer_root / "node_modules" / "@type-bridge").mkdir(parents=True)

        if supplied_package is None:
            packed = subprocess.run(
                [
                    "npm",
                    "pack",
                    "--ignore-scripts",
                    "--json",
                    "--pack-destination",
                    str(pack_root),
                ],
                check=False,
                cwd=NODE_PACKAGE_DIR,
                capture_output=True,
                text=True,
            )
            if packed.returncode != 0:
                raise AssertionError(
                    f"npm pack failed\nstdout:\n{packed.stdout}\nstderr:\n{packed.stderr}"
                )
            try:
                pack_info = parse_npm_pack_manifest(packed.stdout)
                packed_paths = {entry["path"] for entry in pack_info["files"]}
                tarball = pack_root / pack_info["filename"]
            except (IndexError, KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
                raise AssertionError(f"could not parse npm pack output: {packed.stdout}") from error
        else:
            tarball = supplied_package
            packed_paths: set[str] = set()
            seen_paths: set[str] = set()
            with tarfile.open(tarball, "r:gz") as archive:
                for member in archive.getmembers():
                    if not (member.isfile() or member.isdir()):
                        raise AssertionError(
                            f"prebuilt Node artifact has a non-regular member: {member.name}"
                        )
                    if member.name.rstrip("/") == "package":
                        continue
                    if not member.name.startswith("package/"):
                        raise AssertionError(
                            f"prebuilt Node artifact entry is outside package/: {member.name}"
                        )
                    relative = member.name.removeprefix("package/").rstrip("/")
                    path = PurePosixPath(relative)
                    if (
                        not relative
                        or "\\" in relative
                        or path.is_absolute()
                        or path.as_posix() != relative
                        or any(part in {"", ".", ".."} for part in path.parts)
                    ):
                        raise AssertionError(
                            f"prebuilt Node artifact has an unsafe path: {member.name}"
                        )
                    if relative in seen_paths:
                        raise AssertionError(
                            f"prebuilt Node artifact has a duplicate path: {relative}"
                        )
                    seen_paths.add(relative)
                    if member.isfile():
                        packed_paths.add(relative)

        if not required_paths <= packed_paths or not any(
            path.endswith(".node") for path in packed_paths
        ):
            raise AssertionError(f"packed Node artifact is incomplete: {sorted(packed_paths)}")

        with tarfile.open(tarball, "r:gz") as archive:
            archive.extractall(unpack_root, filter="data")
        extracted_root = unpack_root / "package"
        installed_root = consumer_root / "node_modules" / "@type-bridge" / "node"
        shutil.move(extracted_root, installed_root)

        env = dict(os.environ)
        env.pop("NODE_PATH", None)
        env.pop("NODE_OPTIONS", None)
        env.pop("TYPE_BRIDGE_NODE_NATIVE_PATH", None)
        env.pop("NODE_EXTRA_CA_CERTS", None)
        env.pop("TYPE_BRIDGE_V2_TYPEDB_TLS_ENABLED", None)
        env.pop("TYPE_BRIDGE_V2_TYPEDB_TLS_ROOT_CA", None)
        env["TYPEDB_ADDRESS"] = address
        env["TYPEDB_HTTP_PORT"] = str(http_port)
        env["TYPE_BRIDGE_PARITY_DATABASE"] = database
        env["TYPE_BRIDGE_PACKED_CONSUMER_ROOT"] = str(consumer_root)
        env["TYPE_BRIDGE_SOURCE_PACKAGE_ROOT"] = str(NODE_PACKAGE_DIR)
        env.update(extra_environment)
        completed = subprocess.run(
            ["node", str(reader)],
            check=False,
            cwd=consumer_root,
            env=env,
            capture_output=True,
            text=True,
        )
        if completed.returncode != 0:
            raise AssertionError(
                f"Packed Node {description} reader failed\n"
                f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
            )
        return json.loads(completed.stdout)
    finally:
        shutil.rmtree(temp_root, ignore_errors=True)


def read_typed_query_with_wheel_python(
    address: str,
    database: str,
    *,
    http_port: int,
) -> dict[str, Any] | None:
    """Run the live typed-query reader from isolated extracted wheel artifacts.

    Local parity runs may omit prebuilt wheel paths and still exercise the source
    Python and packed Node surfaces. Strict CI parity requires both wheel paths,
    so the built-wheel proof cannot silently disappear from the release gate.
    """
    strict = os.environ.get("TYPE_BRIDGE_PARITY_STRICT") == "1"
    root_raw = os.environ.get(PARITY_ROOT_WHEEL_ENV)
    core_raw = os.environ.get(PARITY_CORE_WHEEL_ENV)
    pyright = shutil.which("pyright")
    missing = [
        label
        for label, value in (
            (PARITY_ROOT_WHEEL_ENV, root_raw),
            (PARITY_CORE_WHEEL_ENV, core_raw),
            ("pyright executable", pyright),
        )
        if value is None
    ]
    if missing:
        if strict:
            raise AssertionError(
                "strict live parity requires extracted-wheel Python acceptance; missing "
                + ", ".join(missing)
            )
        return None

    assert root_raw is not None
    assert core_raw is not None
    assert pyright is not None
    root_wheel = Path(root_raw).expanduser().resolve()
    core_wheel = Path(core_raw).expanduser().resolve()
    absent = [str(path) for path in (root_wheel, core_wheel) if not path.is_file()]
    if absent:
        message = "configured parity wheel does not exist: " + ", ".join(absent)
        if strict:
            raise AssertionError(message)
        return None

    completed = subprocess.run(
        [
            sys.executable,
            str(TYPED_PYTHON_ARTIFACT_RUNNER),
            "--root-wheel",
            str(root_wheel),
            "--core-wheel",
            str(core_wheel),
            "--python",
            sys.executable,
            "--pyright",
            pyright,
            "--live-address",
            address,
            "--live-database",
            database,
            "--live-http-port",
            str(http_port),
            "--live-fixture",
            str(TYPED_QUERY_FIXTURE),
        ],
        check=False,
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise AssertionError(
            "Built-wheel Python typed-query parity reader failed\n"
            f"stdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    try:
        report = json.loads(completed.stdout)
        live = report["live"]
    except (KeyError, TypeError, json.JSONDecodeError) as error:
        raise AssertionError(
            f"Built-wheel Python artifact runner returned an invalid report: {completed.stdout}"
        ) from error
    if not isinstance(live, dict):
        raise AssertionError(f"Built-wheel Python live report is not an object: {live!r}")
    return live


def assert_typed_node_output_matches_expected(raw_output: dict[str, Any]) -> None:
    """Assert the typed reader's entity toDict() output matches the value oracle.

    The typed reader feeds the SAME ``expected-canonical.json`` oracle the
    dynamic ``node_reader.cjs`` satisfies, reusing the single canonicalizer
    (:func:`_canonical_value` and friends). This is the VALUE-parity gate; the
    descriptor SHAPE-parity gate is Plan 10's byte-identity check. The two are
    complementary, not duplicative.
    """
    contract = load_fixture_contract()
    actual = canonicalize_typed_reader_output(raw_output, contract)
    expected = {
        "fixture_id": contract["expected"]["fixture_id"],
        "version": contract["expected"]["version"],
        "entities": contract["expected"]["entities"],
        # The typed reader covers entity value parity; relations (with role
        # players) stay the dynamic reader's job — typed relation toDict()
        # excludes roles by contract.
        "relations": [],
    }
    actual_json = canonical_json(actual)
    expected_json = canonical_json(expected)
    if actual_json != expected_json:
        diff = "\n".join(
            difflib.unified_diff(
                expected_json.splitlines(),
                actual_json.splitlines(),
                fromfile="expected-canonical.json",
                tofile="typed-node-reader",
                lineterm="",
            )
        )
        raise AssertionError(f"Typed reader canonical entity output drifted:\n{diff}")


def assert_typed_relation_attributes_match_expected(raw_output: dict[str, Any]) -> None:
    """Assert each relation's toDict() attribute shape matches the value oracle.

    Typed relation ``toDict()`` emits attribute fields only (role players are
    excluded by contract), so this checks the relation ``attributes`` sub-shape
    against ``expected-canonical.json`` — not the full role-bearing relation
    oracle, which the dynamic reader covers.
    """
    contract = load_fixture_contract()
    descriptors = _descriptor_maps(contract["descriptors"])
    expected_by_type = {rel["type"]: rel for rel in contract["expected"]["relations"]}
    for section in raw_output.get("relations", []):
        type_name = section["type_name"]
        descriptor = descriptors[type_name]
        expected_attributes = expected_by_type[type_name]["attributes"]
        for row in section["rows"]:
            actual_attributes = _canonical_typed_attributes(row, descriptor)
            if canonical_json(actual_attributes) != canonical_json(expected_attributes):
                raise AssertionError(
                    f"Typed relation '{type_name}' attribute toDict drifted:\n"
                    f"expected={canonical_json(expected_attributes)}\n"
                    f"actual={canonical_json(actual_attributes)}"
                )


def canonicalize_typed_reader_output(
    raw_output: dict[str, Any],
    contract: dict[str, Any],
) -> dict[str, Any]:
    """Canonicalize typed reader entity output, reusing the single canonicalizer."""
    descriptors = _descriptor_maps(contract["descriptors"])
    entities = [
        _canonical_typed_entity(row, descriptors[section["type_name"]], section["type_name"])
        for section in raw_output["entities"]
        for row in section["rows"]
    ]
    return {
        "fixture_id": contract["expected"]["fixture_id"],
        "version": contract["expected"]["version"],
        "entities": sorted(entities, key=lambda row: row["stable_id"]),
        "relations": [],
    }


def _canonical_typed_entity(
    row: dict[str, Any],
    descriptor: dict[str, Any],
    type_name: str,
) -> dict[str, Any]:
    return {
        "stable_id": row["id"],
        "type": type_name,
        "attributes": _canonical_typed_attributes(row, descriptor),
    }


def _canonical_typed_attributes(
    row: dict[str, Any],
    descriptor: dict[str, Any],
) -> dict[str, Any]:
    """Normalize a toDict() plain dict to the canonical attribute shape.

    The toDict() value is already a plain primitive keyed by field name, so this
    reuses :func:`_canonical_value` (the single value normalizer — long → decimal
    string, decimal strip, datetime trimming) without a second canonicalizer.
    """
    attrs_by_field = {attr["field_name"]: attr for attr in descriptor["owned_attributes"]}
    attributes: dict[str, Any] = {}
    for field_name, value in row.items():
        attr = attrs_by_field[field_name]
        if _is_multi_value_attribute(attr):
            canonical_values = [_canonical_value(item, attr["value_type"]) for item in value]
            canonical_values.sort(key=canonical_json)
            attributes[field_name] = canonical_values
        else:
            attributes[field_name] = _canonical_value(value, attr["value_type"])
    return dict(sorted(attributes.items()))


def _build_entity(row: dict[str, Any]) -> Any:
    kwargs = _build_attribute_kwargs(row["attributes"])
    if row["type"] == "parity-person":
        kwargs.setdefault("tags", [])
    return ENTITY_CLASSES[row["type"]](**kwargs)


def _build_relation(row: dict[str, Any], entities_by_id: dict[str, Any]) -> Any:
    kwargs = _build_attribute_kwargs(row["attributes"])
    roles = {
        role_name: [entities_by_id[player["stable_id"]] for player in players]
        for role_name, players in row["roles"].items()
    }
    if row["type"] == "parity-membership":
        return ParityMembership(
            member=roles["member"][0],
            organization=roles["organization"][0],
            evidence=roles["evidence"],
            **kwargs,
        )
    if row["type"] == "parity-token-origin":
        return ParityTokenOrigin(
            token=roles["token"][0],
            issue=roles["issue"][0],
            **kwargs,
        )
    raise AssertionError(f"unknown relation fixture type: {row['type']}")


def _build_attribute_kwargs(attributes: dict[str, Any]) -> dict[str, Any]:
    kwargs: dict[str, Any] = {}
    for field_name, value in attributes.items():
        attr_cls = ATTRIBUTE_CLASSES[field_name]
        if isinstance(value, list):
            kwargs[field_name] = [attr_cls(_python_value(item)) for item in value]
        else:
            kwargs[field_name] = attr_cls(_python_value(value))
    return kwargs


def _python_value(value: dict[str, Any]) -> Any:
    raw_value = value["value"]
    if value["type"] in {"datetime", "datetime-tz"}:
        return datetime.fromisoformat(raw_value)
    return raw_value


def _descriptor_maps(descriptors: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {
        descriptor["type_name"]: descriptor
        for section in ("entities", "relations")
        for descriptor in descriptors[section]
    }


def _canonical_entity(row: dict[str, Any], descriptor: dict[str, Any]) -> dict[str, Any]:
    attributes = _canonical_attributes(row["attributes"], descriptor)
    return {
        "stable_id": attributes["id"]["value"],
        "type": row["type_name"],
        "attributes": attributes,
    }


def _canonical_relation(
    row: dict[str, Any],
    descriptor: dict[str, Any],
    contract: dict[str, Any],
) -> dict[str, Any]:
    attributes = _canonical_attributes(row["attributes"], descriptor)
    roles = _canonical_roles(row["role_players"])
    relation = {
        "stable_id": _relation_stable_id(row["type_name"], attributes, roles, contract),
        "type": row["type_name"],
        "attributes": attributes,
        "roles": roles,
    }
    return relation


def _canonical_attributes(
    raw_attributes: list[list[Any]],
    descriptor: dict[str, Any],
) -> dict[str, Any]:
    values_by_attr: dict[str, list[Any]] = {}
    for attr_name, value in raw_attributes:
        values_by_attr.setdefault(attr_name, []).append(value)

    attributes: dict[str, Any] = {}
    for attr in descriptor["owned_attributes"]:
        values = values_by_attr.get(attr["attr_name"], [])
        if not values:
            continue
        canonical_values = [_canonical_value(value, attr["value_type"]) for value in values]
        canonical_values.sort(key=canonical_json)
        if _is_multi_value_attribute(attr):
            attributes[attr["field_name"]] = canonical_values
        else:
            attributes[attr["field_name"]] = canonical_values[0]
    return dict(sorted(attributes.items()))


def _canonical_roles(raw_players: list[dict[str, Any]]) -> dict[str, list[dict[str, str]]]:
    roles: dict[str, list[dict[str, str]]] = {}
    for player in raw_players:
        stable_id = _stable_id_from_player(player)
        roles.setdefault(player["role_name"], []).append(
            {
                "stable_id": stable_id,
                "type": player["player_type_name"],
            }
        )
    return {
        role_name: sorted(players, key=lambda player: player["stable_id"])
        for role_name, players in sorted(roles.items())
    }


def _canonical_value(value: Any, value_type: str) -> dict[str, Any]:
    raw_value = _unwrap_node_value(value)
    if value_type == "long":
        raw_value = str(raw_value)
    elif value_type == "decimal":
        raw_value = str(raw_value).removesuffix("dec")
    elif value_type == "datetime":
        raw_value = _trim_zero_nanoseconds(str(raw_value))
    elif value_type == "datetime-tz":
        raw_value = _trim_zero_nanoseconds(str(raw_value).replace("Z", "+00:00"))
    return {
        "type": value_type,
        "value": raw_value,
    }


def _unwrap_node_value(value: Any) -> Any:
    if isinstance(value, dict):
        for key in NODE_VALUE_TYPES:
            if key in value:
                return value[key]
        if "value" in value:
            return _unwrap_node_value(value["value"])
    return value


def _trim_zero_nanoseconds(value: str) -> str:
    return value.replace(".000000000", "")


def _is_multi_value_attribute(attribute: dict[str, Any]) -> bool:
    for annotation in attribute["annotations"]:
        if isinstance(annotation, dict) and "Card" in annotation:
            _, max_card = annotation["Card"]
            return max_card is None or max_card > 1
    return False


def _stable_id_from_player(player: dict[str, Any]) -> str:
    for attr_name, value in player.get("attributes", []):
        if attr_name == "parity-id":
            return str(_unwrap_node_value(value))
    raise AssertionError(f"role player is missing parity-id: {player}")


def _merge_relation_rows(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    merged: dict[tuple[Any, ...], dict[str, Any]] = {}
    seen_players: dict[tuple[Any, ...], set[tuple[Any, ...]]] = {}
    for row in rows:
        key = (
            row.get("iid"),
            row.get("type_name"),
            canonical_json(row.get("attributes", [])),
        )
        target = merged.setdefault(
            key,
            {
                **row,
                "role_players": [],
            },
        )
        player_keys = seen_players.setdefault(key, set())
        for player in row.get("role_players", []):
            player_key = (
                player.get("role_name"),
                player.get("player_type_name"),
                _stable_id_from_player(player),
            )
            if player_key in player_keys:
                continue
            player_keys.add(player_key)
            target["role_players"].append(player)
    return list(merged.values())


def _relation_stable_id(
    type_name: str,
    attributes: dict[str, Any],
    roles: dict[str, list[dict[str, str]]],
    contract: dict[str, Any],
) -> str:
    signatures = {
        _relation_signature(row["type"], row["attributes"], row["roles"]): row["stable_id"]
        for row in contract["write_data"]["relations"]
    }
    signature = _relation_signature(type_name, attributes, roles)
    try:
        return signatures[signature]
    except KeyError as exc:
        raise AssertionError(f"could not match relation fixture signature: {signature}") from exc


def _relation_signature(
    type_name: str,
    attributes: dict[str, Any],
    roles: dict[str, list[dict[str, str]]],
) -> str:
    value = {
        "type": type_name,
        "attributes": attributes,
        "roles": {
            role_name: sorted(players, key=lambda player: player["stable_id"])
            for role_name, players in sorted(roles.items())
        },
    }
    return canonical_json(value)
