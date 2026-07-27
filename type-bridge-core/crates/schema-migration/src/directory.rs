//! Cross-platform capability authority for one canonical migration directory.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::path::{Component, Path};

#[cfg(unix)]
use cap_fs_ext::OpenOptionsMaybeDirExt as _;
use cap_fs_ext::{DirExt as _, FollowSymlinks, OpenOptionsFollowExt as _};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};

/// One direct entry observed through a retained migration-directory handle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationDirectoryEntry {
    file_name: OsString,
    is_directory: bool,
    is_regular: bool,
}

impl MigrationDirectoryEntry {
    /// Return the direct entry name.
    pub fn file_name(&self) -> &OsStr {
        &self.file_name
    }

    /// Return whether this entry was observed as a directory.
    pub const fn is_directory(&self) -> bool {
        self.is_directory
    }

    /// Return whether this entry was observed as a regular file.
    pub const fn is_regular(&self) -> bool {
        self.is_regular
    }
}

/// A retained cross-platform capability for one migration directory.
///
/// Every child operation is relative to the open directory handle. The
/// workspace constructor walks each component with no-follow semantics, so a
/// concurrent rename or symlink replacement of the configured pathname cannot
/// redirect later discovery or publication.
pub struct MigrationDirectory {
    directory: Dir,
}

/// Exclusive advisory lock shared by canonical migration publishers.
///
/// The lock file is opened relative to the retained directory and never
/// follows a symbolic link. Holding this guard prevents cooperating migration
/// authoring and adoption publishers from inspecting and publishing against
/// different directory histories.
pub struct MigrationAuthoringLock<'a> {
    _file: File,
    pub(crate) directory: &'a MigrationDirectory,
}

impl std::fmt::Debug for MigrationDirectory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MigrationDirectory")
            .finish_non_exhaustive()
    }
}

impl MigrationDirectory {
    /// Open an ambient path for compatibility callers of the path-based APIs.
    pub fn open_ambient(path: &Path) -> io::Result<Self> {
        Ok(Self {
            directory: Dir::open_ambient_dir(path, ambient_authority())?,
        })
    }

    /// Open a confined relative directory beneath an ambient root.
    ///
    /// Each component is opened without following a final symlink. When
    /// `create` is true, missing components are created relative to the
    /// already retained parent handle and then opened under the same rule.
    pub fn open_beneath(root: &Path, relative: &Path, create: bool) -> io::Result<Self> {
        let directory = Dir::open_ambient_dir(root, ambient_authority())?;
        Self::open_beneath_directory(&directory, relative, create)
    }

