"""Checked JSON sidecar generation for Python-only migration histories.

The native (Rust) migration path executes only ``NNNN_name.json`` sidecars;
a ``.py``-only migration that 1.5.x ran through dynamic import has no
executable representation there. This module converts such histories by
loading each migration through the released :class:`MigrationLoader`
(dynamic import fallback intact) and lowering it with the released
:func:`~type_bridge.migration._lower.lower_migration` — the exact lowering
the Python executor uses — so the generated sidecar and the historical
execution semantics agree by construction.

Conversion preflights every collision before publication and uses a retained
journal for resumability. A late interruption can leave only immutable,
plan-identical partial artifacts plus that journal; rerunning the same command
resumes them, while native adoption fails closed until completion. Every
recognized migration-shaped source
receives separate archival adoption metadata read through the frozen trusted
Python loader. Sources that V1 ignored because they expose no public
``Migration`` subclass receive checksum-bound ignored-source evidence and stay
out of the graph. Migrations whose operations are serializable also receive an
executable sidecar; nonportable operations such as ``ops.RunPython``
deliberately receive only the archive because adoption verifies already-applied
history and never replays them. Released empty migrations are archived as
checksum-bound no-ops that inherit one exact snapshot authority from all of
their parents.

Usage::

    python -m type_bridge.migration.sidecar path/to/migrations
"""

from __future__ import annotations

import errno
import hashlib
import heapq
import json
import struct
from dataclasses import dataclass
from pathlib import Path

from type_bridge import _rust_runtime
from type_bridge.migration import operations as ops
from type_bridge.migration._adoption_authority import (
    AdoptionDirectoryAuthority,
    AdoptionDirectoryEntry,
    AdoptionDirectoryError,
)
from type_bridge.migration._adoption_import import (
    CapturedImportDirectory,
    CapturedImportInput,
    RetainedImportError,
    RetainedImportMirror,
    _adoption_temporary_identity,
    _is_recognized_root_name,
    _is_snapshot_version_name,
    retained_import_mirror,
)
from type_bridge.migration._lower import lower_migration, lower_validation_graph
from type_bridge.migration.loader import IgnoredMigrationSource, LoadedMigration, MigrationLoader

__all__ = ["SidecarConversionError", "generate_sidecars"]

_MAX_ARTIFACT_BYTES = 16 * 1024 * 1024
_MAX_HISTORY_BYTES = 256 * 1024 * 1024
_MAX_DIRECTORY_ENTRIES = 65_536
_MAX_JOURNAL_BYTES = 4 * 1024
_CONVERSION_JOURNAL = ".typebridge-adoption-conversion.json"


class SidecarConversionError(Exception):
    """A migration history cannot produce trustworthy archival artifacts.

    ``blockers`` maps each migration or artifact identity to the failed
    trust/integrity condition. Preflight failures write nothing. A failure
    after the conversion journal is published can leave resumable immutable
    artifacts, and native adoption rejects the history until a retry clears
    the journal.
    """

    def __init__(self, blockers: dict[str, str]) -> None:
        self.blockers = dict(blockers)
        details = "; ".join(f"{name}: {reason}" for name, reason in sorted(self.blockers.items()))
        super().__init__(
            f"checked adoption metadata conversion did not complete. Blockers: {details}."
        )


@dataclass
class _PendingSidecar:
    source_path: Path
    archive_json: str
    spec_json: str | None

    @property
    def sidecar_path(self) -> Path:
        return self.source_path.with_suffix(".json")

    @property
    def archive_path(self) -> Path:
        return self.source_path.with_suffix(".adoption.json")


@dataclass(frozen=True)
class _CapturedInput:
    path: Path
    entry: AdoptionDirectoryEntry
    limit: int
    sha256: str


@dataclass(frozen=True)
class _SnapshotAuthority:
    app_label: str
    migration_name: str
    schema_hash: str


@dataclass(frozen=True)
class _SchemaBinding:
    effect: str
    authority: _SnapshotAuthority


def generate_sidecars(migrations_dir: Path) -> list[Path]:
    """Generate checked JSON sidecars for every py-only migration in a directory.

    Discovers the history through the released loader's frozen Python import
    path, lowers each executable migration with the released execution
    lowering, and binds every migration-shaped source to its ``.py`` checksum.

    Returns:
        Paths of the adoption records and executable sidecars written, in
        source order. Empty when every required artifact already exists.

    Raises:
        SidecarConversionError: When the graph is invalid, a schema-affecting
            migration lacks its exact immutable snapshot, or a schema-neutral
            RunPython/no-op migration cannot inherit one converged parent
            authority. Nothing is written in that case.
        MigrationLoadError: When discovery itself fails (unreadable file,
            stale existing sidecar, broken migration module).
    """
    try:
        authority = AdoptionDirectoryAuthority.open(migrations_dir)
    except OSError as error:
        raise SidecarConversionError(
            {migrations_dir.name: "migration directory authority cannot be retained"}
        ) from error
    with authority:
        try:
            return _generate_sidecars_in(migrations_dir, authority)
        except AdoptionDirectoryError as error:
            if error.errno == errno.EILSEQ:
                raise SidecarConversionError(
                    {
                        migrations_dir.name: (
                            "a migration-shaped filename is not valid UTF-8; "
                            "the native JSON identity cannot represent this released-Unix "
                            "filename safely"
                        )
                    }
                ) from error
            raise SidecarConversionError(
                {migrations_dir.name: "migration authority changed during conversion"}
            ) from error


