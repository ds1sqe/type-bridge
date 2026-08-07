//! Retained filesystem authority for one physical workspace root.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io::{self, Read as _, Seek as _, Write as _};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use cap_fs_ext::DirExt as _;
use cap_std::ambient_authority;
use cap_std::fs::{Dir, Metadata};
use sha2::{Digest as _, Sha256};
use type_bridge_schema::{
    SchemaSourceCapture, SchemaSourceIdentity, SchemaSourceKind, SchemaSourceObservation,
    SchemaSourceRevision, SchemaSourceService, SchemaSourceServiceError,
};
use type_bridge_schema_migration::validate_portable_direct_child;

use crate::{
    WorkspaceConfigError, WorkspaceConfigErrorCode, WorkspaceRoot, WorkspaceServiceError,
    portable_path_collision_key, workspace_paths_overlap,
};

/// A retained, no-follow capability for one canonical workspace root.
///
/// Schema discovery and all later output or migration operations derive from
/// this descriptor. Renaming or replacing the ambient pathname after it opens
/// cannot split reads and writes across different physical roots.
pub struct WorkspaceDirectoryAuthority {
    directory: Dir,
    root: WorkspaceRoot,
}

/// A retained, confined directory used for generated workspace artifacts.
pub struct WorkspaceOutputDirectory {
    directory: Dir,
    display_path: PathBuf,
}

impl std::fmt::Debug for WorkspaceOutputDirectory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceOutputDirectory")
            .field("display_path", &self.display_path)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for WorkspaceDirectoryAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkspaceDirectoryAuthority")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl WorkspaceDirectoryAuthority {
    /// Open a canonical root component-by-component without following links.
    pub fn open(root: WorkspaceRoot) -> Result<Self, WorkspaceConfigError> {
        let directory = retain_canonical_root(root.as_path()).map_err(|_| {
            WorkspaceConfigError::new(
                WorkspaceConfigErrorCode::WorkspaceRootCanonicalizationFailed,
                "workspace root cannot be retained as directory authority",
            )
            .with_detail("workspace_root_authority_unavailable")
        })?;
        Ok(Self { directory, root })
    }

    /// Return the canonical display spelling bound to this authority.
    #[must_use]
    pub const fn root(&self) -> &WorkspaceRoot {
        &self.root
    }

    /// Return the retained root descriptor for handle-relative consumers.
    #[must_use]
    pub(crate) const fn directory(&self) -> &Dir {
        &self.directory
    }

    /// Clone the retained root into a generated-output authority.
    pub fn output_root(&self) -> Result<WorkspaceOutputDirectory, String> {
        WorkspaceOutputDirectory::from_root(&self.directory, self.root.as_path())
    }

    /// Capture one confined file with the schema loader's bounded semantics.
    pub fn capture_relative_file(
        &self,
        relative: &Path,
        maximum_bytes: usize,
    ) -> Result<SchemaSourceCapture, WorkspaceServiceError> {
        let absolute = self.root.as_path().join(relative);
        let capture = self
            .capture_file(&absolute, maximum_bytes)
            .map_err(|_| WorkspaceServiceError::new("workspace_file_read_failed"))?;
        if capture.before().kind() != SchemaSourceKind::File || capture.before() != capture.after()
        {
            return Err(WorkspaceServiceError::new(
                "workspace_file_changed_during_capture",
            ));
        }
        Ok(capture)
    }

    fn relative_path<'a>(&self, path: &'a Path) -> Result<&'a Path, SchemaSourceServiceError> {
        let relative = path
            .strip_prefix(self.root.as_path())
            .map_err(|_| SchemaSourceServiceError)?;
        if relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
            && !relative.as_os_str().is_empty()
        {
            return Err(SchemaSourceServiceError);
        }
        Ok(if relative.as_os_str().is_empty() {
            Path::new(".")
        } else {
            relative
        })
    }
}

impl WorkspaceOutputDirectory {
    pub(crate) fn from_root(directory: &Dir, display_path: &Path) -> Result<Self, String> {
        Ok(Self {
            directory: directory
                .try_clone()
                .map_err(|error| format!("cannot retain output root: {error}"))?,
            display_path: display_path.to_path_buf(),
        })
    }

