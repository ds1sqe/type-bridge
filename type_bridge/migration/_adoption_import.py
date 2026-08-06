"""Private, retained import environment for archive migration adoption.

The released loader imports a migrations package from an ambient filesystem
path.  Adoption cannot do that safely: a path component can be redirected
after the source checksum is captured, and normal imports may write
``__pycache__`` into the authority being verified.  This module materializes
the bounded, no-follow bytes retained by :mod:`_adoption_authority` into a
private temporary package and isolates that package's import namespace for
the duration of discovery and lowering.
"""

from __future__ import annotations

import _imp
import hashlib
import importlib
import json
import re
import sys
import tempfile
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from importlib.abc import Loader, MetaPathFinder
from importlib.machinery import ModuleSpec, PathFinder
from importlib.util import spec_from_loader
from pathlib import Path
from types import ModuleType
from typing import Any

from type_bridge.migration._adoption_authority import (
    AdoptionDirectoryAuthority,
    AdoptionDirectoryEntry,
)

_MIGRATION_SOURCE = re.compile(r"^[0-9]{4}_.*\.py$")
_SNAPSHOT_VERSION = re.compile(r"^v[0-9]{4}$")
_ADOPTION_TEMPORARY = re.compile(
    r"^\.tb-adopt-(?:pub|rm|gc)-[0-9a-f]{64}-[0-9a-f]{64}-(?:0|[1-9][0-9]{0,2})\.tmp$"
)
_MAX_ARTIFACT_BYTES = 16 * 1024 * 1024
_MAX_HISTORY_BYTES = 256 * 1024 * 1024
_MAX_DIRECTORY_ENTRIES = 65_536


class RetainedImportError(ValueError):
    """The retained package cannot be mirrored as one coherent import tree."""


@dataclass(frozen=True)
class CapturedImportInput:
    """One exact retained file used to construct the private import tree."""

    path: Path
    entry: AdoptionDirectoryEntry
    limit: int
    sha256: str


@dataclass(frozen=True)
class CapturedImportDirectory:
    """Exact child membership of one enumerated retained directory."""

    path: Path
    entry: AdoptionDirectoryEntry | None
    children: tuple[AdoptionDirectoryEntry, ...]


@dataclass(frozen=True)
class RetainedImportMirror:
    """Materialized package location and every retained byte authority used."""

    package_dir: Path
    captured_inputs: tuple[CapturedImportInput, ...]
    captured_directories: tuple[CapturedImportDirectory, ...]
    recovery_temporaries: tuple[CapturedImportInput, ...]
    recognized_root_names: frozenset[str]


class _MissingRetainedLoader(Loader):
    def __init__(self, fullname: str) -> None:
        self.fullname = fullname

    def create_module(self, spec: ModuleSpec) -> None:
        return None

    def exec_module(self, module: ModuleType) -> None:
        raise ModuleNotFoundError(
            f"retained adoption package has no module {self.fullname}",
            name=self.fullname,
        )


class _RetainedPackageFinder(MetaPathFinder):
    """Resolve one package namespace only through the private mirror."""

    def __init__(self, package_name: str, package_dir: Path) -> None:
        self.package_name = package_name
        self.package_dir = package_dir

    def find_spec(
        self,
        fullname: str,
        path: Any = None,
        target: ModuleType | None = None,
    ) -> ModuleSpec | None:
        del path, target
        if fullname == self.package_name:
            search_parent = self.package_dir.parent
        elif fullname.startswith(f"{self.package_name}."):
            suffix = fullname[len(self.package_name) + 1 :].split(".")
            search_parent = self.package_dir.joinpath(*suffix[:-1])
        else:
            return None
        spec = PathFinder.find_spec(fullname, [str(search_parent)])
        if spec is not None:
            return spec
        return spec_from_loader(fullname, _MissingRetainedLoader(fullname))