def _generate_sidecars_in(
    migrations_dir: Path,
    authority: AdoptionDirectoryAuthority,
) -> list[Path]:
    input_revision = authority.directory_revision()
    try:
        with retained_import_mirror(
            authority,
            migrations_dir,
            input_revision,
        ) as mirror:
            return _generate_sidecars_from_retained_mirror(
                migrations_dir,
                authority,
                input_revision,
                mirror,
            )
    except RetainedImportError as error:
        raise SidecarConversionError(
            {migrations_dir.name: f"retained import package is unsafe: {error}"}
        ) from error


def _generate_sidecars_from_retained_mirror(
    migrations_dir: Path,
    authority: AdoptionDirectoryAuthority,
    input_revision: object,
    mirror: RetainedImportMirror,
) -> list[Path]:
    # Preserve released discovery exactly: a valid sibling sidecar is the
    # migration authority and prevents Python execution. Only sources without
    # sidecars cross the frozen dynamic-import trust boundary.
    loader = MigrationLoader(
        migrations_dir,
        use_sidecars=True,
        adoption_limits=True,
        directory_authority=authority,
        adoption_import_dir=mirror.package_dir,
    )
    loaded_history = loader.discover()
    ignored_sources = loader.ignored_sources
    validation_errors = _rust_runtime.validate_migration_graph(
        lower_validation_graph(loaded_history), []
    )
    if validation_errors:
        blockers = {
            "history": ", ".join(
                f"{error['code']}:{error.get('app_label', '')}.{error.get('name', '')}"
                for error in validation_errors
            )
        }
        raise SidecarConversionError(blockers)

    pending: list[_PendingSidecar] = []
    snapshot_hashes, snapshot_inputs = _snapshot_schema_hashes(
        migrations_dir,
        authority,
        {loaded.migration.name for loaded in loaded_history},
    )
    source_digests = {loaded.path.name: loaded.source_sha256 for loaded in loaded_history} | {
        ignored.path.name: ignored.source_sha256 for ignored in ignored_sources
    }
    captured_inputs = [
        _CapturedInput(
            Path(name),
            entry,
            MigrationLoader.MAX_MIGRATION_FILE_BYTES,
            source_digests[name] or "",
        )
        for name, entry in loader.adoption_entries.items()
        if name in source_digests
    ]
    captured_inputs.extend(snapshot_inputs)
    captured_inputs.extend(
        _CapturedInput(item.path, item.entry, item.limit, item.sha256)
        for item in mirror.captured_inputs
    )
    execution_specs: dict[tuple[str, str], dict[str, object] | None] = {}
    execution_json: dict[tuple[str, str], str | None] = {}
    for loaded in loaded_history:
        key = (loaded.migration.app_label, loaded.migration.name)
        if loaded.execution_spec is not None:
            if loaded.execution_sidecar_json is None:
                raise SidecarConversionError(
                    {loaded.path.stem: "retained sidecar bytes were not captured"}
                )
            execution_specs[key] = loaded.execution_spec
            execution_json[key] = loaded.execution_sidecar_json
            continue
        try:
            spec = lower_migration(loaded.migration, checksum=loaded.checksum)
        except (TypeError, ValueError):
            execution_specs[key] = None
            execution_json[key] = None
            continue
        spec["source_sha256"] = loaded.source_sha256
        execution_specs[key] = spec
        execution_json[key] = _rust_runtime.migration_spec_to_json(spec)

    schema_bindings = _resolve_schema_bindings(
        loaded_history,
        snapshot_hashes,
        execution_specs,
    )
    accepted_existing_sidecars: dict[str, _CapturedInput] = {}

    for loaded in loaded_history:
        key = (loaded.migration.app_label, loaded.migration.name)
        sidecar_path = loaded.path.with_suffix(".json")
        spec = execution_specs[key]
        spec_json = execution_json[key]
        if spec is None or spec_json is None:
            archive_json = _adoption_metadata_json(
                loaded,
                schema_binding=schema_bindings[key],
            )
        else:
            archive_json = _sidecar_adoption_metadata_json(
                loaded,
                spec=spec,
                exact_sidecar_json=spec_json,
                schema_binding=schema_bindings[key],
            )
            if loaded.execution_spec is not None:
                if (
                    loaded.execution_sidecar_entry is None
                    or loaded.execution_sidecar_sha256 is None
                ):
                    raise SidecarConversionError(
                        {loaded.path.stem: "retained sidecar identity was not captured"}
                    )
                accepted_existing_sidecars[sidecar_path.name] = _CapturedInput(
                    Path(sidecar_path.name),
                    loaded.execution_sidecar_entry,
                    _MAX_ARTIFACT_BYTES,
                    loaded.execution_sidecar_sha256,
                )
        pending.append(
            _PendingSidecar(
                source_path=loaded.path,
                archive_json=archive_json,
                spec_json=spec_json,
            )
        )
    for ignored in ignored_sources:
        pending.append(
            _PendingSidecar(
                source_path=ignored.path,
                archive_json=_ignored_source_metadata_json(ignored),
                spec_json=None,
            )
        )
    pending.sort(key=lambda item: item.source_path.name)
    captured_inputs.extend(accepted_existing_sidecars.values())
    captured_inputs = _deduplicate_captured_inputs(captured_inputs)

    # Collision validation happens before the first write. Once the journal
    # is published, a late interruption is resumable from immutable partial
    # outputs rather than rolled back through ambient paths.
    existing: set[str] = set()
    try:
        for item in pending:
            authority.validate_publication_name(item.archive_path.name)
            if _require_absent_or_identical(authority, item.archive_path, item.archive_json):
                existing.add(item.archive_path.name)
            if item.spec_json is not None:
                authority.validate_publication_name(item.sidecar_path.name)
                if item.sidecar_path.name in accepted_existing_sidecars:
                    existing.add(item.sidecar_path.name)
                elif _require_absent_or_identical(
                    authority,
                    item.sidecar_path,
                    item.spec_json,
                ):
                    existing.add(item.sidecar_path.name)
        captured_bytes = _validate_captured_inputs(
            authority,
            input_revision,
            captured_inputs,
        )
        _validate_captured_directories(authority, mirror.captured_directories)
    except AdoptionDirectoryError as error:
        raise SidecarConversionError(
            {migrations_dir.name: "migration authority changed during conversion"}
        ) from error

    journal_json = _conversion_journal_json(captured_inputs, pending)
    journal_bytes = journal_json.encode("utf-8")
    if len(journal_bytes) > _MAX_JOURNAL_BYTES:
        raise SidecarConversionError(
            {migrations_dir.name: "conversion journal exceeds its byte ceiling"}
        )
    recovery_removals, recovery_bytes = _validate_recovery_temporaries(
        authority,
        mirror.recovery_temporaries,
        pending,
        journal_bytes,
    )
    stable_inputs = captured_inputs
    publication_bytes = _planned_publication_bytes(pending, accepted_existing_sidecars)
    if captured_bytes + recovery_bytes + publication_bytes > _MAX_HISTORY_BYTES:
        raise SidecarConversionError(
            {migrations_dir.name: "conversion history exceeds the 256 MiB aggregate ceiling"}
        )

    journal_path = migrations_dir / _CONVERSION_JOURNAL
    journal_exists = _require_absent_or_identical(authority, journal_path, journal_json)
    if not journal_exists:
        _write_atomic_no_replace(authority, journal_path, journal_json)
    journal_capture = _capture_expected_publication(
        authority,
        journal_path,
        journal_json,
    )
    journal_entry = journal_capture.entry

    written: list[Path] = []
    publication_inputs: dict[str, _CapturedInput] = {}
    expected_root_membership = set(mirror.recognized_root_names)
    expected_root_membership.add(_CONVERSION_JOURNAL)
    for item in pending:
        expected_root_membership.add(item.archive_path.name)
        if item.spec_json is not None:
            expected_root_membership.add(item.sidecar_path.name)
    current_revision = authority.directory_revision()
    try:
        _validate_captured_inputs(authority, current_revision, stable_inputs)
        _validate_captured_directories(
            authority,
            mirror.captured_directories,
            root_additions=(journal_entry,),
            root_removals=recovery_removals,
        )
        for item in pending:
            publications = [(item.archive_path, item.archive_json)]
            if item.spec_json is not None:
                publications.append((item.sidecar_path, item.spec_json))
            for path, contents in publications:
                accepted_sidecar = accepted_existing_sidecars.get(path.name)
                if path.name not in existing:
                    try:
                        _write_atomic_no_replace(authority, path, contents)
                        written.append(path)
                    except SidecarConversionError:
                        if not _require_absent_or_identical(authority, path, contents):
                            raise
                publication_inputs[path.name] = (
                    accepted_sidecar
                    if accepted_sidecar is not None
                    else _capture_expected_publication(authority, path, contents)
                )
        current_revision = authority.directory_revision()
        _validate_recognized_root_membership(authority, expected_root_membership)
        publication_entries = tuple(captured.entry for captured in publication_inputs.values())
        root_additions = (journal_entry, *publication_entries)
        _validate_captured_directories(
            authority,
            mirror.captured_directories,
            root_additions=root_additions,
            root_removals=recovery_removals,
        )
        final_snapshot_hashes, _ = _snapshot_schema_hashes(
            migrations_dir,
            authority,
            {loaded.migration.name for loaded in loaded_history},
        )
        if final_snapshot_hashes != snapshot_hashes:
            raise SidecarConversionError(
                {migrations_dir.name: "selected snapshot membership changed during publication"}
            )
        _validate_captured_directories(
            authority,
            mirror.captured_directories,
            root_additions=root_additions,
            root_removals=recovery_removals,
        )
        final_inputs = _deduplicate_captured_inputs(
            stable_inputs + list(publication_inputs.values())
        )
        _validate_captured_inputs(
            authority,
            current_revision,
            final_inputs,
        )
        removed = authority.remove_if_matches(
            _CONVERSION_JOURNAL,
            journal_entry,
            journal_bytes,
        )
        if not removed:
            raise SidecarConversionError(
                {migrations_dir.name: "conversion journal could not be cleared exactly"}
            )
        _validate_captured_directories(
            authority,
            mirror.captured_directories,
            root_additions=publication_entries,
            root_removals=recovery_removals | frozenset({_CONVERSION_JOURNAL}),
        )
        return written
    except Exception as error:
        if isinstance(error, SidecarConversionError):
            raise
        raise SidecarConversionError(
            {migrations_dir.name: "captured migration authority changed during publication"}
        ) from error