    /// Open or create a real directory path beneath this retained authority.
    pub fn open_beneath(&self, relative: &Path) -> Result<Self, String> {
        let mut directory = self
            .directory
            .try_clone()
            .map_err(|error| format!("cannot retain output directory: {error}"))?;
        let mut display_path = self.display_path.clone();
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(format!(
                    "output directory {} is not confined beneath {}",
                    relative.display(),
                    self.display_path.display()
                ));
            };
            display_path.push(name);
            match directory.symlink_metadata(name) {
                Ok(metadata) => {
                    if metadata.file_type().is_symlink() || !metadata.is_dir() {
                        return Err(format!(
                            "output component {} must be a real directory, not a link",
                            display_path.display()
                        ));
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    match directory.create_dir(name) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(error) => {
                            return Err(format!(
                                "cannot create {}: {error}",
                                display_path.display()
                            ));
                        }
                    }
                }
                Err(error) => {
                    return Err(format!(
                        "cannot inspect {}: {error}",
                        display_path.display()
                    ));
                }
            }
            directory = directory.open_dir_nofollow(name).map_err(|error| {
                format!(
                    "output component {} must be a real directory, not a link: {error}",
                    display_path.display()
                )
            })?;
        }
        Ok(Self {
            directory,
            display_path,
        })
    }

    /// Return the configured path for diagnostics and operator output only.
    #[must_use]
    pub fn display_path(&self) -> &Path {
        self.display_path.as_path()
    }

    /// Transactionally replace one direct regular output through this authority.
    pub fn write_atomic(&self, file_name: &std::ffi::OsStr, bytes: &[u8]) -> Result<(), String> {
        let file_name = validate_portable_direct_child(file_name).map_err(|error| {
            format!(
                "generated output name is not a portable direct child beneath {}: {error}",
                self.display_path.display()
            )
        })?;
        self.write_atomic_batch([(Path::new(file_name), bytes)])
    }

    /// Atomically replace a generation of confined regular output files.
    ///
    /// Paths are relative to this retained directory and every component must
    /// be a portable direct-child name. The input order is the publication
    /// order. Every destination is validated and every same-directory
    /// temporary is written and flushed before the first destination changes.
    /// If ordinary publication or directory-flush work fails, all previously
    /// accepted files are restored byte-for-byte and newly introduced files
    /// are removed. This is generation-atomic within one running process; it
    /// does not claim crash-atomic publication across filesystems.
    pub fn write_atomic_batch<I, P, B>(&self, outputs: I) -> Result<(), String>
    where
        I: IntoIterator<Item = (P, B)>,
        P: AsRef<Path>,
        B: AsRef<[u8]>,
    {
        let outputs = outputs
            .into_iter()
            .map(|(path, bytes)| (path.as_ref().to_path_buf(), bytes))
            .collect();
        self.write_atomic_batch_with(outputs, |_| Ok(()))
    }

    fn write_atomic_batch_with<B, F>(
        &self,
        outputs: Vec<(PathBuf, B)>,
        mut after_destination_displaced: F,
    ) -> Result<(), String>
    where
        B: AsRef<[u8]>,
        F: FnMut(usize) -> Result<(), String>,
    {
        let plans = validate_batch_plans(self, outputs)?;
        if plans.is_empty() {
            return Ok(());
        }

        let mut entries = Vec::with_capacity(plans.len());
        let mut payloads = Vec::with_capacity(plans.len());
        for plan in plans {
            let parent_path = plan.relative_path.parent().unwrap_or_else(|| Path::new(""));
            let parent = self.open_beneath(parent_path)?;
            let file_name = plan
                .relative_path
                .file_name()
                .expect("validated output path has a final component")
                .to_os_string();
            let had_original = open_regular_output(&parent, &file_name)?.is_some();
            let replacement = bytes_identity(plan.bytes.as_ref())?;
            entries.push(PreparedBatchEntry {
                parent,
                file_name,
                had_original,
                original: None,
                replacement,
                temporary_name: None,
                temporary_live: false,
                backup_name: None,
                backup_live: false,
                backup_file: None,
                destination_state: DestinationState::Untouched,
            });
            payloads.push(plan.bytes);
        }

        if let Err(error) = prepare_batch_sidecars(&mut entries, &payloads)
            .and_then(|()| sync_batch_parents(&entries))
        {
            return Err(clean_up_unpublished_batch(&mut entries, error));
        }

        let publication = (|| -> Result<(), String> {
            for (index, entry) in entries.iter_mut().enumerate() {
                require_unchanged_destination(entry)?;
                if entry.had_original {
                    entry
                        .parent
                        .directory
                        .remove_file(&entry.file_name)
                        .map_err(|error| {
                            format!(
                                "cannot displace existing output {} before publication: {error}",
                                entry.parent.display_path.join(&entry.file_name).display()
                            )
                        })?;
                    entry.destination_state = DestinationState::VacantForPublication;
                }
                after_destination_displaced(index)?;
                let temporary = entry
                    .temporary_name
                    .as_ref()
                    .expect("prepared output has a temporary name")
                    .clone();
                entry
                    .parent
                    .directory
                    .hard_link(&temporary, &entry.parent.directory, &entry.file_name)
                    .map_err(|error| {
                        format!(
                            "cannot publish {} without replacing a concurrent destination: {error}",
                            entry.parent.display_path.join(&entry.file_name).display()
                        )
                    })?;
                entry.destination_state = DestinationState::ReplacementPublished;
                entry
                    .parent
                    .directory
                    .remove_file(&temporary)
                    .map_err(|error| {
                        format!(
                            "cannot retire published generation temporary {}: {error}",
                            entry.parent.display_path.join(temporary).display()
                        )
                    })?;
                entry.temporary_live = false;
            }
            sync_batch_parents(&entries)?;
            clean_up_published_backups(&mut entries)?;
            sync_batch_parents(&entries)?;
            require_no_batch_sidecars(&entries)
        })();

        match publication {
            Ok(()) => Ok(()),
            Err(error) => Err(rollback_batch(&mut entries, error)),
        }
    }

    fn sync_all(&self) -> Result<(), String> {
        #[cfg(unix)]
        {
            use cap_fs_ext::{
                FollowSymlinks, OpenOptionsFollowExt as _, OpenOptionsMaybeDirExt as _,
            };
            let mut options = cap_std::fs::OpenOptions::new();
            options
                .read(true)
                .maybe_dir(true)
                .follow(FollowSymlinks::No);
            self.directory
                .open_with(".", &options)
                .and_then(|directory| directory.into_std().sync_all())
                .map_err(|error| {
                    format!(
                        "cannot flush directory {}: {error}",
                        self.display_path.display()
                    )
                })
        }
        #[cfg(not(unix))]
        {
            Ok(())
        }
    }
}