class _MirrorBuilder:
    def __init__(
        self,
        authority: AdoptionDirectoryAuthority,
        package_dir: Path,
    ) -> None:
        self.authority = authority
        self.package_dir = package_dir
        self.captured: dict[Path, CapturedImportInput] = {}
        self.captured_directories: dict[Path, CapturedImportDirectory] = {}
        self.recovery_temporaries: dict[Path, CapturedImportInput] = {}
        self.total_bytes = 0
        self.total_entries = 0

    def build(self) -> RetainedImportMirror:
        root_entries = self._entries(Path("."))
        recognized_root_names = frozenset(
            entry.name for entry in root_entries if _is_recognized_root_name(entry.name)
        )
        orphan_archives = sorted(
            name
            for name in recognized_root_names
            if name.endswith(".adoption.json")
            and f"{name.removesuffix('.adoption.json')}.py" not in recognized_root_names
        )
        if orphan_archives:
            raise RetainedImportError(
                "adoption metadata has no Python source to verify: " + ", ".join(orphan_archives)
            )
        generic_directories: list[tuple[Path, AdoptionDirectoryEntry]] = []
        snapshots_entry: AdoptionDirectoryEntry | None = None
        for entry in root_entries:
            relative = Path(entry.name)
            if entry.name == "snapshots":
                snapshots_entry = entry
                continue
            # Native publication can leave proof-bearing direct children if
            # the process is killed. Capture them outside the executable
            # mirror; the converter later requires each target/body digest to
            # match its newly reconstructed plan before accepting the retry.
            if _is_adoption_temporary_name(entry.name):
                self._capture_recovery_temporary(relative, entry)
                continue
            if entry.name.endswith(".py"):
                if entry.is_file() and not entry.is_symlink():
                    self._capture_python(relative, entry)
                elif _MIGRATION_SOURCE.fullmatch(entry.name):
                    raise RetainedImportError(f"migration source {relative} is not a regular file")
            elif (
                entry.is_file()
                and not entry.is_symlink()
                and not _is_recognized_root_name(entry.name)
            ):
                self._capture_file(relative, entry)
            elif entry.is_dir() and not entry.is_symlink() and entry.name != "__pycache__":
                self._materialize_directory(relative)
                generic_directories.append((relative, entry))

        self._capture_python_packages(generic_directories)
        if snapshots_entry is not None:
            self._capture_snapshots(snapshots_entry)

        return RetainedImportMirror(
            package_dir=self.package_dir,
            captured_inputs=tuple(self.captured[path] for path in sorted(self.captured)),
            captured_directories=tuple(
                self.captured_directories[path] for path in sorted(self.captured_directories)
            ),
            recovery_temporaries=tuple(
                self.recovery_temporaries[path] for path in sorted(self.recovery_temporaries)
            ),
            recognized_root_names=recognized_root_names,
        )

    def _capture_python_packages(
        self,
        initial: list[tuple[Path, AdoptionDirectoryEntry]],
    ) -> None:
        pending = list(reversed(sorted(initial, key=lambda item: str(item[0]))))
        while pending:
            relative, directory_entry = pending.pop()
            children = self._entries(relative, expected_directory=directory_entry)
            for child in children:
                child_relative = relative / child.name
                if child.name == "__pycache__":
                    continue
                if child.is_file() and not child.is_symlink():
                    self._capture_file(child_relative, child)
                elif child.is_dir() and not child.is_symlink():
                    self._materialize_directory(child_relative)
                    pending.append((child_relative, child))

    def _capture_snapshots(
        self,
        snapshots_entry: AdoptionDirectoryEntry,
    ) -> None:
        if snapshots_entry.is_symlink() or not snapshots_entry.is_dir():
            raise RetainedImportError("snapshots authority is not a retained directory")
        self._materialize_directory(Path("snapshots"))
        snapshot_entries = self._entries(
            Path("snapshots"),
            expected_directory=snapshots_entry,
        )
        generic_directories: list[tuple[Path, AdoptionDirectoryEntry]] = []
        for entry in snapshot_entries:
            relative = Path("snapshots") / entry.name
            if entry.is_file() and not entry.is_symlink():
                self._capture_file(relative, entry)
                continue
            if not _is_snapshot_version_name(entry.name):
                if entry.is_dir() and not entry.is_symlink() and entry.name != "__pycache__":
                    self._materialize_directory(relative)
                    generic_directories.append((relative, entry))
                continue
            if entry.is_symlink() or not entry.is_dir():
                raise RetainedImportError(
                    f"snapshot package {entry.name} is not a retained directory"
                )
            self._materialize_directory(relative)
            manifest_path = relative / "snapshot.json"
            self._charge_entries(1)
            manifest_entry = self.authority.inspect(
                manifest_path,
                expected_parent=entry,
            )
            if (
                manifest_entry is None
                or manifest_entry.is_symlink()
                or not manifest_entry.is_file()
            ):
                raise RetainedImportError(f"snapshot {entry.name} has no regular manifest")
            manifest_bytes = self._read(manifest_path, manifest_entry)
            try:
                manifest = json.loads(manifest_bytes.decode("utf-8"))
            except (UnicodeDecodeError, json.JSONDecodeError) as error:
                raise RetainedImportError(
                    f"snapshot {entry.name} manifest is not valid UTF-8 JSON"
                ) from error
            if manifest.get("version") != entry.name:
                raise RetainedImportError(f"snapshot {entry.name} manifest identity differs")
            source = manifest.get("source_migration")
            if not isinstance(source, str) or not source:
                raise RetainedImportError(f"snapshot {entry.name} has no source migration")
            file_hashes = manifest.get("file_hashes")
            if not isinstance(file_hashes, dict):
                raise RetainedImportError(f"snapshot {entry.name} has no file hash manifest")
            self._charge_entries(len(file_hashes))
            expected_names: set[str] = set()
            for raw_name, raw_digest in sorted(file_hashes.items()):
                if not isinstance(raw_name, str) or not _is_direct_name(raw_name):
                    raise RetainedImportError(
                        f"snapshot {entry.name} manifest contains a non-direct file name"
                    )
                if not isinstance(raw_digest, str) or not _is_sha256(raw_digest):
                    raise RetainedImportError(
                        f"snapshot {entry.name} manifest contains an invalid file digest"
                    )
                expected_names.add(raw_name)
                file_path = relative / raw_name
                file_entry = self.authority.inspect(file_path, expected_parent=entry)
                if file_entry is None or file_entry.is_symlink() or not file_entry.is_file():
                    raise RetainedImportError(
                        f"snapshot {entry.name} manifest file {raw_name} is not regular"
                    )
                body = self._read(file_path, file_entry)
                if hashlib.sha256(body).hexdigest() != raw_digest:
                    raise RetainedImportError(
                        f"snapshot {entry.name} manifest file {raw_name} changed"
                    )
                self._materialize(file_path, body)

            # Only manifest-bound files enter the executable mirror. A direct
            # lookup avoids letting unrelated children amplify enumeration;
            # an attempted import of an unbound helper fails closed.
            self._materialize(manifest_path, manifest_bytes)
        self._capture_python_packages(generic_directories)

    def _capture_python(self, relative: Path, entry: AdoptionDirectoryEntry) -> None:
        if entry.is_symlink() or not entry.is_file():
            raise RetainedImportError(f"Python import source {relative} is not a regular file")
        self._capture_file(relative, entry)

    def _capture_file(self, relative: Path, entry: AdoptionDirectoryEntry) -> None:
        """Capture one regular package source/resource through retained authority."""
        if entry.is_symlink() or not entry.is_file():
            raise RetainedImportError(f"package resource {relative} is not a regular file")
        self._materialize(relative, self._read(relative, entry))

    def _capture_recovery_temporary(
        self,
        relative: Path,
        entry: AdoptionDirectoryEntry,
    ) -> None:
        if entry.is_symlink() or not entry.is_file():
            raise RetainedImportError(
                f"adoption recovery temporary {relative} is not a regular file"
            )
        body = self.authority.read_bounded(
            relative,
            _MAX_ARTIFACT_BYTES,
            expected=entry,
        )
        self.total_bytes += len(body)
        if self.total_bytes > _MAX_HISTORY_BYTES:
            raise RetainedImportError("retained import tree exceeds the byte ceiling")
        self.recovery_temporaries[relative] = CapturedImportInput(
            relative,
            entry,
            _MAX_ARTIFACT_BYTES,
            hashlib.sha256(body).hexdigest(),
        )

    def _entries(
        self,
        relative: Path,
        *,
        expected_directory: AdoptionDirectoryEntry | None = None,
    ) -> tuple[AdoptionDirectoryEntry, ...]:
        remaining = _MAX_DIRECTORY_ENTRIES - self.total_entries
        if remaining <= 0:
            raise RetainedImportError("retained import tree exceeds the entry ceiling")
        entries = self.authority.entries(
            relative,
            maximum_entries=remaining,
            expected_directory=expected_directory,
        )
        self.total_entries += len(entries)
        if self.total_entries > _MAX_DIRECTORY_ENTRIES:
            raise RetainedImportError("retained import tree exceeds the entry ceiling")
        captured = CapturedImportDirectory(relative, expected_directory, entries)
        previous = self.captured_directories.get(relative)
        if previous is not None and not _same_directory_capture(previous, captured):
            raise RetainedImportError(f"retained import directory {relative} changed")
        self.captured_directories[relative] = captured
        return entries

    def _charge_entries(self, count: int) -> None:
        if count > _MAX_DIRECTORY_ENTRIES - self.total_entries:
            raise RetainedImportError("retained import tree exceeds the entry ceiling")
        self.total_entries += count

    def _read(self, relative: Path, entry: AdoptionDirectoryEntry) -> bytes:
        body = self.authority.read_bounded(
            relative,
            _MAX_ARTIFACT_BYTES,
            expected=entry,
        )
        self.total_bytes += len(body)
        if self.total_bytes > _MAX_HISTORY_BYTES:
            raise RetainedImportError("retained import tree exceeds the byte ceiling")
        digest = hashlib.sha256(body).hexdigest()
        previous = self.captured.get(relative)
        captured = CapturedImportInput(relative, entry, _MAX_ARTIFACT_BYTES, digest)
        if previous is not None and previous != captured:
            raise RetainedImportError(f"retained import source {relative} changed")
        self.captured[relative] = captured
        return body

    def _materialize(self, relative: Path, body: bytes) -> None:
        target = self.package_dir / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(body)

    def _materialize_directory(self, relative: Path) -> None:
        (self.package_dir / relative).mkdir(parents=True, exist_ok=True)