def _adoption_metadata_json(loaded: LoadedMigration, *, schema_binding: _SchemaBinding) -> str:
    if loaded.source_sha256 is None:
        raise SidecarConversionError(
            {loaded.migration.name: "captured Python source has no raw SHA-256 authority"}
        )
    dependencies = [
        {
            "app_label": dependency.app_label,
            "migration_name": dependency.migration_name,
        }
        for dependency in loaded.migration.get_dependencies()
    ]
    digest = hashlib.sha256()
    digest.update(b"typebridge.migration-adoption-metadata/v2\0")

    def field(value: str) -> None:
        encoded = value.encode("utf-8")
        digest.update(struct.pack(">Q", len(encoded)))
        digest.update(encoded)

    field(loaded.migration.app_label)
    field(loaded.migration.name)
    field(loaded.checksum)
    field(loaded.source_sha256)
    field(schema_binding.effect)
    field(schema_binding.authority.app_label)
    field(schema_binding.authority.migration_name)
    field(schema_binding.authority.schema_hash)
    digest.update(struct.pack(">Q", len(dependencies)))
    for dependency in dependencies:
        field(dependency["app_label"])
        field(dependency["migration_name"])
    payload: dict[str, object] = {
        "format": "typebridge.migration-adoption-metadata/v2",
        "app_label": loaded.migration.app_label,
        "name": loaded.migration.name,
        "dependencies": dependencies,
        "checksum": loaded.checksum,
        "source_sha256": loaded.source_sha256,
        "schema_effect": schema_binding.effect,
        "schema_source": {
            "app_label": schema_binding.authority.app_label,
            "migration_name": schema_binding.authority.migration_name,
        },
        "snapshot_schema_hash": schema_binding.authority.schema_hash,
        "metadata_digest": digest.hexdigest(),
    }
    return json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n"