struct BatchPlan<B> {
    relative_path: PathBuf,
    bytes: B,
}

struct PreparedBatchEntry {
    parent: WorkspaceOutputDirectory,
    file_name: OsString,
    had_original: bool,
    original: Option<OutputIdentity>,
    replacement: OutputIdentity,
    temporary_name: Option<OsString>,
    temporary_live: bool,
    backup_name: Option<OsString>,
    backup_live: bool,
    backup_file: Option<std::fs::File>,
    destination_state: DestinationState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DestinationState {
    Untouched,
    VacantForPublication,
    ReplacementPublished,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OutputIdentity {
    length: u64,
    sha256: [u8; 32],
}

fn bytes_identity(bytes: &[u8]) -> Result<OutputIdentity, String> {
    let length =
        u64::try_from(bytes.len()).map_err(|_| "generated output length exceeds u64".to_owned())?;
    let mut sha256 = [0_u8; 32];
    sha256.copy_from_slice(&Sha256::digest(bytes));
    Ok(OutputIdentity { length, sha256 })
}

fn validate_batch_plans<B>(
    root: &WorkspaceOutputDirectory,
    outputs: Vec<(PathBuf, B)>,
) -> Result<Vec<BatchPlan<B>>, String> {
    let mut plans = Vec::with_capacity(outputs.len());
    let mut paths = BTreeMap::new();
    for (relative_path, bytes) in outputs {
        validate_generated_output_path(root, &relative_path)?;
        let collision_key = portable_path_collision_key(&relative_path)
            .expect("validated generated output paths are portable UTF-8");
        if let Some(first) = paths.insert(collision_key, relative_path.clone()) {
            return Err(format!(
                "generated outputs {} and {} collide in one atomic batch after portable path normalization",
                root.display_path.join(first).display(),
                root.display_path.join(relative_path).display(),
            ));
        }
        plans.push(BatchPlan {
            relative_path,
            bytes,
        });
    }
    let paths = paths.into_values().collect::<Vec<_>>();
    for (left_index, left) in paths.iter().enumerate() {
        for right in paths.iter().skip(left_index + 1) {
            if workspace_paths_overlap(left, right) {
                return Err(format!(
                    "generated outputs {} and {} cannot be equal or nested after portable path normalization",
                    root.display_path.join(left).display(),
                    root.display_path.join(right).display(),
                ));
            }
        }
    }

    // Inspect every existing path before open_beneath is allowed to create a
    // missing parent for any target. Later hostile targets therefore cannot
    // leave earlier outputs partially prepared or published.
    for plan in &plans {
        prevalidate_existing_target(root, &plan.relative_path)?;
    }
    Ok(plans)
}

fn validate_generated_output_path(
    root: &WorkspaceOutputDirectory,
    relative_path: &Path,
) -> Result<(), String> {
    if relative_path.as_os_str().is_empty() {
        return Err(format!(
            "generated output path beneath {} is empty",
            root.display_path.display()
        ));
    }
    for component in relative_path.components() {
        let Component::Normal(name) = component else {
            return Err(format!(
                "generated output {} is not confined beneath {}",
                relative_path.display(),
                root.display_path.display()
            ));
        };
        validate_portable_direct_child(name).map_err(|error| {
            format!(
                "generated output component {:?} beneath {} is not portable: {error}",
                name,
                root.display_path.display()
            )
        })?;
    }
    Ok(())
}

fn prevalidate_existing_target(
    root: &WorkspaceOutputDirectory,
    relative_path: &Path,
) -> Result<(), String> {
    let mut directory = root
        .directory
        .try_clone()
        .map_err(|error| format!("cannot retain output root: {error}"))?;
    let mut display_path = root.display_path.clone();
    let parent = relative_path.parent().unwrap_or_else(|| Path::new(""));
    for component in parent.components() {
        let Component::Normal(name) = component else {
            unreachable!("generated output path was already validated")
        };
        display_path.push(name);
        match directory.symlink_metadata(name) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(format!(
                    "output component {} must be a real directory, not a link",
                    display_path.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(format!(
                    "cannot inspect {}: {error}",
                    display_path.display()
                ));
            }
        }
        directory = directory.open_dir_nofollow(name).map_err(|error| {
            format!(
                "output component {} must be a real directory, not a link: {error}",
                display_path.display()
            )
        })?;
    }

    let file_name = relative_path
        .file_name()
        .expect("validated output path has a final component");
    let destination = display_path.join(file_name);
    match directory.symlink_metadata(file_name) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(format!(
            "generated output {} must be a regular file, not a link or special entry",
            destination.display()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("cannot inspect {}: {error}", destination.display())),
    }
}

fn open_regular_output(
    parent: &WorkspaceOutputDirectory,
    file_name: &OsStr,
) -> Result<Option<std::fs::File>, String> {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt as _};

    let destination = parent.display_path.join(file_name);
    match parent.directory.symlink_metadata(file_name) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(format!(
                "generated output {} must be a regular file, not a link or special entry",
                destination.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!("cannot inspect {}: {error}", destination.display()));
        }
    }
    let mut options = cap_std::fs::OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let file = parent
        .directory
        .open_with(file_name, &options)
        .map_err(|error| format!("cannot open {}: {error}", destination.display()))?
        .into_std();
    if !file
        .metadata()
        .map_err(|error| format!("cannot inspect {}: {error}", destination.display()))?
        .is_file()
    {
        return Err(format!(
            "generated output {} must be a regular file, not a link or special entry",
            destination.display()
        ));
    }
    Ok(Some(file))
}

