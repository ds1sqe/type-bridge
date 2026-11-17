"""Per-profile identity of the SDK conformance suites.

Each suite keeps a catalog and profiles.json. Producers select a semantic
profile and its exact live TypeDB server; comparers require a shared profile
and validate its committed fingerprints or native migration-history digest.
"""

from __future__ import annotations

import json
import os
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
MATRIX_FORMAT = "typebridge.sdk-profile-matrix/v1"
CONTRACT_ROOT = "tests/contracts/sdk_conformance"
SUITES = {
    "queries": "queries",
    "models": "models",
    "administration": "administration",
    "serialization": "serialization",
    "artifacts": "artifacts",
}


def suite_directory(suite: str) -> str:
    return f"{CONTRACT_ROOT}/{SUITES[suite]}"


def contract_relative(suite: str, kind: str) -> str:
    if suite not in SUITES:
        raise KeyError(suite)
    return f"{suite_directory(suite)}/{kind}.json"


SEMANTIC_PROFILE_ENV = "TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE"
EXPECTED_SERVER_ENV = "TYPE_BRIDGE_EXPECTED_SERVER_VERSION"
# Measured pairs only; selection never changes the exact detected-version check.
SERVER_PROFILES = {
    "3.12.3": "typedb-3.12.1/v1",
    "3.13.6": "typedb-3.13.6/v1",
    "3.14.0": "typedb-3.13.6/v1",
}


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
    expected_history_fingerprint: str | None = None


def expected_server(environment: Mapping[str, str]) -> str | None:
    """Use the explicit selection, including the existing test.sh/CI version variable."""
    return environment.get(EXPECTED_SERVER_ENV, environment.get("TYPEDB_VERSION"))


def require_server_profile(
    profile: str, server: str, *, error: ErrorFactory = ProfileMatrixError
) -> None:
    """Refuse every unmeasured server/profile pair."""
    if SERVER_PROFILES.get(server) != profile:
        raise error("server_profile_mismatch", f"unmeasured TypeDB {server!r} / {profile!r} pair")


def with_server(
    identity: ProfileIdentity, server: str | None, *, error: ErrorFactory = ProfileMatrixError
) -> ProfileIdentity:
    """Select an exact measured server, preserving the matrix default when unset."""
    if server is None:
        return identity
    require_server_profile(identity.semantic_profile, server, error=error)
    return replace(identity, live_server_version=server)


def matrix_relative(suite: str) -> str:
    return contract_relative(suite, "profiles")


def catalog_relative(suite: str) -> str:
    return contract_relative(suite, "catalog")


def load_matrix(
    suite: str, *, root: Path = ROOT, error: ErrorFactory = ProfileMatrixError
) -> dict[str, ProfileIdentity]:
    """Load and shape-check ``profiles.json``; keys are semantic profiles."""

    if suite not in SUITES:
        raise error("unknown_conformance_set", f"{suite} has no profile matrix")
    try:
        value = json.loads((root / matrix_relative(suite)).read_text(encoding="utf-8"))
    except (OSError, ValueError) as failure:
        raise error("invalid_profile_matrix", f"{suite} matrix is unreadable") from failure
    set_id = suite
    if (
        type(value) is not dict
        or set(value) != {"format", "suite", "catalog_path", "profiles"}
        or value["format"] != MATRIX_FORMAT
        or value["suite"] != set_id
        or value["catalog_path"] != catalog_relative(suite)
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
                "expected_history_fingerprint",
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
        history = entry.get("expected_history_fingerprint")
        if (suite == "administration" or history is not None) and (
            suite != "administration"
            or type(history) is not str
            or len(history) != 64
            or any(character not in "0123456789abcdef" for character in history)
        ):
            raise error(
                "invalid_profile_matrix", f"{set_id} entry {profile!r} history digest is malformed"
            )
        entries[profile] = ProfileIdentity(
            set_id, profile, entry["live_server_version"], fingerprints, history
        )
    return entries


def _fingerprint_profiles(fingerprints: Any) -> set[Any]:
    if type(fingerprints) is not dict or type(fingerprints.get("projections")) is not dict:
        return set()
    values = [fingerprints.get("semantic"), *fingerprints["projections"].values()]
    return {item.get("semantic_profile") if type(item) is dict else None for item in values}


def supported_profiles(suite: str, *, root: Path = ROOT) -> tuple[str, ...]:
    return tuple(load_matrix(suite, root=root))


def select(
    suite: str,
    profile: Any,
    *,
    root: Path = ROOT,
    error: ErrorFactory = ProfileMatrixError,
) -> ProfileIdentity:
    """Return the matrix entry for ``profile`` or fail closed."""

    entries = load_matrix(suite, root=root, error=error)
    if type(profile) is not str or profile not in entries:
        raise error(
            "semantic_profile_mismatch",
            f"{suite} has no matrix entry for profile {profile!r}",
        )
    return entries[profile]


def select_server(
    suite: str,
    server_version: Any,
    *,
    root: Path = ROOT,
    error: ErrorFactory = ProfileMatrixError,
) -> ProfileIdentity:
    """Return the single matrix entry whose live server is ``server_version``."""

    matches = [
        entry
        for entry in load_matrix(suite, root=root, error=error).values()
        if entry.live_server_version == server_version
    ]
    if len(matches) != 1:
        raise error(
            "server_version_mismatch",
            f"{suite} has no matrix entry for TypeDB {server_version!r}",
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


def catalog_profile(suite: str, *, root: Path = ROOT) -> str:
    """The profile a set's catalog was frozen on (its top-level identity)."""

    catalog = json.loads((root / catalog_relative(suite)).read_text(encoding="utf-8"))
    return catalog["fixture"]["semantic_profile"]


def requested(
    suite: str,
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

    environment = os.environ if environment is None else environment
    if profile is None:
        profile = environment.get(SEMANTIC_PROFILE_ENV) or catalog_profile(suite, root=root)
    return with_server(
        select(suite, profile, root=root, error=error),
        expected_server(environment),
        error=error,
    )


def select_lane(
    profile: str, *, root: Path = ROOT, error: ErrorFactory = ProfileMatrixError
) -> ProfileIdentity:
    """Select ``profile`` in every set; all sets must agree on its live server."""

    entries = [select(suite, profile, root=root, error=error) for suite in SUITES]
    if len({entry.live_server_version for entry in entries}) != 1:
        raise error(
            "server_version_mismatch",
            f"conformance sets disagree on the live server for {profile!r}",
        )
    return entries[-1]


def declared_profile(suite: str, report: Any, fallback: str) -> str:
    """The report's own profile when it is a matrix entry of ``{suite}``.

    An undeclared or unsupported profile yields ``fallback`` so the caller's
    exact comparison rejects the report with its usual code.
    """

    value = report_field(report, "semantic_profile")
    return value if value in supported_profiles(suite) else fallback


def report_format(suite: str) -> str:
    """Wire format of the suite's current report contract."""
    if suite not in SUITES:
        raise KeyError(suite)
    return f"typebridge.sdk-{suite}-report/v1"


def report_directory(suite: str) -> str:
    """Report directory below the run's reports root."""
    return SUITES[suite]


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check-server-profile")
    parser.add_argument("--expected-server")
    arguments = parser.parse_args()
    if arguments.check_server_profile is not None:
        require_server_profile(arguments.check_server_profile, arguments.expected_server)
        raise SystemExit(0)
    print(
        json.dumps(
            {
                str(suite): {
                    "suite": suite,
                    "catalog": catalog_relative(suite),
                    "matrix": matrix_relative(suite),
                }
                for suite in SUITES
            }
        )
    )