def _sidecar_adoption_metadata_json(
    loaded: LoadedMigration,
    *,
    spec: dict[str, object],
    exact_sidecar_json: str,
    schema_binding: _SchemaBinding,
) -> str:
    """Bind released sidecar precedence without deriving semantics from Python."""
    if loaded.source_sha256 is None:
        raise SidecarConversionError(
            {loaded.path.stem: "captured Python source has no raw SHA-256 authority"}
        )
    try:
        normalized = _rust_runtime.migration_spec_from_json(
            _rust_runtime.migration_spec_to_json(spec)
        )
    except (TypeError, ValueError) as error:
        raise SidecarConversionError(
            {loaded.path.stem: "sidecar cannot be normalized as a MigrationSpec"}
        ) from error
    dependencies = list(normalized.get("dependencies", []))
    sidecar_checksum = normalized.get("checksum")
    if sidecar_checksum is not None and not isinstance(sidecar_checksum, str):
        raise SidecarConversionError(
            {loaded.path.stem: "sidecar checksum has an invalid normalized type"}
        )
    sidecar_sha256 = hashlib.sha256(exact_sidecar_json.encode("utf-8")).hexdigest()
    digest = hashlib.sha256()
    digest.update(b"typebridge.migration-adoption-sidecar/v1\0")

    def field(value: str) -> None:
        encoded = value.encode("utf-8")
        digest.update(struct.pack(">Q", len(encoded)))
        digest.update(encoded)

    field(loaded.path.stem)
    field(loaded.migration.app_label)
    field(loaded.migration.name)
    field(loaded.checksum)
    field(loaded.source_sha256)
    field(sidecar_sha256)
    if sidecar_checksum is None:
        digest.update(b"\0")
    else:
        digest.update(b"\1")
        field(sidecar_checksum)
    field(schema_binding.effect)
    field(schema_binding.authority.app_label)
    field(schema_binding.authority.migration_name)
    field(schema_binding.authority.schema_hash)
    digest.update(struct.pack(">Q", len(dependencies)))
    for dependency in dependencies:
        if not isinstance(dependency, dict):
            raise SidecarConversionError(
                {loaded.path.stem: "sidecar dependency has an invalid normalized type"}
            )
        field(str(dependency["app_label"]))
        field(str(dependency["migration_name"]))
    payload: dict[str, object] = {
        "format": "typebridge.migration-adoption-sidecar/v1",
        "source_name": loaded.path.stem,
        "app_label": loaded.migration.app_label,
        "name": loaded.migration.name,
        "dependencies": dependencies,
        # This is the checksum V1 placed in its applied ledger. A null or
        # omitted sidecar checksum falls back to this independently computed
        # source checksum; a present checksum was already required to match it.
        "checksum": loaded.checksum,
        "sidecar_checksum": sidecar_checksum,
        "source_sha256": loaded.source_sha256,
        "sidecar_sha256": sidecar_sha256,
        "schema_effect": schema_binding.effect,
        "schema_source": {
            "app_label": schema_binding.authority.app_label,
            "migration_name": schema_binding.authority.migration_name,
        },
        "snapshot_schema_hash": schema_binding.authority.schema_hash,
        "metadata_digest": digest.hexdigest(),
    }
    return json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n"


def _ignored_source_metadata_json(ignored: IgnoredMigrationSource) -> str:
    digest = hashlib.sha256()
    digest.update(b"typebridge.migration-adoption-ignored-source/v1\0")

    def field(value: str) -> None:
        encoded = value.encode("utf-8")
        digest.update(struct.pack(">Q", len(encoded)))
        digest.update(encoded)

    field(ignored.path.stem)
    field(ignored.checksum)
    field(ignored.source_sha256)
    payload = {
        "format": "typebridge.migration-adoption-ignored-source/v1",
        "name": ignored.path.stem,
        "checksum": ignored.checksum,
        "source_sha256": ignored.source_sha256,
        "metadata_digest": digest.hexdigest(),
    }
    return json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n"


