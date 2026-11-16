"""Registry changes in source-controlled consumer locks require explicit selection."""

import re
from pathlib import Path

import pytest

from scripts.ci.refresh_rust_consumer_locks import FIXTURES, validate_registry_changes


def lock(*packages: tuple[str, str, str]) -> bytes:
    return (
        "version = 4\n"
        + "".join(
            f'\n[[package]]\nname = "{name}"\nversion = "{version}"\n'
            f'source = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "{checksum}"\n'
            for name, version, checksum in packages
        )
    ).encode()


def test_path_package_record_changes_preserve_registry_pins() -> None:
    before = lock(("pinned", "1.0.0", "original"))
    after = (
        before + b'\n[[package]]\nname = "local"\nversion = "3.0.0"\ndependencies = ["pinned"]\n'
    )
    validate_registry_changes(before, after, set())


@pytest.mark.parametrize(
    "after",
    [lock(("pinned", "1.1.0", "new")), lock(("pinned", "1.0.0", "changed")), lock()],
)
def test_unselected_version_checksum_or_removal_is_refused(after: bytes) -> None:
    with pytest.raises(ValueError, match="pinned"):
        validate_registry_changes(lock(("pinned", "1.0.0", "original")), after, set())


def test_named_update_does_not_authorize_another_package() -> None:
    before = lock(("selected", "1.0.0", "original"), ("other", "1.0.0", "original"))
    selected = lock(("selected", "1.1.0", "new"), ("other", "1.0.0", "original"))
    validate_registry_changes(before, selected, {"selected"})
    with pytest.raises(ValueError, match="other"):
        validate_registry_changes(
            before, lock(("selected", "1.1.0", "new"), ("other", "1.1.0", "new")), {"selected"}
        )


def test_every_maintained_consumer_lock_is_covered() -> None:
    repository = Path(__file__).resolve().parents[3]
    audit = (repository / "scripts/ci/check_dependency_security.sh").read_text()
    maintained = set(re.findall(r"^\s+(\S*Cargo\.lock)(?:\s|$)", audit, re.MULTILINE))
    # The workspace and standalone rule-wire fixture are not generated SDK consumers.
    maintained -= {
        "type-bridge-core/Cargo.lock",
        "type-bridge-core/crates/core/tests/fixtures/rule-wire-standalone/Cargo.lock",
    }
    assert {entry[0] for entry in FIXTURES} == maintained