fn regular_output_identity(
    parent: &WorkspaceOutputDirectory,
    file_name: &OsStr,
) -> Result<Option<OutputIdentity>, String> {
    let Some(mut file) = open_regular_output(parent, file_name)? else {
        return Ok(None);
    };
    stream_identity(&mut file).map(Some).map_err(|error| {
        format!(
            "cannot fingerprint {}: {error}",
            parent.display_path.join(file_name).display()
        )
    })
}

fn stream_identity(reader: &mut impl io::Read) -> io::Result<OutputIdentity> {
    let mut length = 0_u64;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        length = length
            .checked_add(u64::try_from(read).expect("buffer length fits u64"))
            .ok_or_else(|| io::Error::other("generated output length exceeds u64"))?;
        hasher.update(&buffer[..read]);
    }
    let mut sha256 = [0_u8; 32];
    sha256.copy_from_slice(&hasher.finalize());
    Ok(OutputIdentity { length, sha256 })
}

fn prepare_batch_sidecars<B: AsRef<[u8]>>(
    entries: &mut [PreparedBatchEntry],
    payloads: &[B],
) -> Result<(), String> {
    for (entry, bytes) in entries.iter_mut().zip(payloads) {
        let (temporary, mut temporary_file) =
            create_unique_sidecar(&entry.parent, "typebridge-tmp")?;
        entry.temporary_name = Some(temporary);
        entry.temporary_live = true;
        temporary_file
            .write_all(bytes.as_ref())
            .and_then(|()| temporary_file.sync_all())
            .map_err(|error| {
                format!(
                    "cannot write {}: {error}",
                    entry
                        .parent
                        .display_path
                        .join(
                            entry
                                .temporary_name
                                .as_ref()
                                .expect("live temporary has a name")
                        )
                        .display()
                )
            })?;

        if entry.had_original {
            let mut original =
                open_regular_output(&entry.parent, &entry.file_name)?.ok_or_else(|| {
                    format!(
                        "generated output {} disappeared while its atomic batch was prepared",
                        entry.parent.display_path.join(&entry.file_name).display()
                    )
                })?;
            let (backup, mut backup_file) =
                create_unique_sidecar(&entry.parent, "typebridge-backup")?;
            entry.backup_name = Some(backup);
            entry.backup_live = true;
            let identity = stream_copy_identity(&mut original, &mut backup_file)
                .and_then(|identity| {
                    backup_file.sync_all()?;
                    backup_file.seek(io::SeekFrom::Start(0))?;
                    Ok(identity)
                })
                .map_err(|error| {
                    format!(
                        "cannot prepare rollback backup {}: {error}",
                        entry
                            .parent
                            .display_path
                            .join(entry.backup_name.as_ref().expect("live backup has a name"))
                            .display()
                    )
                })?;
            entry.original = Some(identity);
            entry.backup_file = Some(backup_file);
        }
    }
    Ok(())
}

