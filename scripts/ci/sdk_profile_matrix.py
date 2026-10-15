"""TypeDB version matrix of the SDK conformance sets.

Each set ``sdk-vN`` keeps one catalog whose top-level identity is the profile
it was frozen on. ``sdk-vN/profiles-vN.json`` lists the identity a conforming
report carries on every supported semantic profile: the exact live server and,
for sets that pin them, the semantic and binding projection fingerprints. The
catalog's own profile is one entry and repeats the catalog's values.

Comparers read the profile a report set declares, require one shared profile,
and validate against that entry instead of a hard-coded profile.
"""

from __future__ import annotations

import json
import os
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
MATRIX_FORMAT = "typebridge.sdk-profile-matrix/v1"
CONTRACT_ROOT = "tests/contracts/sdk_conformance"
# The smoke set remains until it is folded into queries.
SUITES = {
    "queries": "sdk-v2",
    "models": "models",
    "administration": "administration",
    "serialization": "serialization",
    "artifacts": "sdk-v6",
}
SET_VERSIONS = (1, 2, 3, 4, 5, 6)


def suite_directory(suite: str) -> str:
    return f"{CONTRACT_ROOT}/{SUITES[suite]}"


def contract_relative(version: int, kind: str) -> str:
    directory = "sdk-v1" if version == 1 else SUITES[tuple(SUITES)[version - 2]]
    suffix = f"-v{version}" if directory.startswith("sdk-v") else ""
    return f"{CONTRACT_ROOT}/{directory}/{kind}{suffix}.json"


SEMANTIC_PROFILE_ENV = "TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE"

ErrorFactory = Callable[[str, str], Exception]


class ProfileMatrixError(ValueError):
    """Stable matrix failure with a machine-readable code."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code


@dataclass(frozen=True)
class ProfileIdentity:
    """Expected identity of one conformance set on one semantic profile."""

    suite: str
    semantic_profile: str
    live_server_version: str
    expected_fingerprints: dict[str, Any] | None


def matrix_relative(version: int) -> str:
    return contract_relative(version, "profiles")


def catalog_relative(version: int) -> str:
    return contract_relative(version, "catalog")


def load_matrix(
    version: int, *, root: Path = ROOT, error: ErrorFactory = ProfileMatrixError
) -> dict[str, ProfileIdentity]:
    """Load and shape-check ``profiles-vN.json``; keys are semantic profiles."""

    if version not in SET_VERSIONS:
        raise error("unknown_conformance_set", f"sdk-v{version} has no profile matrix")
    try:
        value = json.loads((root / matrix_relative(version)).read_text(encoding="utf-8"))
    except (OSError, ValueError) as failure:
        raise error("invalid_profile_matrix", f"sdk-v{version} matrix is unreadable") from failure
    set_id = "sdk-v1" if version == 1 else tuple(SUITES)[version - 2]
    if (
        type(value) is not dict
        or set(value) != {"format", "suite", "catalog_path", "profiles"}
        or value["format"] != MATRIX_FORMAT
        or value["suite"] != set_id
        or value["catalog_path"] != catalog_relative(version)
        or type(value["profiles"]) is not dict
        or not value["profiles"]
    ):
        raise error("invalid_profile_matrix", f"{set_id} matrix is not the committed shape")
    entries: dict[str, ProfileIdentity] = {}
    for profile, entry in value["profiles"].items():
        if (
            type(entry) is not dict
            or not {"live_server_version"}
            <= set(entry)
            <= {
                "live_server_version",
                "expected_fingerprints",
            }
            or type(entry["live_server_version"]) is not str
        ):
            raise error("invalid_profile_matrix", f"{set_id} entry {profile!r} is malformed")
        fingerprints = entry.get("expected_fingerprints")
        if fingerprints is not None and _fingerprint_profiles(fingerprints) != {profile}:
            raise error(
                "invalid_profile_matrix",
                f"{set_id} entry {profile!r} fingerprints name another profile",
            )
        entries[profile] = ProfileIdentity(
            set_id, profile, entry["live_server_version"], fingerprints
        )
    return entries


def _fingerprint_profiles(fingerprints: Any) -> set[Any]:
    if type(fingerprints) is not dict or type(fingerprints.get("projections")) is not dict:
        return set()
    values = [fingerprints.get("semantic"), *fingerprints["projections"].values()]
    return {item.get("semantic_profile") if type(item) is dict else None for item in values}


def supported_profiles(version: int, *, root: Path = ROOT) -> tuple[str, ...]:
    return tuple(load_matrix(version, root=root))


def select(
    version: int,
    profile: Any,
    *,
    root: Path = ROOT,
    error: ErrorFactory = ProfileMatrixError,
) -> ProfileIdentity:
    """Return the matrix entry for ``profile`` or fail closed."""

    entries = load_matrix(version, root=root, error=error)
    if type(profile) is not str or profile not in entries:
        raise error(
            "semantic_profile_mismatch",
            f"sdk-v{version} has no matrix entry for profile {profile!r}",
        )
    return entries[profile]


def select_server(
    version: int,
    server_version: Any,
    *,
    root: Path = ROOT,
    error: ErrorFactory = ProfileMatrixError,
) -> ProfileIdentity:
    """Return the single matrix entry whose live server is ``server_version``."""

    matches = [
        entry
        for entry in load_matrix(version, root=root, error=error).values()
        if entry.live_server_version == server_version
    ]
    if len(matches) != 1:
        raise error(
            "server_version_mismatch",
            f"sdk-v{version} has no matrix entry for TypeDB {server_version!r}",
        )
    return matches[0]


def shared_value(
    values: Iterable[Any], label: str, *, error: ErrorFactory = ProfileMatrixError
) -> Any:
    """Return the one value every report declares; mixed declarations fail.

    Malformed (non-string) declarations are ignored here so that the caller's
    exact report validation reports them with its own codes.
    """

    declared = {value for value in values if type(value) is str}
    if len(declared) > 1:
        raise error(
            f"mixed_{label.replace(' ', '_')}",
            f"reports in one comparison declare different {label}s: {sorted(declared)}",
        )
    return next(iter(declared), None)


def report_field(report: Any, *path: str) -> Any:
    """Read ``report[path...]`` without trusting the report's shape."""

    value = report
    for key in path:
        if type(value) is not dict:
            return None
        value = value.get(key)
    return value


