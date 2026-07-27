"""Cross-platform retained authority for legacy adoption conversion.

Released V1 migration discovery keeps its ambient :class:`~pathlib.Path`
behavior. Only the V2 converter opts into this private wrapper around the
mandatory Rust core's cap-std authority: the caller-supplied root is followed
once, then all recognized descendants are opened relative and no-follow.
"""

from __future__ import annotations

import errno
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from type_bridge import _rust_runtime


class AdoptionDirectoryError(OSError):
    """The retained adoption directory cannot provide coherent authority."""


@dataclass(frozen=True)
class AdoptionDirectoryEntry:
    """One direct child captured during a stable bounded enumeration."""

    name: str
    revision: object
    _native: Any

    @classmethod
    def from_native(
        cls,
        native: Any,
        *,
        reject_non_utf8: bool = False,
    ) -> AdoptionDirectoryEntry | None:
        try:
            name = str(native.name)
        except ValueError as error:
            raw_name = bytes(native.name_bytes())
            if reject_non_utf8 or _looks_migration_shaped(raw_name):
                raise AdoptionDirectoryError(
                    errno.EILSEQ,
                    "adoption_relevant_entry_name_is_not_utf8",
                ) from error
            return None
        return cls(name=name, revision=native, _native=native)

    def is_file(self) -> bool:
        return bool(self._native.is_file())

    def is_dir(self) -> bool:
        return bool(self._native.is_directory())

    def is_symlink(self) -> bool:
        return bool(self._native.is_symlink())

    def same_identity(self, other: AdoptionDirectoryEntry) -> bool:
        """Return whether two captures name the same exact entry revision."""
        return bool(self._native.same_identity(other._native))


class AdoptionDirectoryAuthority:
    """One retained cap-std migration-directory authority."""

    def __init__(self, display_path: Path, native: Any) -> None:
        self.display_path = display_path
        self._native: Any | None = native

    @classmethod
    def open(cls, path: Path) -> AdoptionDirectoryAuthority:
        """Follow the supplied root once and retain the resulting directory."""
        absolute = path if path.is_absolute() else Path.cwd() / path
        try:
            native = _rust_runtime.rust_core().PyAdoptionDirectoryAuthority.open(str(absolute))
        except Exception as error:
            raise _translated(error) from error
        return cls(absolute, native)

    def __enter__(self) -> AdoptionDirectoryAuthority:
        return self

    def __exit__(self, *_: object) -> None:
        self.close()

    def close(self) -> None:
        self._native = None

    def directory_revision(self) -> object:
        """Return the retained root's opaque mutation-sensitive identity."""
        try:
            return self._require_native().directory_revision()
        except Exception as error:
            raise _translated(error) from error

    def require_directory_revision(self, expected: object) -> None:
        try:
            self._require_native().require_directory_revision(expected)
        except Exception as error:
            raise _translated(error) from error

    def entries(
        self,
        relative: Path = Path("."),
        *,
        maximum_entries: int = 65_536,
        expected_directory: AdoptionDirectoryEntry | None = None,
        reject_non_utf8: bool = False,
    ) -> tuple[AdoptionDirectoryEntry, ...]:
        """Capture sorted children, stopping at the entry ceiling plus one."""
        try:
            native_entries = self._require_native().entries(
                str(relative),
                maximum_entries,
                None if expected_directory is None else expected_directory._native,
            )
        except Exception as error:
            raise _translated(error) from error
        captured = (
            AdoptionDirectoryEntry.from_native(
                entry,
                reject_non_utf8=reject_non_utf8,
            )
            for entry in native_entries
        )
        return tuple(entry for entry in captured if entry is not None)

    def inspect(
        self,
        relative: Path,
        *,
        expected_parent: AdoptionDirectoryEntry | None = None,
    ) -> AdoptionDirectoryEntry | None:
        """Inspect one descendant without following its final component."""
        try:
            native = self._require_native().inspect(
                str(relative),
                None if expected_parent is None else expected_parent._native,
            )
        except Exception as error:
            raise _translated(error) from error
        if native is None:
            return None
        entry = AdoptionDirectoryEntry.from_native(native)
        if entry is None:
            raise AdoptionDirectoryError(
                errno.EILSEQ,
                "adoption_requested_entry_name_is_not_utf8",
            )
        return entry

    def read_bounded(
        self,
        relative: Path,
        limit: int,
        *,
        expected: AdoptionDirectoryEntry | None = None,
    ) -> bytes:
        """Read one stable regular descendant through retained authority."""
        try:
            return bytes(
                self._require_native().read_bounded(
                    str(relative),
                    limit,
                    None if expected is None else expected._native,
                )
            )
        except Exception as error:
            raise _translated(error) from error

    def inspect_direct(self, name: str) -> AdoptionDirectoryEntry | None:
        """Inspect one direct output child without following it."""
        return self.inspect(Path(name))

    def write_atomic_no_replace(self, name: str, contents: bytes) -> None:
        """Publish one direct child atomically without replacing authority."""
        try:
            self._require_native().write_atomic_no_replace(name, contents)
        except Exception as error:
            raise _translated(error) from error

    def validate_publication_name(self, name: str) -> None:
        """Fail when a final name is not a safe child on the current filesystem."""
        try:
            self._require_native().validate_publication_name(name)
        except Exception as error:
            raise _translated(error) from error

    def remove_if_matches(
        self,
        name: str,
        expected: AdoptionDirectoryEntry,
        expected_bytes: bytes,
    ) -> bool:
        """Remove only the exact invocation-owned revision and body."""
        try:
            return bool(
                self._require_native().remove_if_matches(
                    name,
                    expected._native,
                    expected_bytes,
                )
            )
        except Exception as error:
            raise _translated(error) from error

    def remove_owned_temporary_if_matches(
        self,
        name: str,
        target: str,
        expected: AdoptionDirectoryEntry,
        expected_bytes: bytes,
    ) -> bool:
        """Remove one exact proof-bearing temporary owned by this plan."""
        try:
            return bool(
                self._require_native().remove_owned_temporary_if_matches(
                    name,
                    target,
                    expected._native,
                    expected_bytes,
                )
            )
        except Exception as error:
            raise _translated(error) from error

    def _require_native(self) -> Any:
        if self._native is None:
            raise AdoptionDirectoryError(errno.EBADF, "adoption_directory_authority_closed")
        return self._native


def _translated(error: Exception) -> AdoptionDirectoryError:
    message = str(error)
    if "byte ceiling" in message:
        code = errno.EFBIG
    elif "entry ceiling" in message:
        code = errno.E2BIG
    elif "publication" in message:
        code = errno.EEXIST
    elif "changed" in message:
        code = errno.ESTALE
    else:
        code = errno.EINVAL
    return AdoptionDirectoryError(code, message)


def _looks_migration_shaped(name: bytes) -> bool:
    return (
        len(name) >= 8
        and name[:4].isdigit()
        and name[4:5] == b"_"
        and (name.endswith(b".py") or name.endswith(b".json") or name.endswith(b".adoption.json"))
    )
