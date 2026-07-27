//! Retained filesystem authority for one physical workspace root.

use std::ffi::OsString;
use std::io::{self, Read as _};
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use cap_fs_ext::DirExt as _;
use cap_std::ambient_authority;
use cap_std::fs::{Dir, Metadata};
use type_bridge_schema::{
    SchemaSourceCapture, SchemaSourceIdentity, SchemaSourceKind, SchemaSourceObservation,
    SchemaSourceRevision, SchemaSourceService, SchemaSourceServiceError,
};
use type_bridge_schema_migration::validate_portable_direct_child;

use crate::{WorkspaceConfigError, WorkspaceConfigErrorCode, WorkspaceRoot, WorkspaceServiceError};

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

    /// Atomically replace one direct regular output through this authority.
    pub fn write_atomic(&self, file_name: &std::ffi::OsStr, bytes: &[u8]) -> Result<(), String> {
        use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt as _};
        use std::io::Write as _;

        let file_name = validate_portable_direct_child(file_name).map_err(|error| {
            format!(
                "generated output name is not a portable direct child beneath {}: {error}",
                self.display_path.display()
            )
        })?;
        let destination = self.display_path.join(file_name);
        match self.directory.symlink_metadata(file_name) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(format!(
                    "generated output {} must be a regular file, not a link or special entry",
                    destination.display()
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("cannot inspect {}: {error}", destination.display()));
            }
        }

        let portable_name = file_name
            .to_str()
            .ok_or_else(|| format!("{} has no UTF-8 file name", destination.display()))?;
        let mut temporary = None;
        for attempt in 0..128_u64 {
            let candidate = unique_output_temporary_name(portable_name, attempt);
            let mut options = cap_std::fs::OpenOptions::new();
            options
                .write(true)
                .create_new(true)
                .follow(FollowSymlinks::No);
            match self.directory.open_with(&candidate, &options) {
                Ok(file) => {
                    let mut file = file.into_std();
                    if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
                        let _ = self.directory.remove_file(&candidate);
                        return Err(format!(
                            "cannot write {}: {error}",
                            self.display_path.join(&candidate).display()
                        ));
                    }
                    temporary = Some(candidate);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(format!(
                        "cannot create {}: {error}",
                        self.display_path.join(&candidate).display()
                    ));
                }
            }
        }
        let temporary = temporary.ok_or_else(|| {
            format!(
                "cannot allocate a unique temporary beside {}",
                destination.display()
            )
        })?;
        self.directory
            .rename(&temporary, &self.directory, file_name)
            .map_err(|error| {
                let _ = self.directory.remove_file(&temporary);
                format!("cannot publish {}: {error}", destination.display())
            })?;
        self.sync_all()
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

fn unique_output_temporary_name(name: &str, attempt: u64) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_OUTPUT_TEMPORARY: AtomicU64 = AtomicU64::new(1);
    let nonce = NEXT_OUTPUT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    format!(".{name}.{}.{}.{}.tmp", std::process::id(), nonce, attempt)
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