def _resolve_schema_bindings(
    loaded_history: list[LoadedMigration],
    snapshot_hashes: dict[str, str],
    execution_specs: dict[tuple[str, str], dict[str, object] | None] | None = None,
) -> dict[tuple[str, str], _SchemaBinding]:
    by_key = {
        (loaded.migration.app_label, loaded.migration.name): loaded for loaded in loaded_history
    }
    resolved: dict[tuple[str, str], _SchemaBinding] = {}
    pending: dict[tuple[str, str], tuple[str, list[tuple[str, str]]]] = {}
    dependents: dict[tuple[str, str], list[tuple[str, str]]] = {}
    if execution_specs is None:
        execution_specs = {key: None for key in by_key}

    for key, loaded in by_key.items():
        exact_hash = snapshot_hashes.get(loaded.migration.name)
        if exact_hash is not None:
            resolved[key] = _SchemaBinding(
                effect="snapshot",
                authority=_SnapshotAuthority(key[0], key[1], exact_hash),
            )
            continue

        migration = loaded.migration
        execution_spec = execution_specs[key]
        if execution_spec is not None:
            operations = execution_spec.get("operations")
            if operations == []:
                schema_effect = "unchanged_noop"
            elif (
                isinstance(operations, list)
                and operations
                and all(
                    isinstance(operation, dict) and operation.get("kind") == "copy_attribute"
                    for operation in operations
                )
            ):
                schema_effect = "unchanged_copy_attribute"
            else:
                raise SidecarConversionError(
                    {
                        migration.name: (
                            "sidecar has no exact immutable snapshot and contains "
                            "operations that are not exclusively schema-neutral copy_attribute"
                        )
                    }
                )
        elif not migration.models and not migration.operations:
            schema_effect = "unchanged_noop"
        elif not migration.models and all(
            isinstance(operation, ops.RunPython) for operation in migration.operations
        ):
            schema_effect = "unchanged_run_python"
        else:
            raise SidecarConversionError(
                {
                    migration.name: (
                        "migration has no exact immutable snapshot and is not a "
                        "models-free RunPython-only or empty schema-neutral migration"
                    )
                }
            )
        dependencies = migration.get_dependencies()
        if not dependencies:
            raise SidecarConversionError(
                {migration.name: "schema-neutral migration has no snapshot-bound dependency"}
            )
        dependency_keys = [
            (dependency.app_label, dependency.migration_name) for dependency in dependencies
        ]
        missing = [dependency for dependency in dependency_keys if dependency not in by_key]
        if missing:
            raise SidecarConversionError(
                {
                    migration.name: (
                        "schema authority dependency is absent from the migration history: "
                        + ", ".join(f"{app}.{name}" for app, name in sorted(missing))
                    )
                }
            )
        pending[key] = (schema_effect, dependency_keys)
        for dependency in dependency_keys:
            dependents.setdefault(dependency, []).append(key)

    unresolved_parent_counts = {
        key: sum(dependency not in resolved for dependency in dependencies)
        for key, (_, dependencies) in pending.items()
    }
    ready = [key for key, count in unresolved_parent_counts.items() if count == 0]
    heapq.heapify(ready)

    while ready:
        key = heapq.heappop(ready)
        schema_effect, dependencies = pending[key]
        parents = [resolved[dependency] for dependency in dependencies]
        schema_hash = parents[0].authority.schema_hash
        if any(parent.authority.schema_hash != schema_hash for parent in parents[1:]):
            raise SidecarConversionError(
                {
                    key[1]: (
                        "schema-neutral merge dependencies resolve to divergent snapshot "
                        "authority records with different schema hashes"
                    )
                }
            )
        authority = min(
            (parent.authority for parent in parents),
            key=lambda candidate: (
                candidate.app_label,
                candidate.migration_name,
                candidate.schema_hash,
            ),
        )
        resolved[key] = _SchemaBinding(effect=schema_effect, authority=authority)
        for dependent in dependents.get(key, []):
            unresolved_parent_counts[dependent] -= 1
            if unresolved_parent_counts[dependent] == 0:
                heapq.heappush(ready, dependent)

    if len(resolved) != len(by_key):
        unresolved = sorted(key for key in by_key if key not in resolved)
        raise SidecarConversionError(
            {
                unresolved[0][1]: (
                    "schema authority dependency graph could not be resolved; "
                    "the history is cyclic or has no reachable snapshot authority"
                )
            }
        )
    owners_by_source: dict[str, set[str]] = {}
    for binding in resolved.values():
        owners_by_source.setdefault(binding.authority.migration_name, set()).add(
            binding.authority.app_label
        )
    ambiguous = [(source, owners) for source, owners in owners_by_source.items() if len(owners) > 1]
    if ambiguous:
        source, owners = min(ambiguous, key=lambda item: item[0])
        raise SidecarConversionError(
            {
                source: (
                    "snapshot source is ambiguous across app labels: " + ", ".join(sorted(owners))
                )
            }
        )
    return resolved


