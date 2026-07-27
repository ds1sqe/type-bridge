//! Read-only legacy-history evidence used by the V2 adoption checkpoint.
//!
//! Adoption never executes a legacy operation. A separate archival record is
//! therefore required even when released discovery selected an executable JSON
//! sidecar, and also covers migrations whose Python operation graph cannot be
//! lowered (notably `RunPython`). Sidecar authority binds and reparses both
//! exact files; source authority binds frozen Python identity and dependencies.
//! Every record carries an explicit immutable schema authority and cannot enter
//! the native execution loader. Proven schema-neutral `RunPython`, empty, and
//! sidecar `CopyAttribute` histories may inherit the one snapshot authority
//! shared by all parents.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
#[cfg(test)]
use std::fs;
use std::io;
use std::io::{Read as _, Write as _};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use cap_fs_ext::{DirExt as _, FollowSymlinks, OpenOptionsFollowExt as _};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, Metadata};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

#[cfg(test)]
use crate::checksum::migration_file_checksum;
use crate::error::{MigrationError, Result};
use crate::graph::validate_graph;
use crate::spec::{MigrationDependencySpec, MigrationGraph, MigrationSpec, OperationSpec};

/// Frozen discriminator for non-executable legacy adoption metadata.
pub const LEGACY_ADOPTION_METADATA_V2: &str = "typebridge.migration-adoption-metadata/v2";
/// Frozen discriminator for released sidecar-precedence adoption metadata.
pub const LEGACY_SIDECAR_ADOPTION_METADATA_V1: &str = "typebridge.migration-adoption-sidecar/v1";
/// Frozen discriminator for a migration-shaped Python source ignored by V1.
pub const LEGACY_IGNORED_SOURCE_METADATA_V1: &str =
    "typebridge.migration-adoption-ignored-source/v1";

/// Maximum number of entries retained while inspecting one legacy directory.
pub const MAX_LEGACY_DIRECTORY_ENTRIES: usize = 65_536;
/// Maximum bytes read from any one legacy artifact.
pub const MAX_LEGACY_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
/// Maximum aggregate bytes retained while inspecting one legacy history.
pub const MAX_LEGACY_HISTORY_BYTES: usize = 256 * 1024 * 1024;

const ADOPTION_SUFFIX: &str = ".adoption.json";
const SNAPSHOT_MANIFEST: &str = "snapshot.json";
const SNAPSHOT_SCHEMA: &str = "schema.tql";
const CONVERSION_JOURNAL: &str = ".typebridge-adoption-conversion.json";

#[cfg(test)]
thread_local! {
    static TEST_CAPTURED_DIRECTORY_ENTRIES: std::cell::Cell<usize> = const {
        std::cell::Cell::new(0)
    };
    static TEST_BEFORE_JOURNAL_QUARANTINE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
    static TEST_AFTER_SNAPSHOT_SCAN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LegacyEntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

/// Opaque, mutation-sensitive identity for one retained adoption entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyMetadataRevision {
    kind: LegacyEntryKind,
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    modified_seconds: i64,
    #[cfg(unix)]
    modified_nanoseconds: i64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanoseconds: i64,
    #[cfg(windows)]
    volume_serial_number: u32,
    #[cfg(windows)]
    file_index: u64,
    #[cfg(windows)]
    number_of_links: u32,
    #[cfg(windows)]
    file_attributes: u32,
    #[cfg(windows)]
    creation_time: u64,
    #[cfg(windows)]
    last_write_time: u64,
}

impl LegacyMetadataRevision {
    fn capture(metadata: &Metadata) -> Result<Self> {
        let kind = if metadata.is_file() {
            LegacyEntryKind::File
        } else if metadata.is_dir() {
            LegacyEntryKind::Directory
        } else if metadata.is_symlink() {
            LegacyEntryKind::Symlink
        } else {
            LegacyEntryKind::Other
        };
        #[cfg(unix)]
        {
            use cap_std::fs::MetadataExt as _;
            Ok(Self {
                kind,
                len: metadata.len(),
                modified: metadata.modified().ok().map(|value| value.into_std()),
                created: metadata.created().ok().map(|value| value.into_std()),
                device: metadata.dev(),
                inode: metadata.ino(),
                modified_seconds: metadata.mtime(),
                modified_nanoseconds: metadata.mtime_nsec(),
                changed_seconds: metadata.ctime(),
                changed_nanoseconds: metadata.ctime_nsec(),
            })
        }
        #[cfg(windows)]
        {
            use cap_fs_ext::MetadataExt as _;
            use cap_std::fs::MetadataExt as _;
            let volume_serial_number = metadata.volume_serial_number().ok_or_else(|| {
                loader_error("legacy authority metadata has no stable Windows volume identity")
            })?;
            let file_index = metadata.file_index().ok_or_else(|| {
                loader_error("legacy authority metadata has no stable Windows file identity")
            })?;
            let number_of_links = metadata.number_of_links().ok_or_else(|| {
                loader_error("legacy authority metadata has no stable Windows link identity")
            })?;
            Ok(Self {
                kind,
                len: metadata.len(),
                modified: metadata.modified().ok().map(|value| value.into_std()),
                created: metadata.created().ok().map(|value| value.into_std()),
                volume_serial_number,
                file_index,
                number_of_links,
                file_attributes: metadata.file_attributes(),
                creation_time: metadata.creation_time(),
                last_write_time: metadata.last_write_time(),
            })
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (kind, metadata);
            Err(loader_error(
                "legacy directory authority is unsupported without stable filesystem identity",
            ))
        }
    }

    fn same_retained_directory(&self, other: &Self) -> bool {
        if self.kind != LegacyEntryKind::Directory || other.kind != LegacyEntryKind::Directory {
            return false;
        }
        #[cfg(unix)]
        {
            // Renaming the caller-supplied root changes only its ctime on
            // Unix. The retained directory capability remains the same
            // authority; child membership changes still alter size/mtime and
            // every relevant child is independently revision-checked.
            self.len == other.len
                && self.modified == other.modified
                && self.created == other.created
                && self.device == other.device
                && self.inode == other.inode
                && self.modified_seconds == other.modified_seconds
                && self.modified_nanoseconds == other.modified_nanoseconds
        }
        #[cfg(not(unix))]
        {
            self == other
        }
    }

    fn same_file_after_rename(&self, other: &Self) -> bool {
        if self.kind != LegacyEntryKind::File || other.kind != LegacyEntryKind::File {
            return false;
        }
        #[cfg(unix)]
        {
            self.len == other.len
                && self.modified == other.modified
                && self.created == other.created
                && self.device == other.device
                && self.inode == other.inode
                && self.modified_seconds == other.modified_seconds
                && self.modified_nanoseconds == other.modified_nanoseconds
        }
        #[cfg(not(unix))]
        {
            self == other
        }
    }
}

/// One no-follow directory entry captured through retained adoption authority.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LegacyDirectoryEntry {
    name: OsString,
    revision: LegacyMetadataRevision,
}

impl LegacyDirectoryEntry {
    /// Return the direct-child name bound by this observation.
    pub fn name(&self) -> &OsStr {
        &self.name
    }

    /// Return the opaque entry revision token.
    pub const fn revision(&self) -> &LegacyMetadataRevision {
        &self.revision
    }

    /// Return whether this observation names a regular file.
    pub const fn is_file(&self) -> bool {
        matches!(self.revision.kind, LegacyEntryKind::File)
    }

    /// Return whether this observation names a real directory.
    pub const fn is_directory(&self) -> bool {
        matches!(self.revision.kind, LegacyEntryKind::Directory)
    }

    /// Return whether this observation names a symbolic link.
    pub const fn is_symlink(&self) -> bool {
        matches!(self.revision.kind, LegacyEntryKind::Symlink)
    }
}

#[derive(Clone, Debug)]
struct LegacyDirectoryCapture {
    revision: LegacyMetadataRevision,
    entries: Vec<LegacyDirectoryEntry>,
}

/// Retained, cross-platform directory authority for legacy adoption only.
pub struct LegacyDirectoryAuthority {
    directory: Dir,
    display_path: PathBuf,
}

impl std::fmt::Debug for LegacyDirectoryAuthority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LegacyDirectoryAuthority")
            .field("display_path", &self.display_path)
            .finish_non_exhaustive()
    }
}

impl LegacyDirectoryAuthority {
    /// Follow the caller-supplied root once and retain the resulting directory.
    pub fn open_root(path: &Path) -> Result<Self> {
        let display_path = lexical_absolute(path).map_err(|_| {
            loader_error("legacy directory cannot be retained as filesystem authority")
        })?;
        let directory =
            Dir::open_ambient_dir(&display_path, ambient_authority()).map_err(|_| {
                loader_error("legacy directory cannot be retained as filesystem authority")
            })?;
        Ok(Self {
            directory,
            display_path,
        })
    }

    fn open_child(&self, entry: &LegacyDirectoryEntry) -> Result<Self> {
        if entry.revision.kind != LegacyEntryKind::Directory {
            return Err(loader_error(
                "legacy directory entry is not a retained real directory",
            ));
        }
        let directory = self
            .directory
            .open_dir_nofollow(&entry.name)
            .map_err(|_| loader_error("legacy directory descendant cannot be retained"))?;
        let actual = LegacyMetadataRevision::capture(
            &directory
                .metadata(".")
                .map_err(|_| loader_error("legacy directory descendant cannot be inspected"))?,
        )?;
        if !actual.same_retained_directory(&entry.revision) {
            return Err(loader_error(
                "legacy directory descendant changed after enumeration",
            ));
        }
        Ok(Self {
            directory,
            display_path: self.display_path.join(&entry.name),
        })
    }

    fn capture(&self) -> Result<LegacyDirectoryCapture> {
        self.capture_with_limit(MAX_LEGACY_DIRECTORY_ENTRIES)
    }

    fn capture_with_limit(&self, maximum_entries: usize) -> Result<LegacyDirectoryCapture> {
        let before = self.revision()?;
        let mut entries = Vec::new();
        for entry in self
            .directory
            .read_dir(".")
            .map_err(|_| loader_error("legacy directory authority cannot be enumerated"))?
        {
            if entries.len() == maximum_entries {
                return Err(loader_error("legacy directory exceeds the entry ceiling"));
            }
            let entry = entry
                .map_err(|_| loader_error("legacy directory authority cannot be enumerated"))?;
            let name = entry.file_name();
            let metadata = self
                .directory
                .symlink_metadata(&name)
                .map_err(|_| loader_error("legacy directory entry cannot be inspected"))?;
            entries.push(LegacyDirectoryEntry {
                name,
                revision: LegacyMetadataRevision::capture(&metadata)?,
            });
            #[cfg(test)]
            TEST_CAPTURED_DIRECTORY_ENTRIES.with(|count| count.set(count.get() + 1));
        }
        entries.sort_by(|left, right| left.name.cmp(&right.name));
        let after = self.revision()?;
        if !before.same_retained_directory(&after) {
            return Err(loader_error(
                "legacy directory changed during bounded enumeration",
            ));
        }
        Ok(LegacyDirectoryCapture {
            revision: after,
            entries,
        })
    }

    fn require_revision(&self, expected: &LegacyMetadataRevision) -> Result<()> {
        if !self.revision()?.same_retained_directory(expected) {
            return Err(loader_error(
                "legacy directory changed during bounded adoption",
            ));
        }
        Ok(())
    }

    fn require_capture(&self, capture: &LegacyDirectoryCapture) -> Result<()> {
        self.require_revision(&capture.revision)?;
        for expected in &capture.entries {
            let metadata = self
                .directory
                .symlink_metadata(&expected.name)
                .map_err(|_| loader_error("legacy retained entry is absent during revalidation"))?;
            if LegacyMetadataRevision::capture(&metadata)? != expected.revision {
                return Err(loader_error(
                    "legacy retained entry changed after bounded capture",
                ));
            }
        }
        self.require_revision(&capture.revision)
    }

    fn require_filtered_capture(
        &self,
        capture: &LegacyDirectoryCapture,
        include: impl Fn(&OsStr) -> bool,
        message: &'static str,
    ) -> Result<()> {
        self.require_capture(capture)?;
        let mut observed = self.capture()?;
        observed.entries.retain(|entry| include(&entry.name));
        if observed.entries != capture.entries {
            return Err(loader_error(message));
        }
        self.require_capture(capture)
    }

    fn require_recognized_root_capture(&self, capture: &LegacyDirectoryCapture) -> Result<()> {
        self.require_filtered_capture(
            capture,
            is_recognized_legacy_root_name,
            "recognized legacy migration membership changed after bounded capture",
        )
    }

    fn require_snapshot_version_capture(&self, capture: &LegacyDirectoryCapture) -> Result<()> {
        self.require_filtered_capture(
            capture,
            |name| name.to_str().is_some_and(is_snapshot_version),
            "legacy snapshot version membership changed after bounded capture",
        )
    }

    fn revision(&self) -> Result<LegacyMetadataRevision> {
        self.directory
            .metadata(".")
            .map_err(|_| loader_error("legacy directory authority cannot be inspected"))
            .and_then(|metadata| LegacyMetadataRevision::capture(&metadata))
    }

    /// Return an opaque revision of the retained root.
    pub fn directory_revision(&self) -> Result<LegacyMetadataRevision> {
        self.revision()
    }

    /// Reject when the retained root no longer has the supplied revision.
    pub fn require_directory_revision(&self, expected: &LegacyMetadataRevision) -> Result<()> {
        self.require_revision(expected)
    }

    /// Enumerate one retained descendant, stopping before retaining entry N+1.
    pub fn entries_relative(
        &self,
        relative: &Path,
        maximum_entries: usize,
        expected_directory: Option<&LegacyDirectoryEntry>,
    ) -> Result<Vec<LegacyDirectoryEntry>> {
        let directory = self.open_relative_directory(relative)?;
        if let Some(expected) = expected_directory
            && (expected.revision.kind != LegacyEntryKind::Directory
                || !directory
                    .revision()?
                    .same_retained_directory(&expected.revision))
        {
            return Err(loader_error(
                "legacy directory descendant changed after enumeration",
            ));
        }
        let capture =
            directory.capture_with_limit(maximum_entries.min(MAX_LEGACY_DIRECTORY_ENTRIES))?;
        if let Some(expected) = expected_directory
            && !capture.revision.same_retained_directory(&expected.revision)
        {
            return Err(loader_error(
                "legacy directory descendant changed during enumeration",
            ));
        }
        Ok(capture.entries)
    }

    /// Inspect one retained descendant without following its final component.
    pub fn inspect_relative(
        &self,
        relative: &Path,
        expected_parent: Option<&LegacyDirectoryEntry>,
    ) -> Result<Option<LegacyDirectoryEntry>> {
        let (parent, name) = self.open_relative_parent(relative)?;
        if let Some(expected) = expected_parent
            && (expected.revision.kind != LegacyEntryKind::Directory
                || !parent
                    .revision()?
                    .same_retained_directory(&expected.revision))
        {
            return Err(loader_error(
                "legacy directory descendant changed after enumeration",
            ));
        }
        match parent.directory.symlink_metadata(&name) {
            Ok(metadata) => Ok(Some(LegacyDirectoryEntry {
                name,
                revision: LegacyMetadataRevision::capture(&metadata)?,
            })),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(loader_error("legacy directory entry cannot be inspected")),
        }
    }

    /// Read one stable regular descendant through retained no-follow authority.
    pub fn read_relative_bounded(
        &self,
        relative: &Path,
        limit: usize,
        expected: Option<&LegacyDirectoryEntry>,
    ) -> Result<Vec<u8>> {
        let (parent, name) = self.open_relative_parent(relative)?;
        let observed = match expected {
            Some(entry) if entry.name == name => entry.clone(),
            Some(_) => {
                return Err(loader_error(
                    "legacy artifact identity does not match its retained name",
                ));
            }
            None => parent
                .inspect_relative(Path::new(&name), None)?
                .ok_or_else(|| loader_error("legacy artifact is absent"))?,
        };
        let mut aggregate = 0;
        read_bounded(
            &parent,
            &observed,
            limit.min(MAX_LEGACY_ARTIFACT_BYTES),
            &mut aggregate,
        )
    }

    /// Publish one direct regular child atomically without replacing authority.
    pub fn write_atomic_no_replace(&self, name: &str, contents: &[u8]) -> Result<()> {
        validate_direct_component(Path::new(name))?;
        if contents.len() > MAX_LEGACY_ARTIFACT_BYTES {
            return Err(loader_error(
                "legacy artifact publication exceeds the byte ceiling",
            ));
        }
        let mut temporary = None;
        for attempt in 0..128_u64 {
            let candidate = adoption_temporary_name("pub", name, contents, attempt);
            let mut options = cap_std::fs::OpenOptions::new();
            options
                .write(true)
                .create_new(true)
                .follow(FollowSymlinks::No);
            match self.directory.open_with(&candidate, &options) {
                Ok(mut file) => {
                    if file
                        .write_all(contents)
                        .and_then(|()| file.sync_all())
                        .is_err()
                    {
                        // Close first so Windows can unlink the failed
                        // temporary as well. Final publication has not begun.
                        drop(file);
                        let _ = self.directory.remove_file(&candidate);
                        return Err(loader_error("legacy artifact temporary cannot be flushed"));
                    }
                    temporary = Some(candidate);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(_) => {
                    return Err(loader_error("legacy artifact temporary cannot be created"));
                }
            }
        }
        let temporary = temporary
            .ok_or_else(|| loader_error("legacy artifact temporary name ceiling exhausted"))?;
        let publication = self.directory.hard_link(&temporary, &self.directory, name);
        let _ = self.directory.remove_file(&temporary);
        publication.map_err(|_| loader_error("legacy artifact no-replace publication failed"))?;
        self.sync_directory()
    }

    /// Validate a prospective final artifact name without mutating the directory.
    pub fn validate_publication_name(&self, name: &str) -> Result<()> {
        let _ = self;
        validate_direct_component(Path::new(name))
    }

    /// Remove one proof-bearing converter temporary after exact current-plan
    /// validation by the caller.
    ///
    /// The source is first moved without replacement into the equally
    /// proof-bearing `gc` namespace. Its retained inode and exact body are
    /// rechecked there before unlink, so a raced replacement is restored and
    /// never deleted. A crash after the rename leaves another recoverable
    /// current-plan temporary rather than unowned hidden state.
    pub fn remove_owned_temporary_if_matches(
        &self,
        name: &str,
        target: &str,
        expected: &LegacyDirectoryEntry,
        expected_bytes: &[u8],
    ) -> Result<bool> {
        validate_direct_component(Path::new(name))?;
        validate_direct_component(Path::new(target))?;
        if expected.name != OsStr::new(name)
            || expected_bytes.len() > MAX_LEGACY_ARTIFACT_BYTES
            || !adoption_temporary_matches(name, target, expected_bytes)
        {
            return Err(loader_error(
                "adoption temporary identity does not match its target and exact body",
            ));
        }
        let Some(observed) = self.inspect_relative(Path::new(name), None)? else {
            return Ok(false);
        };
        if observed != *expected
            || self.read_relative_bounded(Path::new(name), expected_bytes.len(), Some(expected))?
                != expected_bytes
            || self.inspect_relative(Path::new(name), None)?.as_ref() != Some(expected)
        {
            return Ok(false);
        }

        let mut quarantine = None;
        for attempt in 0..128_u64 {
            let candidate = adoption_temporary_name("gc", target, expected_bytes, attempt);
            if candidate == name {
                continue;
            }
            match self.rename_no_replace(OsStr::new(name), OsStr::new(&candidate)) {
                Ok(()) => {
                    quarantine = Some(candidate);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(_) => {
                    return Err(loader_error(
                        "adoption temporary cannot be quarantined exactly",
                    ));
                }
            }
        }
        let quarantine = quarantine
            .ok_or_else(|| loader_error("adoption temporary quarantine name ceiling exhausted"))?;
        let moved = self.inspect_relative(Path::new(&quarantine), None)?;
        let moved_matches = match moved {
            Some(ref entry) if entry.revision.same_file_after_rename(&expected.revision) => self
                .read_relative_bounded(Path::new(&quarantine), expected_bytes.len(), Some(entry))
                .is_ok_and(|bytes| bytes == expected_bytes),
            _ => false,
        };
        if !moved_matches {
            self.rename_no_replace(OsStr::new(&quarantine), OsStr::new(name))
                .map_err(|_| {
                    loader_error(
                        "adoption temporary quarantine mismatch could not be restored without replacement",
                    )
                })?;
            self.sync_directory()?;
            return Ok(false);
        }
        self.directory.remove_file(&quarantine).map_err(|_| {
            loader_error("adoption temporary quarantine could not remove the exact publication")
        })?;
        self.sync_directory()?;
        Ok(true)
    }

    /// Remove one invocation-owned output only while revision and bytes match.
    pub fn remove_if_matches(
        &self,
        name: &str,
        expected: &LegacyDirectoryEntry,
        expected_bytes: &[u8],
    ) -> Result<bool> {
        validate_direct_component(Path::new(name))?;
        if name != CONVERSION_JOURNAL
            || expected.name != OsStr::new(name)
            || expected_bytes.len() > MAX_LEGACY_ARTIFACT_BYTES
        {
            return Err(loader_error(
                "legacy journal removal identity does not match its publication",
            ));
        }
        let Some(observed) = self.inspect_relative(Path::new(name), None)? else {
            return Ok(false);
        };
        if observed != *expected {
            return Ok(false);
        }
        let bytes =
            self.read_relative_bounded(Path::new(name), expected_bytes.len(), Some(expected))?;
        if bytes != expected_bytes {
            return Ok(false);
        }
        let Some(revalidated) = self.inspect_relative(Path::new(name), None)? else {
            return Ok(false);
        };
        if revalidated != *expected {
            return Ok(false);
        }

        #[cfg(test)]
        TEST_BEFORE_JOURNAL_QUARANTINE.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook();
            }
        });