fn create_unique_sidecar(
    parent: &WorkspaceOutputDirectory,
    kind: &str,
) -> Result<(OsString, std::fs::File), String> {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt as _};

    for attempt in 0..128_u64 {
        let candidate = unique_output_sidecar_name(kind, attempt);
        let mut options = cap_std::fs::OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        match parent.directory.open_with(&candidate, &options) {
            Ok(file) => return Ok((candidate.into(), file.into_std())),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "cannot create {}: {error}",
                    parent.display_path.join(&candidate).display()
                ));
            }
        }
    }
    Err(format!(
        "cannot allocate a unique {kind} file beneath {}",
        parent.display_path.display()
    ))
}

fn stream_copy_identity(
    reader: &mut impl io::Read,
    writer: &mut impl io::Write,
) -> io::Result<OutputIdentity> {
    let mut length = 0_u64;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read])?;
        length = length
            .checked_add(u64::try_from(read).expect("buffer length fits u64"))
            .ok_or_else(|| io::Error::other("generated output length exceeds u64"))?;
        hasher.update(&buffer[..read]);
    }
    let mut sha256 = [0_u8; 32];
    sha256.copy_from_slice(&hasher.finalize());
    Ok(OutputIdentity { length, sha256 })
}

fn require_unchanged_destination(entry: &PreparedBatchEntry) -> Result<(), String> {
    let current = regular_output_identity(&entry.parent, &entry.file_name)?;
    if current == entry.original {
        Ok(())
    } else {
        Err(format!(
            "generated output {} changed while its atomic batch was prepared",
            entry.parent.display_path.join(&entry.file_name).display()
        ))
    }
}

fn sync_batch_parents(entries: &[PreparedBatchEntry]) -> Result<(), String> {
    let mut synced = BTreeSet::new();
    for entry in entries {
        if synced.insert(entry.parent.display_path.clone()) {
            entry.parent.sync_all()?;
        }
    }
    Ok(())
}

fn clean_up_published_backups(entries: &mut [PreparedBatchEntry]) -> Result<(), String> {
    for entry in entries {
        if entry.backup_live {
            let backup = entry.backup_name.as_ref().expect("live backup has a name");
            entry
                .parent
                .directory
                .remove_file(backup)
                .map_err(|error| {
                    format!(
                        "cannot remove completed generation backup {}: {error}",
                        entry.parent.display_path.join(backup).display()
                    )
                })?;
            entry.backup_live = false;
        }
    }
    Ok(())
}