def catalog_profile(version: int, *, root: Path = ROOT) -> str:
    """The profile a set's catalog was frozen on (its top-level identity)."""

    catalog = json.loads((root / catalog_relative(version)).read_text(encoding="utf-8"))
    return catalog["fixture"]["semantic_profile"]


def requested(
    version: int,
    profile: str | None = None,
    environment: Mapping[str, str] | None = None,
    *,
    root: Path = ROOT,
    error: ErrorFactory = ProfileMatrixError,
) -> ProfileIdentity:
    """Select the entry a producer targets.

    The explicit ``profile`` wins, then ``TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE``
    from ``environment`` (``os.environ`` when omitted), then the catalog's own
    profile.
    """

    if profile is None:
        environment = os.environ if environment is None else environment
        profile = environment.get(SEMANTIC_PROFILE_ENV) or catalog_profile(version, root=root)
    return select(version, profile, root=root, error=error)


def select_lane(
    profile: str, *, root: Path = ROOT, error: ErrorFactory = ProfileMatrixError
) -> ProfileIdentity:
    """Select ``profile`` in every set; all sets must agree on its live server."""

    entries = [select(version, profile, root=root, error=error) for version in SET_VERSIONS]
    if len({entry.live_server_version for entry in entries}) != 1:
        raise error(
            "server_version_mismatch",
            f"conformance sets disagree on the live server for {profile!r}",
        )
    return entries[-1]


def declared_profile(version: int, report: Any, fallback: str) -> str:
    """The report's own profile when it is a matrix entry of ``sdk-v{version}``.

    An undeclared or unsupported profile yields ``fallback`` so the caller's
    exact comparison rejects the report with its usual code.
    """

    value = report_field(report, "semantic_profile")
    return value if value in supported_profiles(version) else fallback


def report_directory(version: int) -> str:
    """Report location while the numbered producers are migrated."""
    return f"v{version}"


if __name__ == "__main__":
    print(
        json.dumps(
            {
                str(version): {
                    "suite": "sdk-v1" if version == 1 else tuple(SUITES)[version - 2],
                    "catalog": catalog_relative(version),
                    "matrix": matrix_relative(version),
                }
                for version in SET_VERSIONS
            }
        )
    )