@contextmanager
def retained_import_mirror(
    authority: AdoptionDirectoryAuthority,
    migrations_dir: Path,
    expected_root_revision: object,
) -> Iterator[RetainedImportMirror]:
    """Yield one private package containing only retained, checked bytes."""
    with tempfile.TemporaryDirectory(prefix="typebridge-adoption-") as temporary:
        package_dir = Path(temporary) / migrations_dir.name
        package_dir.mkdir()
        mirror = _MirrorBuilder(authority, package_dir).build()
        authority.require_directory_revision(expected_root_revision)

        parent = str(package_dir.parent)
        released_parent = str(migrations_dir.parent.resolve())
        package_name = migrations_dir.name
        previous_path = list(sys.path)
        previous_meta_path = list(sys.meta_path)
        retained_finder = _RetainedPackageFinder(package_name, package_dir)
        _imp.acquire_lock()
        saved_modules = {
            name: module
            for name, module in sys.modules.items()
            if name == package_name or name.startswith(f"{package_name}.")
        }
        try:
            for name in saved_modules:
                sys.modules.pop(name, None)
            sys.path.insert(0, parent)
            # V1 temporarily exposes the migrations directory's parent so a
            # historical migration can use its released absolute-helper
            # imports. Keep the retained package first and force that package
            # namespace through `_RetainedPackageFinder`; only unrelated
            # ambient imports retain the V1 lookup contract.
            if released_parent not in previous_path:
                sys.path.insert(1, released_parent)
            sys.meta_path.insert(0, retained_finder)
            importlib.invalidate_caches()
            yield mirror
        finally:
            for name in list(sys.modules):
                if name == package_name or name.startswith(f"{package_name}."):
                    sys.modules.pop(name, None)
            sys.modules.update(saved_modules)
            sys.path[:] = previous_path
            sys.meta_path[:] = previous_meta_path
            importlib.invalidate_caches()
            _imp.release_lock()