fn clean_up_unpublished_batch(entries: &mut [PreparedBatchEntry], primary: String) -> String {
    let mut errors = Vec::new();
    clean_up_batch_sidecars(entries, &mut errors);
    if let Err(error) = sync_batch_parents(entries) {
        errors.push(error);
    }
    if let Err(error) = require_no_batch_sidecars(entries) {
        errors.push(error);
    }
    append_rollback_errors(primary, errors)
}

fn rollback_batch(entries: &mut [PreparedBatchEntry], primary: String) -> String {
    let mut errors = Vec::new();
    for entry in entries.iter_mut().rev() {
        if entry.destination_state == DestinationState::Untouched {
            continue;
        }
        if entry.destination_state == DestinationState::ReplacementPublished {
            match regular_output_identity(&entry.parent, &entry.file_name) {
                Ok(Some(current)) if current == entry.replacement => {
                    match entry.parent.directory.remove_file(&entry.file_name) {
                        Ok(()) => {
                            entry.destination_state = DestinationState::VacantForPublication;
                        }
                        Err(error) => {
                            errors.push(format!(
                                "cannot remove newly published output {} before rollback: {error}",
                                entry.parent.display_path.join(&entry.file_name).display()
                            ));
                            continue;
                        }
                    }
                }
                Ok(current) => {
                    errors.push(format!(
                        "cannot roll back {} because the published destination changed: expected {:?}, found {current:?}",
                        entry.parent.display_path.join(&entry.file_name).display(),
                        entry.replacement
                    ));
                    continue;
                }
                Err(error) => {
                    errors.push(error);
                    continue;
                }
            }
        }
        if !require_vacant_rollback_destination(entry, &mut errors) {
            continue;
        }
        match entry.original {
            Some(_) => {
                let restored_from_backup = if entry.backup_live {
                    let backup = entry.backup_name.as_ref().expect("live backup has a name");
                    match entry.parent.directory.rename(
                        backup,
                        &entry.parent.directory,
                        &entry.file_name,
                    ) {
                        Ok(()) => {
                            entry.backup_live = false;
                            true
                        }
                        Err(error) => {
                            errors.push(format!(
                                "cannot restore {} from its generation backup: {error}",
                                entry.parent.display_path.join(&entry.file_name).display()
                            ));
                            false
                        }
                    }
                } else {
                    false
                };
                let restored = if restored_from_backup {
                    true
                } else {
                    match restore_original_from_retained_backup(entry) {
                        Ok(()) => true,
                        Err(error) => {
                            errors.push(error);
                            false
                        }
                    }
                };
                if restored {
                    entry.destination_state = DestinationState::Untouched;
                }
            }
            None => entry.destination_state = DestinationState::Untouched,
        }
    }
    clean_up_batch_sidecars(entries, &mut errors);
    if let Err(error) = sync_batch_parents(entries) {
        errors.push(error);
    }
    for entry in entries.iter() {
        match regular_output_identity(&entry.parent, &entry.file_name) {
            Ok(current) if current == entry.original => {}
            Ok(_) => errors.push(format!(
                "rollback did not restore byte-identical output {}",
                entry.parent.display_path.join(&entry.file_name).display()
            )),
            Err(error) => errors.push(error),
        }
    }
    if let Err(error) = require_no_batch_sidecars(entries) {
        errors.push(error);
    }
    append_rollback_errors(primary, errors)
}

fn require_vacant_rollback_destination(
    entry: &PreparedBatchEntry,
    errors: &mut Vec<String>,
) -> bool {
    match entry.parent.directory.symlink_metadata(&entry.file_name) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        Ok(_) => {
            errors.push(format!(
                "cannot restore {} because its destination reappeared during publication",
                entry.parent.display_path.join(&entry.file_name).display()
            ));
            false
        }
        Err(error) => {
            errors.push(format!(
                "cannot inspect {} before rollback: {error}",
                entry.parent.display_path.join(&entry.file_name).display()
            ));
            false
        }
    }
}