        // Move the name away atomically without replacement, then prove the
        // moved inode/body before unlinking it. A swap at the final public
        // name is therefore quarantined and restored, never deleted.
        let mut quarantine = None;
        for attempt in 0..128_u64 {
            let candidate = adoption_temporary_name("rm", name, expected_bytes, attempt);
            match self.rename_no_replace(OsStr::new(name), OsStr::new(&candidate)) {
                Ok(()) => {
                    quarantine = Some(candidate);
                    break;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(_) => {
                    return Err(loader_error("legacy journal cannot be quarantined exactly"));
                }
            }
        }
        let quarantine = quarantine
            .ok_or_else(|| loader_error("legacy journal quarantine name ceiling exhausted"))?;
        let moved = self.inspect_relative(Path::new(&quarantine), None)?;
        let moved_matches = match moved {
            Some(ref entry) if entry.revision.same_file_after_rename(&expected.revision) => self
                .read_relative_bounded(Path::new(&quarantine), expected_bytes.len(), Some(entry))
                .is_ok_and(|bytes| bytes == expected_bytes),
            _ => false,
        };
        if !moved_matches {
            self.rename_no_replace(OsStr::new(&quarantine), OsStr::new(name))
                .map_err(|_| {
                    loader_error(
                        "legacy journal quarantine mismatch could not be restored without replacement",
                    )
                })?;
            self.sync_directory()?;
            return Ok(false);
        }
        self.directory.remove_file(&quarantine).map_err(|_| {
            loader_error("legacy journal quarantine could not remove the exact publication")
        })?;
        self.sync_directory()?;
        Ok(true)
    }

    #[allow(clippy::needless_return)] // Each cfg branch is a complete platform implementation.
    fn rename_no_replace(&self, source: &OsStr, destination: &OsStr) -> io::Result<()> {
        #[cfg(any(
            target_os = "android",
            target_os = "linux",
            target_os = "macos",
            target_os = "ios",
            target_os = "redox"
        ))]
        {
            use std::os::fd::AsFd as _;

            return rustix::fs::renameat_with(
                self.directory.as_fd(),
                source,
                self.directory.as_fd(),
                destination,
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(|error| io::Error::from_raw_os_error(error.raw_os_error()));
        }
        #[cfg(windows)]
        {
            // Windows rename fails rather than replacing an existing target.
            return self.directory.rename(source, &self.directory, destination);
        }
        #[cfg(not(any(
            target_os = "android",
            target_os = "linux",
            target_os = "macos",
            target_os = "ios",
            target_os = "redox",
            windows
        )))]
        {
            let _ = (source, destination);
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "conditional journal quarantine is unsupported on this platform",
            ))
        }
    }

    fn open_relative_directory(&self, relative: &Path) -> Result<Self> {
        let mut directory = self
            .directory
            .try_clone()
            .map_err(|_| loader_error("legacy directory authority cannot be cloned"))?;
        let mut display_path = self.display_path.clone();
        for component in relative_components(relative)? {
            directory = directory
                .open_dir_nofollow(&component)
                .map_err(|_| loader_error("legacy directory descendant cannot be retained"))?;
            display_path.push(component);
        }
        Ok(Self {
            directory,
            display_path,
        })
    }

    fn open_relative_parent(&self, relative: &Path) -> Result<(Self, OsString)> {
        let mut components = relative_components(relative)?;
        let name = components
            .pop()
            .ok_or_else(|| loader_error("legacy artifact name is empty"))?;
        let parent = components.iter().collect::<PathBuf>();
        Ok((self.open_relative_directory(&parent)?, name))
    }

    fn sync_directory(&self) -> Result<()> {
        #[cfg(unix)]
        {
            use cap_fs_ext::OpenOptionsMaybeDirExt as _;
            let mut options = cap_std::fs::OpenOptions::new();
            options
                .read(true)
                .maybe_dir(true)
                .follow(FollowSymlinks::No);
            self.directory
                .open_with(".", &options)
                .and_then(|directory| directory.sync_all())
                .map_err(|_| loader_error("legacy directory publication cannot be flushed"))
        }
        #[cfg(not(unix))]
        {
            Ok(())
        }
    }
}

fn relative_components(path: &Path) -> Result<Vec<OsString>> {
    let mut names = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => names.push(name.to_owned()),
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                return Err(loader_error(
                    "legacy authority path is not a confined relative descendant",
                ));
            }
        }
    }
    Ok(names)
}

fn validate_direct_component(path: &Path) -> Result<()> {
    let components = relative_components(path)?;
    if components.len() != 1 || !is_current_platform_direct_child(path.as_os_str()) {
        return Err(loader_error(
            "legacy authority publication name is not a safe direct child on this platform",
        ));
    }
    Ok(())
}

fn adoption_temporary_name(kind: &str, target: &str, contents: &[u8], attempt: u64) -> String {
    let target_digest = hex_digest(Sha256::digest(target.as_bytes()));
    let contents_digest = hex_digest(Sha256::digest(contents));
    format!(".tb-adopt-{kind}-{target_digest}-{contents_digest}-{attempt}.tmp")
}

fn adoption_temporary_matches(name: &str, target: &str, contents: &[u8]) -> bool {
    let Some(identity) = name
        .strip_prefix(".tb-adopt-")
        .and_then(|value| value.strip_suffix(".tmp"))
    else {
        return false;
    };
    let mut fields = identity.split('-');
    let (Some(kind), Some(target_digest), Some(contents_digest), Some(raw_attempt)) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return false;
    };
    if fields.next().is_some()
        || !matches!(kind, "pub" | "rm" | "gc")
        || !is_lower_hex(target_digest, 64)
        || !is_lower_hex(contents_digest, 64)
        || (raw_attempt.len() > 1 && raw_attempt.starts_with('0'))
        || raw_attempt
            .parse::<u64>()
            .map_or(true, |attempt| attempt >= 128)
    {
        return false;
    }
    target_digest == hex_digest(Sha256::digest(target.as_bytes()))
        && contents_digest == hex_digest(Sha256::digest(contents))
}

fn lexical_absolute(path: &Path) -> io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    if !absolute.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "legacy directory path is not absolute",
        ));
    }
    Ok(absolute)
}

/// Trusted-reader classification of one legacy migration's schema effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacySchemaEffect {
    /// This migration owns an exact immutable snapshot.
    Snapshot,
    /// This models-free migration contains only released `RunPython` data work.
    UnchangedRunPython,
    /// This released models-free migration has no operations.
    UnchangedNoop,
    /// This migration executes only released `CopyAttribute` DML operations.
    UnchangedCopyAttribute,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SnapshotAuthority {
    source: MigrationDependencySpec,
    schema_hash: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SchemaBinding {
    effect: LegacySchemaEffect,
    authority: SnapshotAuthority,
}

/// Non-executable identity record emitted by the frozen trusted Python reader.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyAdoptionMetadata {
    format: String,
    app_label: String,
    name: String,
    #[serde(default)]
    dependencies: Vec<MigrationDependencySpec>,
    checksum: String,
    source_sha256: String,
    schema_effect: LegacySchemaEffect,
    schema_source: MigrationDependencySpec,
    snapshot_schema_hash: String,
    metadata_digest: String,
}

impl LegacyAdoptionMetadata {
    /// Construct and domain-bind one archival record.
    #[allow(clippy::too_many_arguments)] // The public constructor mirrors the signed record fields.
    pub fn new(
        app_label: impl Into<String>,
        name: impl Into<String>,
        dependencies: Vec<MigrationDependencySpec>,
        checksum: impl Into<String>,
        source_sha256: impl Into<String>,
        schema_effect: LegacySchemaEffect,
        schema_source: MigrationDependencySpec,
        snapshot_schema_hash: impl Into<String>,
    ) -> Result<Self> {
        let mut metadata = Self {
            format: LEGACY_ADOPTION_METADATA_V2.to_owned(),
            app_label: app_label.into(),
            name: name.into(),
            dependencies,
            checksum: checksum.into(),
            source_sha256: source_sha256.into(),
            schema_effect,
            schema_source,
            snapshot_schema_hash: snapshot_schema_hash.into(),
            metadata_digest: String::new(),
        };
        metadata.validate_fields()?;
        metadata.metadata_digest = metadata.expected_digest();
        Ok(metadata)
    }

    /// Verify the discriminator, source checksum, and domain-separated digest.
    pub fn verify(&self) -> Result<()> {
        self.validate_fields()?;
        if self.metadata_digest != self.expected_digest() {
            return Err(loader_error(
                "legacy adoption metadata digest does not match its identity and dependencies",
            ));
        }
        Ok(())
    }

    /// Return the Python-source checksum bound by this archive.
    pub fn checksum(&self) -> &str {
        &self.checksum
    }

    /// Return the exact raw Python-source SHA-256 bound by this archive.
    pub fn source_sha256(&self) -> &str {
        &self.source_sha256
    }

    fn validate_fields(&self) -> Result<()> {
        if self.format != LEGACY_ADOPTION_METADATA_V2 {
            return Err(loader_error(
                "legacy adoption metadata uses an unsupported format discriminator",
            ));
        }
        if self.app_label.is_empty() || self.name.is_empty() {
            return Err(loader_error(
                "legacy adoption metadata has an empty migration identity",
            ));
        }
        if self.checksum.len() != 16
            || !self
                .checksum
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(loader_error(
                "legacy adoption metadata carries a malformed Python-source checksum",
            ));
        }
        if !is_lower_hex(&self.source_sha256, 64) {
            return Err(loader_error(
                "legacy adoption metadata carries a malformed raw-source digest",
            ));
        }
        if (self.metadata_digest.len() != 64 && !self.metadata_digest.is_empty())
            || self
                .metadata_digest
                .bytes()
                .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
        {
            return Err(loader_error(
                "legacy adoption metadata carries a malformed metadata digest",
            ));
        }
        if self.schema_source.app_label.is_empty() || self.schema_source.migration_name.is_empty() {
            return Err(loader_error(
                "legacy adoption metadata has an empty snapshot source identity",
            ));
        }
        if !is_lower_hex(&self.snapshot_schema_hash, 64) {
            return Err(loader_error(
                "legacy adoption metadata carries a malformed snapshot schema hash",
            ));
        }
        if self.schema_effect == LegacySchemaEffect::Snapshot
            && (self.schema_source.app_label != self.app_label
                || self.schema_source.migration_name != self.name)
        {
            return Err(loader_error(
                "snapshot schema effect must name the migration that owns the snapshot",
            ));
        }
        if matches!(
            self.schema_effect,
            LegacySchemaEffect::UnchangedRunPython
                | LegacySchemaEffect::UnchangedNoop
                | LegacySchemaEffect::UnchangedCopyAttribute
        ) && self.dependencies.is_empty()
        {
            return Err(loader_error(
                "unchanged schema effect requires a snapshot-bound dependency",
            ));
        }
        if self.schema_effect == LegacySchemaEffect::UnchangedCopyAttribute {
            return Err(loader_error(
                "Python-source adoption metadata cannot claim a sidecar-only copy-attribute effect",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"typebridge.migration-adoption-metadata/v2\0");
        hash_field(&mut hasher, self.app_label.as_bytes());
        hash_field(&mut hasher, self.name.as_bytes());
        hash_field(&mut hasher, self.checksum.as_bytes());
        hash_field(&mut hasher, self.source_sha256.as_bytes());
        let schema_effect: &[u8] = match self.schema_effect {
            LegacySchemaEffect::Snapshot => b"snapshot",
            LegacySchemaEffect::UnchangedRunPython => b"unchanged_run_python",
            LegacySchemaEffect::UnchangedNoop => b"unchanged_noop",
            LegacySchemaEffect::UnchangedCopyAttribute => b"unchanged_copy_attribute",
        };
        hash_field(&mut hasher, schema_effect);
        hash_field(&mut hasher, self.schema_source.app_label.as_bytes());
        hash_field(&mut hasher, self.schema_source.migration_name.as_bytes());
        hash_field(&mut hasher, self.snapshot_schema_hash.as_bytes());
        hasher.update(
            u64::try_from(self.dependencies.len())
                .unwrap_or(u64::MAX)
                .to_be_bytes(),
        );
        for dependency in &self.dependencies {
            hash_field(&mut hasher, dependency.app_label.as_bytes());
            hash_field(&mut hasher, dependency.migration_name.as_bytes());
        }
        hex_digest(hasher.finalize())
    }

    fn schema_binding(&self) -> SchemaBinding {
        SchemaBinding {
            effect: self.schema_effect,
            authority: SnapshotAuthority {
                source: self.schema_source.clone(),
                schema_hash: self.snapshot_schema_hash.clone(),
            },
        }
    }

    fn into_spec(self) -> MigrationSpec {
        MigrationSpec {
            app_label: self.app_label,
            name: self.name,
            dependencies: self.dependencies,
            operations: Vec::new(),
            checksum: Some(self.checksum),
            source_sha256: Some(self.source_sha256),
            // Archival metadata is deliberately never executable. This value
            // is only a graph DTO inside `LegacyAdoptionHistory`.
            reversible: false,
        }
    }
}

/// Non-executable authority for a migration selected from a released sidecar.
///
/// Released Python discovery prefers the sidecar and does not import the
/// sibling source. This record therefore binds both exact files and the
/// effective V1 graph identity/checksum while the native reader independently
/// parses the retained sidecar before admitting the graph member.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacySidecarAdoptionMetadata {
    format: String,
    source_name: String,
    app_label: String,
    name: String,
    #[serde(default)]
    dependencies: Vec<MigrationDependencySpec>,
    checksum: String,
    #[serde(default)]
    sidecar_checksum: Option<String>,
    source_sha256: String,
    sidecar_sha256: String,
    schema_effect: LegacySchemaEffect,
    schema_source: MigrationDependencySpec,
    snapshot_schema_hash: String,
    metadata_digest: String,
}