def _is_direct_name(name: str) -> bool:
    return bool(name) and Path(name).name == name and name not in {".", ".."}


def _same_directory_capture(
    left: CapturedImportDirectory,
    right: CapturedImportDirectory,
) -> bool:
    if left.path != right.path or len(left.children) != len(right.children):
        return False
    if (left.entry is None) != (right.entry is None):
        return False
    if left.entry is not None and right.entry is not None:
        if not left.entry.same_identity(right.entry):
            return False
    return all(
        left_child.same_identity(right_child)
        for left_child, right_child in zip(left.children, right.children, strict=True)
    )


def _is_sha256(value: str) -> bool:
    return len(value) == 64 and all(character in "0123456789abcdef" for character in value)


def _is_snapshot_version_name(name: str) -> bool:
    return bool(_SNAPSHOT_VERSION.fullmatch(name))


def _is_adoption_temporary_name(name: str) -> bool:
    return bool(_ADOPTION_TEMPORARY.fullmatch(name))


def _adoption_temporary_identity(name: str) -> tuple[str, str, str, int] | None:
    match = _ADOPTION_TEMPORARY.fullmatch(name)
    if match is None:
        return None
    kind, target_sha256, contents_sha256, raw_attempt = (
        name.removeprefix(".tb-adopt-").removesuffix(".tmp").split("-")
    )
    attempt = int(raw_attempt)
    if attempt >= 128:
        return None
    return kind, target_sha256, contents_sha256, attempt


def _is_recognized_root_name(name: str) -> bool:
    if name == ".typebridge-adoption-conversion.json":
        return True
    if name.endswith(".adoption.json"):
        return bool(_MIGRATION_SOURCE.fullmatch(f"{name.removesuffix('.adoption.json')}.py"))
    if name.endswith(".json"):
        return bool(_MIGRATION_SOURCE.fullmatch(f"{name.removesuffix('.json')}.py"))
    return bool(_MIGRATION_SOURCE.fullmatch(name))