fn restore_original_from_retained_backup(entry: &mut PreparedBatchEntry) -> Result<(), String> {
    let expected = entry
        .original
        .expect("published pre-existing output has an identity");
    let backup_file = entry
        .backup_file
        .as_mut()
        .expect("published pre-existing output retains its backup handle");
    backup_file
        .seek(io::SeekFrom::Start(0))
        .map_err(|error| format!("cannot rewind retained generation backup: {error}"))?;
    let (restore, mut restore_file) = create_unique_sidecar(&entry.parent, "typebridge-rollback")?;
    let restoration = (|| -> Result<(), String> {
        let restored = stream_copy_identity(backup_file, &mut restore_file)
            .and_then(|identity| {
                restore_file.sync_all()?;
                Ok(identity)
            })
            .map_err(|error| format!("cannot copy retained generation backup: {error}"))?;
        if restored != expected {
            return Err("retained generation backup identity changed before rollback".to_owned());
        }
        entry
            .parent
            .directory
            .rename(&restore, &entry.parent.directory, &entry.file_name)
            .map_err(|error| {
                format!(
                    "cannot restore original output {}: {error}",
                    entry.parent.display_path.join(&entry.file_name).display()
                )
            })
    })();
    if let Err(primary) = restoration {
        return match entry.parent.directory.remove_file(&restore) {
            Ok(()) => Err(primary),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Err(primary),
            Err(error) => Err(format!(
                "{primary}; cannot remove rollback sidecar {}: {error}",
                entry.parent.display_path.join(&restore).display()
            )),
        };
    }
    Ok(())
}

fn clean_up_batch_sidecars(entries: &mut [PreparedBatchEntry], errors: &mut Vec<String>) {
    for entry in entries {
        let mut sidecars = vec![(&entry.temporary_name, &mut entry.temporary_live)];
        if entry.destination_state == DestinationState::Untouched {
            sidecars.push((&entry.backup_name, &mut entry.backup_live));
        }
        for (name, live) in sidecars {
            if !*live {
                continue;
            }
            let name = name.as_ref().expect("live sidecar has a name");
            match entry.parent.directory.remove_file(name) {
                Ok(()) => *live = false,
                Err(error) if error.kind() == io::ErrorKind::NotFound => *live = false,
                Err(error) => errors.push(format!(
                    "cannot remove generation sidecar {}: {error}",
                    entry.parent.display_path.join(name).display()
                )),
            }
        }
    }
}