impl LegacySidecarAdoptionMetadata {
    /// Construct and domain-bind one released-sidecar authority record.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        source_name: impl Into<String>,
        app_label: impl Into<String>,
        name: impl Into<String>,
        dependencies: Vec<MigrationDependencySpec>,
        checksum: impl Into<String>,
        sidecar_checksum: Option<String>,
        source_sha256: impl Into<String>,
        sidecar_sha256: impl Into<String>,
        schema_effect: LegacySchemaEffect,
        schema_source: MigrationDependencySpec,
        snapshot_schema_hash: impl Into<String>,
    ) -> Result<Self> {
        let mut metadata = Self {
            format: LEGACY_SIDECAR_ADOPTION_METADATA_V1.to_owned(),
            source_name: source_name.into(),
            app_label: app_label.into(),
            name: name.into(),
            dependencies,
            checksum: checksum.into(),
            sidecar_checksum,
            source_sha256: source_sha256.into(),
            sidecar_sha256: sidecar_sha256.into(),
            schema_effect,
            schema_source,
            snapshot_schema_hash: snapshot_schema_hash.into(),
            metadata_digest: String::new(),
        };
        metadata.validate_fields()?;
        metadata.metadata_digest = metadata.expected_digest();
        Ok(metadata)
    }

    fn verify(&self) -> Result<()> {
        self.validate_fields()?;
        if self.metadata_digest != self.expected_digest() {
            return Err(loader_error(
                "legacy sidecar adoption metadata digest does not match its authority",
            ));
        }
        Ok(())
    }

    fn validate_fields(&self) -> Result<()> {
        if self.format != LEGACY_SIDECAR_ADOPTION_METADATA_V1 {
            return Err(loader_error(
                "legacy sidecar adoption metadata uses an unsupported format discriminator",
            ));
        }
        if !is_migration_stem(&self.source_name) {
            return Err(loader_error(
                "legacy sidecar adoption metadata has an invalid source filename stem",
            ));
        }
        if self.app_label.is_empty() || self.name.is_empty() {
            return Err(loader_error(
                "legacy sidecar adoption metadata has an empty effective identity",
            ));
        }
        if !is_lower_hex(&self.checksum, 16) {
            return Err(loader_error(
                "legacy sidecar adoption metadata carries a malformed effective checksum",
            ));
        }
        if let Some(sidecar_checksum) = &self.sidecar_checksum
            && (!is_lower_hex(sidecar_checksum, 16) || sidecar_checksum != &self.checksum)
        {
            return Err(loader_error(
                "legacy sidecar adoption metadata carries an invalid released checksum binding",
            ));
        }
        if !is_lower_hex(&self.source_sha256, 64) || !is_lower_hex(&self.sidecar_sha256, 64) {
            return Err(loader_error(
                "legacy sidecar adoption metadata carries a malformed exact-file digest",
            ));
        }
        if !self.metadata_digest.is_empty() && !is_lower_hex(&self.metadata_digest, 64) {
            return Err(loader_error(
                "legacy sidecar adoption metadata carries a malformed metadata digest",
            ));
        }
        if self.schema_source.app_label.is_empty() || self.schema_source.migration_name.is_empty() {
            return Err(loader_error(
                "legacy sidecar adoption metadata has an empty snapshot source identity",
            ));
        }
        if !is_lower_hex(&self.snapshot_schema_hash, 64) {
            return Err(loader_error(
                "legacy sidecar adoption metadata carries a malformed snapshot schema hash",
            ));
        }
        if self.schema_effect == LegacySchemaEffect::Snapshot
            && (self.schema_source.app_label != self.app_label
                || self.schema_source.migration_name != self.name)
        {
            return Err(loader_error(
                "sidecar snapshot schema effect must name the effective migration identity",
            ));
        }
        if self.schema_effect != LegacySchemaEffect::Snapshot && self.dependencies.is_empty() {
            return Err(loader_error(
                "unchanged sidecar schema effect requires a snapshot-bound dependency",
            ));
        }
        if self.schema_effect == LegacySchemaEffect::UnchangedRunPython {
            return Err(loader_error(
                "released sidecar authority cannot claim a Python-only schema effect",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"typebridge.migration-adoption-sidecar/v1\0");
        hash_field(&mut hasher, self.source_name.as_bytes());
        hash_field(&mut hasher, self.app_label.as_bytes());
        hash_field(&mut hasher, self.name.as_bytes());
        hash_field(&mut hasher, self.checksum.as_bytes());
        hash_field(&mut hasher, self.source_sha256.as_bytes());
        hash_field(&mut hasher, self.sidecar_sha256.as_bytes());
        match &self.sidecar_checksum {
            None => hasher.update([0]),
            Some(checksum) => {
                hasher.update([1]);
                hash_field(&mut hasher, checksum.as_bytes());
            }
        }
        let schema_effect: &[u8] = match self.schema_effect {
            LegacySchemaEffect::Snapshot => b"snapshot",
            LegacySchemaEffect::UnchangedRunPython => b"unchanged_run_python",
            LegacySchemaEffect::UnchangedNoop => b"unchanged_noop",
            LegacySchemaEffect::UnchangedCopyAttribute => b"unchanged_copy_attribute",
        };
        hash_field(&mut hasher, schema_effect);
        hash_field(&mut hasher, self.schema_source.app_label.as_bytes());
        hash_field(&mut hasher, self.schema_source.migration_name.as_bytes());
        hash_field(&mut hasher, self.snapshot_schema_hash.as_bytes());
        hasher.update(
            u64::try_from(self.dependencies.len())
                .unwrap_or(u64::MAX)
                .to_be_bytes(),
        );
        for dependency in &self.dependencies {
            hash_field(&mut hasher, dependency.app_label.as_bytes());
            hash_field(&mut hasher, dependency.migration_name.as_bytes());
        }
        hex_digest(hasher.finalize())
    }

    fn schema_binding(&self) -> SchemaBinding {
        SchemaBinding {
            effect: self.schema_effect,
            authority: SnapshotAuthority {
                source: self.schema_source.clone(),
                schema_hash: self.snapshot_schema_hash.clone(),
            },
        }
    }

    fn into_spec(
        self,
        mut sidecar: MigrationSpec,
        fallback_app_label: &str,
    ) -> Result<MigrationSpec> {
        let effective_app_label = if sidecar.app_label.is_empty() {
            fallback_app_label
        } else {
            sidecar.app_label.as_str()
        };
        let effective_name = if sidecar.name.is_empty() {
            self.source_name.as_str()
        } else {
            sidecar.name.as_str()
        };
        if effective_app_label != self.app_label
            || effective_name != self.name
            || sidecar.dependencies != self.dependencies
            || sidecar.checksum != self.sidecar_checksum
        {
            return Err(loader_error(
                "retained legacy sidecar semantics differ from their adoption metadata",
            ));
        }
        match self.schema_effect {
            LegacySchemaEffect::Snapshot => {}
            LegacySchemaEffect::UnchangedNoop if sidecar.operations.is_empty() => {}
            LegacySchemaEffect::UnchangedCopyAttribute
                if !sidecar.operations.is_empty()
                    && sidecar.operations.iter().all(|operation| {
                        matches!(operation, OperationSpec::CopyAttribute { .. })
                    }) => {}
            LegacySchemaEffect::UnchangedRunPython
            | LegacySchemaEffect::UnchangedNoop
            | LegacySchemaEffect::UnchangedCopyAttribute => {
                return Err(loader_error(
                    "retained legacy sidecar operations contradict their schema-effect binding",
                ));
            }
        }
        sidecar.app_label = self.app_label;
        sidecar.name = self.name;
        sidecar.checksum = Some(self.checksum);
        sidecar.source_sha256 = Some(self.source_sha256);
        // Adoption never replays an operation, even though the exact sidecar
        // remains independently bound and classified above.
        sidecar.operations.clear();
        Ok(sidecar)
    }
}

/// Checksum-bound evidence for a migration-shaped source that V1 ignored.
///
/// The released Python loader executes each `NNNN_*.py` module but omits it
/// from migration history when the module exposes no public `Migration`
/// subclass. The frozen converter records that classification without
/// inventing a graph node, and the native adoption reader verifies this
/// record before preserving the same omission.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyIgnoredSourceMetadata {
    format: String,
    name: String,
    checksum: String,
    source_sha256: String,
    metadata_digest: String,
}

impl LegacyIgnoredSourceMetadata {
    /// Construct and domain-bind one ignored-source record.
    pub fn new(
        name: impl Into<String>,
        checksum: impl Into<String>,
        source_sha256: impl Into<String>,
    ) -> Result<Self> {
        let mut metadata = Self {
            format: LEGACY_IGNORED_SOURCE_METADATA_V1.to_owned(),
            name: name.into(),
            checksum: checksum.into(),
            source_sha256: source_sha256.into(),
            metadata_digest: String::new(),
        };
        metadata.validate_fields()?;
        metadata.metadata_digest = metadata.expected_digest();
        Ok(metadata)
    }

    /// Verify the discriminator and domain-separated source identity digest.
    pub fn verify(&self) -> Result<()> {
        self.validate_fields()?;
        if self.metadata_digest != self.expected_digest() {
            return Err(loader_error(
                "ignored legacy source metadata digest does not match its identity",
            ));
        }
        Ok(())
    }

    /// Return the filename stem classified by the frozen Python reader.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Return the Python-source checksum bound by this record.
    pub fn checksum(&self) -> &str {
        &self.checksum
    }

    /// Return the exact raw Python-source SHA-256 bound by this record.
    pub fn source_sha256(&self) -> &str {
        &self.source_sha256
    }

    fn validate_fields(&self) -> Result<()> {
        if self.format != LEGACY_IGNORED_SOURCE_METADATA_V1 {
            return Err(loader_error(
                "ignored legacy source metadata uses an unsupported format discriminator",
            ));
        }
        if !is_migration_stem(&self.name) {
            return Err(loader_error(
                "ignored legacy source metadata has an invalid migration filename stem",
            ));
        }
        if !is_lower_hex(&self.checksum, 16) {
            return Err(loader_error(
                "ignored legacy source metadata carries a malformed Python-source checksum",
            ));
        }
        if !is_lower_hex(&self.source_sha256, 64) {
            return Err(loader_error(
                "ignored legacy source metadata carries a malformed raw-source digest",
            ));
        }
        if !self.metadata_digest.is_empty() && !is_lower_hex(&self.metadata_digest, 64) {
            return Err(loader_error(
                "ignored legacy source metadata carries a malformed metadata digest",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"typebridge.migration-adoption-ignored-source/v1\0");
        hash_field(&mut hasher, self.name.as_bytes());
        hash_field(&mut hasher, self.checksum.as_bytes());
        hash_field(&mut hasher, self.source_sha256.as_bytes());
        hex_digest(hasher.finalize())
    }
}

#[derive(Deserialize)]
struct LegacyMetadataFormatProbe {
    format: String,
}

enum LegacySourceMetadata {
    Migration(LegacyAdoptionMetadata),
    Sidecar(LegacySidecarAdoptionMetadata),
    Ignored(LegacyIgnoredSourceMetadata),
}

/// Checked graph evidence that is intentionally not an executable graph API.
pub struct LegacyAdoptionHistory {
    graph: MigrationGraph,
    directory: LegacyDirectoryAuthority,
    directory_capture: LegacyDirectoryCapture,
    snapshots: Option<(LegacyDirectoryAuthority, LegacyDirectoryCapture)>,
    snapshot_authorities: BTreeMap<(String, String), SnapshotAuthority>,
    root_file_digests: BTreeMap<OsString, String>,
    consumed_bytes: usize,
}

impl std::fmt::Debug for LegacyAdoptionHistory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LegacyAdoptionHistory")
            .field("graph", &self.graph)
            .field("directory", &self.directory.display_path)
            .field("snapshot_authorities", &self.snapshot_authorities)
            .finish_non_exhaustive()
    }
}

impl LegacyAdoptionHistory {
    /// Borrow the frozen graph for continuity/frontier validation only.
    pub const fn graph(&self) -> &MigrationGraph {
        &self.graph
    }

    /// Return the checked legacy directory that owns this evidence.
    pub fn directory(&self) -> &Path {
        &self.directory.display_path
    }

    /// Re-run the complete bounded authority verification at a use boundary.
    ///
    /// Callers that await external state or are about to publish/apply a
    /// durable checkpoint must invoke this after the await and immediately
    /// before consuming [`Self::graph`]. This re-enumerates filtered history
    /// membership and re-verifies the authoritative snapshot bytes and hashes,
    /// including in-place child-file edits that directory metadata cannot bind.
    pub fn require_unchanged(&self) -> Result<()> {
        reconstruct_legacy_head(self).map(|_| ())
    }

    /// Revalidate and require the exact previously reconstructed head bytes.
    pub fn require_unchanged_head(&self, expected: &VerifiedLegacyHead) -> Result<()> {
        let observed = reconstruct_legacy_head(self)?;
        if &observed != expected {
            return Err(loader_error(
                "legacy reconstructed head changed after its checked capture",
            ));
        }
        Ok(())
    }

    fn require_retained_membership(&self) -> Result<()> {
        self.directory
            .require_recognized_root_capture(&self.directory_capture)?;
        let mut aggregate = 0usize;
        for (name, expected_digest) in &self.root_file_digests {
            let entry = self
                .directory_capture
                .entries
                .iter()
                .find(|entry| &entry.name == name)
                .ok_or_else(|| loader_error("retained root digest entry disappeared"))?;
            let bytes = read_bounded(
                &self.directory,
                entry,
                MAX_LEGACY_ARTIFACT_BYTES,
                &mut aggregate,
            )?;
            if hex_digest(Sha256::digest(&bytes)) != *expected_digest {
                return Err(loader_error(
                    "recognized legacy root file body changed after checked capture",
                ));
            }
        }
        if let Some((snapshots, capture)) = &self.snapshots {
            snapshots.require_snapshot_version_capture(capture)?;
        }
        Ok(())
    }

    fn heads(&self) -> Result<Vec<&MigrationSpec>> {
        let depended_on = self
            .graph
            .migrations
            .iter()
            .flat_map(|migration| {
                migration.dependencies.iter().map(|dependency| {
                    (
                        dependency.app_label.clone(),
                        dependency.migration_name.clone(),
                    )
                })
            })
            .collect::<BTreeSet<_>>();
        let mut heads = self
            .graph
            .migrations
            .iter()
            .filter(|migration| {
                !depended_on.contains(&(migration.app_label.clone(), migration.name.clone()))
            })
            .collect::<Vec<_>>();
        if heads.is_empty() {
            return Err(loader_error(
                "legacy adoption history has no graph head after validation",
            ));
        }
        heads.sort_by(|left, right| {
            (&left.app_label, &left.name).cmp(&(&right.app_label, &right.name))
        });
        Ok(heads)
    }
}

/// Independently reconstructed legacy head loaded from a verified snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedLegacyHead {
    source_migration: String,
    schema_typeql: String,
}

impl VerifiedLegacyHead {
    /// Return the migration whose immutable snapshot reconstructed this head.
    pub fn source_migration(&self) -> &str {
        &self.source_migration
    }

    /// Return the exact verified snapshot TypeQL bytes as UTF-8 text.
    pub fn schema_typeql(&self) -> &str {
        &self.schema_typeql
    }
}

fn parse_legacy_source_metadata(bytes: &[u8], path: &Path) -> Result<LegacySourceMetadata> {
    let probe: LegacyMetadataFormatProbe = serde_json::from_slice(bytes).map_err(|error| {
        loader_error(format!(
            "failed to inspect adoption metadata {}: {error}",
            path.display()
        ))
    })?;
    match probe.format.as_str() {
        LEGACY_ADOPTION_METADATA_V2 => serde_json::from_slice(bytes)
            .map(LegacySourceMetadata::Migration)
            .map_err(|error| {
                loader_error(format!(
                    "failed to parse adoption metadata {}: {error}",
                    path.display()
                ))
            }),
        LEGACY_SIDECAR_ADOPTION_METADATA_V1 => serde_json::from_slice(bytes)
            .map(LegacySourceMetadata::Sidecar)
            .map_err(|error| {
                loader_error(format!(
                    "failed to parse sidecar adoption metadata {}: {error}",
                    path.display()
                ))
            }),
        LEGACY_IGNORED_SOURCE_METADATA_V1 => serde_json::from_slice(bytes)
            .map(LegacySourceMetadata::Ignored)
            .map_err(|error| {
                loader_error(format!(
                    "failed to parse ignored-source adoption metadata {}: {error}",
                    path.display()
                ))
            }),
        _ => Err(loader_error(
            "legacy adoption metadata uses an unsupported format discriminator",
        )),
    }
}

/// Load identity/dependency/checksum evidence for an already-applied history.
///
/// Every Python file requires a non-executable `NNNN_name.adoption.json`
/// archive produced by the frozen trusted reader. A V1 migration graph member
/// carries full graph and schema-authority metadata; a released sidecar-backed
/// member carries a distinct record binding both exact files and its normalized
/// effective graph semantics; a source with no public `Migration` subclass
/// carries a smaller ignored-source record and remains excluded from the graph.
pub fn load_adoption_history(directory: &Path) -> Result<LegacyAdoptionHistory> {
    let directory = LegacyDirectoryAuthority::open_root(directory)?;
    load_adoption_history_in(directory)
}

fn load_adoption_history_in(directory: LegacyDirectoryAuthority) -> Result<LegacyAdoptionHistory> {
    let fallback_app_label = directory
        .display_path
        .file_name()
        .and_then(OsStr::to_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| loader_error("legacy migration directory has no UTF-8 app-label name"))?
        .to_owned();
    let mut directory_capture = directory.capture()?;
    let mut python = BTreeMap::<String, LegacyDirectoryEntry>::new();
    let mut sidecars = BTreeMap::<String, LegacyDirectoryEntry>::new();
    let mut archives = BTreeMap::<String, LegacyDirectoryEntry>::new();

    if directory_capture
        .entries
        .iter()
        .any(|entry| entry.name == OsStr::new(CONVERSION_JOURNAL))
    {
        return Err(loader_error(
            "legacy adoption conversion journal is present; resume the trusted converter",
        ));
    }

    for entry in &directory_capture.entries {
        let Some(name) = entry.name.to_str() else {
            if looks_migration_shaped_bytes(&entry.name) {
                return Err(loader_error(
                    "recognized legacy migration filename is not valid UTF-8",
                ));
            }
            continue;
        };
        if let Some(stem) = name.strip_suffix(ADOPTION_SUFFIX) {
            if is_migration_stem(stem) {
                require_regular(entry)?;
                archives.insert(stem.to_owned(), entry.clone());
            }
            continue;
        }
        let Some((stem, extension)) = name.rsplit_once('.') else {
            continue;
        };
        if !is_migration_stem(stem) {
            continue;
        }
        match extension {
            "py" => {
                require_regular(entry)?;
                python.insert(stem.to_owned(), entry.clone());
            }
            "json" => {
                sidecars.insert(stem.to_owned(), entry.clone());
            }
            _ => {}
        }
    }

    for stem in archives.keys() {
        if !python.contains_key(stem) {
            return Err(loader_error(format!(
                "legacy metadata {stem} has no Python source to verify"
            )));
        }
    }

    let mut retained_names = python
        .values()
        .chain(sidecars.values())
        .chain(archives.values())
        .map(|entry| entry.name.clone())
        .collect::<BTreeSet<_>>();
    retained_names.insert(OsString::from("snapshots"));
    directory_capture
        .entries
        .retain(|entry| retained_names.contains(&entry.name));

    let mut aggregate = 0usize;
    let mut migrations = Vec::with_capacity(python.len());
    let mut schema_bindings = BTreeMap::new();
    let mut root_file_digests = BTreeMap::<OsString, String>::new();
    for (stem, py_entry) in python {
        let py_bytes = read_bounded(
            &directory,
            &py_entry,
            MAX_LEGACY_ARTIFACT_BYTES,
            &mut aggregate,
        )?;
        let source_sha256 = hex_digest(Sha256::digest(&py_bytes));
        root_file_digests.insert(py_entry.name.clone(), source_sha256.clone());
        let source_metadata = if let Some(archive_entry) = archives.get(&stem) {
            let bytes = read_bounded(
                &directory,
                archive_entry,
                MAX_LEGACY_ARTIFACT_BYTES,
                &mut aggregate,
            )?;
            root_file_digests.insert(
                archive_entry.name.clone(),
                hex_digest(Sha256::digest(&bytes)),
            );
            parse_legacy_source_metadata(&bytes, &directory.display_path.join(&archive_entry.name))?
        } else {
            return Err(loader_error(format!(
                "migration {stem} has no checksum-bound adoption metadata; run `python -m type_bridge.migration.sidecar {}` through the trusted Python environment",
                directory.display_path.display()
            )));
        };

        match source_metadata {
            LegacySourceMetadata::Migration(archive) => {
                archive.verify()?;
                if sidecars.contains_key(&stem) {
                    return Err(loader_error(format!(
                        "source-authoritative adoption metadata for {stem} conflicts with a retained JSON sidecar"
                    )));
                }
                if archive.source_sha256() != source_sha256 {
                    return Err(loader_error(format!(
                        "adoption metadata drift detected for {stem}: raw Python source digest differs"
                    )));
                }
                if archive.name != stem {
                    return Err(loader_error(format!(
                        "legacy metadata identity {} does not match filename stem {stem}",
                        archive.name
                    )));
                }
                if archive.app_label != fallback_app_label {
                    return Err(loader_error(format!(
                        "legacy metadata owner {} does not match migration directory app label {fallback_app_label}",
                        archive.app_label
                    )));
                }
                schema_bindings.insert(
                    (archive.app_label.clone(), archive.name.clone()),
                    archive.schema_binding(),
                );
                migrations.push(archive.into_spec());
            }
            LegacySourceMetadata::Sidecar(archive) => {
                archive.verify()?;
                if archive.source_sha256 != source_sha256 {
                    return Err(loader_error(format!(
                        "sidecar adoption metadata drift detected for {stem}: raw Python source digest differs"
                    )));
                }
                if archive.source_name != stem {
                    return Err(loader_error(format!(
                        "sidecar adoption source identity {} does not match filename stem {stem}",
                        archive.source_name
                    )));
                }
                let sidecar_entry = sidecars.get(&stem).ok_or_else(|| {
                    loader_error(format!(
                        "sidecar-authoritative migration {stem} has no retained JSON sidecar"
                    ))
                })?;
                let sidecar_bytes = read_bounded(
                    &directory,
                    sidecar_entry,
                    MAX_LEGACY_ARTIFACT_BYTES,
                    &mut aggregate,
                )?;
                let sidecar_sha256 = hex_digest(Sha256::digest(&sidecar_bytes));
                if archive.sidecar_sha256 != sidecar_sha256 {
                    return Err(loader_error(format!(
                        "sidecar adoption metadata drift detected for {stem}: exact JSON digest differs"
                    )));
                }
                root_file_digests.insert(sidecar_entry.name.clone(), sidecar_sha256);
                let sidecar: MigrationSpec =
                    serde_json::from_slice(&sidecar_bytes).map_err(|error| {
                        loader_error(format!(
                            "failed to parse retained sidecar {}: {error}",
                            directory.display_path.join(&sidecar_entry.name).display()
                        ))
                    })?;
                let key = (archive.app_label.clone(), archive.name.clone());
                schema_bindings.insert(key, archive.schema_binding());
                migrations.push(archive.into_spec(sidecar, &fallback_app_label)?);
            }
            LegacySourceMetadata::Ignored(ignored) => {
                ignored.verify()?;
                if sidecars.contains_key(&stem) {
                    return Err(loader_error(format!(
                        "ignored-source adoption metadata for {stem} conflicts with a retained JSON sidecar"
                    )));
                }
                if ignored.source_sha256() != source_sha256 {
                    return Err(loader_error(format!(
                        "ignored-source adoption metadata drift detected for {stem}: raw Python source digest differs"
                    )));
                }
                if ignored.name() != stem {
                    return Err(loader_error(format!(
                        "ignored-source metadata identity {} does not match filename stem {stem}",
                        ignored.name()
                    )));
                }
            }
        }
    }
    for entry in sidecars.values() {
        if entry.is_file() && !root_file_digests.contains_key(&entry.name) {
            let bytes = read_bounded(&directory, entry, MAX_LEGACY_ARTIFACT_BYTES, &mut aggregate)?;
            root_file_digests.insert(entry.name.clone(), hex_digest(Sha256::digest(&bytes)));
        }
    }

    let graph = MigrationGraph { migrations };
    let errors = validate_graph(&graph, &[]);
    if !errors.is_empty() {
        return Err(MigrationError::Planning { errors });
    }
    let snapshot_authorities = resolve_schema_bindings(&graph, &schema_bindings)?;
    let snapshots = if let Some(entry) = directory_capture
        .entries
        .iter()
        .find(|entry| entry.name == OsStr::new("snapshots"))
    {
        let retained = directory.open_child(entry)?;
        let mut capture = retained.capture()?;
        capture
            .entries
            .retain(|candidate| candidate.name.to_str().is_some_and(is_snapshot_version));
        Some((retained, capture))
    } else {
        None
    };
    if let Some((snapshots, capture)) = &snapshots {
        snapshots.require_snapshot_version_capture(capture)?;
    }
    directory.require_recognized_root_capture(&directory_capture)?;
    Ok(LegacyAdoptionHistory {
        graph,
        directory,
        directory_capture,
        snapshots,
        snapshot_authorities,
        root_file_digests,
        consumed_bytes: aggregate,
    })
}