def _snapshot_schema_hashes(
    migrations_dir: Path,
    authority: AdoptionDirectoryAuthority,
    needed_sources: set[str],
) -> tuple[dict[str, str], list[_CapturedInput]]:
    snapshots_entry = authority.inspect_direct("snapshots")
    if snapshots_entry is None:
        return {}, []
    if snapshots_entry.is_symlink() or not snapshots_entry.is_dir():
        raise SidecarConversionError(
            {migrations_dir.name: "snapshots authority must be a real directory"}
        )

    try:
        entries = authority.entries(
            Path("snapshots"),
            maximum_entries=_MAX_DIRECTORY_ENTRIES,
            expected_directory=snapshots_entry,
        )
    except AdoptionDirectoryError as error:
        raise SidecarConversionError(
            {migrations_dir.name: "snapshot authority changed during enumeration"}
        ) from error
    aggregate = 0
    total_entries = len(entries)
    hashes: dict[str, str] = {}
    schema_bytes_by_source: dict[str, bytes] = {}
    captured_inputs: list[_CapturedInput] = []
    for entry in entries:
        if not _is_snapshot_version_name(entry.name):
            continue
        if entry.is_symlink() or not entry.is_dir():
            raise SidecarConversionError(
                {migrations_dir.name: f"unsafe snapshot directory {entry.name}"}
            )
        relative_directory = Path("snapshots") / entry.name
        if total_entries >= _MAX_DIRECTORY_ENTRIES:
            raise SidecarConversionError(
                {migrations_dir.name: "snapshot tree exceeds the entry ceiling"}
            )
        total_entries += 1
        try:
            manifest_entry = authority.inspect(
                relative_directory / "snapshot.json",
                expected_parent=entry,
            )
        except AdoptionDirectoryError as error:
            raise SidecarConversionError(
                {entry.name: "snapshot directory changed during manifest inspection"}
            ) from error
        if manifest_entry is None or manifest_entry.is_symlink() or not manifest_entry.is_file():
            raise SidecarConversionError({entry.name: "snapshot.json is absent or not regular"})
        manifest_path = relative_directory / "snapshot.json"
        try:
            manifest_bytes = authority.read_bounded(
                manifest_path,
                _MAX_ARTIFACT_BYTES,
                expected=manifest_entry,
            )
        except AdoptionDirectoryError as error:
            raise SidecarConversionError(
                {relative_directory.name: "snapshot manifest changed during capture"}
            ) from error
        aggregate += len(manifest_bytes)
        captured_inputs.append(
            _CapturedInput(
                manifest_path,
                manifest_entry,
                _MAX_ARTIFACT_BYTES,
                hashlib.sha256(manifest_bytes).hexdigest(),
            )
        )
        if aggregate > _MAX_HISTORY_BYTES:
            raise SidecarConversionError(
                {migrations_dir.name: "snapshot history exceeds the aggregate byte ceiling"}
            )
        try:
            metadata = json.loads(manifest_bytes.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise SidecarConversionError(
                {relative_directory.name: "snapshot manifest is not valid UTF-8 JSON"}
            ) from error
        if metadata.get("version") != entry.name:
            raise SidecarConversionError(
                {relative_directory.name: "snapshot manifest version identity differs"}
            )
        migration_name = metadata.get("source_migration")
        if not isinstance(migration_name, str) or not migration_name:
            raise SidecarConversionError(
                {manifest_path.parent.name: "snapshot source_migration is absent"}
            )
        if migration_name not in needed_sources:
            continue

        file_hashes = metadata.get("file_hashes")
        if not isinstance(file_hashes, dict):
            raise SidecarConversionError(
                {relative_directory.name: "snapshot file hash manifest is absent"}
            )
        validated_hashes: dict[str, str] = {}
        for raw_name, raw_hash in file_hashes.items():
            if (
                not isinstance(raw_name, str)
                or not raw_name
                or Path(raw_name).name != raw_name
                or raw_name in {".", ".."}
                or raw_name == "snapshot.json"
                or not isinstance(raw_hash, str)
                or len(raw_hash) != 64
                or any(character not in "0123456789abcdef" for character in raw_hash)
            ):
                raise SidecarConversionError(
                    {relative_directory.name: "snapshot file hash identity is invalid"}
                )
            validated_hashes[raw_name] = raw_hash
        if "schema.tql" not in validated_hashes:
            raise SidecarConversionError(
                {relative_directory.name: "snapshot manifest does not bind schema.tql"}
            )
        if len(validated_hashes) > _MAX_DIRECTORY_ENTRIES - total_entries:
            raise SidecarConversionError(
                {migrations_dir.name: "snapshot tree exceeds the entry ceiling"}
            )
        total_entries += len(validated_hashes)

        # Released snapshot validation treats the manifest as the closed set of
        # authoritative files but ignores unbound children such as __pycache__.
        # Open only the manifest-bound names through the retained directory;
        # never enumerate, follow, hash, or materialize ambient children.
        schema_bytes: bytes | None = None
        for filename, expected_hash in sorted(validated_hashes.items()):
            artifact_path = relative_directory / filename
            try:
                artifact_entry = authority.inspect(artifact_path, expected_parent=entry)
                if (
                    artifact_entry is None
                    or artifact_entry.is_symlink()
                    or not artifact_entry.is_file()
                ):
                    raise SidecarConversionError(
                        {
                            relative_directory.name: (
                                f"snapshot-bound file {filename} is absent or not regular"
                            )
                        }
                    )
                artifact_bytes = authority.read_bounded(
                    artifact_path,
                    _MAX_ARTIFACT_BYTES,
                    expected=artifact_entry,
                )
            except AdoptionDirectoryError as error:
                raise SidecarConversionError(
                    {
                        relative_directory.name: (
                            f"snapshot-bound file {filename} changed during capture"
                        )
                    }
                ) from error
            aggregate += len(artifact_bytes)
            if aggregate > _MAX_HISTORY_BYTES:
                raise SidecarConversionError(
                    {migrations_dir.name: "snapshot history exceeds the aggregate byte ceiling"}
                )
            actual_hash = hashlib.sha256(artifact_bytes).hexdigest()
            if actual_hash != expected_hash:
                raise SidecarConversionError(
                    {relative_directory.name: (f"snapshot file hash mismatch for {filename}")}
                )
            captured_inputs.append(
                _CapturedInput(
                    artifact_path,
                    artifact_entry,
                    _MAX_ARTIFACT_BYTES,
                    actual_hash,
                )
            )
            if filename == "schema.tql":
                schema_bytes = artifact_bytes

        if schema_bytes is None:
            raise SidecarConversionError({relative_directory.name: "snapshot schema.tql is absent"})
        schema_hash = hashlib.sha256(schema_bytes).hexdigest()
        if metadata.get("schema_hash") != schema_hash:
            raise SidecarConversionError(
                {migration_name: "snapshot schema hash mismatch at schema.tql"}
            )
        if migration_name in hashes and (
            hashes[migration_name] != schema_hash
            or schema_bytes_by_source[migration_name] != schema_bytes
        ):
            raise SidecarConversionError(
                {
                    migration_name: (
                        "snapshot source name is ambiguous across non-equivalent schemas"
                    )
                }
            )
        hashes[migration_name] = schema_hash
        schema_bytes_by_source[migration_name] = schema_bytes
    return hashes, captured_inputs


def _require_absent_or_identical(
    authority: AdoptionDirectoryAuthority,
    path: Path,
    contents: str,
) -> bool:
    entry = authority.inspect_direct(path.name)
    if entry is None:
        return False
    if entry.is_symlink() or not entry.is_file():
        raise SidecarConversionError({path.stem: "artifact path is a link or special entry"})
    try:
        existing = authority.read_bounded(
            Path(path.name),
            _MAX_ARTIFACT_BYTES,
            expected=entry,
        ).decode("utf-8")
    except UnicodeDecodeError as error:
        raise SidecarConversionError({path.stem: "existing artifact is not UTF-8"}) from error
    except AdoptionDirectoryError as error:
        raise SidecarConversionError(
            {path.stem: "existing artifact changed during validation"}
        ) from error
    if existing != contents:
        raise SidecarConversionError({path.stem: "existing artifact has different contents"})
    return True


def _capture_expected_publication(
    authority: AdoptionDirectoryAuthority,
    path: Path,
    contents: str,
) -> _CapturedInput:
    expected_bytes = contents.encode("utf-8")
    entry = authority.inspect_direct(path.name)
    if entry is None or entry.is_symlink() or not entry.is_file():
        raise SidecarConversionError({path.stem: "published artifact is not a regular file"})
    try:
        observed = authority.read_bounded(
            Path(path.name),
            _MAX_ARTIFACT_BYTES,
            expected=entry,
        )
    except AdoptionDirectoryError as error:
        raise SidecarConversionError(
            {path.stem: "published artifact changed while retaining its identity"}
        ) from error
    if observed != expected_bytes:
        raise SidecarConversionError(
            {path.stem: "published artifact body differs from the conversion plan"}
        )
    return _CapturedInput(
        Path(path.name),
        entry,
        _MAX_ARTIFACT_BYTES,
        hashlib.sha256(expected_bytes).hexdigest(),
    )


def _write_atomic_no_replace(
    authority: AdoptionDirectoryAuthority,
    path: Path,
    contents: str,
) -> None:
    encoded = contents.encode("utf-8")
    if len(encoded) > _MAX_ARTIFACT_BYTES:
        raise SidecarConversionError({path.stem: "generated artifact exceeds 16 MiB"})
    try:
        authority.write_atomic_no_replace(path.name, encoded)
    except OSError as error:
        raise SidecarConversionError(
            {path.stem: "artifact publication was rejected without replacement"}
        ) from error


def _validate_recovery_temporaries(
    authority: AdoptionDirectoryAuthority,
    temporaries: tuple[CapturedImportInput, ...],
    pending: list[_PendingSidecar],
    journal_bytes: bytes,
) -> tuple[frozenset[str], int]:
    planned: dict[str, bytes] = {_CONVERSION_JOURNAL: journal_bytes}
    for item in pending:
        outputs = [(item.archive_path.name, item.archive_json.encode("utf-8"))]
        if item.spec_json is not None:
            outputs.append((item.sidecar_path.name, item.spec_json.encode("utf-8")))
        for target, contents in outputs:
            previous = planned.get(target)
            if previous is not None and previous != contents:
                raise SidecarConversionError(
                    {target: "current conversion plan assigns conflicting output bodies"}
                )
            planned[target] = contents

    removed: set[str] = set()
    aggregate = 0
    for temporary in temporaries:
        identity = _adoption_temporary_identity(temporary.path.name)
        if identity is None:
            raise SidecarConversionError(
                {str(temporary.path): "adoption recovery temporary identity is invalid"}
            )
        kind, target_sha256, contents_sha256, _ = identity
        if contents_sha256 != temporary.sha256:
            raise SidecarConversionError(
                {
                    str(temporary.path): (
                        "adoption recovery temporary body does not match its proof-bearing name"
                    )
                }
            )
        candidates = [
            (target, contents)
            for target, contents in planned.items()
            if hashlib.sha256(target.encode("utf-8")).hexdigest() == target_sha256
            and hashlib.sha256(contents).hexdigest() == contents_sha256
            and (kind in {"pub", "gc"} or target == _CONVERSION_JOURNAL)
        ]
        if len(candidates) != 1:
            raise SidecarConversionError(
                {
                    str(temporary.path): (
                        "adoption recovery temporary is not owned by the current conversion plan"
                    )
                }
            )
        target, expected_contents = candidates[0]
        observed_contents = authority.read_bounded(
            temporary.path,
            temporary.limit,
            expected=temporary.entry,
        )
        if observed_contents != expected_contents:
            raise SidecarConversionError(
                {
                    str(temporary.path): (
                        "adoption recovery temporary differs from its exact planned output"
                    )
                }
            )
        aggregate += len(observed_contents)
        if aggregate > _MAX_HISTORY_BYTES:
            raise SidecarConversionError(
                {str(temporary.path): "adoption recovery temporaries exceed the byte ceiling"}
            )
        if not authority.remove_owned_temporary_if_matches(
            temporary.path.name,
            target,
            temporary.entry,
            expected_contents,
        ):
            raise SidecarConversionError(
                {
                    str(temporary.path): (
                        "owned adoption recovery temporary could not be removed exactly"
                    )
                }
            )
        removed.add(temporary.path.name)
    return frozenset(removed), aggregate


def _validate_captured_inputs(
    authority: AdoptionDirectoryAuthority,
    expected_root_revision: object,
    captured_inputs: list[_CapturedInput],
) -> int:
    authority.require_directory_revision(expected_root_revision)
    aggregate = 0
    for captured in _deduplicate_captured_inputs(captured_inputs):
        contents = authority.read_bounded(
            captured.path,
            captured.limit,
            expected=captured.entry,
        )
        if hashlib.sha256(contents).hexdigest() != captured.sha256:
            raise AdoptionDirectoryError(
                errno.ESTALE,
                "captured_adoption_input_body_changed",
            )
        aggregate += len(contents)
        if aggregate > _MAX_HISTORY_BYTES:
            raise AdoptionDirectoryError(
                errno.E2BIG,
                "captured_adoption_inputs_exceed_revalidation_budget",
            )
    authority.require_directory_revision(expected_root_revision)
    return aggregate


def _validate_captured_directories(
    authority: AdoptionDirectoryAuthority,
    captured_directories: tuple[CapturedImportDirectory, ...],
    *,
    root_additions: tuple[AdoptionDirectoryEntry, ...] = (),
    root_removals: frozenset[str] = frozenset(),
) -> None:
    for captured in captured_directories:
        expected = {entry.name: entry for entry in captured.children}
        if captured.path == Path("."):
            for name in root_removals:
                expected.pop(name, None)
            for entry in root_additions:
                previous = expected.get(entry.name)
                if previous is not None and not previous.same_identity(entry):
                    raise SidecarConversionError(
                        {entry.name: ("captured root entry identity changed during conversion")}
                    )
                expected[entry.name] = entry
        observed = authority.entries(
            captured.path,
            maximum_entries=_MAX_DIRECTORY_ENTRIES,
            expected_directory=captured.entry,
        )
        observed_by_name = {entry.name: entry for entry in observed}
        if observed_by_name.keys() != expected.keys() or any(
            not entry.same_identity(observed_by_name[name]) for name, entry in expected.items()
        ):
            raise SidecarConversionError(
                {str(captured.path): ("captured directory membership changed during conversion")}
            )


def _validate_recognized_root_membership(
    authority: AdoptionDirectoryAuthority,
    expected_names: set[str],
) -> None:
    entries = authority.entries(maximum_entries=_MAX_DIRECTORY_ENTRIES)
    observed_names = {entry.name for entry in entries if _is_recognized_root_name(entry.name)}
    if observed_names != expected_names:
        raise SidecarConversionError(
            {
                "history": (
                    "recognized migration membership changed during conversion "
                    f"(added={sorted(observed_names - expected_names)}, "
                    f"removed={sorted(expected_names - observed_names)})"
                )
            }
        )


def _deduplicate_captured_inputs(
    captured_inputs: list[_CapturedInput],
) -> list[_CapturedInput]:
    by_path: dict[Path, _CapturedInput] = {}
    for captured in captured_inputs:
        previous = by_path.get(captured.path)
        if previous is not None and (
            previous.limit != captured.limit or previous.sha256 != captured.sha256
        ):
            raise SidecarConversionError({str(captured.path): "captured input identities disagree"})
        if previous is None:
            by_path[captured.path] = captured
    return [by_path[path] for path in sorted(by_path)]


def _planned_publication_bytes(
    pending: list[_PendingSidecar],
    accepted_existing_sidecars: dict[str, _CapturedInput],
) -> int:
    aggregate = 0
    for item in pending:
        bodies = [(item.archive_path.name, item.archive_json)]
        if item.spec_json is not None:
            bodies.append((item.sidecar_path.name, item.spec_json))
        for name, body in bodies:
            if name in accepted_existing_sidecars:
                continue
            body_bytes = body.encode("utf-8")
            if len(body_bytes) > _MAX_ARTIFACT_BYTES:
                raise SidecarConversionError({Path(name).stem: "generated artifact exceeds 16 MiB"})
            aggregate += len(body_bytes)
            if aggregate > _MAX_HISTORY_BYTES:
                return aggregate
    return aggregate


def _conversion_journal_json(
    captured_inputs: list[_CapturedInput],
    pending: list[_PendingSidecar],
) -> str:
    digest = hashlib.sha256()
    digest.update(b"typebridge.adoption-conversion-journal/v1\0")

    def field(value: str) -> None:
        encoded = value.encode("utf-8")
        digest.update(struct.pack(">Q", len(encoded)))
        digest.update(encoded)

    planned_digests: dict[Path, str] = {}
    for item in pending:
        planned_digests[Path(item.archive_path.name)] = hashlib.sha256(
            item.archive_json.encode()
        ).hexdigest()
        if item.spec_json is not None:
            planned_digests[Path(item.sidecar_path.name)] = hashlib.sha256(
                item.spec_json.encode()
            ).hexdigest()

    for captured in sorted(captured_inputs, key=lambda item: str(item.path)):
        # A newly published canonical output becomes a captured existing
        # sidecar on retry. Keep the plan digest classification-stable by
        # representing identical bytes only in the planned-output domain.
        # A released legacy sidecar with different formatting/omitted fields
        # remains an independently bound input.
        if planned_digests.get(captured.path) == captured.sha256:
            continue
        field(str(captured.path))
        field(captured.sha256)
    for item in pending:
        field(item.archive_path.name)
        field(hashlib.sha256(item.archive_json.encode()).hexdigest())
        if item.spec_json is not None:
            field(item.sidecar_path.name)
            field(hashlib.sha256(item.spec_json.encode()).hexdigest())
    payload = {
        "format": "typebridge.adoption-conversion-journal/v1",
        "plan_sha256": digest.hexdigest(),
    }
    return json.dumps(payload, sort_keys=True, separators=(",", ":")) + "\n"


def _main() -> int:
    import argparse

    parser = argparse.ArgumentParser(
        prog="python -m type_bridge.migration.sidecar",
        description=(
            "Generate checked adoption metadata and, where representable, JSON "
            "execution sidecars so the native migration path can adopt a "
            "Python-only history."
        ),
    )
    parser.add_argument(
        "migrations_dir",
        type=Path,
        help="Directory containing NNNN_name.py migration files",
    )
    args = parser.parse_args()

    try:
        written = generate_sidecars(args.migrations_dir)
    except Exception as error:  # argparse-style CLI boundary: report, nonzero exit
        print(f"error: {error}")
        return 1

    if not written:
        print(
            "all migrations carry adoption metadata and every natively executable "
            "migration carries an execution sidecar; nothing to do"
        )
    else:
        for path in written:
            print(f"wrote {path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())