fn require_no_batch_sidecars(entries: &[PreparedBatchEntry]) -> Result<(), String> {
    for entry in entries {
        for name in [&entry.temporary_name, &entry.backup_name]
            .into_iter()
            .flatten()
        {
            match entry.parent.directory.symlink_metadata(name) {
                Ok(_) => {
                    return Err(format!(
                        "generation sidecar {} was not removed",
                        entry.parent.display_path.join(name).display()
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(format!(
                        "cannot verify generation sidecar cleanup for {}: {error}",
                        entry.parent.display_path.join(name).display()
                    ));
                }
            }
        }
    }
    Ok(())
}

fn append_rollback_errors(primary: String, errors: Vec<String>) -> String {
    if errors.is_empty() {
        primary
    } else {
        format!(
            "{primary}; atomic generation rollback failed: {}",
            errors.join("; ")
        )
    }
}

fn unique_output_sidecar_name(kind: &str, attempt: u64) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_OUTPUT_TEMPORARY: AtomicU64 = AtomicU64::new(1);
    let nonce = NEXT_OUTPUT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    format!(".{kind}.{}.{}.{}", std::process::id(), nonce, attempt)
}

impl SchemaSourceService for WorkspaceDirectoryAuthority {
    fn canonicalize(&self, path: &Path) -> Result<PathBuf, SchemaSourceServiceError> {
        let relative = self.relative_path(path)?;
        let canonical = self
            .directory
            .canonicalize(relative)
            .map_err(|_| SchemaSourceServiceError)?;
        if canonical.is_absolute()
            || canonical
                .components()
                .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
        {
            return Err(SchemaSourceServiceError);
        }
        Ok(self.root.as_path().join(canonical))
    }

    fn metadata(&self, path: &Path) -> Result<SchemaSourceObservation, SchemaSourceServiceError> {
        let relative = self.relative_path(path)?;
        let metadata = self
            .directory
            .metadata(relative)
            .map_err(|_| SchemaSourceServiceError)?;
        observation(&metadata, path)
    }

    fn symlink_metadata(
        &self,
        path: &Path,
    ) -> Result<SchemaSourceObservation, SchemaSourceServiceError> {
        let relative = self.relative_path(path)?;
        let metadata = self
            .directory
            .symlink_metadata(relative)
            .map_err(|_| SchemaSourceServiceError)?;
        observation(&metadata, path)
    }

    fn read_directory_names(&self, path: &Path) -> Result<Vec<OsString>, SchemaSourceServiceError> {
        let relative = self.relative_path(path)?;
        let mut names = self
            .directory
            .read_dir(relative)
            .map_err(|_| SchemaSourceServiceError)?
            .map(|entry| {
                entry
                    .map(|entry| entry.file_name())
                    .map_err(|_| SchemaSourceServiceError)
            })
            .collect::<Result<Vec<_>, _>>()?;
        names.sort();
        Ok(names)
    }

    fn capture_file(
        &self,
        path: &Path,
        maximum_bytes: usize,
    ) -> Result<SchemaSourceCapture, SchemaSourceServiceError> {
        let relative = self.relative_path(path)?;
        let mut file = self
            .directory
            .open(relative)
            .map_err(|_| SchemaSourceServiceError)?;
        let before = observation(
            &file.metadata().map_err(|_| SchemaSourceServiceError)?,
            path,
        )?;
        let read_limit = u64::try_from(maximum_bytes)
            .unwrap_or(u64::MAX)
            .saturating_add(1);
        let mut bytes = Vec::new();
        (&mut file)
            .take(read_limit)
            .read_to_end(&mut bytes)
            .map_err(|_| SchemaSourceServiceError)?;
        let after = observation(
            &file.metadata().map_err(|_| SchemaSourceServiceError)?,
            path,
        )?;
        Ok(SchemaSourceCapture::new(bytes, before, after))
    }
}

pub(crate) fn retain_canonical_root(root: &Path) -> io::Result<Dir> {
    let mut anchor = PathBuf::new();
    let mut names = Vec::new();
    let mut reached_name = false;

    for component in root.components() {
        match component {
            Component::Prefix(prefix) if !reached_name => anchor.push(prefix.as_os_str()),
            Component::RootDir if !reached_name => anchor.push(component.as_os_str()),
            Component::Normal(name) => {
                reached_name = true;
                names.push(name.to_owned());
            }
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "workspace root is not a canonical absolute directory",
                ));
            }
        }
    }
    if anchor.as_os_str().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "workspace root is not absolute",
        ));
    }

    let mut directory = Dir::open_ambient_dir(anchor, ambient_authority())?;
    for name in names {
        directory = directory.open_dir_nofollow(name)?;
    }
    Ok(directory)
}

fn observation(
    metadata: &Metadata,
    path: &Path,
) -> Result<SchemaSourceObservation, SchemaSourceServiceError> {
    let modified = metadata
        .modified()
        .map_err(|_| SchemaSourceServiceError)?
        .into_std();
    let revision = match modified.duration_since(UNIX_EPOCH) {
        Ok(duration) => format!("after:{}:{}", duration.as_secs(), duration.subsec_nanos()),
        Err(error) => {
            let duration = error.duration();
            format!("before:{}:{}", duration.as_secs(), duration.subsec_nanos())
        }
    };
    let kind = if metadata.is_file() {
        SchemaSourceKind::File
    } else if metadata.is_dir() {
        SchemaSourceKind::Directory
    } else if metadata.is_symlink() {
        SchemaSourceKind::Symlink
    } else {
        SchemaSourceKind::Other
    };
    Ok(SchemaSourceObservation::new(
        source_identity(metadata, path)?,
        SchemaSourceRevision::new(revision)?,
        metadata.len(),
        kind,
    ))
}

#[cfg(unix)]
fn source_identity(
    metadata: &Metadata,
    _path: &Path,
) -> Result<SchemaSourceIdentity, SchemaSourceServiceError> {
    use cap_std::fs::MetadataExt as _;
    SchemaSourceIdentity::new(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(unix))]
fn source_identity(
    _metadata: &Metadata,
    path: &Path,
) -> Result<SchemaSourceIdentity, SchemaSourceServiceError> {
    SchemaSourceIdentity::new(path.to_string_lossy())
}

#[cfg(test)]
#[path = "authority_atomic_batch_tests.rs"]
mod atomic_batch_tests;