fn resolve_schema_bindings(
    graph: &MigrationGraph,
    bindings: &BTreeMap<(String, String), SchemaBinding>,
) -> Result<BTreeMap<(String, String), SnapshotAuthority>> {
    let mut resolved = BTreeMap::new();
    let mut pending = BTreeMap::<
        (String, String),
        (LegacySchemaEffect, SnapshotAuthority, Vec<(String, String)>),
    >::new();
    let mut dependents = BTreeMap::<(String, String), Vec<(String, String)>>::new();

    for migration in &graph.migrations {
        let key = (migration.app_label.clone(), migration.name.clone());
        let binding = bindings.get(&key).ok_or_else(|| {
            loader_error(format!(
                "legacy migration {}.{} has no schema-effect binding",
                migration.app_label, migration.name
            ))
        })?;
        match binding.effect {
            LegacySchemaEffect::Snapshot => {
                resolved.insert(key, binding.authority.clone());
            }
            LegacySchemaEffect::UnchangedRunPython
            | LegacySchemaEffect::UnchangedNoop
            | LegacySchemaEffect::UnchangedCopyAttribute => {
                let dependencies = migration
                    .dependencies
                    .iter()
                    .map(|dependency| {
                        (
                            dependency.app_label.clone(),
                            dependency.migration_name.clone(),
                        )
                    })
                    .collect::<Vec<_>>();
                if dependencies.is_empty() {
                    return Err(loader_error(format!(
                        "unchanged migration {}.{} has no parent authority",
                        migration.app_label, migration.name
                    )));
                }
                for dependency in &dependencies {
                    dependents
                        .entry(dependency.clone())
                        .or_default()
                        .push(key.clone());
                }
                pending.insert(
                    key,
                    (binding.effect, binding.authority.clone(), dependencies),
                );
            }
        }
    }

    let mut unresolved_parent_counts = pending
        .iter()
        .map(|(key, (_, _, dependencies))| {
            (
                key.clone(),
                dependencies
                    .iter()
                    .filter(|dependency| !resolved.contains_key(*dependency))
                    .count(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut ready = unresolved_parent_counts
        .iter()
        .filter_map(|(key, count)| (*count == 0).then_some(key.clone()))
        .collect::<BTreeSet<_>>();

    while let Some(key) = ready.pop_first() {
        let (_, bound_authority, dependencies) = pending
            .get(&key)
            .ok_or_else(|| loader_error("legacy schema binding work item disappeared"))?;
        let parents = dependencies
            .iter()
            .map(|dependency| {
                resolved.get(dependency).ok_or_else(|| {
                    loader_error("legacy schema binding parent was not resolved deterministically")
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let (parent, remaining_parents) = parents.split_first().ok_or_else(|| {
            loader_error(format!(
                "unchanged migration {}.{} has no parent authority",
                key.0, key.1
            ))
        })?;
        let parent = *parent;
        if remaining_parents
            .iter()
            .any(|authority| authority.schema_hash != parent.schema_hash)
        {
            return Err(loader_error(format!(
                "unchanged migration {}.{} merges divergent snapshot authorities with different schema hashes",
                key.0, key.1
            )));
        }
        let canonical_parent = parents
            .iter()
            .copied()
            .min_by(|left, right| {
                (
                    &left.source.app_label,
                    &left.source.migration_name,
                    &left.schema_hash,
                )
                    .cmp(&(
                        &right.source.app_label,
                        &right.source.migration_name,
                        &right.schema_hash,
                    ))
            })
            .ok_or_else(|| loader_error("legacy schema binding parent set disappeared"))?;
        if bound_authority != canonical_parent {
            return Err(loader_error(format!(
                "unchanged migration {}.{} does not bind its deterministic parent snapshot authority",
                key.0, key.1
            )));
        }
        resolved.insert(key.clone(), bound_authority.clone());
        if let Some(children) = dependents.get(&key) {
            for child in children {
                let remaining = unresolved_parent_counts.get_mut(child).ok_or_else(|| {
                    loader_error("legacy schema binding dependency count disappeared")
                })?;
                *remaining = remaining.saturating_sub(1);
                if *remaining == 0 {
                    ready.insert(child.clone());
                }
            }
        }
    }
    if resolved.len() != graph.migrations.len() {
        return Err(loader_error(
            "legacy schema-effect bindings could not be resolved over the validated graph",
        ));
    }
    require_unambiguous_snapshot_owners(&resolved)?;
    Ok(resolved)
}

fn require_unambiguous_snapshot_owners(
    authorities: &BTreeMap<(String, String), SnapshotAuthority>,
) -> Result<()> {
    let mut owners_by_source = BTreeMap::<&str, BTreeSet<&str>>::new();
    for authority in authorities.values() {
        owners_by_source
            .entry(authority.source.migration_name.as_str())
            .or_default()
            .insert(authority.source.app_label.as_str());
    }
    if let Some((source, owners)) = owners_by_source
        .into_iter()
        .find(|(_, owners)| owners.len() > 1)
    {
        return Err(loader_error(format!(
            "legacy snapshot source {source} is ambiguous across app labels: {}",
            owners.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    Ok(())
}

struct SnapshotManifest {
    version: String,
    source_migration: String,
    schema_hash: String,
    file_hashes: BTreeMap<String, String>,
}

fn parse_snapshot_manifest(bytes: &[u8], path: &Path) -> Result<SnapshotManifest> {
    // Frozen Python V1 used `json.loads`: unknown members are ignored by the
    // snapshot reader, duplicate object keys are last-value-wins, integers are
    // arbitrary precision, and the non-standard NaN/Infinity constants are
    // admitted. Replace only Python's three bare value constants outside
    // strings with same-width JSON numbers; this retains offsets without
    // accepting JSON5 syntax. Values are first retained as raw spans: serde's
    // raw-value scanner skips nested ignored members iteratively, so released
    // manifests deeper than serde_json's recursive Value ceiling remain valid
    // without exposing this bounded reader to recursive allocation or drop.
    // BTreeMap insertion also preserves Python's last-key-wins behavior before
    // the four authoritative members are type-checked. Python also retains
    // escaped lone surrogates in object keys, while Rust strings cannot
    // represent them. Normalize only lone surrogate escapes in root keys to a
    // non-ASCII scalar: those keys cannot name an authoritative ASCII member,
    // so normalization collisions stay among ignored metadata keys and their
    // values remain ignored as raw spans.
    let mut python_compatible = sanitize_python_json_constants(bytes);
    sanitize_python_json_root_key_surrogates(&mut python_compatible);
    let mut fields: BTreeMap<String, Box<serde_json::value::RawValue>> =
        serde_json::from_slice(&python_compatible).map_err(|error| {
            loader_error(format!(
                "failed to parse snapshot manifest {}: {error}",
                path.display()
            ))
        })?;
    let version = parse_snapshot_string_field(&mut fields, "version", path)?;
    let source_migration = parse_snapshot_string_field(&mut fields, "source_migration", path)?;
    let schema_hash = parse_snapshot_string_field(&mut fields, "schema_hash", path)?;
    let file_hashes_raw = fields.remove("file_hashes").ok_or_else(|| {
        loader_error(format!(
            "failed to validate snapshot manifest {}: missing field `file_hashes`",
            path.display()
        ))
    })?;
    let raw_file_hashes: BTreeMap<String, Box<serde_json::value::RawValue>> =
        serde_json::from_str(file_hashes_raw.get()).map_err(|error| {
            loader_error(format!(
                "failed to validate snapshot manifest {}: file_hashes: {error}",
                path.display()
            ))
        })?;
    let mut file_hashes = BTreeMap::new();
    for (name, raw_hash) in raw_file_hashes {
        let hash = serde_json::from_str(raw_hash.get()).map_err(|error| {
            loader_error(format!(
                "failed to validate snapshot manifest {}: file_hashes[{name:?}]: {error}",
                path.display()
            ))
        })?;
        file_hashes.insert(name, hash);
    }
    Ok(SnapshotManifest {
        version,
        source_migration,
        schema_hash,
        file_hashes,
    })
}

fn parse_snapshot_string_field(
    fields: &mut BTreeMap<String, Box<serde_json::value::RawValue>>,
    name: &'static str,
    path: &Path,
) -> Result<String> {
    let raw = fields.remove(name).ok_or_else(|| {
        loader_error(format!(
            "failed to validate snapshot manifest {}: missing field `{name}`",
            path.display()
        ))
    })?;
    serde_json::from_str(raw.get()).map_err(|error| {
        loader_error(format!(
            "failed to validate snapshot manifest {}: {name}: {error}",
            path.display()
        ))
    })
}

fn sanitize_python_json_constants(bytes: &[u8]) -> Vec<u8> {
    const CONSTANTS: [(&[u8], &[u8]); 3] = [
        (b"-Infinity", b"-0.000000"),
        (b"Infinity", b"0.000000"),
        (b"NaN", b"0.0"),
    ];
    let mut sanitized = bytes.to_vec();
    let mut offset = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    while offset < bytes.len() {
        let byte = bytes[offset];
        if in_string {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            offset += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            offset += 1;
            continue;
        }
        let mut replaced = false;
        for (constant, replacement) in CONSTANTS {
            let end = offset.saturating_add(constant.len());
            if bytes.get(offset..end) == Some(constant)
                && is_json_value_boundary_before(bytes, offset)
                && is_json_value_boundary_after(bytes, end)
            {
                sanitized[offset..end].copy_from_slice(replacement);
                offset = end;
                replaced = true;
                break;
            }
        }
        if !replaced {
            offset += 1;
        }
    }
    sanitized
}

fn sanitize_python_json_root_key_surrogates(bytes: &mut [u8]) {
    let mut offset = 0usize;
    let mut object_depth = 0usize;
    let mut array_depth = 0usize;
    while offset < bytes.len() {
        match bytes[offset] {
            b'{' => {
                object_depth = object_depth.saturating_add(1);
                offset += 1;
            }
            b'}' => {
                object_depth = object_depth.saturating_sub(1);
                offset += 1;
            }
            b'[' => {
                array_depth = array_depth.saturating_add(1);
                offset += 1;
            }
            b']' => {
                array_depth = array_depth.saturating_sub(1);
                offset += 1;
            }
            b'"' => {
                let Some(end) = scan_json_string_end(bytes, offset) else {
                    return;
                };
                let mut following = end + 1;
                while bytes
                    .get(following)
                    .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
                {
                    following += 1;
                }
                if object_depth == 1 && array_depth == 0 && bytes.get(following) == Some(&b':') {
                    replace_lone_surrogate_escapes(bytes, offset + 1, end);
                }
                offset = end + 1;
            }
            _ => offset += 1,
        }
    }
}

fn scan_json_string_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut offset = start + 1;
    while offset < bytes.len() {
        match bytes[offset] {
            b'"' => return Some(offset),
            b'\\' => offset = offset.saturating_add(2),
            _ => offset += 1,
        }
    }
    None
}

fn parse_json_hex_code_unit(bytes: &[u8]) -> Option<u16> {
    if bytes.len() != 4 {
        return None;
    }
    bytes.iter().try_fold(0u16, |value, byte| {
        let digit = match byte {
            b'0'..=b'9' => u16::from(byte - b'0'),
            b'a'..=b'f' => u16::from(byte - b'a') + 10,
            b'A'..=b'F' => u16::from(byte - b'A') + 10,
            _ => return None,
        };
        Some((value << 4) | digit)
    })
}

fn replace_lone_surrogate_escapes(bytes: &mut [u8], start: usize, end: usize) {
    let mut offset = start;
    while offset < end {
        if bytes[offset] != b'\\' {
            offset += 1;
            continue;
        }
        let Some(code_unit) = bytes
            .get(offset + 2..offset + 6)
            .filter(|_| bytes.get(offset + 1) == Some(&b'u'))
            .and_then(parse_json_hex_code_unit)
        else {
            offset = offset.saturating_add(2);
            continue;
        };
        if (0xd800..=0xdbff).contains(&code_unit) {
            let paired_low = bytes
                .get(offset + 8..offset + 12)
                .filter(|_| {
                    bytes.get(offset + 6) == Some(&b'\\') && bytes.get(offset + 7) == Some(&b'u')
                })
                .and_then(parse_json_hex_code_unit)
                .is_some_and(|low| (0xdc00..=0xdfff).contains(&low));
            if paired_low {
                offset += 12;
                continue;
            }
        }
        if (0xd800..=0xdfff).contains(&code_unit) {
            bytes[offset + 2..offset + 6].copy_from_slice(b"fffd");
        }
        offset += 6;
    }
}

fn is_json_value_boundary_before(bytes: &[u8], offset: usize) -> bool {
    offset == 0
        || matches!(
            bytes[offset - 1],
            b'[' | b'{' | b',' | b':' | b' ' | b'\t' | b'\r' | b'\n'
        )
}

fn is_json_value_boundary_after(bytes: &[u8], offset: usize) -> bool {
    offset == bytes.len()
        || matches!(
            bytes[offset],
            b']' | b'}' | b',' | b' ' | b'\t' | b'\r' | b'\n'
        )
}

/// Reconstruct the independently verified schema at a checked legacy frontier.
///
/// A released legacy graph may have more than one applied head. Every head is
/// resolved to and reconstructed from its own checksum-bound snapshot
/// authority. Adoption proceeds only when those independently verified
/// authorities have the same schema hash and exact schema bytes; the complete
/// head set remains in [`LegacyAdoptionHistory::graph`] for frontier import.
pub fn reconstruct_legacy_head(history: &LegacyAdoptionHistory) -> Result<VerifiedLegacyHead> {
    require_unambiguous_snapshot_owners(&history.snapshot_authorities)?;
    let heads = history.heads()?;
    history.require_retained_membership()?;
    let (snapshots, snapshot_capture) = history.snapshots.as_ref().ok_or_else(|| {
        loader_error("legacy snapshots authority was absent when adoption history was retained")
    })?;
    let needed_authorities = history
        .snapshot_authorities
        .values()
        .map(|authority| {
            (
                authority.source.app_label.clone(),
                authority.source.migration_name.clone(),
                authority.schema_hash.clone(),
            )
        })
        .collect::<BTreeSet<_>>();
    let mut expected_hashes_by_source = BTreeMap::<String, BTreeSet<String>>::new();
    for (_, source, schema_hash) in &needed_authorities {
        expected_hashes_by_source
            .entry(source.clone())
            .or_default()
            .insert(schema_hash.clone());
    }
    let mut available = BTreeMap::<(String, String), VerifiedLegacyHead>::new();
    let mut verified_snapshot_captures = Vec::new();
    let mut scanned_snapshot_manifests = Vec::new();
    let mut aggregate = history.consumed_bytes;
    let mut nested_entries = snapshot_capture.entries.len();
    for entry in &snapshot_capture.entries {
        let Some(version) = entry.name.to_str() else {
            continue;
        };
        if !is_snapshot_version(version) {
            continue;
        }
        if nested_entries == MAX_LEGACY_DIRECTORY_ENTRIES {
            return Err(loader_error(
                "legacy snapshot tree exceeds the entry ceiling",
            ));
        }
        nested_entries += 1;
        let snapshot = snapshots.open_child(entry)?;
        let snapshot_before = snapshot.revision()?;
        let manifest_entry = snapshot
            .inspect_relative(Path::new(SNAPSHOT_MANIFEST), None)?
            .ok_or_else(|| loader_error("legacy snapshot manifest is absent"))?;
        require_regular(&manifest_entry)?;
        let bytes = read_bounded(
            &snapshot,
            &manifest_entry,
            MAX_LEGACY_ARTIFACT_BYTES,
            &mut aggregate,
        )?;
        scanned_snapshot_manifests.push((
            PathBuf::from(version).join(SNAPSHOT_MANIFEST),
            manifest_entry.clone(),
            hex_digest(Sha256::digest(&bytes)),
        ));
        let manifest =
            parse_snapshot_manifest(&bytes, &snapshot.display_path.join(SNAPSHOT_MANIFEST))?;
        if manifest.version != version {
            return Err(loader_error(
                "legacy snapshot records a different version identity",
            ));
        }
        snapshot.require_revision(&snapshot_before)?;
        let candidate_key = (
            manifest.source_migration.clone(),
            manifest.schema_hash.clone(),
        );
        let Some(expected_hashes) = expected_hashes_by_source.get(&manifest.source_migration)
        else {
            continue;
        };
        if !expected_hashes.contains(&manifest.schema_hash) {
            return Err(loader_error(format!(
                "legacy snapshot source {} has a non-equivalent schema hash claim",
                manifest.source_migration
            )));
        }

        let remaining_entries = MAX_LEGACY_DIRECTORY_ENTRIES.saturating_sub(nested_entries);
        let capture = snapshot.capture_with_limit(remaining_entries)?;
        nested_entries = nested_entries.saturating_add(capture.entries.len());
        let captured_manifest_entry = capture
            .entries
            .iter()
            .find(|candidate| candidate.name == OsStr::new(SNAPSHOT_MANIFEST))
            .ok_or_else(|| loader_error("legacy snapshot manifest is absent"))?;
        require_regular(captured_manifest_entry)?;
        let mut revalidation_bytes = 0usize;
        let captured_bytes = read_bounded(
            &snapshot,
            captured_manifest_entry,
            MAX_LEGACY_ARTIFACT_BYTES,
            &mut revalidation_bytes,
        )?;
        if captured_bytes != bytes {
            return Err(loader_error(
                "legacy snapshot manifest changed before authoritative verification",
            ));
        }
        let captured_manifest = parse_snapshot_manifest(
            &captured_bytes,
            &snapshot.display_path.join(SNAPSHOT_MANIFEST),
        )?;
        let verified = verify_snapshot(&snapshot, &capture, &captured_manifest, &mut aggregate)?;
        match available.get(&candidate_key) {
            Some(existing) if existing.schema_typeql != verified.schema_typeql => {
                return Err(loader_error(format!(
                    "legacy snapshot source {} has non-equivalent candidates for one schema hash",
                    captured_manifest.source_migration
                )));
            }
            Some(_) => {}
            None => {
                available.insert(candidate_key, verified);
            }
        }
        snapshot.require_capture(&capture)?;
        verified_snapshot_captures.push((snapshot, capture));
    }
    #[cfg(test)]
    TEST_AFTER_SNAPSHOT_SCAN.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
    require_scanned_snapshot_manifests(snapshots, &scanned_snapshot_manifests)?;
    history.require_retained_membership()?;

    for (_, source, schema_hash) in &needed_authorities {
        if !available.contains_key(&(source.clone(), schema_hash.clone())) {
            return Err(loader_error(format!(
                "legacy snapshot authority {source} has no immutable snapshot with its trusted schema hash"
            )));
        }
    }

    let mut verified_by_authority = BTreeMap::<(String, String, String), VerifiedLegacyHead>::new();
    let mut converged: Option<(String, VerifiedLegacyHead)> = None;
    for head in heads {
        let authority = history
            .snapshot_authorities
            .get(&(head.app_label.clone(), head.name.clone()))
            .ok_or_else(|| {
                loader_error(format!(
                    "legacy graph head {} has no resolved snapshot authority",
                    head.name
                ))
            })?;
        let authority_key = (
            authority.source.app_label.clone(),
            authority.source.migration_name.clone(),
            authority.schema_hash.clone(),
        );
        let verified = if let Some(verified) = verified_by_authority.get(&authority_key) {
            verified.clone()
        } else {
            let verified = available
                .get(&(
                    authority.source.migration_name.clone(),
                    authority.schema_hash.clone(),
                ))
                .ok_or_else(|| {
                    loader_error(format!(
                        "legacy graph head {} resolves to snapshot source {} but no immutable snapshot with its trusted schema hash exists",
                        head.name, authority.source.migration_name
                    ))
                })?
                .clone();
            verified_by_authority.insert(authority_key, verified.clone());
            verified
        };

        match &converged {
            Some((schema_hash, existing))
                if schema_hash != &authority.schema_hash
                    || existing.schema_typeql != verified.schema_typeql =>
            {
                return Err(loader_error(
                    "legacy graph heads resolve to divergent authoritative snapshots",
                ));
            }
            Some(_) => {}
            None => converged = Some((authority.schema_hash.clone(), verified)),
        }
    }

    for (snapshot, capture) in &verified_snapshot_captures {
        snapshot.require_capture(capture)?;
    }
    require_scanned_snapshot_manifests(snapshots, &scanned_snapshot_manifests)?;
    history.require_retained_membership()?;
    converged
        .map(|(_, verified)| verified)
        .ok_or_else(|| loader_error("legacy adoption history has no verified graph head"))
}

fn require_scanned_snapshot_manifests(
    snapshots: &LegacyDirectoryAuthority,
    manifests: &[(PathBuf, LegacyDirectoryEntry, String)],
) -> Result<()> {
    let mut aggregate = 0usize;
    for (relative, entry, expected_digest) in manifests {
        let bytes = snapshots
            .read_relative_bounded(relative, MAX_LEGACY_ARTIFACT_BYTES, Some(entry))
            .map_err(|_| {
                loader_error("scanned legacy snapshot manifest changed after bounded scan")
            })?;
        aggregate = aggregate.saturating_add(bytes.len());
        if aggregate > MAX_LEGACY_HISTORY_BYTES
            || hex_digest(Sha256::digest(&bytes)) != *expected_digest
        {
            return Err(loader_error(
                "scanned legacy snapshot manifest changed after bounded scan",
            ));
        }
    }
    Ok(())
}

fn verify_snapshot(
    directory: &LegacyDirectoryAuthority,
    capture: &LegacyDirectoryCapture,
    manifest: &SnapshotManifest,
    aggregate: &mut usize,
) -> Result<VerifiedLegacyHead> {
    if !is_lower_hex(&manifest.schema_hash, 64) {
        return Err(loader_error("legacy snapshot schema hash is malformed"));
    }
    if manifest.file_hashes.len() > MAX_LEGACY_DIRECTORY_ENTRIES {
        return Err(loader_error(
            "legacy snapshot file manifest exceeds the entry ceiling",
        ));
    }
    let expected_files = manifest
        .file_hashes
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    if !expected_files.contains(SNAPSHOT_SCHEMA) {
        return Err(loader_error(
            "legacy snapshot manifest does not bind schema.tql",
        ));
    }
    for (name, hash) in &manifest.file_hashes {
        if Path::new(name).components().count() != 1 || !is_lower_hex(hash, 64) {
            return Err(loader_error(
                "legacy snapshot manifest contains an invalid file identity",
            ));
        }
    }

    let mut observed = BTreeSet::new();
    let mut schema = None;
    for entry in &capture.entries {
        if entry.name == OsStr::new(SNAPSHOT_MANIFEST) {
            require_regular(entry)?;
            continue;
        }
        // Released V1 validates only files named by snapshot.json. Ambient
        // direct children (most commonly __pycache__) are not semantic
        // snapshot authority: retain the directory identity, but never open,
        // follow, hash, or materialize those unbound entries.
        let Some(name) = entry.name.to_str() else {
            continue;
        };
        if !expected_files.contains(name) {
            continue;
        }
        require_regular(entry)?;
        let bytes = read_bounded(directory, entry, MAX_LEGACY_ARTIFACT_BYTES, aggregate)?;
        let actual = hex_digest(Sha256::digest(&bytes));
        if manifest.file_hashes.get(name) != Some(&actual) {
            return Err(loader_error(format!(
                "legacy snapshot file hash mismatch for {name}"
            )));
        }
        observed.insert(name.to_owned());
        if name == SNAPSHOT_SCHEMA {
            schema = Some(
                String::from_utf8(bytes)
                    .map_err(|_| loader_error("legacy snapshot schema.tql is not valid UTF-8"))?,
            );
        }
    }
    if observed != expected_files {
        return Err(loader_error(
            "legacy snapshot is missing a file bound by snapshot.json",
        ));
    }
    let schema_typeql =
        schema.ok_or_else(|| loader_error("legacy snapshot schema.tql is absent"))?;
    if hex_digest(Sha256::digest(schema_typeql.as_bytes())) != manifest.schema_hash {
        return Err(loader_error(
            "legacy snapshot schema hash disagrees with schema.tql",
        ));
    }
    directory.require_capture(capture)?;
    Ok(VerifiedLegacyHead {
        source_migration: manifest.source_migration.clone(),
        schema_typeql,
    })
}

fn read_bounded(
    directory: &LegacyDirectoryAuthority,
    entry: &LegacyDirectoryEntry,
    limit: usize,
    aggregate: &mut usize,
) -> Result<Vec<u8>> {
    let mut file = crate::loader::open_regular_readonly_nofollow(&directory.directory, &entry.name)
        .map_err(|_| loader_error("legacy artifact cannot be opened through retained authority"))?;
    let before = LegacyMetadataRevision::capture(
        &file
            .metadata()
            .map_err(|_| loader_error("legacy artifact metadata cannot be inspected"))?,
    )?;
    if before != entry.revision {
        return Err(loader_error(
            "legacy artifact changed after bounded directory enumeration",
        ));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| loader_error("legacy artifact failed during bounded read"))?;
    let after = LegacyMetadataRevision::capture(
        &file
            .metadata()
            .map_err(|_| loader_error("legacy artifact metadata cannot be re-inspected"))?,
    )?;
    if before != after {
        return Err(loader_error("legacy artifact changed during bounded read"));
    }
    if bytes.len() > limit {
        return Err(loader_error("legacy artifact exceeds the byte ceiling"));
    }
    *aggregate = aggregate.saturating_add(bytes.len());
    if *aggregate > MAX_LEGACY_HISTORY_BYTES {
        return Err(loader_error(
            "legacy history exceeds the aggregate byte ceiling",
        ));
    }
    Ok(bytes)
}

fn require_regular(entry: &LegacyDirectoryEntry) -> Result<()> {
    if entry.revision.kind != LegacyEntryKind::File {
        return Err(loader_error(
            "legacy authority must be a regular file, not a link or special entry",
        ));
    }
    Ok(())
}

fn is_migration_stem(stem: &str) -> bool {
    let bytes = stem.as_bytes();
    bytes.len() >= 5 && bytes[..4].iter().all(u8::is_ascii_digit) && bytes[4] == b'_'
}

fn looks_migration_shaped_bytes(name: &OsStr) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let bytes = name.as_bytes();
        bytes.len() >= 8
            && bytes[..4].iter().all(u8::is_ascii_digit)
            && bytes[4] == b'_'
            && (bytes.ends_with(b".py")
                || bytes.ends_with(b".json")
                || bytes.ends_with(ADOPTION_SUFFIX.as_bytes()))
    }
    #[cfg(not(unix))]
    {
        let _ = name;
        false
    }
}

fn is_recognized_legacy_root_name(name: &OsStr) -> bool {
    if name == OsStr::new(CONVERSION_JOURNAL) || name == OsStr::new("snapshots") {
        return true;
    }
    let Some(name) = name.to_str() else {
        return looks_migration_shaped_bytes(name);
    };
    if let Some(stem) = name.strip_suffix(ADOPTION_SUFFIX) {
        return is_migration_stem(stem);
    }
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    matches!(extension, "py" | "json") && is_migration_stem(stem)
}

fn is_current_platform_direct_child(name: &OsStr) -> bool {
    let path = Path::new(name);
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return false;
    }
    #[cfg(not(windows))]
    {
        !name.is_empty()
    }
    #[cfg(windows)]
    {
        let Some(portable) = name.to_str() else {
            return false;
        };
        let trimmed = portable.trim_end_matches(['.', ' ']);
        let windows_stem = portable
            .split('.')
            .next()
            .unwrap_or(portable)
            .trim_end_matches(['.', ' '])
            .to_ascii_uppercase();
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
        !portable.is_empty()
            && !portable.contains(['<', '>', ':', '"', '/', '\\', '|', '?', '*', '\0'])
            && !portable.chars().any(char::is_control)
            && trimmed == portable
            && !windows_device
    }
}

fn is_snapshot_version(value: &str) -> bool {
    value.len() == 5
        && value.starts_with('v')
        && value.as_bytes()[1..].iter().all(u8::is_ascii_digit)
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hash_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value);
}

fn hex_digest(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn loader_error(message: impl Into<String>) -> MigrationError {
    MigrationError::Loader {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dependency(name: &str) -> MigrationDependencySpec {
        MigrationDependencySpec {
            app_label: "example".to_owned(),
            migration_name: name.to_owned(),
        }
    }

    fn archive(
        name: &str,
        dependencies: Vec<MigrationDependencySpec>,
        source_text: &str,
        effect: LegacySchemaEffect,
        schema_source: &str,
        schema_hash: &str,
    ) -> LegacyAdoptionMetadata {
        LegacyAdoptionMetadata::new(
            "example",
            name,
            dependencies,
            migration_file_checksum(source_text),
            hex_digest(Sha256::digest(source_text.as_bytes())),
            effect,
            dependency(schema_source),
            schema_hash,
        )
        .expect("valid archive")
    }

    fn write_archive(directory: &Path, name: &str, source: &str, archive: &LegacyAdoptionMetadata) {
        let mut archive = archive.clone();
        let directory_app_label = directory
            .file_name()
            .and_then(OsStr::to_str)
            .expect("test directory app label");
        if archive.app_label == "example" && directory_app_label != "example" {
            archive.app_label = directory_app_label.to_owned();
            for dependency in &mut archive.dependencies {
                if dependency.app_label == "example" {
                    dependency.app_label = directory_app_label.to_owned();
                }
            }
            if archive.schema_source.app_label == "example" {
                archive.schema_source.app_label = directory_app_label.to_owned();
            }
            archive.metadata_digest = archive.expected_digest();
        }
        fs::write(directory.join(format!("{name}.py")), source).expect("Python source");
        fs::write(
            directory.join(format!("{name}{ADOPTION_SUFFIX}")),
            serde_json::to_vec(&archive).expect("archive JSON"),
        )
        .expect("archive writes");
    }

    fn sidecar_spec(
        app_label: &str,
        name: &str,
        dependencies: Vec<MigrationDependencySpec>,
        source: &str,
        operations: Vec<OperationSpec>,
    ) -> MigrationSpec {
        MigrationSpec {
            app_label: app_label.to_owned(),
            name: name.to_owned(),
            dependencies,
            operations,
            checksum: Some(migration_file_checksum(source)),
            source_sha256: Some("0".repeat(64)),
            reversible: true,
        }
    }

    fn sidecar_archive(
        source_name: &str,
        source: &str,
        sidecar: &MigrationSpec,
        sidecar_bytes: &[u8],
        effect: LegacySchemaEffect,
        schema_source: &str,
        schema_hash: &str,
    ) -> LegacySidecarAdoptionMetadata {
        let effective_app_label = if sidecar.app_label.is_empty() {
            "example"
        } else {
            &sidecar.app_label
        };
        let effective_name = if sidecar.name.is_empty() {
            source_name
        } else {
            &sidecar.name
        };
        LegacySidecarAdoptionMetadata::new(
            source_name,
            effective_app_label,
            effective_name,
            sidecar.dependencies.clone(),
            migration_file_checksum(source),
            sidecar.checksum.clone(),
            hex_digest(Sha256::digest(source.as_bytes())),
            hex_digest(Sha256::digest(sidecar_bytes)),
            effect,
            dependency(schema_source),
            schema_hash,
        )
        .expect("valid sidecar archive")
    }

    fn write_sidecar_archive(
        directory: &Path,
        source_name: &str,
        source: &str,
        sidecar: &MigrationSpec,
        archive: &LegacySidecarAdoptionMetadata,
    ) {
        fs::write(directory.join(format!("{source_name}.py")), source)
            .expect("sidecar Python source");
        fs::write(
            directory.join(format!("{source_name}.json")),
            serde_json::to_vec(sidecar).expect("sidecar JSON"),
        )
        .expect("sidecar writes");
        fs::write(
            directory.join(format!("{source_name}{ADOPTION_SUFFIX}")),
            serde_json::to_vec(archive).expect("sidecar adoption JSON"),
        )
        .expect("sidecar archive writes");
    }

    fn ignored(name: &str, source: &str) -> LegacyIgnoredSourceMetadata {
        LegacyIgnoredSourceMetadata::new(
            name,
            migration_file_checksum(source),
            hex_digest(Sha256::digest(source.as_bytes())),
        )
        .expect("valid ignored-source metadata")
    }

    fn write_ignored(
        directory: &Path,
        name: &str,
        source: &str,
        metadata: &LegacyIgnoredSourceMetadata,
    ) {
        fs::write(directory.join(format!("{name}.py")), source).expect("ignored Python source");
        fs::write(
            directory.join(format!("{name}{ADOPTION_SUFFIX}")),
            serde_json::to_vec(metadata).expect("ignored-source metadata JSON"),
        )
        .expect("ignored-source metadata writes");
    }

    fn write_snapshot(directory: &Path, version: &str, source: &str, schema: &str) {
        let snapshot = directory.join(format!("snapshots/{version}"));
        fs::create_dir_all(&snapshot).expect("snapshot directory");
        fs::write(snapshot.join(SNAPSHOT_SCHEMA), schema).expect("schema writes");
        let schema_hash = hex_digest(Sha256::digest(schema.as_bytes()));
        let manifest = serde_json::json!({
            "version": version,
            "source_migration": source,
            "schema_hash": schema_hash,
            "file_hashes": {"schema.tql": hex_digest(Sha256::digest(schema.as_bytes()))},
            "type_bridge_version": "1.5.11",
            "type_bridge_core_version": "1.5.11"
        });
        fs::write(
            snapshot.join(SNAPSHOT_MANIFEST),
            serde_json::to_vec(&manifest).expect("manifest JSON"),
        )
        .expect("manifest writes");
    }

    fn write_snapshot_migration(
        directory: &Path,
        name: &str,
        dependencies: Vec<MigrationDependencySpec>,
        source: &str,
        version: &str,
        schema: &str,
    ) -> String {
        let schema_hash = hex_digest(Sha256::digest(schema.as_bytes()));
        let metadata = archive(
            name,
            dependencies,
            source,
            LegacySchemaEffect::Snapshot,
            name,
            &schema_hash,
        );
        write_archive(directory, name, source, &metadata);
        write_snapshot(directory, version, name, schema);
        schema_hash
    }

    fn reset_captured_entry_count() {
        TEST_CAPTURED_DIRECTORY_ENTRIES.with(|count| count.set(0));
    }

    fn captured_entry_count() -> usize {
        TEST_CAPTURED_DIRECTORY_ENTRIES.with(std::cell::Cell::get)
    }

    #[test]
    fn archive_digest_binds_dependencies_effect_and_snapshot_source() {
        let cross_language = LegacyAdoptionMetadata::new(
            "example",
            "0001_initial",
            Vec::new(),
            "d3461e95d22dcdb8",
            hex_digest(Sha256::digest(b"class Migration: pass\n")),
            LegacySchemaEffect::Snapshot,
            dependency("0001_initial"),
            "0ec6fdcdeecccbcd6373795bb147f598b03b4f588ec28b9256b2b420e8dd36a9",
        )
        .expect("cross-language fixture");
        assert_eq!(
            cross_language.metadata_digest,
            "02b001d9756a151fec7e78f26cbc923ec563b7c0440b71b87a74dac099428431",
        );

        let no_op = LegacyAdoptionMetadata::new(
            "example",
            "0002_empty",
            vec![dependency("0001_initial")],
            "0123456789abcdef",
            hex_digest(Sha256::digest(b"class Migration: pass\n")),
            LegacySchemaEffect::UnchangedNoop,
            dependency("0001_initial"),
            "a".repeat(64),
        )
        .expect("checksum-bound no-op archive");
        assert_eq!(
            no_op.metadata_digest,
            "5493f6efe6d50ba3f4072ad435e18fc7a702f20741d02dfdbcc6bee2c0415163",
        );

        let source = "class Migration: pass\n";
        let archive = archive(
            "0002_backfill",
            vec![dependency("0001_initial")],
            source,
            LegacySchemaEffect::UnchangedRunPython,
            "0001_initial",
            &"a".repeat(64),
        );
        archive.verify().expect("digest verifies");

        let original = archive.clone();
        let mut tampered = original.clone();
        tampered.dependencies[0].migration_name = "0009_forged".to_owned();
        assert!(tampered.verify().is_err());

        let mut tampered = original.clone();
        tampered.schema_source.migration_name = "0009_forged".to_owned();
        assert!(tampered.verify().is_err());

        let mut tampered = original.clone();
        tampered.schema_effect = LegacySchemaEffect::Snapshot;
        assert!(tampered.verify().is_err());

        let mut tampered = original;
        let replacement = if tampered.metadata_digest.starts_with('0') {
            "1"
        } else {
            "0"
        };
        tampered.metadata_digest.replace_range(..1, replacement);
        assert!(tampered.verify().is_err());
    }

    #[test]
    fn sidecar_digest_binds_both_files_and_effective_semantics() {
        let source = "raise RuntimeError('sidecar prevents import')\n";
        let sidecar = sidecar_spec(
            "legacy_app",
            "0001_effective",
            Vec::new(),
            source,
            Vec::new(),
        );
        let bytes = serde_json::to_vec(&sidecar).expect("sidecar JSON");
        let archive = LegacySidecarAdoptionMetadata::new(
            "0001_raw",
            "legacy_app",
            "0001_effective",
            Vec::new(),
            migration_file_checksum(source),
            sidecar.checksum.clone(),
            hex_digest(Sha256::digest(source.as_bytes())),
            hex_digest(Sha256::digest(&bytes)),
            LegacySchemaEffect::Snapshot,
            MigrationDependencySpec {
                app_label: "legacy_app".to_owned(),
                migration_name: "0001_effective".to_owned(),
            },
            "a".repeat(64),
        )
        .expect("sidecar metadata");
        archive.verify().expect("sidecar digest verifies");

        let mut tampered = archive.clone();
        tampered.sidecar_sha256 = "b".repeat(64);
        assert!(tampered.verify().is_err());

        let mut tampered = archive.clone();
        tampered.dependencies.push(MigrationDependencySpec {
            app_label: "legacy_app".to_owned(),
            migration_name: "0000_forged".to_owned(),
        });
        assert!(tampered.verify().is_err());

        let mut tampered = archive;
        tampered.schema_effect = LegacySchemaEffect::UnchangedNoop;
        assert!(tampered.verify().is_err());
    }

    #[test]
    fn sidecar_archive_digest_matches_the_python_converter_fixture() {
        let source = concat!(
            "from typing import ClassVar\n\n",
            "from type_bridge.migration import Migration\n",
            "from type_bridge.migration.operations import Operation\n",
            "from type_bridge.migration import operations as ops\n\n\n",
            "class LegacyMigration(Migration):\n",
            "    dependencies: ClassVar[list[tuple[str, str]]] = []\n",
            "    operations: ClassVar[list[Operation]] = [\n",
            "        ops.RunTypeQL(\n",
            "            forward=\"define attribute snapshot-name, value string;\",\n",
            "            reverse=\"undefine attribute snapshot-name;\",\n",
            "        ),\n",
            "    ]\n",
        );
        let source_sha256 = hex_digest(Sha256::digest(source.as_bytes()));
        let sidecar = MigrationSpec {
            app_label: "migrations".to_owned(),
            name: "0001_initial".to_owned(),
            dependencies: Vec::new(),
            operations: vec![OperationSpec::RunTypeql {
                forward: "define attribute snapshot-name, value string;".to_owned(),
                reverse: Some("undefine attribute snapshot-name;".to_owned()),
            }],
            checksum: Some(migration_file_checksum(source)),
            source_sha256: Some(source_sha256.clone()),
            reversible: true,
        };
        let sidecar_bytes = serde_json::to_vec(&sidecar).expect("sidecar fixture JSON");
        let schema_hash = hex_digest(Sha256::digest(
            b"define\nattribute snapshot-name, value string;\n",
        ));
        let archive = LegacySidecarAdoptionMetadata::new(
            "0001_initial",
            "migrations",
            "0001_initial",
            Vec::new(),
            migration_file_checksum(source),
            sidecar.checksum.clone(),
            source_sha256,
            hex_digest(Sha256::digest(&sidecar_bytes)),
            LegacySchemaEffect::Snapshot,
            MigrationDependencySpec {
                app_label: "migrations".to_owned(),
                migration_name: "0001_initial".to_owned(),
            },
            schema_hash,
        )
        .expect("cross-language sidecar archive");
        assert_eq!(
            archive.metadata_digest,
            "268a1a09328bf87482f53818d93d1b613c415694138caccc7f8a5f6df1f0abe4"
        );
    }

    #[test]
    fn sidecar_copy_attribute_history_loads_without_child_snapshot() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let directory = temporary.path().join("example");
        fs::create_dir(&directory).expect("legacy directory");
        let schema = "define\nattribute parent-schema, value string;\n";
        let schema_hash = write_snapshot_migration(
            &directory,
            "0001_initial",
            Vec::new(),
            "class Migration: pass\n",
            "v0001",
            schema,
        );
        let source = "raise RuntimeError('released sidecar wins')\n";
        let sidecar = sidecar_spec(
            "example",
            "0002_backfill",
            vec![dependency("0001_initial")],
            source,
            vec![OperationSpec::CopyAttribute {
                owner: None,
                source: None,
                dest: None,
                filter: None,
                forward: Some("match $x isa person; insert $x has name 'x';".to_owned()),
                reverse: None,
            }],
        );
        let sidecar_bytes = serde_json::to_vec(&sidecar).expect("sidecar JSON");
        let archive = sidecar_archive(
            "0002_backfill",
            source,
            &sidecar,
            &sidecar_bytes,
            LegacySchemaEffect::UnchangedCopyAttribute,
            "0001_initial",
            &schema_hash,
        );
        write_sidecar_archive(&directory, "0002_backfill", source, &sidecar, &archive);

        let history = load_adoption_history(&directory).expect("sidecar history loads");
        assert_eq!(history.graph().migrations.len(), 2);
        assert_eq!(history.graph().migrations[1].name, "0002_backfill");
        assert!(history.graph().migrations[1].operations.is_empty());
        assert_eq!(
            history.graph().migrations[1].checksum.as_deref(),
            Some(migration_file_checksum(source).as_str())
        );
        let head = reconstruct_legacy_head(&history).expect("parent snapshot reconstructs");
        assert_eq!(head.schema_typeql(), schema);
    }

    #[test]
    fn optional_checksum_sidecar_preserves_divergent_effective_identity() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let directory = temporary.path().join("migrations");
        fs::create_dir(&directory).expect("legacy directory");
        let source = "raise RuntimeError('released sidecar wins')\n";
        let mut sidecar = sidecar_spec(
            "legacy_app",
            "0001_effective",
            Vec::new(),
            source,
            Vec::new(),
        );
        sidecar.checksum = None;
        // Deliberately stale: released Python ignores this additive field.
        sidecar.source_sha256 = Some("0".repeat(64));
        let sidecar_bytes = serde_json::to_vec(&sidecar).expect("sidecar JSON");
        let schema = "define\nattribute effective-identity, value string;\n";
        let schema_hash = hex_digest(Sha256::digest(schema.as_bytes()));
        let archive = LegacySidecarAdoptionMetadata::new(
            "0001_raw",
            "legacy_app",
            "0001_effective",
            Vec::new(),
            migration_file_checksum(source),
            None,
            hex_digest(Sha256::digest(source.as_bytes())),
            hex_digest(Sha256::digest(&sidecar_bytes)),
            LegacySchemaEffect::Snapshot,
            MigrationDependencySpec {
                app_label: "legacy_app".to_owned(),
                migration_name: "0001_effective".to_owned(),
            },
            &schema_hash,
        )
        .expect("sidecar archive");
        write_sidecar_archive(&directory, "0001_raw", source, &sidecar, &archive);
        write_snapshot(&directory, "v0001", "0001_effective", schema);

        let history = load_adoption_history(&directory).expect("released sidecar loads");
        let migration = &history.graph().migrations[0];
        assert_eq!(migration.app_label, "legacy_app");
        assert_eq!(migration.name, "0001_effective");
        assert_eq!(
            migration.checksum.as_deref(),
            Some(migration_file_checksum(source).as_str())
        );
    }

    #[test]
    fn mixed_copy_and_schema_sidecar_cannot_claim_copy_only_effect() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let directory = temporary.path().join("example");
        fs::create_dir(&directory).expect("legacy directory");
        let schema = "define\nattribute parent-schema, value string;\n";
        let schema_hash = write_snapshot_migration(
            &directory,
            "0001_initial",
            Vec::new(),
            "class Migration: pass\n",
            "v0001",
            schema,
        );
        let source = "# released sidecar authority\n";
        let sidecar = sidecar_spec(
            "example",
            "0002_mixed",
            vec![dependency("0001_initial")],
            source,
            vec![
                OperationSpec::CopyAttribute {
                    owner: None,
                    source: None,
                    dest: None,
                    filter: None,
                    forward: Some("match $x isa person; insert $x has name 'x';".to_owned()),
                    reverse: None,
                },
                OperationSpec::RunTypeql {
                    forward: "define attribute schema-change, value string;".to_owned(),
                    reverse: None,
                },
            ],
        );
        let bytes = serde_json::to_vec(&sidecar).expect("sidecar JSON");
        let archive = sidecar_archive(
            "0002_mixed",
            source,
            &sidecar,
            &bytes,
            LegacySchemaEffect::UnchangedCopyAttribute,
            "0001_initial",
            &schema_hash,
        );
        write_sidecar_archive(&directory, "0002_mixed", source, &sidecar, &archive);

        let error = load_adoption_history(&directory)
            .expect_err("mixed sidecar cannot inherit a snapshot")
            .to_string();
        assert!(error.contains("operations contradict"), "{error}");
    }

    #[test]
    fn retained_sidecar_tamper_is_rejected_by_exact_digest() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let directory = temporary.path().join("example");
        fs::create_dir(&directory).expect("legacy directory");
        let source = "raise RuntimeError('not imported')\n";
        let mut sidecar = sidecar_spec("example", "0001_initial", Vec::new(), source, Vec::new());
        let sidecar_bytes = serde_json::to_vec(&sidecar).expect("sidecar JSON");
        let schema = "define\nattribute exact-sidecar, value string;\n";
        let schema_hash = hex_digest(Sha256::digest(schema.as_bytes()));
        let archive = sidecar_archive(
            "0001_initial",
            source,
            &sidecar,
            &sidecar_bytes,
            LegacySchemaEffect::Snapshot,
            "0001_initial",
            &schema_hash,
        );
        write_sidecar_archive(&directory, "0001_initial", source, &sidecar, &archive);
        write_snapshot(&directory, "v0001", "0001_initial", schema);
        sidecar.reversible = false;
        fs::write(
            directory.join("0001_initial.json"),
            serde_json::to_vec(&sidecar).expect("tampered sidecar JSON"),
        )
        .expect("sidecar tampers");

        let error = load_adoption_history(&directory)
            .expect_err("exact sidecar tamper must reject")
            .to_string();
        assert!(error.contains("exact JSON digest differs"), "{error}");
    }

    #[test]
    fn empty_sidecar_app_label_cannot_forge_fallback_owner() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let directory = temporary.path().join("migrations");
        fs::create_dir(&directory).expect("legacy directory");
        let source = "raise RuntimeError('not imported')\n";
        let sidecar = sidecar_spec("", "0001_initial", Vec::new(), source, Vec::new());
        let sidecar_bytes = serde_json::to_vec(&sidecar).expect("sidecar JSON");
        let schema = "define\nattribute owner-check, value string;\n";
        let schema_hash = hex_digest(Sha256::digest(schema.as_bytes()));
        let archive = LegacySidecarAdoptionMetadata::new(
            "0001_initial",
            "forged_owner",
            "0001_initial",
            Vec::new(),
            migration_file_checksum(source),
            sidecar.checksum.clone(),
            hex_digest(Sha256::digest(source.as_bytes())),
            hex_digest(Sha256::digest(&sidecar_bytes)),
            LegacySchemaEffect::Snapshot,
            MigrationDependencySpec {
                app_label: "forged_owner".to_owned(),
                migration_name: "0001_initial".to_owned(),
            },
            &schema_hash,
        )
        .expect("forged archive is internally self-consistent");
        write_sidecar_archive(&directory, "0001_initial", source, &sidecar, &archive);
        write_snapshot(&directory, "v0001", "0001_initial", schema);

        let error = load_adoption_history(&directory)
            .expect_err("directory fallback must reject forged owner")
            .to_string();
        assert!(
            error.contains("semantics differ from their adoption metadata"),
            "{error}"
        );
    }

    #[test]
    fn empty_sidecar_identity_uses_directory_and_source_fallbacks() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let directory = temporary.path().join("migrations");
        fs::create_dir(&directory).expect("legacy directory");
        let source = "raise RuntimeError('not imported')\n";
        let sidecar = sidecar_spec("", "", Vec::new(), source, Vec::new());
        let sidecar_bytes = serde_json::to_vec(&sidecar).expect("sidecar JSON");
        let schema = "define\nattribute fallback-owner, value string;\n";
        let schema_hash = hex_digest(Sha256::digest(schema.as_bytes()));
        let archive = LegacySidecarAdoptionMetadata::new(
            "0001_fallback",
            "migrations",
            "0001_fallback",
            Vec::new(),
            migration_file_checksum(source),
            sidecar.checksum.clone(),
            hex_digest(Sha256::digest(source.as_bytes())),
            hex_digest(Sha256::digest(&sidecar_bytes)),
            LegacySchemaEffect::Snapshot,
            MigrationDependencySpec {
                app_label: "migrations".to_owned(),
                migration_name: "0001_fallback".to_owned(),
            },
            &schema_hash,
        )
        .expect("fallback archive");
        write_sidecar_archive(&directory, "0001_fallback", source, &sidecar, &archive);
        write_snapshot(&directory, "v0001", "0001_fallback", schema);

        let history = load_adoption_history(&directory).expect("fallback sidecar loads");
        assert_eq!(history.graph().migrations[0].app_label, "migrations");
        assert_eq!(history.graph().migrations[0].name, "0001_fallback");
        assert_eq!(
            reconstruct_legacy_head(&history)
                .expect("fallback head reconstructs")
                .schema_typeql(),
            schema
        );
    }

    #[test]
    fn source_archive_rejects_a_later_execution_sidecar() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let directory = temporary.path().join("example");
        fs::create_dir(&directory).expect("legacy directory");
        let source = "class Migration: pass\n";
        write_snapshot_migration(
            &directory,
            "0001_initial",
            Vec::new(),
            source,
            "v0001",
            "define\nattribute source-authority, value string;\n",
        );
        let sidecar = sidecar_spec("example", "0001_initial", Vec::new(), source, Vec::new());
        fs::write(
            directory.join("0001_initial.json"),
            serde_json::to_vec(&sidecar).expect("sidecar JSON"),
        )
        .expect("late sidecar writes");

        let error = load_adoption_history(&directory)
            .expect_err("released sidecar precedence must invalidate a source archive")
            .to_string();
        assert!(
            error.contains("source-authoritative adoption metadata"),
            "{error}"
        );
        assert!(
            error.contains("conflicts with a retained JSON sidecar"),
            "{error}"
        );
    }

    #[test]
    fn ignored_archive_rejects_a_later_execution_sidecar() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let directory = temporary.path().join("example");
        fs::create_dir(&directory).expect("legacy directory");
        let source = "# no public Migration subclass\n";
        write_ignored(
            &directory,
            "0001_notes",
            source,
            &ignored("0001_notes", source),
        );
        let sidecar = sidecar_spec("example", "0001_notes", Vec::new(), source, Vec::new());
        fs::write(
            directory.join("0001_notes.json"),
            serde_json::to_vec(&sidecar).expect("sidecar JSON"),
        )
        .expect("late sidecar writes");

        let error = load_adoption_history(&directory)
            .expect_err("released sidecar precedence must invalidate ignored evidence")
            .to_string();
        assert!(
            error.contains("ignored-source adoption metadata"),
            "{error}"
        );
        assert!(
            error.contains("conflicts with a retained JSON sidecar"),
            "{error}"
        );
    }

    #[test]
    fn source_archive_owner_must_match_the_migration_directory() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let directory = temporary.path().join("migrations");
        fs::create_dir(&directory).expect("legacy directory");
        let source = "class Migration: pass\n";
        let schema = "define\nattribute source-owner, value string;\n";
        let schema_hash = hex_digest(Sha256::digest(schema.as_bytes()));
        let archive = LegacyAdoptionMetadata::new(
            "forged_owner",
            "0001_initial",
            Vec::new(),
            migration_file_checksum(source),
            hex_digest(Sha256::digest(source.as_bytes())),
            LegacySchemaEffect::Snapshot,
            MigrationDependencySpec {
                app_label: "forged_owner".to_owned(),
                migration_name: "0001_initial".to_owned(),
            },
            &schema_hash,
        )
        .expect("internally bound forged archive");
        write_archive(&directory, "0001_initial", source, &archive);
        write_snapshot(&directory, "v0001", "0001_initial", schema);

        let error = load_adoption_history(&directory)
            .expect_err("released source imports always use the directory app label")
            .to_string();
        assert!(
            error.contains("does not match migration directory app label"),
            "{error}"
        );
    }

    #[test]
    fn orphan_execution_sidecars_remain_ignored_like_released_discovery() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let directory = temporary.path().join("example");
        fs::create_dir(&directory).expect("legacy directory");
        let schema = "define\nattribute orphan-compatible, value string;\n";
        write_snapshot_migration(
            &directory,
            "0001_initial",
            Vec::new(),
            "class Migration: pass\n",
            "v0001",
            schema,
        );
        fs::write(
            directory.join("0009_deleted.json"),
            b"{ stale orphan sidecar",
        )
        .expect("orphan sidecar writes");

        let history = load_adoption_history(&directory).expect("orphan sidecar is ignored");
        assert_eq!(history.graph().migrations.len(), 1);
        let head = reconstruct_legacy_head(&history).expect("snapshot reconstructs");
        assert_eq!(head.schema_typeql(), schema);
    }

    #[test]
    fn snapshot_manifest_preserves_released_python_json_semantics() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let schema = "define\nattribute python-json, value string;\n";
        write_snapshot_migration(
            directory.path(),
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            schema,
        );
        let schema_hash = hex_digest(Sha256::digest(schema.as_bytes()));
        let huge_integer = "9".repeat(400);
        let deep_ignored = format!("{}0{}", "[".repeat(512), "]".repeat(512));
        let manifest = format!(
            concat!(
                "{{\"version\":\"v0001\",",
                "\"source_migration\":false,",
                "\"source_migration\":\"0001_initial\",",
                "\"schema_hash\":\"{schema_hash}\",",
                "\"file_hashes\":{{\"schema.tql\":\"{forged}\",",
                "\"schema.tql\":\"{schema_hash}\"}},",
                "\"type_bridge_version\":NaN,",
                "\"type_bridge_core_version\":Infinity,",
                "\"ignored_negative\":-Infinity,",
                "\"ignored_huge\":{huge_integer},",
                "\"ignored_deep\":{deep_ignored},",
                "\"\\ud800leading-surrogate-key\":0,",
                "\"trailing-surrogate-key\\udc00\":0,",
                "\"ignored_nested\":{{\"\\ud800nested-surrogate-key\":0}},",
                "\"ignored_string\":\"NaN Infinity -Infinity\"}}"
            ),
            forged = "0".repeat(64),
            schema_hash = schema_hash,
            huge_integer = huge_integer,
            deep_ignored = deep_ignored,
        );
        fs::write(
            directory.path().join("snapshots/v0001/snapshot.json"),
            manifest,
        )
        .expect("Python-compatible manifest writes");

        let history = load_adoption_history(directory.path()).expect(
            "Python json constants, big integers, deep ignored members, and duplicate keys remain accepted",
        );
        let head = reconstruct_legacy_head(&history).expect("last duplicate values authorize");
        assert_eq!(head.source_migration(), "0001_initial");
        assert_eq!(head.schema_typeql(), schema);
    }

    #[test]
    fn snapshot_manifest_root_key_normalization_is_narrow() {
        let mut manifest = br#"{
            "\ud800leading": 0,
            "trailing\udc00": 0,
            "paired\ud83d\ude00": 0,
            "escaped-backslash\\ud800": 0,
            "nested": {"\ud800nested": 0}
        }"#
        .to_vec();
        sanitize_python_json_root_key_surrogates(&mut manifest);

        assert_eq!(
            manifest,
            br#"{
            "\ufffdleading": 0,
            "trailing\ufffd": 0,
            "paired\ud83d\ude00": 0,
            "escaped-backslash\\ud800": 0,
            "nested": {"\ud800nested": 0}
        }"#
        );
    }

    #[test]
    fn snapshot_manifest_rejects_lone_surrogates_in_authoritative_values() {
        let schema_hash = "0".repeat(64);
        let valid_fields = [
            ("version", "\"v0001\""),
            ("source_migration", "\"0001_initial\""),
            ("schema_hash", &format!("\"{schema_hash}\"")),
        ];
        for (malformed_field, malformed_value) in [
            ("version", "\"\\ud800v0001\""),
            ("source_migration", "\"0001_initial\\udc00\""),
            ("schema_hash", "\"\\ud800\""),
        ] {
            let fields = valid_fields
                .iter()
                .map(|(name, value)| {
                    let value = if *name == malformed_field {
                        malformed_value
                    } else {
                        value
                    };
                    format!("\"{name}\":{value}")
                })
                .chain(std::iter::once("\"file_hashes\":{}".to_owned()))
                .collect::<Vec<_>>()
                .join(",");
            let error = parse_snapshot_manifest(
                format!("{{{fields}}}").as_bytes(),
                Path::new("snapshot.json"),
            )
            .err()
            .expect("lone surrogate in an authoritative string remains invalid")
            .to_string();
            assert!(error.contains(malformed_field), "{error}");
        }
    }

    #[test]
    fn snapshot_manifest_version_metadata_remains_optional() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let schema = "define\nattribute optional-version, value string;\n";
        write_snapshot_migration(
            directory.path(),
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            schema,
        );
        let manifest_path = directory.path().join("snapshots/v0001/snapshot.json");
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).expect("manifest reads"))
                .expect("manifest parses");
        let object = manifest.as_object_mut().expect("manifest object");
        object.remove("type_bridge_version");
        object.remove("type_bridge_core_version");
        fs::write(
            &manifest_path,
            serde_json::to_vec(&manifest).expect("manifest serializes"),
        )
        .expect("manifest without informational versions writes");

        let history = load_adoption_history(directory.path())
            .expect("released snapshots did not require informational versions");
        assert_eq!(
            reconstruct_legacy_head(&history)
                .expect("snapshot reconstructs")
                .schema_typeql(),
            schema
        );
    }

    #[test]
    fn ignored_source_digest_binds_name_and_python_checksum() {
        let source = "\"\"\"Historical migration notes.\"\"\"\n";
        let metadata = ignored("0000_notes", source);
        metadata.verify().expect("ignored-source digest verifies");
        assert_eq!(metadata.checksum, "e14f63ac2d07fa3b");
        assert_eq!(
            metadata.metadata_digest,
            "0c03755f3652a25c5d52bff5367ff8fa70edc23cb5f25c76c953dbd4993b60f2",
        );

        let original = metadata.clone();
        let mut tampered = original.clone();
        tampered.name = "0009_forged".to_owned();
        assert!(tampered.verify().is_err());

        let mut tampered = original.clone();
        tampered.checksum = "0123456789abcdef".to_owned();
        assert!(tampered.verify().is_err());

        let mut tampered = original;
        let replacement = if tampered.metadata_digest.starts_with('0') {
            "1"
        } else {
            "0"
        };
        tampered.metadata_digest.replace_range(..1, replacement);
        assert!(tampered.verify().is_err());
    }

    #[test]
    fn ignored_sources_are_verified_and_excluded_from_the_legacy_graph() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let schema = "define\nattribute name, value string;\n";
        write_snapshot_migration(
            directory.path(),
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            schema,
        );
        let notes = "\"\"\"Notes retained beside migration history.\"\"\"\n";
        write_ignored(
            directory.path(),
            "0000_notes",
            notes,
            &ignored("0000_notes", notes),
        );
        let disabled = "class _DisabledMigration: pass\n";
        write_ignored(
            directory.path(),
            "0002_disabled",
            disabled,
            &ignored("0002_disabled", disabled),
        );

        let history = load_adoption_history(directory.path()).expect("ignored sources verify");

        assert_eq!(history.graph().migrations.len(), 1);
        assert_eq!(history.graph().migrations[0].name, "0001_initial");
        let head = reconstruct_legacy_head(&history).expect("normal head reconstructs");
        assert_eq!(head.schema_typeql(), schema);
    }

    #[test]
    fn reconstruction_continues_the_history_wide_byte_budget() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let schema = "define\nattribute budgeted, value string;\n";
        write_snapshot_migration(
            directory.path(),
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            schema,
        );
        let mut history = load_adoption_history(directory.path()).expect("history loads");
        history.consumed_bytes = MAX_LEGACY_HISTORY_BYTES;

        let error = reconstruct_legacy_head(&history)
            .expect_err("snapshot reads must continue the source/archive budget");

        assert!(error.to_string().contains("aggregate byte ceiling"));
    }

    #[test]
    fn ignored_source_tampering_and_forged_duplicate_records_fail_closed() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let notes = "# release notes\n";
        write_ignored(
            directory.path(),
            "0000_notes",
            notes,
            &ignored("0000_notes", notes),
        );
        fs::write(directory.path().join("0000_notes.py"), "# tampered notes\n")
            .expect("ignored source tampers");
        let error = load_adoption_history(directory.path())
            .expect_err("source drift cannot retain ignored classification");
        assert!(
            error
                .to_string()
                .contains("ignored-source adoption metadata drift")
        );

        let duplicate_directory = tempfile::tempdir().expect("duplicate legacy directory");
        let duplicated = ignored("0000_notes", notes);
        write_ignored(duplicate_directory.path(), "0000_notes", notes, &duplicated);
        write_ignored(
            duplicate_directory.path(),
            "0003_forged",
            notes,
            &duplicated,
        );
        let error = load_adoption_history(duplicate_directory.path())
            .expect_err("one signed identity cannot be duplicated under a forged filename");
        assert!(error.to_string().contains("does not match filename stem"));
    }

    #[test]
    fn duplicate_fields_in_ignored_source_metadata_are_rejected() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let source = "# notes\n";
        let metadata = ignored("0000_notes", source);
        fs::write(directory.path().join("0000_notes.py"), source).expect("Python source");
        fs::write(
            directory.path().join("0000_notes.adoption.json"),
            format!(
                "{{\"format\":\"{}\",\"name\":\"0000_notes\",\"name\":\"0000_forged\",\"checksum\":\"{}\",\"metadata_digest\":\"{}\"}}",
                LEGACY_IGNORED_SOURCE_METADATA_V1, metadata.checksum, metadata.metadata_digest,
            ),
        )
        .expect("duplicate metadata writes");

        let error = load_adoption_history(directory.path())
            .expect_err("duplicate identity fields must never be last-value-wins");
        assert!(error.to_string().contains("duplicate field"));
    }

    #[test]
    fn ignored_source_metadata_uses_the_legacy_artifact_byte_ceiling() {
        let directory = tempfile::tempdir().expect("legacy directory");
        fs::write(directory.path().join("0000_notes.py"), "# notes\n")
            .expect("ignored Python source");
        fs::write(
            directory.path().join("0000_notes.adoption.json"),
            vec![b' '; MAX_LEGACY_ARTIFACT_BYTES + 1],
        )
        .expect("oversized ignored-source metadata writes");

        let error = load_adoption_history(directory.path())
            .expect_err("ignored-source metadata must use the common bounded reader");
        assert!(error.to_string().contains("exceeds the byte ceiling"));
    }

    #[test]
    fn root_noop_requires_an_explicit_snapshot_authority() {
        let no_snapshot = LegacyAdoptionMetadata::new(
            "example",
            "0001_empty",
            Vec::new(),
            "0123456789abcdef",
            hex_digest(Sha256::digest(b"class Migration: pass\n")),
            LegacySchemaEffect::UnchangedNoop,
            dependency("0001_empty"),
            "a".repeat(64),
        )
        .expect_err("an authority-less root no-op must fail closed");
        assert!(
            no_snapshot
                .to_string()
                .contains("snapshot-bound dependency")
        );

        let directory = tempfile::tempdir().expect("legacy directory");
        write_snapshot_migration(
            directory.path(),
            "0001_baseline",
            Vec::new(),
            "class Migration:\n    operations = []\n",
            "v0001",
            "define\n",
        );
        let history = load_adoption_history(directory.path()).expect("baseline history loads");
        let head = reconstruct_legacy_head(&history).expect("empty baseline snapshot reconstructs");
        assert_eq!(head.source_migration(), "0001_baseline");
        assert_eq!(head.schema_typeql(), "define\n");
        let authority = type_bridge_schema_compat::parse_adopted_genesis_authority(
            type_bridge_contract::schema::DocumentId::new("legacy-empty-baseline.typeql")
                .expect("document identity"),
            head.schema_typeql(),
        )
        .expect("the reconstructed empty baseline must pass the production adoption parser");
        assert_eq!(authority.declared().facts().len(), 0);
    }

    #[test]
    fn run_python_head_inherits_and_reconstructs_its_parent_snapshot() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let initial_source = "class Migration:\n    operations = [RunTypeQL()]\n";
        let backfill_source = "class Migration:\n    operations = [RunPython()]\n";
        let schema = "define\nattribute tag, value string;\nentity person, owns tag[] @distinct;\n";
        let schema_hash = hex_digest(Sha256::digest(schema.as_bytes()));

        let initial = archive(
            "0001_initial",
            Vec::new(),
            initial_source,
            LegacySchemaEffect::Snapshot,
            "0001_initial",
            &schema_hash,
        );
        write_archive(directory.path(), "0001_initial", initial_source, &initial);
        let backfill = archive(
            "0002_backfill",
            vec![dependency("0001_initial")],
            backfill_source,
            LegacySchemaEffect::UnchangedRunPython,
            "0001_initial",
            &schema_hash,
        );
        write_archive(
            directory.path(),
            "0002_backfill",
            backfill_source,
            &backfill,
        );
        write_snapshot(directory.path(), "v0001", "0001_initial", schema);

        let history = load_adoption_history(directory.path()).expect("archival history loads");
        let head = reconstruct_legacy_head(&history).expect("snapshot reconstructs head");
        assert_eq!(head.source_migration(), "0001_initial");
        assert_eq!(head.schema_typeql(), schema);
        assert!(history.graph().migrations[0].operations.is_empty());
    }

    #[test]
    fn noop_head_inherits_and_reconstructs_its_parent_snapshot() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let initial_source = "class Migration:\n    operations = [RunTypeQL()]\n";
        let empty_source = "class Migration:\n    operations = []\n";
        let schema = "define\nattribute tag, value string;\nentity person, owns tag[] @distinct;\n";
        let schema_hash = write_snapshot_migration(
            directory.path(),
            "0001_initial",
            Vec::new(),
            initial_source,
            "v0001",
            schema,
        );
        let empty = archive(
            "0002_empty",
            vec![dependency("0001_initial")],
            empty_source,
            LegacySchemaEffect::UnchangedNoop,
            "0001_initial",
            &schema_hash,
        );
        write_archive(directory.path(), "0002_empty", empty_source, &empty);

        let history = load_adoption_history(directory.path()).expect("no-op history loads");
        let head = reconstruct_legacy_head(&history).expect("parent snapshot reconstructs head");
        assert_eq!(head.source_migration(), "0001_initial");
        assert_eq!(head.schema_typeql(), schema);
        assert_eq!(history.graph().migrations.len(), 2);
        assert!(history.graph().migrations[1].operations.is_empty());
    }

    #[test]
    fn noop_merge_inherits_one_converged_parent_authority() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let schema = "define\nattribute name, value string;\nentity person, owns name;\n";
        let schema_hash = write_snapshot_migration(
            directory.path(),
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            schema,
        );
        for (name, dependencies) in [
            ("0002_left", vec![dependency("0001_initial")]),
            ("0003_right", vec![dependency("0001_initial")]),
            (
                "0004_merge",
                vec![dependency("0002_left"), dependency("0003_right")],
            ),
        ] {
            let source = format!("class {name}:\n    operations = []\n");
            let metadata = archive(
                name,
                dependencies,
                &source,
                LegacySchemaEffect::UnchangedNoop,
                "0001_initial",
                &schema_hash,
            );
            write_archive(directory.path(), name, &source, &metadata);
        }

        let history = load_adoption_history(directory.path()).expect("converged no-op merge loads");
        let heads = history.heads().expect("validated heads");
        assert_eq!(heads.len(), 1);
        assert_eq!(heads[0].name, "0004_merge");
        let reconstructed =
            reconstruct_legacy_head(&history).expect("one parent authority reconstructs");
        assert_eq!(reconstructed.schema_typeql(), schema);
    }

    #[test]
    fn noop_merge_accepts_distinct_parent_owners_with_the_same_schema() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let schema = "define\nattribute shared, value string;\n";
        let schema_hash = write_snapshot_migration(
            directory.path(),
            "0001_left",
            Vec::new(),
            "class Left: pass\n",
            "v0001",
            schema,
        );
        write_snapshot_migration(
            directory.path(),
            "0002_right",
            Vec::new(),
            "class Right: pass\n",
            "v0002",
            schema,
        );
        let merge_source = "class Merge:\n    operations = []\n";
        let merge = archive(
            "0003_merge",
            vec![dependency("0001_left"), dependency("0002_right")],
            merge_source,
            LegacySchemaEffect::UnchangedNoop,
            "0001_left",
            &schema_hash,
        );
        write_archive(directory.path(), "0003_merge", merge_source, &merge);

        let history = load_adoption_history(directory.path())
            .expect("equal schemas with distinct snapshot owners converge");
        let head = reconstruct_legacy_head(&history)
            .expect("all distinct snapshot owners verify before convergence");
        assert_eq!(head.source_migration(), "0001_left");
        assert_eq!(head.schema_typeql(), schema);
    }

    #[test]
    fn convergent_multi_head_history_reconstructs_one_authoritative_schema() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let schema = "define\nattribute name, value string;\nentity person, owns name;\n";
        write_snapshot_migration(
            directory.path(),
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            schema,
        );
        write_snapshot_migration(
            directory.path(),
            "0002_left",
            vec![dependency("0001_initial")],
            "class Left: pass\n",
            "v0002",
            schema,
        );
        write_snapshot_migration(
            directory.path(),
            "0003_right",
            vec![dependency("0001_initial")],
            "class Right: pass\n",
            "v0003",
            schema,
        );

        let history = load_adoption_history(directory.path()).expect("multi-head history loads");
        assert_eq!(history.heads().expect("validated heads").len(), 2);
        let reconstructed =
            reconstruct_legacy_head(&history).expect("equal head snapshots converge");
        assert_eq!(reconstructed.source_migration(), "0002_left");
        assert_eq!(reconstructed.schema_typeql(), schema);
    }

    #[test]
    fn divergent_multi_head_history_fails_closed() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let initial_schema = "define\nattribute name, value string;\n";
        write_snapshot_migration(
            directory.path(),
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            initial_schema,
        );
        write_snapshot_migration(
            directory.path(),
            "0002_left",
            vec![dependency("0001_initial")],
            "class Left: pass\n",
            "v0002",
            "define\nattribute name, value string;\nentity person, owns name;\n",
        );
        write_snapshot_migration(
            directory.path(),
            "0003_right",
            vec![dependency("0001_initial")],
            "class Right: pass\n",
            "v0003",
            "define\nattribute name, value string;\nentity company, owns name;\n",
        );

        let history =
            load_adoption_history(directory.path()).expect("valid branched history loads");
        let error = reconstruct_legacy_head(&history)
            .expect_err("different authoritative head snapshots cannot be adopted");
        assert!(
            error
                .to_string()
                .contains("divergent authoritative snapshots")
        );
    }

    #[test]
    fn run_python_inherited_head_converges_with_an_exact_sibling_head() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let schema = "define\nattribute name, value string;\nentity person, owns name;\n";
        let schema_hash = write_snapshot_migration(
            directory.path(),
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            schema,
        );
        write_snapshot_migration(
            directory.path(),
            "0002_exact",
            vec![dependency("0001_initial")],
            "class Exact: pass\n",
            "v0002",
            schema,
        );
        let backfill_source = "class Backfill:\n    operations = [RunPython()]\n";
        let backfill = archive(
            "0003_backfill",
            vec![dependency("0001_initial")],
            backfill_source,
            LegacySchemaEffect::UnchangedRunPython,
            "0001_initial",
            &schema_hash,
        );
        write_archive(
            directory.path(),
            "0003_backfill",
            backfill_source,
            &backfill,
        );

        let history = load_adoption_history(directory.path()).expect("inherited history loads");
        assert_eq!(history.heads().expect("validated heads").len(), 2);
        let reconstructed =
            reconstruct_legacy_head(&history).expect("inherited and exact authorities converge");
        assert_eq!(reconstructed.schema_typeql(), schema);
    }

    #[test]
    fn unchanged_merge_rejects_divergent_parent_snapshots() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let left_source = "class Left: pass\n";
        let right_source = "class Right: pass\n";
        let merge_source = "class Merge: pass\n";
        let left_hash = "a".repeat(64);
        let right_hash = "b".repeat(64);
        write_archive(
            directory.path(),
            "0001_left",
            left_source,
            &archive(
                "0001_left",
                Vec::new(),
                left_source,
                LegacySchemaEffect::Snapshot,
                "0001_left",
                &left_hash,
            ),
        );
        write_archive(
            directory.path(),
            "0002_right",
            right_source,
            &archive(
                "0002_right",
                Vec::new(),
                right_source,
                LegacySchemaEffect::Snapshot,
                "0002_right",
                &right_hash,
            ),
        );
        write_archive(
            directory.path(),
            "0003_merge",
            merge_source,
            &archive(
                "0003_merge",
                vec![dependency("0001_left"), dependency("0002_right")],
                merge_source,
                LegacySchemaEffect::UnchangedRunPython,
                "0001_left",
                &left_hash,
            ),
        );

        let error = load_adoption_history(directory.path())
            .expect_err("a divergent schema-neutral merge has no one head authority");
        assert!(error.to_string().contains("divergent snapshot authorities"));
    }

    #[test]
    fn noop_merge_rejects_divergent_parent_snapshots() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let left_source = "class Left: pass\n";
        let right_source = "class Right: pass\n";
        let merge_source = "class Merge:\n    operations = []\n";
        let left_hash = "a".repeat(64);
        let right_hash = "b".repeat(64);
        write_archive(
            directory.path(),
            "0001_left",
            left_source,
            &archive(
                "0001_left",
                Vec::new(),
                left_source,
                LegacySchemaEffect::Snapshot,
                "0001_left",
                &left_hash,
            ),
        );
        write_archive(
            directory.path(),
            "0002_right",
            right_source,
            &archive(
                "0002_right",
                Vec::new(),
                right_source,
                LegacySchemaEffect::Snapshot,
                "0002_right",
                &right_hash,
            ),
        );
        write_archive(
            directory.path(),
            "0003_merge",
            merge_source,
            &archive(
                "0003_merge",
                vec![dependency("0001_left"), dependency("0002_right")],
                merge_source,
                LegacySchemaEffect::UnchangedNoop,
                "0001_left",
                &left_hash,
            ),
        );

        let error = load_adoption_history(directory.path())
            .expect_err("a no-op merge cannot choose between divergent authorities");
        assert!(error.to_string().contains("divergent snapshot authorities"));
    }

    #[cfg(unix)]
    #[test]
    fn supplied_root_symlink_is_followed_once_and_later_replacement_cannot_redirect_reads() {
        use std::os::unix::fs::symlink;

        let original = tempfile::tempdir().expect("original legacy directory");
        let replacement = tempfile::tempdir().expect("replacement legacy directory");
        let links = tempfile::tempdir().expect("link parent");
        let original_directory = original.path().join("legacy");
        let replacement_directory = replacement.path().join("legacy");
        fs::create_dir(&original_directory).expect("original legacy root");
        fs::create_dir(&replacement_directory).expect("replacement legacy root");
        let schema = "define\nattribute original-name, value string;\n";
        write_snapshot_migration(
            &original_directory,
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            schema,
        );
        write_snapshot_migration(
            &replacement_directory,
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            "define\nattribute replacement-name, value string;\n",
        );
        let link = links.path().join("legacy");
        symlink(&original_directory, &link).expect("root symlink");

        let history = load_adoption_history(&link).expect("root symlink is a supported input");
        fs::remove_file(&link).expect("old root symlink removes");
        symlink(&replacement_directory, &link).expect("replacement root symlink");

        let reconstructed = reconstruct_legacy_head(&history)
            .expect("retained authority reconstructs the original tree");
        assert_eq!(reconstructed.schema_typeql(), schema);
    }

    #[cfg(unix)]
    #[test]
    fn retained_root_survives_atomic_path_replacement_without_mixing_trees() {
        let parent = tempfile::tempdir().expect("legacy parent");
        let configured = parent.path().join("legacy");
        let held = parent.path().join("held");
        fs::create_dir(&configured).expect("legacy root");
        let schema = "define\nattribute held-name, value string;\n";
        write_snapshot_migration(
            &configured,
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            schema,
        );
        let authority = LegacyDirectoryAuthority::open_root(&configured)
            .expect("legacy root authority retains");
        fs::rename(&configured, &held).expect("original root moves");
        fs::create_dir(&configured).expect("replacement root");
        write_snapshot_migration(
            &configured,
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            "define\nattribute replacement-name, value string;\n",
        );

        let history = load_adoption_history_in(authority).expect("held history loads coherently");
        let reconstructed = reconstruct_legacy_head(&history).expect("held snapshot reconstructs");
        assert_eq!(reconstructed.schema_typeql(), schema);
    }

    #[cfg(unix)]
    #[test]
    fn retained_publication_never_lands_in_an_ambient_replacement_root() {
        let parent = tempfile::tempdir().expect("legacy parent");
        let configured = parent.path().join("legacy");
        let held = parent.path().join("held");
        fs::create_dir(&configured).expect("legacy root");
        let authority = LegacyDirectoryAuthority::open_root(&configured)
            .expect("legacy root authority retains");
        fs::rename(&configured, &held).expect("original root moves");
        fs::create_dir(&configured).expect("replacement root");

        authority
            .write_atomic_no_replace("authority.json", b"held")
            .expect("publication stays on the retained root");

        assert_eq!(
            fs::read(held.join("authority.json")).expect("held publication reads"),
            b"held"
        );
        assert!(!configured.join("authority.json").exists());
    }

    #[test]
    fn journal_swap_at_removal_is_restored_and_never_deleted() {
        let directory = tempfile::tempdir().expect("journal directory");
        let authority = LegacyDirectoryAuthority::open_root(directory.path())
            .expect("journal authority retains");
        let journal = directory.path().join(CONVERSION_JOURNAL);
        let held = directory.path().join("held-original.json");
        let expected_bytes = b"expected journal\n";
        let replacement_bytes = b"attacker journal\n";
        fs::write(&journal, expected_bytes).expect("expected journal writes");
        let expected = authority
            .inspect_relative(Path::new(CONVERSION_JOURNAL), None)
            .expect("journal inspection succeeds")
            .expect("journal exists");
        let hook_journal = journal.clone();
        let hook_held = held.clone();
        TEST_BEFORE_JOURNAL_QUARANTINE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::rename(&hook_journal, &hook_held).expect("expected journal moves");
                fs::write(&hook_journal, replacement_bytes).expect("replacement journal writes");
            }));
        });

        let removed = authority
            .remove_if_matches(CONVERSION_JOURNAL, &expected, expected_bytes)
            .expect("mismatched quarantine restores without replacement");

        assert!(!removed);
        assert_eq!(
            fs::read(&journal).expect("replacement remains at public name"),
            replacement_bytes
        );
        assert_eq!(
            fs::read(&held).expect("original was moved by the test attacker"),
            expected_bytes
        );
        assert!(
            fs::read_dir(directory.path())
                .expect("directory reads")
                .all(|entry| !entry
                    .expect("entry reads")
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".tb-adopt-rm-"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn in_place_source_change_is_rejected_even_when_parent_directory_is_unchanged() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let path = directory.path().join("0001_initial.py");
        fs::write(&path, b"original-bytes\n").expect("original source");
        let authority = LegacyDirectoryAuthority::open_root(directory.path())
            .expect("legacy root authority retains");
        let capture = authority.capture().expect("root captures");
        let source = capture
            .entries
            .iter()
            .find(|entry| entry.name == OsStr::new("0001_initial.py"))
            .expect("source observation");
        let directory_revision = authority.directory_revision().expect("root revision");
        fs::write(&path, b"replaced-bytes\n").expect("same-length in-place replacement");
        authority
            .require_directory_revision(&directory_revision)
            .expect("in-place body write does not mutate the parent directory");

        let error = authority
            .read_relative_bounded(Path::new("0001_initial.py"), 1024, Some(source))
            .expect_err("captured file revision rejects in-place content drift");
        assert!(
            error
                .to_string()
                .contains("changed after bounded directory enumeration")
        );
    }

    #[test]
    fn recognized_root_body_digest_closes_metadata_equality_seam() {
        let directory = tempfile::tempdir().expect("legacy directory");
        write_snapshot_migration(
            directory.path(),
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            "define\nattribute root-digest, value string;\n",
        );
        let mut history = load_adoption_history(directory.path()).expect("history loads");
        fs::write(
            directory.path().join("0001_initial.py"),
            "class Changed: pass\n",
        )
        .expect("same-length source body changes");

        // Model a filesystem whose observable revision tuple aliases the old
        // and new body: preserve the originally captured digest while updating
        // only the test seam's metadata observation to the current revision.
        let observed = history
            .directory
            .inspect_relative(Path::new("0001_initial.py"), None)
            .expect("source inspects")
            .expect("source remains present");
        let captured = history
            .directory_capture
            .entries
            .iter_mut()
            .find(|entry| entry.name == OsStr::new("0001_initial.py"))
            .expect("source capture exists");
        *captured = observed;

        let error = history
            .require_unchanged()
            .expect_err("exact root body digest must reject aliased metadata")
            .to_string();
        assert!(
            error.contains("recognized legacy root file body changed"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn recognized_root_rename_is_rejected_even_when_directory_mtime_is_restored() {
        use std::fs::{File, FileTimes};

        let directory = tempfile::tempdir().expect("legacy directory");
        write_snapshot_migration(
            directory.path(),
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            "define\nattribute root-membership, value string;\n",
        );
        let padding = directory.path().join("padding-file.json");
        fs::write(&padding, b"unrelated padding\n").expect("padding file");
        let history = load_adoption_history(directory.path()).expect("history loads");
        let modified = fs::metadata(directory.path())
            .expect("root metadata")
            .modified()
            .expect("root modification time");

        fs::rename(&padding, directory.path().join("0009_deleted.json"))
            .expect("padding becomes a released sidecar name");
        File::open(directory.path())
            .expect("root opens")
            .set_times(FileTimes::new().set_modified(modified))
            .expect("root mtime restores");

        let error = history
            .require_unchanged()
            .expect_err("filtered root membership must not rely on restorable mtime")
            .to_string();
        assert!(
            error.contains("recognized legacy migration membership changed"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_version_rename_is_rejected_even_when_directory_mtime_is_restored() {
        use std::fs::{File, FileTimes};

        let directory = tempfile::tempdir().expect("snapshot directory");
        fs::create_dir(directory.path().join("x0009")).expect("padding snapshot directory");
        let authority = LegacyDirectoryAuthority::open_root(directory.path())
            .expect("snapshot authority retains");
        let mut capture = authority.capture().expect("snapshot root captures");
        capture
            .entries
            .retain(|entry| entry.name.to_str().is_some_and(is_snapshot_version));
        let modified = fs::metadata(directory.path())
            .expect("snapshot root metadata")
            .modified()
            .expect("snapshot root modification time");

        fs::rename(
            directory.path().join("x0009"),
            directory.path().join("v0009"),
        )
        .expect("padding becomes a snapshot version");
        File::open(directory.path())
            .expect("snapshot root opens")
            .set_times(FileTimes::new().set_modified(modified))
            .expect("snapshot root mtime restores");

        let error = authority
            .require_snapshot_version_capture(&capture)
            .expect_err("filtered snapshot membership must not rely on restorable mtime")
            .to_string();
        assert!(
            error.contains("snapshot version membership changed"),
            "{error}"
        );
    }

    #[test]
    fn require_unchanged_reverifies_snapshot_file_contents() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let original = "define\nattribute original-name, value string;\n";
        write_snapshot_migration(
            directory.path(),
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            original,
        );
        let history = load_adoption_history(directory.path()).expect("history loads");
        assert_eq!(
            reconstruct_legacy_head(&history)
                .expect("initial snapshot reconstructs")
                .schema_typeql(),
            original
        );
        fs::write(
            directory.path().join("snapshots/v0001/schema.tql"),
            "define\nattribute replaced-name, value string;\n",
        )
        .expect("same-length snapshot body changes in place");

        let error = history
            .require_unchanged()
            .expect_err("use-point validation must rehash snapshot children")
            .to_string();
        assert!(error.contains("snapshot file hash mismatch"), "{error}");
    }

    #[test]
    fn released_snapshot_ignores_unbound_pycache_children() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let schema = "define\nattribute imported-name, value string;\n";
        write_snapshot_migration(
            directory.path(),
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            schema,
        );
        let cache = directory.path().join("snapshots/v0001/__pycache__");
        fs::create_dir(&cache).expect("import cache directory");
        fs::write(cache.join("entities.cpython-313.pyc"), b"ambient cache")
            .expect("import cache body");

        let history = load_adoption_history(directory.path()).expect("history loads");
        let reconstructed =
            reconstruct_legacy_head(&history).expect("unbound cache is not snapshot authority");

        assert_eq!(reconstructed.schema_typeql(), schema);
    }

    #[test]
    fn irrelevant_snapshot_manifest_change_during_scan_fails_closed() {
        let directory = tempfile::tempdir().expect("legacy directory");
        write_snapshot_migration(
            directory.path(),
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            "define\nattribute scan-race, value string;\n",
        );
        write_snapshot(
            directory.path(),
            "v0009",
            "9999_irrelevant",
            "define\nattribute irrelevant, value string;\n",
        );
        let history = load_adoption_history(directory.path()).expect("history loads");
        let irrelevant_manifest = directory.path().join("snapshots/v0009/snapshot.json");
        TEST_AFTER_SNAPSHOT_SCAN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::write(&irrelevant_manifest, b"{}").expect("irrelevant manifest mutates");
            }));
        });

        let error = reconstruct_legacy_head(&history)
            .expect_err("every scanned manifest must stay exact through authority selection")
            .to_string();
        assert!(
            error.contains("scanned legacy snapshot manifest changed after bounded scan"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_descendant_swap_is_rejected_before_reconstruction() {
        let directory = tempfile::tempdir().expect("legacy directory");
        write_snapshot_migration(
            directory.path(),
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            "define\nattribute original-name, value string;\n",
        );
        let history = load_adoption_history(directory.path()).expect("history loads");
        let version = directory.path().join("snapshots/v0001");
        let held = directory.path().join("snapshots/v0001-held");
        fs::rename(&version, &held).expect("snapshot version moves");
        write_snapshot(
            directory.path(),
            "v0001",
            "0001_initial",
            "define\nattribute replacement-name, value string;\n",
        );

        let error = reconstruct_legacy_head(&history)
            .expect_err("snapshot-root revision rejects the descendant swap");
        assert!(
            error
                .to_string()
                .contains("retained entry changed after bounded capture")
        );
    }

    #[test]
    fn duplicate_snapshot_source_names_across_apps_are_ambiguous() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let schema = "define\nattribute shared-name, value string;\n";
        let schema_hash = hex_digest(Sha256::digest(schema.as_bytes()));
        write_snapshot(directory.path(), "v0001", "0001_initial", schema);
        let directory_authority = LegacyDirectoryAuthority::open_root(directory.path())
            .expect("legacy root authority retains");
        let directory_capture = directory_authority.capture().expect("root captures");
        let snapshots_entry = directory_capture
            .entries
            .iter()
            .find(|entry| entry.name == OsStr::new("snapshots"))
            .expect("snapshots observation");
        let snapshots = directory_authority
            .open_child(snapshots_entry)
            .expect("snapshots retain");
        let snapshot_capture = snapshots.capture().expect("snapshot versions capture");
        let migrations = ["app_a", "app_b"]
            .into_iter()
            .map(|app_label| MigrationSpec {
                app_label: app_label.to_owned(),
                name: "0001_initial".to_owned(),
                dependencies: Vec::new(),
                operations: Vec::new(),
                checksum: Some("0123456789abcdef".to_owned()),
                source_sha256: None,
                reversible: false,
            })
            .collect::<Vec<_>>();
        let snapshot_authorities = ["app_a", "app_b"]
            .into_iter()
            .map(|app_label| {
                (
                    (app_label.to_owned(), "0001_initial".to_owned()),
                    SnapshotAuthority {
                        source: MigrationDependencySpec {
                            app_label: app_label.to_owned(),
                            migration_name: "0001_initial".to_owned(),
                        },
                        schema_hash: schema_hash.clone(),
                    },
                )
            })
            .collect();
        let history = LegacyAdoptionHistory {
            graph: MigrationGraph { migrations },
            directory: directory_authority,
            directory_capture,
            snapshots: Some((snapshots, snapshot_capture)),
            snapshot_authorities,
            root_file_digests: BTreeMap::new(),
            consumed_bytes: 0,
        };

        let error = reconstruct_legacy_head(&history)
            .expect_err("an app-less V1 snapshot cannot authorize two app owners")
            .to_string();
        assert!(error.contains("ambiguous across app labels"), "{error}");
    }

    #[test]
    fn native_load_rejects_cross_app_claims_on_one_snapshot_source() {
        let temporary = tempfile::tempdir().expect("temporary root");
        let directory = temporary.path().join("migrations");
        fs::create_dir(&directory).expect("legacy directory");
        let schema = "define\nattribute shared-source, value string;\n";
        let schema_hash = hex_digest(Sha256::digest(schema.as_bytes()));
        write_snapshot(&directory, "v0001", "0001_shared", schema);

        for (source_name, app_label) in [("0001_a", "app_a"), ("0001_b", "app_b")] {
            let source = "raise RuntimeError('released sidecar wins')\n";
            let sidecar = sidecar_spec(app_label, "0001_shared", Vec::new(), source, Vec::new());
            let sidecar_bytes = serde_json::to_vec(&sidecar).expect("sidecar JSON");
            let archive = LegacySidecarAdoptionMetadata::new(
                source_name,
                app_label,
                "0001_shared",
                Vec::new(),
                migration_file_checksum(source),
                sidecar.checksum.clone(),
                hex_digest(Sha256::digest(source.as_bytes())),
                hex_digest(Sha256::digest(&sidecar_bytes)),
                LegacySchemaEffect::Snapshot,
                MigrationDependencySpec {
                    app_label: app_label.to_owned(),
                    migration_name: "0001_shared".to_owned(),
                },
                &schema_hash,
            )
            .expect("cross-app sidecar archive");
            write_sidecar_archive(&directory, source_name, source, &sidecar, &archive);
        }

        let error = load_adoption_history(&directory)
            .expect_err("native load must reject app-less snapshot owner ambiguity")
            .to_string();
        assert!(error.contains("ambiguous across app labels"), "{error}");
    }

    #[test]
    fn irrelevant_snapshot_children_are_not_recursively_enumerated() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let schema = "define\nattribute relevant-name, value string;\n";
        write_snapshot_migration(
            directory.path(),
            "0001_initial",
            Vec::new(),
            "class Initial: pass\n",
            "v0001",
            schema,
        );
        for index in 1000..1128 {
            let version = format!("v{index:04}");
            write_snapshot(
                directory.path(),
                &version,
                "9999_irrelevant",
                "define\nattribute irrelevant-name, value string;\n",
            );
            let snapshot = directory.path().join("snapshots").join(version);
            for extra in 0..32 {
                fs::write(snapshot.join(format!("extra-{extra:02}.txt")), b"ignored")
                    .expect("irrelevant extra writes");
            }
        }
        let history = load_adoption_history(directory.path()).expect("history loads");
        let top_level_revalidation_entries = history.directory_capture.entries.len()
            + history
                .snapshots
                .as_ref()
                .expect("snapshot authority")
                .1
                .entries
                .len();
        reset_captured_entry_count();
        let started = std::time::Instant::now();

        let reconstructed = reconstruct_legacy_head(&history).expect("relevant snapshot verifies");

        assert_eq!(reconstructed.schema_typeql(), schema);
        assert_eq!(
            captured_entry_count(),
            3 * top_level_revalidation_entries + 2,
            "only exact root/version revalidation and the relevant snapshot children may be enumerated"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }

    #[test]
    fn ceiling_scale_reversed_chain_validates_resolves_and_finds_head_within_budget() {
        const NODE_COUNT: usize = 65_536;
        let schema_hash = "a".repeat(64);
        let root_authority = SnapshotAuthority {
            source: MigrationDependencySpec {
                app_label: "app".to_owned(),
                migration_name: "node_00000".to_owned(),
            },
            schema_hash,
        };
        let mut migrations = Vec::with_capacity(NODE_COUNT);
        let mut bindings = BTreeMap::new();
        for index in (0..NODE_COUNT).rev() {
            let name = format!("node_{index:05}");
            let dependencies = if index == 0 {
                Vec::new()
            } else {
                vec![MigrationDependencySpec {
                    app_label: "app".to_owned(),
                    migration_name: format!("node_{:05}", index - 1),
                }]
            };
            migrations.push(MigrationSpec {
                app_label: "app".to_owned(),
                name: name.clone(),
                dependencies,
                operations: Vec::new(),
                checksum: Some(format!("checksum-{index}")),
                source_sha256: None,
                reversible: false,
            });
            bindings.insert(
                ("app".to_owned(), name),
                SchemaBinding {
                    effect: if index == 0 {
                        LegacySchemaEffect::Snapshot
                    } else {
                        LegacySchemaEffect::UnchangedNoop
                    },
                    authority: root_authority.clone(),
                },
            );
        }
        let graph = MigrationGraph { migrations };
        let started = std::time::Instant::now();
        assert!(validate_graph(&graph, &[]).is_empty());
        let resolved = resolve_schema_bindings(&graph, &bindings).expect("chain resolves");
        let directory = tempfile::tempdir().expect("authority directory");
        let directory_authority =
            LegacyDirectoryAuthority::open_root(directory.path()).expect("directory authority");
        let directory_capture = directory_authority.capture().expect("directory captures");
        let history = LegacyAdoptionHistory {
            graph,
            directory: directory_authority,
            directory_capture,
            snapshots: None,
            snapshot_authorities: resolved,
            root_file_digests: BTreeMap::new(),
            consumed_bytes: 0,
        };
        let heads = history.heads().expect("head resolves");

        assert_eq!(heads.len(), 1);
        assert_eq!(heads[0].name, "node_65535");
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }

    #[test]
    fn executable_sidecar_alone_is_not_adoption_graph_authority() {
        let directory = tempfile::tempdir().expect("legacy directory");
        let source = "class Migration: pass\n";
        fs::write(directory.path().join("0001_initial.py"), source).expect("Python source");
        let spec = MigrationSpec {
            app_label: "example".to_owned(),
            name: "0001_initial".to_owned(),
            dependencies: Vec::new(),
            operations: Vec::new(),
            checksum: Some(migration_file_checksum(source)),
            source_sha256: None,
            reversible: true,
        };
        fs::write(
            directory.path().join("0001_initial.json"),
            serde_json::to_vec(&spec).expect("sidecar JSON"),
        )
        .expect("sidecar writes");

        let error = load_adoption_history(directory.path())
            .expect_err("independently editable sidecar dependencies are insufficient");
        assert!(error.to_string().contains("adoption metadata"));
    }
}