    /// Open a confined relative directory beneath a retained root capability.
    ///
    /// Unlike [`Self::open_beneath`], this never resolves the root through an
    /// ambient pathname. Replacing or relinking the name by which the caller
    /// originally opened `root` therefore cannot redirect this operation.
    pub fn open_beneath_directory(root: &Dir, relative: &Path, create: bool) -> io::Result<Self> {
        let mut directory = root.try_clone()?;
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "migration directory is not a confined relative path",
                ));
            };
            let next = match directory.open_dir_nofollow(name) {
                Ok(next) => next,
                Err(error) if create && error.kind() == io::ErrorKind::NotFound => {
                    match directory.create_dir(name) {
                        Ok(()) => {}
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error),
                    }
                    directory.open_dir_nofollow(name)?
                }
                Err(error) => return Err(error),
            };
            directory = next;
        }
        Ok(Self { directory })
    }

    /// Enumerate at most `limit` direct entries through the retained handle.
    pub fn entries(&self, limit: usize) -> io::Result<Vec<MigrationDirectoryEntry>> {
        let mut entries = Vec::new();
        for entry in self.directory.entries()? {
            if entries.len() == limit {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "migration directory exceeds the entry ceiling",
                ));
            }
            let entry = entry?;
            let file_type = entry.file_type()?;
            entries.push(MigrationDirectoryEntry {
                file_name: entry.file_name(),
                is_directory: file_type.is_dir(),
                is_regular: file_type.is_file(),
            });
        }
        Ok(entries)
    }

    /// Open one direct regular child for reading without following a symlink.
    pub fn open_regular_readonly(&self, name: &OsStr) -> io::Result<File> {
        let name = validate_portable_direct_child(name)?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        let file = self.directory.open_with(name, &options)?.into_std();
        require_regular(&file)?;
        Ok(file)
    }

    /// Open or create one direct regular read-write child without following a symlink.
    pub fn open_regular_lock(&self, name: &OsStr) -> io::Result<File> {
        let name = validate_portable_direct_child(name)?;
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create(true)
            .follow(FollowSymlinks::No);
        // `open(2)` with `O_CREAT` is not atomic against a concurrent
        // creator of the same name on macOS: the process losing the
        // creation race can observe its original lookup miss as a
        // spurious `NotFound`. Either race outcome satisfies this
        // call's open-or-create contract, so `NotFound` is transient
        // by construction and retried; a directory that is genuinely
        // gone keeps failing and surfaces after the bounded attempts.
        let mut denied = None;
        for _ in 0..16 {
            match self.directory.open_with(name, &options) {
                Ok(file) => {
                    let file = file.into_std();
                    require_regular(&file)?;
                    return Ok(file);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => denied = Some(error),
                Err(error) => return Err(error),
            }
        }
        Err(denied.expect("every exhausted attempt recorded its refusal"))
    }

    /// Acquire the canonical migration-authoring lock without waiting.
    ///
    /// A lock held by another publisher surfaces as [`io::ErrorKind::WouldBlock`]
    /// on every platform.
    pub fn try_acquire_authoring_lock(&self) -> io::Result<MigrationAuthoringLock<'_>> {
        use fs2::FileExt as _;

        let file = self.open_regular_lock(".typebridge-authoring.lock".as_ref())?;
        file.try_lock_exclusive().map_err(|error| {
            // Windows reports a held lock as ERROR_LOCK_VIOLATION, which the
            // standard library does not classify as WouldBlock; normalize the
            // platform contention error so callers can match a single kind.
            if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
                io::Error::new(io::ErrorKind::WouldBlock, error)
            } else {
                error
            }
        })?;
        Ok(MigrationAuthoringLock {
            _file: file,
            directory: self,
        })
    }

    /// Exclusively create one direct regular child without following a symlink.
    pub fn create_new(&self, name: &OsStr) -> io::Result<File> {
        let name = validate_portable_direct_child(name)?;
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        let file = self.directory.open_with(name, &options)?.into_std();
        require_regular(&file)?;
        Ok(file)
    }

    /// Return whether any direct entry occupies `name`, without following it.
    pub fn entry_exists(&self, name: &OsStr) -> io::Result<bool> {
        let name = validate_portable_direct_child(name)?;
        match self.directory.symlink_metadata(name) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Remove one direct file or symlink entry.
    pub fn remove_file(&self, name: &OsStr) -> io::Result<()> {
        self.directory
            .remove_file(validate_portable_direct_child(name)?)
    }

    /// Publish a direct temporary under a direct final name without replacement.
    pub fn hard_link(&self, temporary: &OsStr, target: &OsStr) -> io::Result<()> {
        self.directory.hard_link(
            validate_portable_direct_child(temporary)?,
            &self.directory,
            validate_portable_direct_child(target)?,
        )
    }

    /// Flush directory metadata where the platform exposes that operation.
    pub fn sync_all(&self) -> io::Result<()> {
        #[cfg(unix)]
        {
            let mut options = OpenOptions::new();
            options
                .read(true)
                .maybe_dir(true)
                .follow(FollowSymlinks::No);
            self.directory
                .open_with(".", &options)?
                .into_std()
                .sync_all()
        }
        #[cfg(not(unix))]
        {
            Ok(())
        }
    }
}

/// Validate one UTF-8, cross-platform direct-child name.
///
/// The accepted vocabulary is deliberately the intersection of the Unix and
/// Windows regular-file namespaces. This prevents an authored name from
/// changing meaning when a workspace or migration history moves between
/// platforms (for example through an alternate data stream or device alias).
pub fn validate_portable_direct_child(name: &OsStr) -> io::Result<&Path> {
    let path = Path::new(name);
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "authority name is not a direct child",
        ));
    }
    let portable = name.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "authority name is not valid UTF-8",
        )
    })?;
    let trimmed = portable.trim_end_matches(['.', ' ']);
    let windows_stem = portable
        .split('.')
        .next()
        .unwrap_or(portable)
        .trim_end_matches(['.', ' ']);
    let windows_stem = windows_stem.to_ascii_uppercase();
    let windows_device = matches!(
        windows_stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|prefix| {
        windows_stem.strip_prefix(prefix).is_some_and(|suffix| {
            matches!(
                suffix,
                "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    });
    if portable.is_empty()
        || portable.contains(['<', '>', ':', '"', '/', '\\', '|', '?', '*', '\0'])
        || portable.chars().any(char::is_control)
        || trimmed != portable
        || windows_device
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "authority name is not portable",
        ));
    }
    Ok(path)
}

fn require_regular(file: &File) -> io::Result<()> {
    if file.metadata()?.is_file() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "migration authority is not a regular file",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    #[test]
    fn direct_child_publication_primitives_round_trip() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let directory =
            MigrationDirectory::open_ambient(temporary.path()).expect("directory capability opens");
        let _lock = directory
            .open_regular_lock(".lock".as_ref())
            .expect("lock opens without following");
        let mut candidate = directory
            .create_new(".candidate.tmp".as_ref())
            .expect("candidate creates exclusively");
        candidate.write_all(b"authority").expect("candidate writes");
        candidate.sync_all().expect("candidate flushes");
        directory
            .hard_link(".candidate.tmp".as_ref(), "authority.json".as_ref())
            .expect("no-replace publication links");
        directory.sync_all().expect("directory flushes");
        let mut published = directory
            .open_regular_readonly("authority.json".as_ref())
            .expect("published authority opens without following");
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut published, &mut bytes).expect("published bytes read");
        assert_eq!(bytes, b"authority");
    }

    #[test]
    fn contended_authoring_lock_surfaces_would_block_on_every_platform() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let directory =
            MigrationDirectory::open_ambient(temporary.path()).expect("directory capability opens");
        let held = directory
            .try_acquire_authoring_lock()
            .expect("first publisher acquires the authoring lock");
        // Windows reports the held lock as ERROR_LOCK_VIOLATION rather than
        // an errno the standard library maps to WouldBlock; the acquisition
        // seam must normalize both platforms to one contention kind.
        let contended = match directory.try_acquire_authoring_lock() {
            Ok(_) => panic!("second publisher must observe contention"),
            Err(error) => error,
        };
        assert_eq!(contended.kind(), io::ErrorKind::WouldBlock);
        drop(held);
        directory
            .try_acquire_authoring_lock()
            .expect("released lock reacquires");
    }

    #[test]
    fn direct_child_rejects_nonportable_and_windows_alias_spellings() {
        for name in [
            "nested/manifest.json",
            "nested\\manifest.json",
            "manifest.json:stream",
            "manifest.json.",
            "manifest.json ",
            "manifest?.json",
            "manifest*.json",
            "manifest<copy>.json",
            "manifest|copy.json",
            "manifest\"copy.json",
            "NUL",
            "con.json",
            "COM0",
            "com1.json",
            "COM¹.log",
            "LPT³",
            "CLOCK$",
            "conout$.txt",
            "line\nbreak",
        ] {
            assert!(
                validate_portable_direct_child(name.as_ref()).is_err(),
                "nonportable name {name:?} was accepted"
            );
        }
        assert!(validate_portable_direct_child(".typebridge-authoring.lock".as_ref()).is_ok());
        assert!(validate_portable_direct_child("0001_init.tbmigration.json".as_ref()).is_ok());
    }
}
