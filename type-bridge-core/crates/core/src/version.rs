//! TypeDB driver / server version gate.
//!
//! This module is the **single source of truth** for the TypeDB compatibility
//! window and protocol-band map.  Every consumer — `type-bridge-orm`,
//! `type-bridge-server`, and the `type_bridge_core` PyO3 extension — reads
//! from here.  Nothing else re-declares the window or band constants.
//!
//! ## Key items
//!
//! | Item | Role |
//! |------|------|
//! | [`MIN_SUPPORTED`](crate::version::MIN_SUPPORTED) | Floor of the support window (`3.11.0`) |
//! | [`MAX_SUPPORTED_LINE`](crate::version::MAX_SUPPORTED_LINE) | Newest measured line as `(major, minor)` (`3.14`) |
//! | [`window_contains`](crate::version::window_contains) | Admission predicate: at least `3.11.0` |
//! | [`semantic_profile_id`](crate::version::semantic_profile_id) | Map any supported patch release to its frozen schema semantic profile |
//! | [`GeneratedPackageRelease`](crate::version::GeneratedPackageRelease) | TypeDB patch floors a generated package's profile connects to |
//! | [`band`](crate::version::band) | Band a driver natively speaks (data; measured against live servers) |
//! | [`server_accepted_bands`](crate::version::server_accepted_bands) | Bands a server accepts connections from (data; measured) |
//! | [`negotiate_server_band`](crate::version::negotiate_server_band) | Pick the embedded band to connect a server with |
//! | [`check_supported`](crate::version::check_supported) | Installed-driver gate: window + driver band ∈ server's accepted set |
//! | [`check_server_supported`](crate::version::check_server_supported) | Embedded-runtime gate: window + accepted ∩ embedded ≠ ∅ |
//! | [`server_version`](crate::version::server_version) | Released Boolean adapter for the HTTP version probe |
//! | [`server_version_plaintext`](crate::version::server_version_plaintext) | Explicit plaintext HTTP version probe |
//! | [`server_version_native_roots`](crate::version::server_version_native_roots) | HTTPS version probe using native trust roots |
//! | [`server_version_custom_root_ca`](crate::version::server_version_custom_root_ca) | HTTPS version probe using one custom CA bundle |
//! | [`VersionError`](crate::version::VersionError) | Typed error for all failure modes |
//!
//! ## Two gates, two questions
//!
//! [`check_supported`](crate::version::check_supported) answers *"is the
//! **installed** single-band Python driver
//! protocol-compatible with this server?"* — it tests whether the server accepts
//! the one driver band the user has installed
//! ([`server_accepted_bands`](crate::version::server_accepted_bands)).
//!
//! [`check_server_supported`](crate::version::check_server_supported) answers
//! *"does **this build** of type-bridge embed a driver that can serve this
//! server?"* — the embedded runtime can carry several bands at once (the
//! default build embeds them all), so it tests whether the server's accepted
//! band set intersects the embedded set.

use std::ffi::OsString;
use std::fmt;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(unix)]
use cap_fs_ext::OpenOptionsSyncExt as _;
use cap_fs_ext::{
    DirExt as _, FollowSymlinks, OpenOptionsFollowExt as _, OpenOptionsMaybeDirExt as _,
};
use cap_std::ambient_authority;
#[cfg(windows)]
use cap_std::fs::OpenOptionsExt as _;
use cap_std::fs::{Dir, OpenOptions};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Version struct
// ---------------------------------------------------------------------------

/// A three-component TypeDB version number (`major.minor.patch`).
///
/// Supports both two-component (`"3.11"` → `patch = 0`) and three-component
/// (`"3.11.5"`) string representations.  Pre-release / build suffixes are
/// **not** accepted — `FromStr` is intentionally strict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Version {
    /// Major version component.
    pub major: u32,
    /// Minor version component.
    pub minor: u32,
    /// Patch version component.
    pub patch: u32,
}

impl Version {
    /// Construct a `Version` from its three components.
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl FromStr for Version {
    type Err = VersionError;

    /// Parse a version string.
    ///
    /// Accepts `"major.minor.patch"` and the two-component shorthand
    /// `"major.minor"` (patch defaults to `0`).  Any other form — including
    /// pre-release or build suffixes — is rejected.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('.').collect();
        match parts.as_slice() {
            [maj, min] => {
                let major = maj
                    .parse::<u32>()
                    .map_err(|_| VersionError::Parse(format!("invalid version string: {s:?}")))?;
                let minor = min
                    .parse::<u32>()
                    .map_err(|_| VersionError::Parse(format!("invalid version string: {s:?}")))?;
                Ok(Version::new(major, minor, 0))
            }
            [maj, min, pat] => {
                let major = maj
                    .parse::<u32>()
                    .map_err(|_| VersionError::Parse(format!("invalid version string: {s:?}")))?;
                let minor = min
                    .parse::<u32>()
                    .map_err(|_| VersionError::Parse(format!("invalid version string: {s:?}")))?;
                let patch = pat
                    .parse::<u32>()
                    .map_err(|_| VersionError::Parse(format!("invalid version string: {s:?}")))?;
                Ok(Version::new(major, minor, patch))
            }
            _ => Err(VersionError::Parse(format!(
                "invalid version string: {s:?}"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// Port constant (SSOT)
// ---------------------------------------------------------------------------

/// TypeDB's default HTTP API port, used by the `/v1/version` probe endpoint.
///
/// This is the single source of truth for the default; every binding that needs
/// a per-call fallback references this constant rather than hard-coding `8000`.
pub const DEFAULT_HTTP_PORT: u16 = 8000;

// A version response is one tiny JSON object. Keep the network allocation
// ceiling private so increasing it remains an explicit security review rather
// than an accidental consequence of an HTTP-client default.
const MAX_VERSION_RESPONSE_BYTES: usize = 4 * 1024;
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Window constants (SSOT — declared exactly once, here)
// ---------------------------------------------------------------------------

/// Floor of the TypeDB support window (`3.11.0`, inclusive).
///
/// Anything below this version is unsupported and must fail fast at connect.
pub const MIN_SUPPORTED: Version = Version::new(3, 11, 0);

/// Newest measured TypeDB line as `(major, minor)`, inclusive.
///
/// Since 2.3.3 this is not an admission ceiling. Newer lines are admitted
/// using this line's protocol and semantic assumptions, with a warning that
/// they are not tested or supported by this TypeBridge release.
pub const MAX_SUPPORTED_LINE: (u32, u32) = (3, 14);

/// Frozen schema semantic profile for every supported TypeDB 3.11 patch.
pub const TYPEDB_3_11_SEMANTIC_PROFILE_ID: &str = "typedb-3.11.5/v1";

/// Frozen schema semantic profile for every supported TypeDB 3.12 patch.
pub const TYPEDB_3_12_SEMANTIC_PROFILE_ID: &str = "typedb-3.12.1/v1";

/// Frozen schema semantic profile for every supported TypeDB 3.13 and 3.14
/// patch. TypeDB 3.14 changed no schema syntax, default or lowering, so it
/// shares the 3.13 profile.
pub const TYPEDB_3_13_SEMANTIC_PROFILE_ID: &str = "typedb-3.13.6/v1";

// ---------------------------------------------------------------------------
// Window predicate
// ---------------------------------------------------------------------------

/// Return `true` when `v` meets the minimum admitted version.
pub fn window_contains(v: &Version) -> bool {
    *v >= MIN_SUPPORTED
}

/// Whether `v` is on a line newer than any measured by this release.
#[must_use]
pub fn newer_than_measured(v: &Version) -> bool {
    (v.major, v.minor) > MAX_SUPPORTED_LINE
}

/// Map a supported TypeDB patch release to its frozen schema semantic profile.
///
/// Semantic profiles describe one measured minor-line behavior contract, not
/// the server's literal patch string. Newer lines reuse the newest measured
/// profile without claiming they are tested or supported. Only versions below
/// the admission floor return `None`.
#[must_use]
pub fn semantic_profile_id(v: &Version) -> Option<&'static str> {
    if !window_contains(v) {
        return None;
    }
    match (v.major, v.minor).min(MAX_SUPPORTED_LINE) {
        (3, 11) => Some(TYPEDB_3_11_SEMANTIC_PROFILE_ID),
        (3, 12) => Some(TYPEDB_3_12_SEMANTIC_PROFILE_ID),
        (3, 13 | 14) => Some(TYPEDB_3_13_SEMANTIC_PROFILE_ID),
        _ => None,
    }
}

/// TypeDB patch floors a generated package connects to, selected by the
/// package's semantic profile.
///
/// Generated packages admit each measured release and later patches on the
/// same minor line. The newest profile also admits newer lines, which are
/// not tested or supported by this TypeBridge release. Older profiles keep
/// their own lines. Only the profiles listed here can connect directly.
///
/// | Package profile | Admitted servers |
/// |-----------------|------------------|
/// | `typedb-3.12.1/v1` | `3.12.3+` |
/// | `typedb-3.13.6/v1` | `3.13.6+`, `3.14.0+`, and newer lines |
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GeneratedPackageRelease {
    servers: &'static [Version],
    admitted: &'static str,
}

const GENERATED_PACKAGE_FEATURE: &str = "generated-package provider compatibility";

impl GeneratedPackageRelease {
    /// Return the patch floors for a generated package's semantic profile,
    /// or `None` when generated packages on that profile cannot connect.
    #[must_use]
    pub fn for_profile(profile: &str) -> Option<Self> {
        const TYPEDB_3_12: &[Version] = &[Version::new(3, 12, 3)];
        const TYPEDB_3_13: &[Version] = &[Version::new(3, 13, 6), Version::new(3, 14, 0)];
        let (servers, admitted) = match profile {
            TYPEDB_3_12_SEMANTIC_PROFILE_ID => (TYPEDB_3_12, "3.12.3+"),
            TYPEDB_3_13_SEMANTIC_PROFILE_ID => (TYPEDB_3_13, "3.13.6+, 3.14.0+, and newer lines"),
            _ => return None,
        };
        Some(Self { servers, admitted })
    }

    /// The newest measured server release, the one a fresh lane runs.
    #[must_use]
    pub fn server(self) -> Version {
        *self.servers.last().expect("every profile admits a server")
    }

    /// Every measured patch floor, oldest first.
    #[must_use]
    pub const fn servers(self) -> &'static [Version] {
        self.servers
    }

    /// Describe an unsupported generated-package server using the admitted set.
    #[must_use]
    pub fn unsupported_version(self, observed: Version) -> VersionError {
        VersionError::FeatureUnsupported {
            feature: GENERATED_PACKAGE_FEATURE,
            server: observed,
            required: self.server(),
            remediation: self.admitted,
        }
    }

    /// Whether `observed` meets a patch floor or is newer than the newest profile.
    #[must_use]
    pub fn admits(self, observed: &Version) -> bool {
        self.servers.iter().any(|floor| {
            (newer_than_measured(observed) && (floor.major, floor.minor) == MAX_SUPPORTED_LINE)
                || (observed.major == floor.major
                    && observed.minor == floor.minor
                    && observed.patch >= floor.patch)
        })
    }
}

/// The admitted patch floors, followed by newer lines for the newest profile.
impl fmt::Display for GeneratedPackageRelease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.admitted)
    }
}

// ---------------------------------------------------------------------------
// Protocol band map (SSOT — measured live)
// ---------------------------------------------------------------------------

/// Return the measured or assumed protocol band for a TypeDB version.
/// Newer lines use the newest measured band; below-floor versions are unmapped.
///
/// The band encodes which gRPC protocol revision a driver of this version
/// speaks — a driver connects successfully only to servers that accept its
/// band (see [`server_accepted_bands`]).  This map lets the gate detect the
/// mismatch **before** attempting a connection.
///
/// | Minor line | Band |
/// |-----------|------|
/// | 3.11 | `8` (protocol 8) |
/// | 3.12 | `9` (protocol 3.12.0-rc0) |
/// | 3.13 | `9` (protocol 3.13.4; wire-compatible with 3.12, measured live) |
/// | 3.14 | `9` (protocol 3.14.0; drivers 3.13.6 and 3.14.0 measured live) |
/// | newer lines | newest measured line's band (unmeasured assumption) |
/// | below 3.11 | `None` |
///
/// When TypeDB ships a new minor line, measure it live and extend **both**
/// this map and [`server_accepted_bands`] — together they are the only data
/// the gate needs to cover the new version.
pub fn band(v: &Version) -> Option<u8> {
    match (v.major, v.minor).min(MAX_SUPPORTED_LINE) {
        (3, 11) => Some(8),
        (3, 12..=14) => Some(9),
        _ => None,
    }
}

/// Return the measured or assumed server protocol bands, native band first.
/// Newer lines reuse the newest measured set; below-floor versions return empty.
///
/// Band acceptance is asymmetric starting with 3.12: server 3.12 retains
/// backward compatibility with band-8 drivers (a 3.11.5 driver completes a
/// full round trip against a 3.12.0 server), while a 3.12 driver's native
/// band-9 protocol is rejected by a 3.11 server at connect.  A single
/// per-version band cannot express that, so servers carry an accepted *set*
/// and drivers keep their single native [`band`].
///
/// Server 3.13 is listed as band 9 only: drivers 3.12.3 and 3.13.6 were
/// measured against it, a band-8 (3.11.5) driver was not, so band 8 is not
/// claimed. The default build embeds band 9, so it serves 3.13 natively.
///
/// Server 3.14 accepts bands 9 and 8: drivers 3.13.6, 3.14.0 (band 9) and
/// 3.11.5 (band 8) each completed connect, version, schema, write and fetch
/// against 3.14.0, so band-8 version discovery works there too.
///
/// HAZARD (measured live on 3.11.5): a band-9 connection attempt does not
/// just fail against a 3.11 server — it crashes the server process.  Never
/// probe band 9 against a server of unknown version; discover through
/// band 8 first (the embedded runtime's gRPC fallback does exactly that).
///
/// Measured live (see [`band`] for the measurement grid):
///
/// | Server line | Accepts |
/// |-------------|---------|
/// | 3.11 | `[8]` |
/// | 3.12 | `[9, 8]` |
/// | 3.13 | `[9]` |
/// | 3.14 | `[9, 8]` |
/// | newer lines | newest measured line's set (unmeasured assumption) |
/// | below 3.11 | `[]` |
pub fn server_accepted_bands(v: &Version) -> &'static [u8] {
    match (v.major, v.minor).min(MAX_SUPPORTED_LINE) {
        (3, 11) => &[8],
        (3, 12 | 14) => &[9, 8],
        (3, 13) => &[9],
        _ => &[],
    }
}

/// Pick the band an embedded runtime should use to connect to `server`.
///
/// Returns the first band in the server's accepted set (native band first,
/// so a build embedding the server's native driver prefers it) that is also
/// in `embedded_bands`, or `None` when the server is unmapped or no embedded
/// driver can serve it.  [`check_server_supported`] is the gate; this is the
/// selection that follows a passing gate.
pub fn negotiate_server_band(server: &Version, embedded_bands: &[u8]) -> Option<u8> {
    server_accepted_bands(server)
        .iter()
        .copied()
        .find(|b| embedded_bands.contains(b))
}

// ---------------------------------------------------------------------------
// Combined gate
// ---------------------------------------------------------------------------

/// Check driver/server admission and measured protocol assumptions.
///
/// Both conditions must hold simultaneously:
///
/// 1. Each endpoint is within the support window (`window_contains`).
/// 2. The server accepts the driver's native protocol band
///    (`band(driver) ∈ server_accepted_bands(server)`).
///
/// The membership test (rather than band equality) carries the measured
/// asymmetry: driver 3.11 ↔ server 3.12 is compatible, driver 3.12 ↔
/// server 3.11 is not.
///
/// # Errors
///
/// Returns [`VersionError::Unsupported`] when either endpoint lies outside the
/// window, or [`VersionError::BandMismatch`] when the endpoints are in-window
/// but the server does not accept the driver's band.
pub fn check_supported(driver: &Version, server: &Version) -> Result<(), VersionError> {
    // Window check — each endpoint independently.
    if !window_contains(driver) {
        return Err(VersionError::Unsupported {
            component: "driver",
            found: *driver,
        });
    }
    if !window_contains(server) {
        return Err(VersionError::Unsupported {
            component: "server",
            found: *server,
        });
    }

    // Acceptance check: the server must accept the driver's native band.
    let driver_band = band(driver);
    match driver_band {
        Some(db) if server_accepted_bands(server).contains(&db) => Ok(()),
        // In-window but rejected (or unmapped — should not occur for any
        // currently-published TypeDB version; fail safe as a mismatch).
        _ => Err(VersionError::BandMismatch {
            driver: *driver,
            server: *server,
            driver_band: driver_band.unwrap_or(0),
            server_band: band(server).unwrap_or(0),
        }),
    }
}

/// Check admission using this build's embedded protocol assumptions.
///
/// This is the gate the embedded driver (the wheel / server binary / Node
/// binding) uses, as opposed to [`check_supported`], which gates an externally
/// **installed** single-band Python driver.  The embedded runtime may carry
/// several protocol bands simultaneously (the default build embeds them all),
/// so this gate tests **set intersection**: the server is served when any band
/// it accepts is present in `embedded_bands`. A 3.12 server accepts `[9, 8]`,
/// so a reduced build embedding only band 8 can still serve it; the default
/// build embeds band 9 and negotiates that native band first.
///
/// `embedded_bands` is supplied by the caller and is cfg-derived from the
/// `band8` / `band9` features compiled into that crate — core
/// declares no band features and never hardcodes the set.
///
/// # Errors
///
/// - [`VersionError::Unsupported`] when `server` is below the admission floor.
///   Newer lines reuse the newest measured protocol assumptions; they are
///   not tested or supported by this TypeBridge release.
/// - [`VersionError::EmbeddedUnavailable`] when `server` is in-window with
///   accepted bands, none of which this build compiled in.  Reachable only in
///   a non-default single-band build; the default build embeds every band an
///   in-window server accepts, so such a server is always served.
pub fn check_server_supported(server: &Version, embedded_bands: &[u8]) -> Result<(), VersionError> {
    // Keep the existing diagnostic class for servers below the admission floor.
    if !window_contains(server) {
        return Err(VersionError::Unsupported {
            component: "server",
            found: *server,
        });
    }

    let accepted = server_accepted_bands(server);
    if accepted.is_empty() {
        // Defensive: every admitted version maps to a measured band set.
        return Err(VersionError::Unsupported {
            component: "server",
            found: *server,
        });
    }

    if accepted.iter().any(|b| embedded_bands.contains(b)) {
        Ok(())
    } else {
        // In-window, mapped, but this build embeds no driver the server accepts.
        Err(VersionError::EmbeddedUnavailable { server: *server })
    }
}

// ---------------------------------------------------------------------------
// Feature gate (server capabilities by version line)
// ---------------------------------------------------------------------------

/// Server capabilities that only exist from a certain TypeDB version line.
///
/// Used to fail fast with an actionable versioned error instead of letting an
/// older server produce a syntax error for TypeQL it cannot parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feature {
    /// `@doc("...")` / `@meta("key", "value")` schema annotations (TypeDB 3.12+).
    SchemaAnnotations,
    /// `given` stage: parameterized multi-row query input (TypeDB 3.12+).
    GivenStage,
}

impl Feature {
    /// Human-readable feature name for error messages.
    pub fn name(self) -> &'static str {
        match self {
            Feature::SchemaAnnotations => "schema annotations (@doc/@meta)",
            Feature::GivenStage => "given-stage parameterized queries",
        }
    }

    /// The minimum server version that supports this feature.
    pub fn minimum_version(self) -> Version {
        match self {
            Feature::SchemaAnnotations => Version::new(3, 12, 0),
            Feature::GivenStage => Version::new(3, 12, 0),
        }
    }

    /// Feature-specific remediation for the versioned error message.
    pub fn remediation(self) -> &'static str {
        match self {
            Feature::SchemaAnnotations => "remove the annotations from the schema",
            Feature::GivenStage => "use per-row queries instead",
        }
    }
}

/// Check that `server` supports `feature`.
///
/// # Errors
///
/// Returns [`VersionError::FeatureUnsupported`] when the server predates the
/// feature's minimum version line.
pub fn check_feature_supported(feature: Feature, server: &Version) -> Result<(), VersionError> {
    let required = feature.minimum_version();
    if (server.major, server.minor) < (required.major, required.minor) {
        return Err(VersionError::FeatureUnsupported {
            feature: feature.name(),
            server: *server,
            required,
            remediation: feature.remediation(),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP probe helpers (pure / unit-testable)
// ---------------------------------------------------------------------------

/// Stable TLS configuration failures for explicit HTTPS version probes.
///
/// This type is deliberately separate from [`VersionError`].  `VersionError`
/// is a released, exhaustively matchable API, while TLS configuration is an
/// additive transport concern.  Callers can match these variants (or use
/// [`TlsConfigurationError::code`]) without parsing an operating-system error
/// string.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TlsConfigurationError {
    /// The platform trust store could not provide any usable roots.
    NativeRootsUnavailable,
    /// The configured custom root path does not identify a regular file.
    CustomRootCaNotFile {
        /// Path supplied by the caller.
        path: PathBuf,
    },
    /// The configured custom root file could not be read.
    CustomRootCaUnreadable {
        /// Path supplied by the caller.
        path: PathBuf,
    },
    /// The custom root bundle exceeds the bounded pre-I/O parser budget.
    CustomRootCaTooLarge {
        /// Path supplied by the caller.
        path: PathBuf,
    },
    /// The custom root file is empty, malformed, contains non-certificate PEM
    /// blocks, or contains a certificate that rustls cannot use as a root.
    CustomRootCaInvalidPem {
        /// Path supplied by the caller.
        path: PathBuf,
    },
    /// The statically selected rustls protocol configuration could not be
    /// constructed.
    ClientConfiguration,
}

impl TlsConfigurationError {
    /// Return the stable machine-readable diagnostic code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::NativeRootsUnavailable => "tls_native_roots_unavailable",
            Self::CustomRootCaNotFile { .. } => "tls_custom_root_ca_not_file",
            Self::CustomRootCaUnreadable { .. } => "tls_custom_root_ca_unreadable",
            Self::CustomRootCaTooLarge { .. } => "tls_custom_root_ca_too_large",
            Self::CustomRootCaInvalidPem { .. } => "tls_custom_root_ca_invalid_pem",
            Self::ClientConfiguration => "tls_client_configuration_invalid",
        }
    }
}

impl fmt::Display for TlsConfigurationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "TLS configuration error [{}]: ", self.code())?;
        match self {
            Self::NativeRootsUnavailable => {
                write!(
                    f,
                    "the platform trust store contains no usable certificates"
                )
            }
            Self::CustomRootCaNotFile { path } => write!(
                f,
                "custom root CA path is not a regular file: {}",
                path.display()
            ),
            Self::CustomRootCaUnreadable { path } => {
                write!(f, "custom root CA file cannot be read: {}", path.display())
            }
            Self::CustomRootCaTooLarge { path } => write!(
                f,
                "custom root CA file exceeds the 1 MiB bundle limit: {}",
                path.display()
            ),
            Self::CustomRootCaInvalidPem { path } => write!(
                f,
                "custom root CA file is not a valid PEM certificate bundle: {}",
                path.display()
            ),
            Self::ClientConfiguration => {
                write!(
                    f,
                    "the rustls client configuration could not be constructed"
                )
            }
        }
    }
}

impl std::error::Error for TlsConfigurationError {}

/// Failure returned by an explicit HTTP/HTTPS version probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionProbeError {
    /// Network, HTTP status, response parsing, or version parsing failure.
    Probe(VersionError),
    /// TLS trust material failed validation before any request was sent.
    TlsConfiguration(TlsConfigurationError),
}

impl VersionProbeError {
    /// Collapse the additive probe error onto the released [`VersionError`]
    /// surface used by [`server_version`].
    #[must_use]
    pub fn into_version_error(self) -> VersionError {
        match self {
            Self::Probe(error) => error,
            Self::TlsConfiguration(error) => VersionError::Probe(error.to_string()),
        }
    }
}

impl From<VersionError> for VersionProbeError {
    fn from(value: VersionError) -> Self {
        Self::Probe(value)
    }
}

impl From<TlsConfigurationError> for VersionProbeError {
    fn from(value: TlsConfigurationError) -> Self {
        Self::TlsConfiguration(value)
    }
}

impl fmt::Display for VersionProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Probe(error) => error.fmt(f),
            Self::TlsConfiguration(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for VersionProbeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Probe(error) => Some(error),
            Self::TlsConfiguration(error) => Some(error),
        }
    }
}

const MAX_CUSTOM_ROOT_CA_BYTES: u64 = 1024 * 1024;

fn validate_custom_root_pem_shape(
    configured_path: &Path,
    bytes: &[u8],
) -> Result<(), TlsConfigurationError> {
    const BEGIN_CERTIFICATE: &[u8] = b"-----BEGIN CERTIFICATE-----";
    const END_CERTIFICATE: &[u8] = b"-----END CERTIFICATE-----";

    let invalid = || TlsConfigurationError::CustomRootCaInvalidPem {
        path: configured_path.to_path_buf(),
    };
    let mut cursor = 0usize;
    let mut certificate_count = 0usize;

    while cursor < bytes.len() {
        while bytes.get(cursor).is_some_and(u8::is_ascii_whitespace) {
            cursor += 1;
        }
        if cursor == bytes.len() {
            break;
        }
        if !bytes[cursor..].starts_with(BEGIN_CERTIFICATE) {
            return Err(invalid());
        }

        let body_start = cursor + BEGIN_CERTIFICATE.len();
        let end_offset = bytes[body_start..]
            .windows(END_CERTIFICATE.len())
            .position(|window| window == END_CERTIFICATE)
            .ok_or_else(&invalid)?;
        cursor = body_start + end_offset + END_CERTIFICATE.len();
        if bytes
            .get(cursor)
            .is_some_and(|byte| !byte.is_ascii_whitespace())
        {
            return Err(invalid());
        }
        certificate_count += 1;
    }

    if certificate_count == 0 {
        return Err(invalid());
    }
    Ok(())
}

fn parse_custom_root_store(
    configured_path: &Path,
    bytes: &[u8],
) -> Result<rustls::RootCertStore, TlsConfigurationError> {
    // Every vendored driver consumes the bundle through `read_to_string`, so
    // reject non-UTF-8 material at the shared boundary instead of allowing
    // different bands to classify the same bytes differently.
    std::str::from_utf8(bytes).map_err(|_| TlsConfigurationError::CustomRootCaInvalidPem {
        path: configured_path.to_path_buf(),
    })?;
    // `rustls-pemfile` intentionally skips text outside recognised PEM
    // sections and silently ignores unknown section labels. A trust bundle is
    // a closed input, so reject that material before asking it to decode the
    // certificate bodies.
    validate_custom_root_pem_shape(configured_path, bytes)?;

    let mut reader = BufReader::new(bytes);
    let mut roots = rustls::RootCertStore::empty();
    let mut certificate_count = 0usize;

    loop {
        let item = rustls_pemfile::read_one(&mut reader).map_err(|_| {
            TlsConfigurationError::CustomRootCaInvalidPem {
                path: configured_path.to_path_buf(),
            }
        })?;
        let Some(item) = item else {
            break;
        };
        let rustls_pemfile::Item::X509Certificate(certificate) = item else {
            return Err(TlsConfigurationError::CustomRootCaInvalidPem {
                path: configured_path.to_path_buf(),
            });
        };
        roots
            .add(certificate)
            .map_err(|_| TlsConfigurationError::CustomRootCaInvalidPem {
                path: configured_path.to_path_buf(),
            })?;
        certificate_count += 1;
    }

    if certificate_count == 0 {
        return Err(TlsConfigurationError::CustomRootCaInvalidPem {
            path: configured_path.to_path_buf(),
        });
    }

    Ok(roots)
}

/// One bounded, validated snapshot of a custom root CA bundle.
///
/// This is a hidden cross-crate implementation seam for the secure TypeDB
/// runtime. Path-based loaders retain both the resolved source parent and the
/// exact source file used for the bounded read; external path authorities can
/// instead supply already-captured bytes. Both paths copy the accepted bytes
/// once into a private snapshot. HTTP trust is built from cached parsed roots.
/// Driver-band lowering is allowed to read only the snapshot handle:
/// `/dev/fd/<fd>` where that path is `/proc`-backed (Linux), a private named
/// snapshot file on the remaining Unixes (macOS `/dev/fd` shares the open
/// file description, so re-opening it would inherit the descriptor offset),
/// or the current handle-derived path while Windows no-write/no-delete
/// handles keep both the snapshot parent and file pinned.
#[doc(hidden)]
#[derive(Clone)]
pub struct RetainedCustomRootCa(Arc<RetainedCustomRootCaInner>);

struct RetainedCustomRootCaInner {
    configured_path: PathBuf,
    bytes: Arc<[u8]>,
    roots: rustls::RootCertStore,
    // Path-based loaders retain their exact source identity. Authorities that
    // already captured bytes through their own directory handle do not need
    // to reopen or retain the caller-controlled source name here.
    _source_directory: Option<Dir>,
    _source_file: Option<std::fs::File>,
    snapshot_file: Mutex<Option<std::fs::File>>,
    #[cfg(windows)]
    snapshot_directory: Option<Dir>,
    #[cfg(any(
        windows,
        all(unix, not(any(target_os = "linux", target_os = "android")))
    ))]
    snapshot_path: PathBuf,
}

impl Drop for RetainedCustomRootCaInner {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            // The snapshot file and its parent deliberately deny delete
            // sharing while the material is live. Close those handles before
            // removing the private name and directory.
            let snapshot = match self.snapshot_file.get_mut() {
                Ok(snapshot) => snapshot,
                Err(poisoned) => poisoned.into_inner(),
            };
            drop(snapshot.take());
            drop(self.snapshot_directory.take());
            let _ = std::fs::remove_file(&self.snapshot_path);
            if let Some(parent) = self.snapshot_path.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
        #[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
        {
            // The named snapshot lives in a private single-use directory;
            // remove both once no retained path is exposed anymore.
            let _ = std::fs::remove_file(&self.snapshot_path);
            if let Some(parent) = self.snapshot_path.parent() {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn create_custom_root_snapshot(
    configured_path: &Path,
    bytes: &[u8],
) -> Result<std::fs::File, TlsConfigurationError> {
    // `tempfile()` is anonymous on Unix. Only the retained descriptor can
    // reach this immutable-by-convention snapshot; configured-path writes,
    // renames, and parent swaps cannot change what a driver reads. This
    // requires the `/proc`-backed `/dev/fd` that hands every open of the
    // snapshot path an independent open file description, which only Linux
    // provides — the remaining Unixes use the named snapshot below.
    let mut snapshot =
        tempfile::tempfile().map_err(|_| TlsConfigurationError::CustomRootCaUnreadable {
            path: configured_path.to_path_buf(),
        })?;
    snapshot
        .write_all(bytes)
        .map_err(|_| TlsConfigurationError::CustomRootCaUnreadable {
            path: configured_path.to_path_buf(),
        })?;
    snapshot
        .flush()
        .map_err(|_| TlsConfigurationError::CustomRootCaUnreadable {
            path: configured_path.to_path_buf(),
        })?;
    snapshot.seek(SeekFrom::Start(0)).map_err(|_| {
        TlsConfigurationError::CustomRootCaUnreadable {
            path: configured_path.to_path_buf(),
        }
    })?;
    Ok(snapshot)
}

/// Create a private named snapshot for Unixes without a `/proc`-backed
/// `/dev/fd` (macOS and the BSDs), where opening `/dev/fd/<fd>` duplicates
/// the retained descriptor and would share its file offset. The snapshot
/// lives under a fresh mode-0700 temporary directory, so the configured
/// path's owner cannot influence what a driver reads from it.
#[cfg_attr(
    any(target_os = "linux", target_os = "android"),
    allow(dead_code, reason = "exercised directly by unit tests on Linux")
)]
#[cfg(unix)]
fn create_named_custom_root_snapshot(
    configured_path: &Path,
    bytes: &[u8],
) -> Result<(std::fs::File, PathBuf), TlsConfigurationError> {
    let unreadable = || TlsConfigurationError::CustomRootCaUnreadable {
        path: configured_path.to_path_buf(),
    };
    let temporary_directory = tempfile::Builder::new()
        .prefix("type-bridge-root-ca-")
        .tempdir()
        .map_err(|_| unreadable())?;
    let directory = Dir::open_ambient_dir(temporary_directory.path(), ambient_authority())
        .map_err(|_| unreadable())?;
    let snapshot_path = temporary_directory.path().join("root-ca.pem");
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    let mut snapshot = directory
        .open_with(Path::new("root-ca.pem"), &options)
        .map(cap_std::fs::File::into_std)
        .map_err(|_| unreadable())?;
    snapshot.write_all(bytes).map_err(|_| unreadable())?;
    snapshot.flush().map_err(|_| unreadable())?;
    snapshot
        .seek(SeekFrom::Start(0))
        .map_err(|_| unreadable())?;

    // Disable TempDir cleanup only after the complete snapshot exists; Drop
    // removes the private name and directory when the material retires.
    let retained_directory = temporary_directory.keep();
    debug_assert_eq!(snapshot_path.parent(), Some(retained_directory.as_path()));
    Ok((snapshot, snapshot_path))
}

#[cfg(windows)]
fn create_custom_root_snapshot(
    configured_path: &Path,
    bytes: &[u8],
) -> Result<(std::fs::File, Dir, PathBuf), TlsConfigurationError> {
    let temporary_directory = tempfile::Builder::new()
        .prefix("type-bridge-root-ca-")
        .tempdir()
        .map_err(|_| TlsConfigurationError::CustomRootCaUnreadable {
            path: configured_path.to_path_buf(),
        })?;
    let directory = Dir::open_ambient_dir(temporary_directory.path(), ambient_authority())
        .map_err(|_| TlsConfigurationError::CustomRootCaUnreadable {
            path: configured_path.to_path_buf(),
        })?;
    let snapshot_path = temporary_directory.path().join("root-ca.pem");
    let mut options = OpenOptions::new();
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    options
        .read(true)
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No)
        .share_mode(FILE_SHARE_READ);
    let mut snapshot = directory
        .open_with(Path::new("root-ca.pem"), &options)
        .map(cap_std::fs::File::into_std)
        .map_err(|_| TlsConfigurationError::CustomRootCaUnreadable {
            path: configured_path.to_path_buf(),
        })?;
    snapshot
        .write_all(bytes)
        .map_err(|_| TlsConfigurationError::CustomRootCaUnreadable {
            path: configured_path.to_path_buf(),
        })?;
    snapshot
        .flush()
        .map_err(|_| TlsConfigurationError::CustomRootCaUnreadable {
            path: configured_path.to_path_buf(),
        })?;
    snapshot.seek(SeekFrom::Start(0)).map_err(|_| {
        TlsConfigurationError::CustomRootCaUnreadable {
            path: configured_path.to_path_buf(),
        }
    })?;

    // Disable TempDir cleanup only after the complete snapshot exists. The
    // retained handles above enforce no-write/no-delete semantics; Drop closes
    // them in the required order and removes this private namespace.
    let retained_directory = temporary_directory.keep();
    debug_assert_eq!(snapshot_path.parent(), Some(retained_directory.as_path()));
    Ok((snapshot, directory, snapshot_path))
}

#[cfg(not(any(unix, windows)))]
fn create_custom_root_snapshot(
    configured_path: &Path,
    _bytes: &[u8],
) -> Result<std::fs::File, TlsConfigurationError> {
    Err(TlsConfigurationError::CustomRootCaUnreadable {
        path: configured_path.to_path_buf(),
    })
}

impl fmt::Debug for RetainedCustomRootCa {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RetainedCustomRootCa")
            .field("configured_path", &self.0.configured_path)
            .field("bytes", &self.0.bytes.len())
            .finish_non_exhaustive()
    }
}

fn open_custom_root_parent_nofollow(
    path: &Path,
    configured_path: &Path,
) -> Result<(Dir, OsString), TlsConfigurationError> {
    let unreadable = || TlsConfigurationError::CustomRootCaUnreadable {
        path: configured_path.to_path_buf(),
    };
    let file_name = path
        .file_name()
        .map(OsString::from)
        .ok_or_else(unreadable)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    // Anchor only at the filesystem root (or the current directory for a
    // relative path), then resolve each named directory through its preceding
    // retained descriptor. A single cap-std open with a multi-component path
    // would still be allowed to follow symlinks in intermediate components.
    let mut components = parent.components();
    let mut anchor = PathBuf::new();
    let mut saw_prefix = false;
    let mut saw_root = false;
    loop {
        match components.clone().next() {
            Some(Component::Prefix(prefix)) if !saw_prefix && !saw_root => {
                anchor.push(prefix.as_os_str());
                saw_prefix = true;
                let _ = components.next();
            }
            Some(Component::RootDir) if !saw_root => {
                anchor.push(Component::RootDir.as_os_str());
                saw_root = true;
                let _ = components.next();
            }
            _ => break,
        }
    }
    // Preserve Windows drive-relative semantics by using the drive prefix as
    // the ambient anchor; named components are still opened one at a time.
    if !saw_prefix && !saw_root {
        anchor.push(".");
    }

    let mut directory =
        Dir::open_ambient_dir(&anchor, ambient_authority()).map_err(|_| unreadable())?;
    let mut ancestors = Vec::new();
    for component in components {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                directory = match ancestors.pop() {
                    Some(parent) => parent,
                    None => directory
                        .open_parent_dir(ambient_authority())
                        .map_err(|_| unreadable())?,
                };
            }
            Component::Normal(name) => {
                let child = directory
                    .open_dir_nofollow(name)
                    .map_err(|_| unreadable())?;
                ancestors.push(directory);
                directory = child;
            }
            Component::Prefix(_) | Component::RootDir => return Err(unreadable()),
        }
    }

    Ok((directory, file_name))
}

impl RetainedCustomRootCa {
    /// Load and validate one already-physical custom root bundle without
    /// following any path component symlinks.
    ///
    /// Workspace and server callers use this entry point after their own
    /// confinement/canonicalization checks. All later HTTP and driver lowering
    /// consumes the cached roots or private byte snapshot.
    #[doc(hidden)]
    pub fn load(path: &Path) -> Result<Self, TlsConfigurationError> {
        Self::load_physical(path, path)
    }

    /// Resolve one raw binding path alias, then apply the physical no-follow
    /// loader while retaining the caller path in diagnostics.
    ///
    /// This entry point is deliberately distinct from [`Self::load`]: only
    /// language-binding inputs that have not already passed workspace or
    /// server confinement may follow a caller-supplied alias once.
    #[doc(hidden)]
    pub fn load_configured_alias(path: &Path) -> Result<Self, TlsConfigurationError> {
        Self::load_configured_alias_with_after_open(path, || {})
    }

    /// Validate and retain bytes already captured through an external path
    /// authority without reopening the diagnostic path.
    ///
    /// Server/workspace configuration loaders use this only after a bounded,
    /// regular-file read through their retained directory handle. All HTTP
    /// and driver consumers receive the private snapshot created here.
    #[doc(hidden)]
    pub fn load_captured_bytes(
        configured_path: &Path,
        bytes: Arc<[u8]>,
    ) -> Result<Self, TlsConfigurationError> {
        Self::from_captured_bytes(configured_path, bytes, None, None)
    }

    fn load_configured_alias_with_after_open<F>(
        configured_path: &Path,
        after_open: F,
    ) -> Result<Self, TlsConfigurationError>
    where
        F: FnOnce(),
    {
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::Yes);
        // A directory must open (Windows needs backup semantics for that) so
        // the not-a-file classification below comes from the opened handle's
        // metadata on every platform, exactly as it does on Unix.
        options.maybe_dir(true);
        #[cfg(unix)]
        options.nonblock(true);
        #[cfg(windows)]
        {
            // Permit concurrent readers, but pin this exact opened identity
            // against writes, deletion, and replacement while it is captured.
            const FILE_SHARE_READ: u32 = 0x0000_0001;
            options.share_mode(FILE_SHARE_READ);
        }
        let file =
            cap_std::fs::File::open_ambient_with(configured_path, &options, ambient_authority())
                .map(cap_std::fs::File::into_std)
                .map_err(|_| TlsConfigurationError::CustomRootCaUnreadable {
                    path: configured_path.to_path_buf(),
                })?;

        // A raw binding path is allowed to traverse aliases, but it is opened
        // only once. Parent or final-component replacement after this point
        // cannot redirect the bytes consumed below.
        after_open();
        Self::load_open_file(configured_path, None, file)
    }

    fn load_physical(
        physical_path: &Path,
        configured_path: &Path,
    ) -> Result<Self, TlsConfigurationError> {
        let (directory, file_name) =
            open_custom_root_parent_nofollow(physical_path, configured_path)?;
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        // A directory must open (Windows needs backup semantics for that) so
        // the not-a-file classification below comes from the opened handle's
        // metadata on every platform, exactly as it does on Unix.
        options.maybe_dir(true);
        #[cfg(unix)]
        options.nonblock(true);
        #[cfg(windows)]
        {
            // Permit the eager read-only opens performed by the vendored drivers,
            // but deny writes, deletion, and replacement while this handle lives.
            const FILE_SHARE_READ: u32 = 0x0000_0001;
            options.share_mode(FILE_SHARE_READ);
        }
        let file = directory
            .open_with(Path::new(&file_name), &options)
            .map(cap_std::fs::File::into_std)
            .map_err(|_| TlsConfigurationError::CustomRootCaUnreadable {
                path: configured_path.to_path_buf(),
            })?;
        Self::load_open_file(configured_path, Some(directory), file)
    }

    fn load_open_file(
        configured_path: &Path,
        source_directory: Option<Dir>,
        mut file: std::fs::File,
    ) -> Result<Self, TlsConfigurationError> {
        // Inspect and read the same open handle. The metadata check provides a
        // cheap rejection, while the limit-plus-one read remains authoritative if
        // another process grows the opened file.
        let before =
            file.metadata()
                .map_err(|_| TlsConfigurationError::CustomRootCaUnreadable {
                    path: configured_path.to_path_buf(),
                })?;
        if !before.is_file() {
            return Err(TlsConfigurationError::CustomRootCaNotFile {
                path: configured_path.to_path_buf(),
            });
        }
        if before.len() > MAX_CUSTOM_ROOT_CA_BYTES {
            return Err(TlsConfigurationError::CustomRootCaTooLarge {
                path: configured_path.to_path_buf(),
            });
        }

        let mut bytes = Vec::new();
        (&mut file)
            .take(MAX_CUSTOM_ROOT_CA_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| TlsConfigurationError::CustomRootCaUnreadable {
                path: configured_path.to_path_buf(),
            })?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_CUSTOM_ROOT_CA_BYTES {
            return Err(TlsConfigurationError::CustomRootCaTooLarge {
                path: configured_path.to_path_buf(),
            });
        }
        file.seek(SeekFrom::Start(0)).map_err(|_| {
            TlsConfigurationError::CustomRootCaUnreadable {
                path: configured_path.to_path_buf(),
            }
        })?;
        let mut verification_bytes = Vec::new();
        (&mut file)
            .take(MAX_CUSTOM_ROOT_CA_BYTES + 1)
            .read_to_end(&mut verification_bytes)
            .map_err(|_| TlsConfigurationError::CustomRootCaUnreadable {
                path: configured_path.to_path_buf(),
            })?;
        let after = file
            .metadata()
            .map_err(|_| TlsConfigurationError::CustomRootCaUnreadable {
                path: configured_path.to_path_buf(),
            })?;
        let timestamps_match = match (before.modified(), after.modified()) {
            (Ok(before), Ok(after)) => before == after,
            (Err(_), Err(_)) => true,
            _ => false,
        };
        if before.len() != after.len()
            || before.len() != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
            || bytes != verification_bytes
            || !timestamps_match
        {
            return Err(TlsConfigurationError::CustomRootCaUnreadable {
                path: configured_path.to_path_buf(),
            });
        }
        file.seek(SeekFrom::Start(0)).map_err(|_| {
            TlsConfigurationError::CustomRootCaUnreadable {
                path: configured_path.to_path_buf(),
            }
        })?;
        Self::from_captured_bytes(configured_path, bytes.into(), source_directory, Some(file))
    }

    fn from_captured_bytes(
        configured_path: &Path,
        bytes: Arc<[u8]>,
        source_directory: Option<Dir>,
        source_file: Option<std::fs::File>,
    ) -> Result<Self, TlsConfigurationError> {
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_CUSTOM_ROOT_CA_BYTES {
            return Err(TlsConfigurationError::CustomRootCaTooLarge {
                path: configured_path.to_path_buf(),
            });
        }
        let roots = parse_custom_root_store(configured_path, &bytes)?;

        #[cfg(any(target_os = "linux", target_os = "android"))]
        let snapshot_file = create_custom_root_snapshot(configured_path, &bytes)?;
        #[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
        let (snapshot_file, snapshot_path) =
            create_named_custom_root_snapshot(configured_path, &bytes)?;
        #[cfg(windows)]
        let (snapshot_file, snapshot_directory, snapshot_path) =
            create_custom_root_snapshot(configured_path, &bytes)?;
        #[cfg(not(any(unix, windows)))]
        let snapshot_file = create_custom_root_snapshot(configured_path, &bytes)?;

        Ok(Self(Arc::new(RetainedCustomRootCaInner {
            configured_path: configured_path.to_path_buf(),
            bytes,
            roots,
            _source_directory: source_directory,
            _source_file: source_file,
            snapshot_file: Mutex::new(Some(snapshot_file)),
            #[cfg(windows)]
            snapshot_directory: Some(snapshot_directory),
            #[cfg(any(
                windows,
                all(unix, not(any(target_os = "linux", target_os = "android")))
            ))]
            snapshot_path,
        })))
    }

    /// Invoke one eager driver lowering operation with a path pinned to the
    /// private captured-byte snapshot.
    ///
    /// The closure must consume the file synchronously.  This matches all
    /// three supported driver constructors, which read the PEM before they
    /// return their TLS configuration object.
    #[doc(hidden)]
    pub fn with_driver_root_path<T>(
        &self,
        lower: impl FnOnce(&Path) -> T,
    ) -> Result<T, TlsConfigurationError> {
        let mut snapshot = self.0.snapshot_file.lock().map_err(|_| {
            TlsConfigurationError::CustomRootCaUnreadable {
                path: self.0.configured_path.clone(),
            }
        })?;
        let file =
            snapshot
                .as_mut()
                .ok_or_else(|| TlsConfigurationError::CustomRootCaUnreadable {
                    path: self.0.configured_path.clone(),
                })?;
        file.seek(SeekFrom::Start(0)).map_err(|_| {
            TlsConfigurationError::CustomRootCaUnreadable {
                path: self.0.configured_path.clone(),
            }
        })?;

        // Only the `/proc`-backed `/dev/fd` gives every open of the retained
        // path an independent open file description at offset zero; on the
        // remaining Unixes that open duplicates this descriptor, offset
        // included, so lowering hands out the named snapshot path instead.
        #[cfg(any(target_os = "linux", target_os = "android"))]
        let retained_path = {
            use std::os::fd::AsRawFd as _;
            PathBuf::from(format!("/dev/fd/{}", file.as_raw_fd()))
        };
        #[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
        let retained_path = self.0.snapshot_path.clone();

        #[cfg(windows)]
        let retained_path = winx::file::get_file_path(file).map_err(|_| {
            TlsConfigurationError::CustomRootCaUnreadable {
                path: self.0.configured_path.clone(),
            }
        })?;

        #[cfg(not(any(unix, windows)))]
        return Err(TlsConfigurationError::CustomRootCaUnreadable {
            path: self.0.configured_path.clone(),
        });

        #[cfg(any(unix, windows))]
        {
            Ok(lower(&retained_path))
        }
    }

    pub(crate) fn root_store(&self) -> rustls::RootCertStore {
        self.0.roots.clone()
    }

    #[cfg(test)]
    fn captured_bytes(&self) -> &[u8] {
        &self.0.bytes
    }
}

fn native_root_store_from_loaded(
    loaded: rustls_native_certs::CertificateResult,
) -> Result<rustls::RootCertStore, TlsConfigurationError> {
    let mut roots = rustls::RootCertStore::empty();
    let (valid_count, _) = roots.add_parsable_certificates(loaded.certs);
    if valid_count == 0 {
        return Err(TlsConfigurationError::NativeRootsUnavailable);
    }
    Ok(roots)
}

pub(crate) fn native_root_store() -> Result<rustls::RootCertStore, TlsConfigurationError> {
    // Platform stores are often heterogeneous. Individual unreadable or
    // malformed entries are not fatal when the same load produced at least
    // one certificate rustls can use.
    native_root_store_from_loaded(rustls_native_certs::load_native_certs())
}

pub(crate) fn tls_agent(
    roots: rustls::RootCertStore,
) -> Result<ureq::Agent, TlsConfigurationError> {
    // ureq's default rustls feature uses webpki roots.  Build the client
    // explicitly so `NativeRoots` means the operating-system store and a
    // custom mode trusts only the supplied PEM bundle.
    let config = rustls::ClientConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_protocol_versions(&[&rustls::version::TLS12, &rustls::version::TLS13])
    .map_err(|_| TlsConfigurationError::ClientConfiguration)?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(ureq::builder()
        .tls_connector(Arc::new(RustlsConnector(Arc::new(config))))
        .redirects(0)
        .try_proxy_from_env(false)
        .build())
}

/// ureq's TLS hook over our rustls client configuration.
///
/// ureq's own rustls support is its `tls` feature, which also links
/// webpki-roots; this connector keeps the root store the caller chose.
struct RustlsConnector(Arc<rustls::ClientConfig>);

impl ureq::TlsConnector for RustlsConnector {
    fn connect(
        &self,
        dns_name: &str,
        mut io: Box<dyn ureq::ReadWrite>,
    ) -> Result<Box<dyn ureq::ReadWrite>, ureq::Error> {
        // URL hosts keep IPv6 brackets; server names do not.
        let host = dns_name
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(dns_name);
        let name = rustls::pki_types::ServerName::try_from(host)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?
            .to_owned();
        let mut connection = rustls::ClientConnection::new(Arc::clone(&self.0), name)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        connection.complete_io(&mut io)?;
        Ok(Box::new(RustlsStream(rustls::StreamOwned::new(
            connection, io,
        ))))
    }
}

#[derive(Debug)]
struct RustlsStream(rustls::StreamOwned<rustls::ClientConnection, Box<dyn ureq::ReadWrite>>);

impl ureq::ReadWrite for RustlsStream {
    fn socket(&self) -> Option<&std::net::TcpStream> {
        self.0.get_ref().socket()
    }
}

impl std::io::Read for RustlsStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}

impl std::io::Write for RustlsStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

pub(crate) fn plaintext_agent() -> ureq::Agent {
    // The returned version authorizes the matching gRPC endpoint. Redirects
    // and ambient proxies could otherwise substitute a different authority.
    ureq::builder()
        .redirects(0)
        .try_proxy_from_env(false)
        .build()
}

/// Validate a custom root CA bundle without performing network I/O.
///
/// The file must be a regular file containing one or more PEM-encoded X.509
/// certificates and no other PEM block types.
pub fn validate_custom_root_ca(path: &Path) -> Result<(), TlsConfigurationError> {
    RetainedCustomRootCa::load_configured_alias(path).map(|_| ())
}

/// Derive the HTTP version endpoint URL from a gRPC-style address.
///
/// The address may be:
/// - `"host:port"` → host extracted, `port` discarded, HTTP port used.
/// - `"host"` → used as-is.
/// - `"scheme://host:port"` → scheme prefix stripped, then as above.
///
/// The default HTTP port is `8000`.  Pass `http_port` to override.
/// Pass `tls = true` to use `https://` instead of `http://`.
pub(crate) fn version_endpoint(address: &str, http_port: u16, tls: bool) -> String {
    // Strip any scheme prefix (e.g. "typedb://", "grpc://", "https://").
    let without_scheme = if let Some(pos) = address.find("://") {
        &address[pos + 3..]
    } else {
        address
    };

    // Extract host: drop trailing ":port" if present.
    let host = if let Some(colon_pos) = without_scheme.rfind(':') {
        &without_scheme[..colon_pos]
    } else {
        without_scheme
    };

    let scheme = if tls { "https" } else { "http" };
    format!("{scheme}://{host}:{http_port}/v1/version")
}

/// Parse the JSON body returned by TypeDB's `GET /v1/version` endpoint.
///
/// Expected shape: `{"distribution": "TypeDB CE", "version": "3.12.1"}`.
/// Any deviation — including a missing `version` field or an unparseable
/// version string — is returned as [`VersionError::Probe`].
pub(crate) fn parse_version_response(json: &str) -> Result<Version, VersionError> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|e| {
        VersionError::Probe(format!("failed to parse version response as JSON: {e}"))
    })?;

    let version_str = value
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            VersionError::Probe("version response JSON missing the \"version\" field".to_string())
        })?;

    version_str.parse::<Version>().map_err(|e| {
        VersionError::Probe(format!(
            "could not parse version field {version_str:?}: {e}"
        ))
    })
}

// ---------------------------------------------------------------------------
// HTTP probe (network)
// ---------------------------------------------------------------------------

use type_bridge_contract::failure::TransportKind;

const UNREACHABLE_PREFIX: &str = "could not reach TypeDB HTTP version endpoint: ";
const UNREACHABLE_GUIDANCE: &str =
    "; ensure the TypeDB HTTP endpoint is reachable and that the port is correct";

/// Request failure with TypeBridge-owned guidance; the URL and provider prose stay out.
fn probe_unreachable(kind: TransportKind) -> VersionError {
    VersionError::Probe(format!(
        "{UNREACHABLE_PREFIX}{}{UNREACHABLE_GUIDANCE}",
        kind.description()
    ))
}

fn probe_transport_failure(kind: TransportKind) -> VersionError {
    VersionError::Probe(format!(
        "HTTP version request failed: {}",
        kind.description()
    ))
}

fn transport_source_kind(
    error: &(dyn std::error::Error + 'static),
    remaining: u8,
) -> Option<TransportKind> {
    if remaining == 0 {
        return None;
    }
    if error.is::<rustls::Error>() {
        return Some(TransportKind::Tls);
    }
    if let Some(error) = error.downcast_ref::<std::io::Error>() {
        if let Some(kind) = error
            .get_ref()
            .and_then(|source| transport_source_kind(source, remaining - 1))
        {
            return Some(kind);
        }
        let kind = match error.kind() {
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => TransportKind::Timeout,
            std::io::ErrorKind::ConnectionRefused => TransportKind::Unavailable,
            std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::NotConnected => TransportKind::Closed,
            _ => TransportKind::Other,
        };
        if kind != TransportKind::Other {
            return Some(kind);
        }
    }
    error
        .source()
        .and_then(|source| transport_source_kind(source, remaining - 1))
}

fn io_transport_kind(error: &std::io::Error) -> Option<TransportKind> {
    transport_source_kind(error, 8)
}

fn http_transport_kind(error: &ureq::Error) -> TransportKind {
    if let Some(kind) = transport_source_kind(error, 8) {
        return kind;
    }
    match error {
        ureq::Error::Status(401 | 403, _) => TransportKind::Authentication,
        ureq::Error::Status(408 | 504, _) => TransportKind::Timeout,
        ureq::Error::Status(502 | 503, _) => TransportKind::Unavailable,
        ureq::Error::Transport(error) => match error.kind() {
            ureq::ErrorKind::Dns
            | ureq::ErrorKind::ConnectionFailed
            | ureq::ErrorKind::ProxyConnect => TransportKind::Unavailable,
            ureq::ErrorKind::ProxyUnauthorized => TransportKind::Authentication,
            ureq::ErrorKind::InsecureRequestHttpsOnly => TransportKind::Tls,
            _ => TransportKind::Other,
        },
        _ => TransportKind::Other,
    }
}

fn server_version_request(request: ureq::Request) -> Result<Version, VersionError> {
    server_version_request_with_timeout(request, VERSION_PROBE_TIMEOUT)
}

fn server_version_request_with_timeout(
    request: ureq::Request,
    timeout: Duration,
) -> Result<Version, VersionError> {
    let response = request
        .timeout(timeout)
        .call()
        .map_err(|error| probe_unreachable(http_transport_kind(&error)))?;
    let mut response_body = Vec::with_capacity(MAX_VERSION_RESPONSE_BYTES + 1);
    response
        .into_reader()
        .take((MAX_VERSION_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut response_body)
        .map_err(|error| {
            probe_transport_failure(io_transport_kind(&error).unwrap_or(TransportKind::Other))
        })?;
    if response_body.len() > MAX_VERSION_RESPONSE_BYTES {
        return Err(VersionError::Probe(format!(
            "HTTP version response exceeds the {MAX_VERSION_RESPONSE_BYTES}-byte limit"
        )));
    }
    let response_body = std::str::from_utf8(&response_body)
        .map_err(|_| VersionError::Probe("HTTP version response is not valid UTF-8".to_owned()))?;

    parse_version_response(response_body).map_err(|e| match e {
        VersionError::Probe(_) => {
            VersionError::Probe("HTTP version response is invalid".to_owned())
        }
        other => other,
    })
}

/// Query the TypeDB HTTP API over plaintext HTTP.
///
/// This function never constructs a TLS client and is the only explicit probe
/// used by [`crate::version::server_version`] when its released `tls` argument
/// is `false`.
///
/// # Errors
///
/// Returns [`VersionProbeError::Probe`] for any network, HTTP, or response
/// parsing failure.
pub fn server_version_plaintext(
    address: &str,
    http_port: u16,
) -> Result<Version, VersionProbeError> {
    let url = version_endpoint(address, http_port, false);
    server_version_request(plaintext_agent().get(&url)).map_err(VersionProbeError::Probe)
}

/// Query the TypeDB HTTP API over HTTPS using native system trust roots.
///
/// Native roots are loaded eagerly, before the request is constructed.  This
/// enabled path does not retry through [`server_version_plaintext`].
///
/// # Errors
///
/// Returns [`VersionProbeError::TlsConfiguration`] if native roots cannot be
/// loaded, or [`VersionProbeError::Probe`] for network, HTTP, or response
/// parsing failures.
pub fn server_version_native_roots(
    address: &str,
    http_port: u16,
) -> Result<Version, VersionProbeError> {
    let agent = tls_agent(native_root_store()?)?;
    let url = version_endpoint(address, http_port, true);
    server_version_request(agent.get(&url)).map_err(VersionProbeError::Probe)
}

/// Query the TypeDB HTTP API over HTTPS using only a custom root CA bundle.
///
/// The PEM bundle is loaded and validated before a request is constructed.
/// This enabled path does not retry through [`server_version_plaintext`].
///
/// # Errors
///
/// Returns [`VersionProbeError::TlsConfiguration`] for an unreadable or
/// invalid root bundle, or [`VersionProbeError::Probe`] for network, HTTP, or
/// response parsing failures.
pub fn server_version_custom_root_ca(
    address: &str,
    http_port: u16,
    root_ca: &Path,
) -> Result<Version, VersionProbeError> {
    let material = RetainedCustomRootCa::load_configured_alias(root_ca)?;
    server_version_retained_custom_root_ca(address, http_port, &material)
}

/// Query the HTTPS version endpoint from already retained custom-root
/// material.
///
/// This hidden cross-crate seam prevents the secure runtime from reopening a
/// caller-controlled path after it has lowered the TypeDB driver bands.
#[doc(hidden)]
pub fn server_version_retained_custom_root_ca(
    address: &str,
    http_port: u16,
    material: &RetainedCustomRootCa,
) -> Result<Version, VersionProbeError> {
    let agent = tls_agent(material.root_store())?;
    let url = version_endpoint(address, http_port, true);
    server_version_request(agent.get(&url)).map_err(VersionProbeError::Probe)
}

/// Query the TypeDB HTTP API for the server version.
///
/// This is the released Boolean adapter.  `false` delegates to
/// [`server_version_plaintext`]; `true` delegates to
/// [`server_version_native_roots`].  New code that carries a typed TLS policy
/// should call the explicit function matching that policy.
///
/// # Errors
///
/// Returns [`VersionError::Probe`] for any TLS configuration, network, HTTP,
/// or parse failure.  The probe never silently returns a default.
pub fn server_version(address: &str, http_port: u16, tls: bool) -> Result<Version, VersionError> {
    let result = if tls {
        server_version_native_roots(address, http_port)
    } else {
        server_version_plaintext(address, http_port)
    };
    result.map_err(VersionProbeError::into_version_error)
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors produced by the version gate.
///
/// The `Display` impl produces human-readable messages that always name the
/// detected version(s) and the supported range or compatible-band remediation.
/// Protocol band numbers are **never** used alone in user-facing text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionError {
    /// One endpoint is below the minimum admitted version.
    ///
    /// Since 2.3.3 this existing variant is no longer produced for versions
    /// above the newest measured line; those are admitted with a warning.
    Unsupported {
        /// Which endpoint (`"driver"` or `"server"`).
        component: &'static str,
        /// The version that was detected.
        found: Version,
    },
    /// Both endpoints are in-window but speak different protocol bands.
    BandMismatch {
        /// The driver version.
        driver: Version,
        /// The server version.
        server: Version,
        /// Band of the driver.
        driver_band: u8,
        /// Band of the server.
        server_band: u8,
    },
    /// An in-window server whose protocol band is mapped, but this build of
    /// type-bridge did not embed a driver for that band.
    ///
    /// Only reachable in a non-default single-band build — the default build
    /// embeds every band, so any in-window server is served.
    EmbeddedUnavailable {
        /// The server version that has no embedded driver in this build.
        server: Version,
    },
    /// The HTTP probe failed (network, HTTP, or response-parse error).
    Probe(String),
    /// A version string could not be parsed.
    Parse(String),
    /// The server is in-window but predates a feature the schema requires.
    FeatureUnsupported {
        /// Human-readable feature name.
        feature: &'static str,
        /// The detected server version.
        server: Version,
        /// The minimum server version that supports the feature.
        required: Version,
        /// Feature-specific remediation appended to the error message.
        remediation: &'static str,
    },
}

/// Human-readable admission range used consistently in error messages.
/// Kept in step with [`MIN_SUPPORTED`] by a test; there is no upper refusal.
const WINDOW_HUMAN: &str = "3.11.0 or newer";

impl VersionError {
    /// Retain fixed HTTP probe diagnoses and locally authored version evidence.
    /// Unrecognized legacy probe strings may contain provider values.
    #[doc(hidden)]
    #[must_use]
    pub fn redacted_probe_description(&self) -> String {
        match self {
            Self::Probe(message) => {
                let fixed_transport = message
                    .strip_prefix("HTTP version request failed: ")
                    .or_else(|| {
                        message
                            .strip_prefix(UNREACHABLE_PREFIX)
                            .and_then(|rest| rest.strip_suffix(UNREACHABLE_GUIDANCE))
                    })
                    .and_then(TransportKind::from_description)
                    .is_some();
                if fixed_transport
                    || message
                        == &format!(
                            "HTTP version response exceeds the {MAX_VERSION_RESPONSE_BYTES}-byte limit"
                        )
                    || matches!(
                        message.as_str(),
                        "HTTP version response is not valid UTF-8"
                            | "HTTP version response is invalid"
                    )
                {
                    message.clone()
                } else {
                    format!(
                        "HTTP version request failed: {}",
                        TransportKind::Other.description()
                    )
                }
            }
            _ => self.to_string(),
        }
    }
}

impl fmt::Display for VersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VersionError::Unsupported { component, found } => write!(
                f,
                "{component} version {found} is outside the admitted range ({WINDOW_HUMAN}); \
                 install a TypeDB {component} in the range {WINDOW_HUMAN}"
            ),
            VersionError::BandMismatch {
                driver,
                server,
                driver_band: _,
                server_band: _,
            } => {
                // Name the server line without prescribing an interpreter-incompatible
                // Python wheel; this core diagnostic is shared by every language.
                let server_line = format!("{}.{}", server.major, server.minor);
                write!(
                    f,
                    "driver {driver} is not protocol-compatible with server {server}; \
                     install a driver line supported by your interpreter and accepted \
                     by the server (server line {server_line}), or use a compatible \
                     server/interpreter combination"
                )
            }
            VersionError::EmbeddedUnavailable { server } => write!(
                f,
                "server {server} requires an embedded driver this build of type-bridge \
                 does not include; rebuild with default features (or use a type-bridge \
                 release built for this server line)"
            ),
            VersionError::Probe(msg) => write!(f, "version probe failed: {msg}"),
            VersionError::Parse(msg) => write!(f, "version parse error: {msg}"),
            VersionError::FeatureUnsupported {
                feature: GENERATED_PACKAGE_FEATURE,
                server,
                remediation,
                ..
            } => write!(
                f,
                "generated package requires TypeDB {remediation}; detected server {server}"
            ),
            VersionError::FeatureUnsupported {
                feature,
                server,
                required,
                remediation,
            } => {
                let required_line = format!("{}.{}", required.major, required.minor);
                write!(
                    f,
                    "{feature} require TypeDB {required_line} or newer; detected server \
                     {server} — upgrade the server to the {required_line} line or \
                     {remediation}"
                )
            }
        }
    }
}

impl std::error::Error for VersionError {}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod transport_diagnostic_tests {
    use super::*;

    #[test]
    fn http_transport_sources_keep_only_fixed_kinds() {
        let cases = [
            (
                std::io::Error::new(std::io::ErrorKind::TimedOut, "SECRET_SENTINEL"),
                TransportKind::Timeout,
            ),
            (
                std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "SECRET_SENTINEL"),
                TransportKind::Unavailable,
            ),
            (
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    rustls::Error::General("SECRET_SENTINEL".to_owned()),
                ),
                TransportKind::Tls,
            ),
            (
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, "SECRET_SENTINEL"),
                TransportKind::Closed,
            ),
            (
                std::io::Error::other("SECRET_SENTINEL timeout TLS"),
                TransportKind::Other,
            ),
        ];
        for (error, expected) in cases {
            let error = ureq::Error::from(error);
            let kind = http_transport_kind(&error);
            assert_eq!(kind, expected);
            let retained = probe_transport_failure(kind);
            assert_eq!(
                retained.redacted_probe_description(),
                format!("HTTP version request failed: {}", kind.description())
            );
            assert!(!format!("{retained:?}").contains("SECRET_SENTINEL"));
            let unreachable = probe_unreachable(kind);
            assert_eq!(
                unreachable.redacted_probe_description(),
                format!(
                    "could not reach TypeDB HTTP version endpoint: {}; ensure the TypeDB HTTP \
                     endpoint is reachable and that the port is correct",
                    kind.description()
                )
            );
        }
        let unauthorized = ureq::Error::Status(
            401,
            ureq::Response::new(401, "SECRET_SENTINEL", "SECRET_SENTINEL").unwrap(),
        );
        assert_eq!(
            http_transport_kind(&unauthorized),
            TransportKind::Authentication
        );
        assert!(
            !probe_unreachable(http_transport_kind(&unauthorized))
                .to_string()
                .contains("SECRET_SENTINEL")
        );
    }

    #[test]
    fn local_version_evidence_survives_and_unrecognized_probe_values_do_not() {
        let local = VersionError::Unsupported {
            component: "server",
            found: Version::new(3, 99, 0),
        };
        assert_eq!(local.redacted_probe_description(), local.to_string());
        let remote = VersionError::Probe("SECRET_SENTINEL timeout TLS".to_owned());
        assert_eq!(
            remote.redacted_probe_description(),
            "HTTP version request failed: other transport"
        );
        let forged = VersionError::Probe(format!(
            "{UNREACHABLE_PREFIX}timeout SECRET_SENTINEL{UNREACHABLE_GUIDANCE}"
        ));
        assert_eq!(
            forged.redacted_probe_description(),
            "HTTP version request failed: other transport"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TLS_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    // -- HTTPS through the rustls connector ------------------------------------

    const TEST_ROOT: &[u8] = include_bytes!("../../../../tests/fixtures/tls/root-ca.pem");
    const TEST_SERVER_CERT: &[u8] =
        include_bytes!("../../../../tests/fixtures/tls/server-cert.pem");
    const TEST_SERVER_KEY: &[u8] = include_bytes!("../../../../tests/fixtures/tls/server-key.pem");

    fn pem_roots(pem: &[u8]) -> rustls::RootCertStore {
        let mut roots = rustls::RootCertStore::empty();
        for cert in rustls_pemfile::certs(&mut &pem[..]) {
            roots
                .add(cert.expect("fixture certificate"))
                .expect("fixture root");
        }
        roots
    }

    /// Serve one HTTPS request on loopback with the test PKI's server
    /// certificate (valid for `localhost` and `127.0.0.1`); answer `ok`.
    fn serve_one_https_request() -> (u16, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let certs = rustls_pemfile::certs(&mut &TEST_SERVER_CERT[..])
            .collect::<Result<Vec<_>, _>>()
            .expect("fixture server certificate");
        let key = rustls_pemfile::private_key(&mut &TEST_SERVER_KEY[..])
            .expect("fixture server key")
            .expect("one key");
        let config = rustls::ServerConfig::builder_with_provider(
            rustls::crypto::ring::default_provider().into(),
        )
        .with_safe_default_protocol_versions()
        .expect("server protocol versions")
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("server certificate");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("local address").port();
        let server = std::thread::spawn(move || {
            let (socket, _) = listener.accept().expect("accept");
            let connection = rustls::ServerConnection::new(Arc::new(config)).expect("session");
            let mut stream = rustls::StreamOwned::new(connection, socket);
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            // A client that refuses the certificate ends the handshake here.
            while !request.ends_with(b"\r\n\r\n") {
                match stream.read(&mut byte) {
                    Ok(1) => request.push(byte[0]),
                    _ => return,
                }
            }
            let _ = stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
            stream.conn.send_close_notify();
            let _ = stream.flush();
        });
        (port, server)
    }

    #[test]
    fn tls_agent_reaches_https_with_the_configured_root() {
        for host in ["localhost", "127.0.0.1"] {
            let (port, server) = serve_one_https_request();
            let agent = tls_agent(pem_roots(TEST_ROOT)).expect("TLS agent");
            let response = agent
                .get(&format!("https://{host}:{port}/"))
                .call()
                .expect("HTTPS request through the rustls connector");
            assert_eq!(response.into_string().expect("body"), "ok");
            server.join().expect("server thread");
        }
    }

    #[test]
    fn tls_agent_refuses_a_server_outside_the_configured_root() {
        let (port, server) = serve_one_https_request();
        // Another CA: the agent trusts only what it was given, never a
        // bundled or platform root.
        let other = include_bytes!("../tests/fixtures/valid-root.pem");
        let agent = tls_agent(pem_roots(other)).expect("TLS agent");
        let error = agent
            .get(&format!("https://localhost:{port}/"))
            .call()
            .expect_err("an untrusted server certificate is refused");
        assert_eq!(http_transport_kind(&error), TransportKind::Tls);
        server.join().expect("server thread");
    }

    // -- FromStr / Display ---------------------------------------------------

    #[test]
    fn parse_three_component() {
        let v: Version = "3.11.5".parse().unwrap();
        assert_eq!(v, Version::new(3, 11, 5));
    }

    #[test]
    fn parse_two_component_patch_zero() {
        let v: Version = "3.11".parse().unwrap();
        assert_eq!(v, Version::new(3, 11, 0));
    }

    #[test]
    fn parse_error_on_garbage() {
        assert!("not-a-version".parse::<Version>().is_err());
    }

    #[test]
    fn parse_error_on_prerelease() {
        // Strict — pre-release suffix is not accepted.
        assert!("3.12.1-alpha".parse::<Version>().is_err());
    }

    #[test]
    fn parse_error_on_single_component() {
        assert!("3".parse::<Version>().is_err());
    }

    #[test]
    fn display_round_trips() {
        let v = Version::new(3, 12, 1);
        assert_eq!(v.to_string(), "3.12.1");
        let v2 = Version::new(3, 11, 0);
        assert_eq!(v2.to_string(), "3.11.0");
    }

    // -- window_contains -----------------------------------------------------

    #[test]
    fn window_rejects_versions_below_3_11() {
        assert!(!window_contains(&Version::new(3, 8, 0)));
        assert!(!window_contains(&Version::new(3, 10, 99)));
    }

    #[test]
    fn window_accepts_3_11_0_floor() {
        assert!(window_contains(&Version::new(3, 11, 0)));
    }

    #[test]
    fn window_accepts_3_11_99() {
        assert!(window_contains(&Version::new(3, 11, 99)));
    }

    #[test]
    fn window_accepts_3_12_0() {
        assert!(window_contains(&Version::new(3, 12, 0)));
    }

    #[test]
    fn window_accepts_3_13_line() {
        assert!(window_contains(&Version::new(3, 13, 0)));
        assert!(window_contains(&Version::new(3, 13, 6)));
        assert!(window_contains(&Version::new(3, 13, 99)));
    }

    #[test]
    fn server_3_14_accepts_bands_9_and_8_and_shares_the_3_13_profile() {
        // Measured live: drivers 3.13.6, 3.14.0 (band 9) and 3.11.5 (band 8)
        // each completed connect, version, schema, write and fetch on 3.14.0.
        let server = Version::new(3, 14, 0);
        assert_eq!(server_accepted_bands(&server), &[9, 8]);
        assert_eq!(negotiate_server_band(&server, &[8, 9]), Some(9));
        assert_eq!(negotiate_server_band(&server, &[8]), Some(8));
        assert_eq!(
            semantic_profile_id(&server),
            Some(TYPEDB_3_13_SEMANTIC_PROFILE_ID)
        );
        check_server_supported(&server, &[8, 9]).expect("3.14 is in window");
    }

    #[test]
    fn newer_lines_use_the_newest_measured_profile_and_protocol() {
        let measured = Version::new(MAX_SUPPORTED_LINE.0, MAX_SUPPORTED_LINE.1, 0);
        assert!(!newer_than_measured(&Version::new(3, 14, 999)));
        for version in [Version::new(3, 15, 0), Version::new(4, 0, 0)] {
            assert!(window_contains(&version));
            assert!(newer_than_measured(&version));
            assert_eq!(
                semantic_profile_id(&version),
                semantic_profile_id(&measured)
            );
            assert_eq!(
                semantic_profile_id(&version),
                Some(TYPEDB_3_13_SEMANTIC_PROFILE_ID)
            );
            assert_eq!(band(&version), band(&measured));
            assert_eq!(
                server_accepted_bands(&version),
                server_accepted_bands(&measured)
            );
            assert_eq!(negotiate_server_band(&version, &[8, 9]), Some(9));
            check_server_supported(&version, &[8, 9]).unwrap();
            check_supported(&measured, &version).unwrap();
        }
    }

    #[test]
    fn window_rejects_2_9_0() {
        assert!(!window_contains(&Version::new(2, 9, 0)));
    }

    #[test]
    fn every_supported_patch_maps_to_its_frozen_semantic_profile() {
        for patch in [0, 1, 4, 5, 99] {
            assert_eq!(
                semantic_profile_id(&Version::new(3, 11, patch)),
                Some(TYPEDB_3_11_SEMANTIC_PROFILE_ID)
            );
            assert_eq!(
                semantic_profile_id(&Version::new(3, 12, patch)),
                Some(TYPEDB_3_12_SEMANTIC_PROFILE_ID)
            );
            assert_eq!(
                semantic_profile_id(&Version::new(3, 13, patch)),
                Some(TYPEDB_3_13_SEMANTIC_PROFILE_ID)
            );
        }
    }

    #[test]
    fn unsupported_lines_have_no_semantic_profile() {
        for version in [Version::new(2, 12, 1), Version::new(3, 10, 99)] {
            assert_eq!(semantic_profile_id(&version), None);
        }
    }

    #[test]
    fn generated_packages_admit_patch_floors_per_profile() {
        let servers =
            |profile| GeneratedPackageRelease::for_profile(profile).map(|r| r.servers().to_vec());
        assert_eq!(
            servers(TYPEDB_3_12_SEMANTIC_PROFILE_ID),
            Some(vec![Version::new(3, 12, 3)])
        );
        assert_eq!(
            servers(TYPEDB_3_13_SEMANTIC_PROFILE_ID),
            Some(vec![Version::new(3, 13, 6), Version::new(3, 14, 0)])
        );
        assert_eq!(servers(TYPEDB_3_11_SEMANTIC_PROFILE_ID), None);
        assert_eq!(servers("typedb-3.14.0/v1"), None);

        let r313 = GeneratedPackageRelease::for_profile(TYPEDB_3_13_SEMANTIC_PROFILE_ID).unwrap();
        assert_eq!(r313.server(), Version::new(3, 14, 0));
        for profile in [
            TYPEDB_3_12_SEMANTIC_PROFILE_ID,
            TYPEDB_3_13_SEMANTIC_PROFILE_ID,
        ] {
            let release = GeneratedPackageRelease::for_profile(profile).unwrap();
            let measured_floors = release
                .servers()
                .iter()
                .map(|floor| format!("{floor}+"))
                .collect::<Vec<_>>()
                .join(", ");
            let admitted = if profile == TYPEDB_3_13_SEMANTIC_PROFILE_ID {
                format!("{measured_floors}, and newer lines")
            } else {
                measured_floors
            };
            assert_eq!(release.to_string(), admitted);
            assert!(
                release
                    .unsupported_version(Version::new(3, 10, 0))
                    .to_string()
                    .contains(&admitted)
            );
        }
        assert!(r313.admits(&Version::new(3, 13, 6)));
        assert!(r313.admits(&Version::new(3, 14, 0)));
        assert!(!r313.admits(&Version::new(3, 13, 5)));
        assert!(r313.admits(&Version::new(3, 14, 1)));
        assert!(r313.admits(&Version::new(3, 13, 9)));
        assert!(r313.admits(&Version::new(3, 15, 0)));
        assert!(r313.admits(&Version::new(4, 0, 0)));
        assert!(!r313.admits(&Version::new(3, 11, 99)));
        let r312 = GeneratedPackageRelease::for_profile(TYPEDB_3_12_SEMANTIC_PROFILE_ID).unwrap();
        assert!(r312.admits(&Version::new(3, 12, 3)));
        assert!(r312.admits(&Version::new(3, 12, 7)));
        assert!(!r312.admits(&Version::new(3, 12, 2)));
        assert!(!r312.admits(&Version::new(3, 13, 9)));
        assert!(!r312.admits(&Version::new(3, 11, 99)));
        assert!(!r312.admits(&Version::new(3, 15, 0)));
        assert!(!r313.admits(&Version::new(3, 12, 3)));
        assert_eq!(r313.to_string(), "3.13.6+, 3.14.0+, and newer lines");
        assert_eq!(
            GeneratedPackageRelease::for_profile(TYPEDB_3_12_SEMANTIC_PROFILE_ID)
                .unwrap()
                .to_string(),
            "3.12.3+"
        );
    }

    // -- band ----------------------------------------------------------------

    #[test]
    fn retired_lines_have_no_active_band() {
        assert_eq!(band(&Version::new(3, 7, 0)), None);
        assert_eq!(band(&Version::new(3, 8, 1)), None);
        assert_eq!(band(&Version::new(3, 10, 4)), None);
    }

    #[test]
    fn band_3_11_is_8() {
        assert_eq!(band(&Version::new(3, 11, 5)), Some(8));
    }

    #[test]
    fn band_3_9_is_none() {
        // 3.9 was never released by TypeDB.
        assert_eq!(band(&Version::new(3, 9, 0)), None);
    }

    #[test]
    fn band_3_12_is_9() {
        assert_eq!(band(&Version::new(3, 12, 0)), Some(9));
    }

    #[test]
    fn band_3_13_is_9() {
        assert_eq!(band(&Version::new(3, 13, 0)), Some(9));
        assert_eq!(band(&Version::new(3, 13, 6)), Some(9));
    }

    #[test]
    fn band_3_14_and_newer_use_9() {
        assert_eq!(band(&Version::new(3, 14, 0)), Some(9));
        assert_eq!(band(&Version::new(3, 15, 0)), Some(9));
    }

    #[test]
    fn band_2_x_is_none() {
        assert_eq!(band(&Version::new(2, 28, 0)), None);
    }

    // -- server_accepted_bands -------------------------------------------------

    #[test]
    fn retired_server_lines_accept_no_active_band() {
        assert!(server_accepted_bands(&Version::new(3, 8, 3)).is_empty());
        assert!(server_accepted_bands(&Version::new(3, 10, 4)).is_empty());
    }

    #[test]
    fn server_3_11_accepts_band8_only() {
        assert_eq!(server_accepted_bands(&Version::new(3, 11, 5)), &[8]);
    }

    #[test]
    fn server_3_12_accepts_bands_9_and_8_native_first() {
        // Measured: server 3.12.0 completes full round trips with both a
        // 3.12.0 (band-9) and a 3.11.5 (band-8) driver.
        assert_eq!(server_accepted_bands(&Version::new(3, 12, 0)), &[9, 8]);
    }

    #[test]
    fn server_3_13_accepts_band9_only() {
        // Measured: drivers 3.12.3 and 3.13.6 round-trip against 3.13.6.
        // Band 8 is unmeasured on 3.13 and therefore not claimed.
        assert_eq!(server_accepted_bands(&Version::new(3, 13, 6)), &[9]);
    }

    #[test]
    fn newer_server_uses_the_measured_set_and_retired_servers_remain_unmapped() {
        assert_eq!(server_accepted_bands(&Version::new(3, 15, 0)), &[9, 8]);
        assert!(server_accepted_bands(&Version::new(2, 28, 0)).is_empty());
    }

    // -- negotiate_server_band -------------------------------------------------

    #[test]
    fn negotiate_3_12_without_band9_picks_band8() {
        // A reduced band-8 build uses the server's compatible fallback.
        assert_eq!(
            negotiate_server_band(&Version::new(3, 12, 0), &[8]),
            Some(8)
        );
    }

    #[test]
    fn negotiate_prefers_native_band_when_embedded() {
        // A build embedding band 9 connects to a 3.12 server natively.
        assert_eq!(
            negotiate_server_band(&Version::new(3, 12, 0), &[8, 9]),
            Some(9)
        );
    }

    #[test]
    fn negotiate_3_13_requires_band9() {
        assert_eq!(
            negotiate_server_band(&Version::new(3, 13, 6), &[8, 9]),
            Some(9)
        );
        assert_eq!(negotiate_server_band(&Version::new(3, 13, 6), &[8]), None);
    }

    #[test]
    fn negotiate_none_when_no_overlap() {
        assert_eq!(negotiate_server_band(&Version::new(3, 11, 5), &[9]), None);
    }

    // -- check_supported -----------------------------------------------------

    #[test]
    fn check_retired_server_rejects_as_unsupported() {
        let err = check_supported(&Version::new(3, 11, 5), &Version::new(3, 10, 4)).unwrap_err();
        assert!(
            matches!(
                err,
                VersionError::Unsupported {
                    component: "server",
                    ..
                }
            ),
            "expected Unsupported(server), got {err:?}"
        );
    }

    #[test]
    fn check_retired_driver_rejects_as_unsupported() {
        let err = check_supported(&Version::new(3, 10, 0), &Version::new(3, 11, 5)).unwrap_err();
        assert!(
            matches!(
                err,
                VersionError::Unsupported {
                    component: "driver",
                    ..
                }
            ),
            "expected Unsupported(driver), got {err:?}"
        );
    }

    #[test]
    fn check_driver_3115_server_3120_accept() {
        // Measured: server 3.12 retains band-8 compatibility.
        assert!(check_supported(&Version::new(3, 11, 5), &Version::new(3, 12, 0)).is_ok());
    }

    #[test]
    fn check_driver_3120_server_3120_accept() {
        assert!(check_supported(&Version::new(3, 12, 0), &Version::new(3, 12, 0)).is_ok());
    }

    #[test]
    fn check_driver_3120_server_3115_reject_band_mismatch() {
        // Measured: a band-9 driver is rejected by a 3.11 server at connect —
        // the asymmetric direction the accepted-set model exists for.
        let err = check_supported(&Version::new(3, 12, 0), &Version::new(3, 11, 5)).unwrap_err();
        assert!(
            matches!(err, VersionError::BandMismatch { .. }),
            "expected BandMismatch, got {err:?}"
        );
    }

    #[test]
    fn check_band9_drivers_and_3_12_3_13_servers_accept_each_other() {
        // Measured: drivers 3.12.3 and 3.13.6 against servers 3.12.3 and 3.13.6.
        for driver in [Version::new(3, 12, 3), Version::new(3, 13, 6)] {
            for server in [Version::new(3, 12, 3), Version::new(3, 13, 6)] {
                assert!(
                    check_supported(&driver, &server).is_ok(),
                    "{driver} x {server}"
                );
            }
        }
    }

    #[test]
    fn check_driver_3115_server_3136_reject_band_mismatch() {
        let err = check_supported(&Version::new(3, 11, 5), &Version::new(3, 13, 6)).unwrap_err();
        assert!(
            matches!(err, VersionError::BandMismatch { .. }),
            "expected BandMismatch, got {err:?}"
        );
    }

    #[test]
    fn check_driver_3136_server_3115_reject_band_mismatch() {
        let err = check_supported(&Version::new(3, 13, 6), &Version::new(3, 11, 5)).unwrap_err();
        assert!(
            matches!(err, VersionError::BandMismatch { .. }),
            "expected BandMismatch, got {err:?}"
        );
    }

    #[test]
    fn newer_driver_uses_the_newest_measured_band() {
        check_supported(&Version::new(3, 15, 0), &Version::new(3, 13, 6)).unwrap();
        check_supported(&Version::new(4, 0, 0), &Version::new(3, 13, 6)).unwrap();
    }

    #[test]
    fn check_driver_370_server_373_reject_window() {
        // 3.7.x is band-A-compatible but below the floor — window check fires.
        let err = check_supported(&Version::new(3, 7, 0), &Version::new(3, 7, 3)).unwrap_err();
        assert!(
            matches!(
                err,
                VersionError::Unsupported {
                    component: "driver",
                    ..
                }
            ),
            "expected Unsupported(driver), got {err:?}"
        );
    }

    #[test]
    fn check_server_2_28_reject_window() {
        let err = check_supported(&Version::new(3, 11, 5), &Version::new(2, 28, 0)).unwrap_err();
        assert!(
            matches!(
                err,
                VersionError::Unsupported {
                    component: "server",
                    ..
                }
            ),
            "expected Unsupported(server), got {err:?}"
        );
    }

    // -- check_server_supported ----------------------------------------------

    #[test]
    fn server_3115_retained_bands_accept() {
        assert!(check_server_supported(&Version::new(3, 11, 5), &[8, 9]).is_ok());
    }

    #[test]
    fn server_373_reject_window() {
        // Below the floor — window class, never a band/embedded message.
        let err = check_server_supported(&Version::new(3, 7, 3), &[8, 9]).unwrap_err();
        assert!(
            matches!(
                err,
                VersionError::Unsupported {
                    component: "server",
                    ..
                }
            ),
            "expected Unsupported(server), got {err:?}"
        );
        assert!(
            err.to_string().contains("outside the admitted range"),
            "expected window message, got {err}"
        );
    }

    #[test]
    fn server_3120_both_bands_accept() {
        // Dual-band server: accepts [9, 8]; the default build's band-8 driver
        // serves it (measured live).
        assert!(check_server_supported(&Version::new(3, 12, 0), &[8, 9]).is_ok());
    }

    #[test]
    fn server_3120_without_retained_overlap_is_unavailable() {
        let err = check_server_supported(&Version::new(3, 12, 0), &[1]).unwrap_err();
        assert!(
            matches!(err, VersionError::EmbeddedUnavailable { .. }),
            "expected EmbeddedUnavailable, got {err:?}"
        );
    }

    #[test]
    fn server_3136_band9_accepts() {
        assert!(check_server_supported(&Version::new(3, 13, 0), &[8, 9]).is_ok());
        assert!(check_server_supported(&Version::new(3, 13, 6), &[9]).is_ok());
    }

    #[test]
    fn server_3136_band8_only_build_is_unavailable() {
        let err = check_server_supported(&Version::new(3, 13, 6), &[8]).unwrap_err();
        assert!(
            matches!(err, VersionError::EmbeddedUnavailable { .. }),
            "expected EmbeddedUnavailable, got {err:?}"
        );
    }

    #[test]
    fn newer_servers_are_admitted_with_measured_protocol_assumptions() {
        check_server_supported(&Version::new(3, 15, 0), &[8, 9]).unwrap();
        check_server_supported(&Version::new(4, 0, 0), &[8, 9]).unwrap();
    }

    #[test]
    fn retired_server_is_unsupported_even_with_retained_bands() {
        let err = check_server_supported(&Version::new(3, 10, 4), &[8, 9]).unwrap_err();
        assert!(
            matches!(
                err,
                VersionError::Unsupported {
                    component: "server",
                    ..
                }
            ),
            "expected Unsupported(server), got {err:?}"
        );
    }

    #[test]
    fn server_3115_band9_only_unavailable() {
        let err = check_server_supported(&Version::new(3, 11, 5), &[9]).unwrap_err();
        assert!(
            matches!(err, VersionError::EmbeddedUnavailable { .. }),
            "expected EmbeddedUnavailable, got {err:?}"
        );
    }

    #[test]
    fn embedded_unavailable_message_names_server_no_forbidden_tokens() {
        let err = VersionError::EmbeddedUnavailable {
            server: Version::new(3, 11, 5),
        };
        let msg = err.to_string();
        assert!(msg.contains("3.11.5"), "missing server version: {msg}");
        // The forbidden tokens must never appear in any gate message.
        for forbidden in ["band 8", "band 9", "0.0.0"] {
            assert!(
                !msg.contains(forbidden),
                "embedded-unavailable message leaked forbidden token {forbidden:?}: {msg}"
            );
        }
    }

    #[test]
    fn check_server_supported_window_reject_no_forbidden_tokens() {
        // The window-class rejection from the embedded gate must also be clean.
        let err = check_server_supported(&Version::new(3, 7, 3), &[8, 9]).unwrap_err();
        let msg = err.to_string();
        for forbidden in ["band 8", "band 9", "0.0.0"] {
            assert!(
                !msg.contains(forbidden),
                "window rejection leaked forbidden token {forbidden:?}: {msg}"
            );
        }
    }

    // -- error Display -------------------------------------------------------

    #[test]
    fn unsupported_message_contains_version_and_window() {
        let err = VersionError::Unsupported {
            component: "server",
            found: Version::new(2, 28, 0),
        };
        let msg = err.to_string();
        assert!(msg.contains("2.28.0"), "missing detected version: {msg}");
        assert!(msg.contains("3.11.0"), "missing window floor: {msg}");
        assert!(
            msg.contains("or newer"),
            "unexpected admission ceiling: {msg}"
        );
    }

    #[test]
    fn window_human_tracks_the_admission_floor() {
        assert_eq!(WINDOW_HUMAN, format!("{MIN_SUPPORTED} or newer"));
    }

    #[test]
    fn band_mismatch_message_contains_versions_and_remediation() {
        let err = VersionError::BandMismatch {
            driver: Version::new(3, 12, 0),
            server: Version::new(3, 11, 5),
            driver_band: 9,
            server_band: 8,
        };
        let msg = err.to_string();
        assert!(msg.contains("3.12.0"), "missing driver version: {msg}");
        assert!(msg.contains("3.11.5"), "missing server version: {msg}");
        // Remediation must mention the server line.
        assert!(
            msg.contains("3.11"),
            "missing server line in remediation: {msg}"
        );
        // Must not expose bare band numbers as the primary explanation.
        assert!(
            !msg.starts_with("band"),
            "message starts with band number: {msg}"
        );
    }

    #[test]
    fn band9_to_band8_mismatch_does_not_prescribe_an_unavailable_driver() {
        let err = VersionError::BandMismatch {
            driver: Version::new(3, 12, 0),
            server: Version::new(3, 11, 5),
            driver_band: 9,
            server_band: 8,
        };
        let msg = err.to_string();
        assert!(msg.contains("3.12.0") && msg.contains("3.11.5"));
        assert!(!msg.contains("~3.11"), "unsafe driver prescription: {msg}");
        assert!(msg.contains("interpreter") && msg.contains("server"));
    }

    // -- version_endpoint ----------------------------------------------------

    #[test]
    fn endpoint_default_port() {
        assert_eq!(
            version_endpoint("localhost", 8000, false),
            "http://localhost:8000/v1/version"
        );
    }

    #[test]
    fn endpoint_port_override() {
        assert_eq!(
            version_endpoint("localhost", 9090, false),
            "http://localhost:9090/v1/version"
        );
    }

    #[test]
    fn endpoint_https() {
        assert_eq!(
            version_endpoint("db.example.com", 8000, true),
            "https://db.example.com:8000/v1/version"
        );
    }

    #[test]
    fn endpoint_strips_grpc_port() {
        // Typical gRPC address "host:1729" — port discarded, HTTP port used.
        assert_eq!(
            version_endpoint("myhost:1729", 8000, false),
            "http://myhost:8000/v1/version"
        );
    }

    #[test]
    fn endpoint_bare_host() {
        assert_eq!(
            version_endpoint("192.168.1.10", 8000, false),
            "http://192.168.1.10:8000/v1/version"
        );
    }

    #[test]
    fn endpoint_strips_scheme_prefix() {
        assert_eq!(
            version_endpoint("typedb://myhost:1729", 8000, false),
            "http://myhost:8000/v1/version"
        );
    }

    fn spawn_chunked_version_endpoint(chunks: Vec<Vec<u8>>) -> (u16, std::thread::JoinHandle<()>) {
        let listener =
            std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind a local version endpoint");
        let port = listener
            .local_addr()
            .expect("read the local version endpoint address")
            .port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept a version request");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("bound local version request reads");
            let mut request = Vec::with_capacity(1024);
            let mut buffer = [0_u8; 512];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).expect("read version request");
                assert_ne!(count, 0, "version request ended before its headers");
                request.extend_from_slice(&buffer[..count]);
                assert!(
                    request.len() <= 16 * 1024,
                    "version request headers exceeded the test ceiling"
                );
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\n\
                      Content-Type: application/json\r\n\
                      Transfer-Encoding: chunked\r\n\
                      Connection: close\r\n\r\n",
                )
                .expect("write version response headers");
            for chunk in chunks {
                write!(stream, "{:x}\r\n", chunk.len()).expect("write chunk length");
                stream.write_all(&chunk).expect("write response chunk");
                stream.write_all(b"\r\n").expect("terminate response chunk");
            }
            stream
                .write_all(b"0\r\n\r\n")
                .expect("terminate chunked response");
        });
        (port, server)
    }

    #[test]
    fn chunked_version_response_within_the_frozen_bound_is_accepted() {
        let body = br#"{"distribution":"TypeDB CE","version":"3.12.1"}"#;
        let chunks = body.chunks(7).map(<[u8]>::to_vec).collect();
        let (port, server) = spawn_chunked_version_endpoint(chunks);

        let version = server_version_plaintext("127.0.0.1:1729", port)
            .expect("a bounded chunked version response is accepted");
        server.join().expect("local version endpoint completes");
        assert_eq!(version, Version::new(3, 12, 1));
    }

    #[test]
    fn oversized_chunked_version_response_is_rejected_at_cap_plus_one() {
        const BODY_SENTINEL: &[u8] = b"private-provider-body-sentinel";
        let mut body = Vec::with_capacity(MAX_VERSION_RESPONSE_BYTES + 1);
        while body.len() <= MAX_VERSION_RESPONSE_BYTES {
            body.extend_from_slice(BODY_SENTINEL);
        }
        body.truncate(MAX_VERSION_RESPONSE_BYTES + 1);
        let chunks = body.chunks(127).map(<[u8]>::to_vec).collect();
        let (port, server) = spawn_chunked_version_endpoint(chunks);

        let error = server_version_plaintext("127.0.0.1:1729", port)
            .expect_err("an oversized chunked response is rejected");
        server.join().expect("local version endpoint completes");
        let message = error.to_string();
        assert!(
            message.contains("4096-byte limit"),
            "bounded failure is stable: {message}"
        );
        assert!(
            !message.contains(std::str::from_utf8(BODY_SENTINEL).unwrap()),
            "response contents must not enter the probe diagnostic: {message}"
        );
    }

    #[test]
    fn version_probe_applies_an_overall_timeout_to_response_reads() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
            .expect("bind a stalled local version endpoint");
        let port = listener
            .local_addr()
            .expect("read stalled version endpoint address")
            .port();
        let server = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().expect("accept a stalled version request");
            std::thread::sleep(Duration::from_millis(250));
        });
        let url = format!("http://127.0.0.1:{port}/v1/version");
        let started = std::time::Instant::now();

        let error = server_version_request_with_timeout(ureq::get(&url), Duration::from_millis(25))
            .expect_err("the response read must honor the overall request timeout");
        let elapsed = started.elapsed();
        server
            .join()
            .expect("stalled local version endpoint completes");
        assert!(
            matches!(error, VersionError::Probe(_)),
            "the timeout stays in the stable probe category: {error:?}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "the bounded test request did not time out promptly: {elapsed:?}"
        );
    }

    #[test]
    fn plaintext_version_probe_rejects_redirected_authority() {
        let target =
            std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind a redirect target endpoint");
        target
            .set_nonblocking(true)
            .expect("make redirect target observation nonblocking");
        let target_port = target
            .local_addr()
            .expect("read redirect target address")
            .port();
        let redirect = std::net::TcpListener::bind(("127.0.0.1", 0))
            .expect("bind a redirecting version endpoint");
        let redirect_port = redirect
            .local_addr()
            .expect("read redirecting version endpoint address")
            .port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = redirect.accept().expect("accept redirected request");
            let response = format!(
                "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{target_port}/v1/version\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream
                .write_all(response.as_bytes())
                .expect("write redirect response");
        });

        let error = server_version_plaintext("127.0.0.1:1729", redirect_port)
            .expect_err("a redirect must not authorize the original provider endpoint");
        server
            .join()
            .expect("redirecting version endpoint completes");
        assert!(
            matches!(error, VersionProbeError::Probe(VersionError::Probe(_))),
            "the redirect stays in the stable probe category: {error:?}"
        );
        assert!(
            matches!(target.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
            "the version probe followed a redirect to another authority"
        );
    }

    #[test]
    fn native_roots_keep_usable_certificates_when_other_entries_fail() {
        let valid = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/valid-root.pem");
        let temporary = tempfile::tempdir().expect("create native-root test directory");
        let missing_directory = temporary.path().join("missing");
        let loaded = rustls_native_certs::load_certs_from_paths(
            Some(valid.as_path()),
            Some(missing_directory.as_path()),
        );
        assert!(
            !loaded.certs.is_empty(),
            "fixture must provide a certificate"
        );
        assert!(
            !loaded.errors.is_empty(),
            "missing directory must provide a deterministic load error"
        );

        let roots = native_root_store_from_loaded(loaded)
            .expect("usable roots must survive unrelated load errors");
        assert!(!roots.is_empty());
    }

    #[test]
    fn native_roots_fail_when_no_loaded_certificate_is_usable() {
        let mut loaded = rustls_native_certs::CertificateResult::default();
        loaded
            .certs
            .push(rustls::pki_types::CertificateDer::from(vec![0_u8; 8]));

        assert!(matches!(
            native_root_store_from_loaded(loaded),
            Err(TlsConfigurationError::NativeRootsUnavailable)
        ));
    }

    #[test]
    fn custom_root_validation_reports_stable_pre_io_errors() {
        let missing =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/does-not-exist.pem");
        let missing_error = validate_custom_root_ca(&missing).unwrap_err();
        assert_eq!(missing_error.code(), "tls_custom_root_ca_unreadable");
        assert!(matches!(
            missing_error,
            TlsConfigurationError::CustomRootCaUnreadable { path } if path == missing
        ));

        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let directory_error = validate_custom_root_ca(&directory).unwrap_err();
        assert_eq!(directory_error.code(), "tls_custom_root_ca_not_file");
        assert!(matches!(
            directory_error,
            TlsConfigurationError::CustomRootCaNotFile { path } if path == directory
        ));

        let invalid =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/invalid-root.pem");
        let invalid_error = validate_custom_root_ca(&invalid).unwrap_err();
        assert_eq!(invalid_error.code(), "tls_custom_root_ca_invalid_pem");
        assert!(matches!(
            invalid_error,
            TlsConfigurationError::CustomRootCaInvalidPem { path } if path == invalid
        ));

        // Test thread names are `::`-separated paths, and `:` is not a legal
        // filename character on Windows.
        let discriminator = std::thread::current()
            .name()
            .unwrap_or("test")
            .replace(':', "-");
        let oversized = std::env::temp_dir().join(format!(
            "type-bridge-oversized-root-{}-{discriminator}.pem",
            std::process::id(),
        ));
        let file = std::fs::File::create(&oversized).expect("create oversized root fixture");
        file.set_len(MAX_CUSTOM_ROOT_CA_BYTES + 1)
            .expect("extend oversized root fixture");
        // Close the writable fixture handle before validating: the loader
        // opens the root CA denying concurrent writers, so a live writer
        // handle on Windows is a sharing violation, not a size failure.
        drop(file);
        let oversized_error = validate_custom_root_ca(&oversized).unwrap_err();
        std::fs::remove_file(&oversized).expect("remove oversized root fixture");
        assert_eq!(oversized_error.code(), "tls_custom_root_ca_too_large");
        assert!(matches!(
            oversized_error,
            TlsConfigurationError::CustomRootCaTooLarge { path } if path == oversized
        ));
    }

    #[test]
    fn physical_loader_classifies_a_directory_as_not_file() {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .canonicalize()
            .expect("resolve physical crate directory");
        let error = match RetainedCustomRootCa::load(&directory) {
            Ok(_) => panic!("a directory must not load as a root CA"),
            Err(error) => error,
        };
        assert_eq!(error.code(), "tls_custom_root_ca_not_file");
        assert!(matches!(
            error,
            TlsConfigurationError::CustomRootCaNotFile { path } if path == directory
        ));
    }

    #[test]
    fn custom_root_parser_rejects_junk_and_unknown_pem_sections() {
        let path = Path::new("configured-root.pem");
        let valid = include_bytes!("../tests/fixtures/valid-root.pem");
        let cases = [
            [b"leading junk\n".as_slice(), valid].concat(),
            [valid.as_slice(), b"\ntrailing junk\n"].concat(),
            [
                b"-----BEGIN TYPEBRIDGE UNKNOWN-----\nAA==\n-----END TYPEBRIDGE UNKNOWN-----\n"
                    .as_slice(),
                valid,
            ]
            .concat(),
        ];

        for bytes in cases {
            assert!(matches!(
                parse_custom_root_store(path, &bytes),
                Err(TlsConfigurationError::CustomRootCaInvalidPem { path: rejected })
                    if rejected == path
            ));
        }

        let padded = [b" \t\r\n".as_slice(), valid.as_slice(), b"\n\r\t "].concat();
        assert!(parse_custom_root_store(path, &padded).is_ok());
    }

    #[test]
    fn custom_root_probe_rejects_material_before_network_io() {
        let missing =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/does-not-exist.pem");
        let error = server_version_custom_root_ca(
            "invalid host that must not be contacted",
            8000,
            &missing,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            VersionProbeError::TlsConfiguration(
                TlsConfigurationError::CustomRootCaUnreadable { path }
            ) if path == missing
        ));
    }

    #[test]
    fn retained_custom_root_survives_configured_file_replacement() {
        let sequence = NEXT_TLS_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "type-bridge-core-root-replacement-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("create custom-root replacement directory");
        // load() requires an already-physical path; the ambient temp
        // directory may sit behind symlinked components (macOS /var).
        let directory = directory
            .canonicalize()
            .expect("resolve physical replacement directory");
        let configured = directory.join("configured.pem");
        let moved = directory.join("loaded.pem");
        let valid_root = include_bytes!("../tests/fixtures/valid-root.pem");
        std::fs::write(&configured, valid_root).expect("write original custom root");

        let material = RetainedCustomRootCa::load(&configured).expect("load original custom root");
        match std::fs::rename(&configured, &moved) {
            Ok(()) => std::fs::write(&configured, b"replacement is not a certificate\n")
                .expect("install replacement custom root"),
            Err(_) => {
                // Windows pins the source path with no-delete and read-only
                // sharing. Replacement and in-place mutation must both fail.
                assert!(
                    std::fs::write(&configured, b"replacement is not a certificate\n").is_err()
                );
            }
        }

        let driver_bytes = material
            .with_driver_root_path(|path| std::fs::read(path))
            .expect("derive retained driver path")
            .expect("read retained driver path");
        assert_eq!(driver_bytes, valid_root);
        assert_eq!(material.captured_bytes(), valid_root);
        assert!(!material.root_store().is_empty());
        tls_agent(material.root_store()).expect("HTTP TLS lowers captured original roots");

        drop(material);
        std::fs::remove_dir_all(&directory).expect("remove replacement directory");
    }

    #[test]
    fn retained_custom_root_survives_parent_directory_swap() {
        let sequence = NEXT_TLS_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "type-bridge-core-root-parent-swap-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&root).expect("create parent-swap root");
        // load() requires an already-physical path; the ambient temp
        // directory may sit behind symlinked components (macOS /var).
        let root = root
            .canonicalize()
            .expect("resolve physical parent-swap root");
        let configured_parent = root.join("configured-parent");
        let moved_parent = root.join("loaded-parent");
        std::fs::create_dir(&configured_parent).expect("create configured parent");
        let configured = configured_parent.join("root.pem");
        let valid_root = include_bytes!("../tests/fixtures/valid-root.pem");
        std::fs::write(&configured, valid_root).expect("write original custom root");

        let material = RetainedCustomRootCa::load(&configured).expect("load original custom root");
        match std::fs::rename(&configured_parent, &moved_parent) {
            Ok(()) => {
                std::fs::create_dir(&configured_parent).expect("install replacement parent");
                std::fs::write(&configured, b"replacement is not a certificate\n")
                    .expect("install replacement parent custom root");
            }
            Err(_) => {
                // Windows' retained parent directory handle denies the swap.
                assert_eq!(std::fs::read(&configured).unwrap(), valid_root);
            }
        }

        let driver_bytes = material
            .with_driver_root_path(|path| std::fs::read(path))
            .expect("derive retained driver path")
            .expect("read retained driver path");
        assert_eq!(driver_bytes, valid_root);
        assert_eq!(material.captured_bytes(), valid_root);
        assert!(!material.root_store().is_empty());
        tls_agent(material.root_store()).expect("HTTP TLS lowers captured original roots");

        drop(material);
        std::fs::remove_dir_all(&root).expect("remove parent-swap directory");
    }

    #[test]
    fn retained_custom_root_snapshot_survives_in_place_source_overwrite() {
        let sequence = NEXT_TLS_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "type-bridge-core-root-overwrite-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("create custom-root overwrite directory");
        // load() requires an already-physical path; the ambient temp
        // directory may sit behind symlinked components (macOS /var).
        let directory = directory
            .canonicalize()
            .expect("resolve physical overwrite directory");
        let configured = directory.join("configured.pem");
        let valid_root = include_bytes!("../tests/fixtures/valid-root.pem");
        std::fs::write(&configured, valid_root).expect("write original custom root");

        let material = RetainedCustomRootCa::load(&configured).expect("load original custom root");
        match std::fs::write(&configured, b"overwritten source is not a certificate\n") {
            Ok(()) => assert_ne!(std::fs::read(&configured).unwrap(), valid_root),
            Err(_) => {
                // Windows' retained source handle denies a writer rather than
                // allowing the source inode to diverge after preparation.
                assert_eq!(std::fs::read(&configured).unwrap(), valid_root);
            }
        }

        let driver_bytes = material
            .with_driver_root_path(|path| std::fs::read(path))
            .expect("derive retained snapshot driver path")
            .expect("read retained snapshot driver path");
        assert_eq!(driver_bytes, valid_root);
        assert_eq!(material.captured_bytes(), valid_root);
        tls_agent(material.root_store()).expect("HTTP TLS lowers captured original roots");

        drop(material);
        std::fs::remove_dir_all(&directory).expect("remove overwrite directory");
    }

    #[cfg(unix)]
    #[test]
    fn named_custom_root_snapshot_path_rereads_independently_of_the_handle() {
        use std::io::Read as _;

        let valid_root = include_bytes!("../tests/fixtures/valid-root.pem");
        let (mut snapshot, snapshot_path) =
            create_named_custom_root_snapshot(Path::new("/configured/root.pem"), valid_root)
                .expect("create named custom-root snapshot");

        // Exhaust the retained handle the way an eager driver read does; a
        // dup-semantics path (macOS /dev/fd) would then read back empty.
        let mut consumed = Vec::new();
        snapshot
            .read_to_end(&mut consumed)
            .expect("consume the retained snapshot handle");
        assert_eq!(consumed, valid_root);
        assert_eq!(
            std::fs::read(&snapshot_path).expect("first independent path read"),
            valid_root
        );
        assert_eq!(
            std::fs::read(&snapshot_path).expect("second independent path read"),
            valid_root
        );

        drop(snapshot);
        std::fs::remove_file(&snapshot_path).expect("remove named snapshot");
        std::fs::remove_dir(snapshot_path.parent().expect("snapshot parent"))
            .expect("remove named snapshot directory");
    }

    #[cfg(unix)]
    #[test]
    fn configured_alias_retains_the_open_target_after_final_name_replacement() {
        use std::os::unix::fs::symlink;

        let sequence = NEXT_TLS_TEMP_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "type-bridge-core-root-swap-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("create custom-root swap directory");
        let configured = directory.join("configured.pem");
        let replacement = directory.join("replacement.pem");
        let valid_root = include_bytes!("../tests/fixtures/valid-root.pem");
        std::fs::write(&configured, valid_root).expect("write initially resolved root");
        std::fs::write(&replacement, b"replacement is not a certificate")
            .expect("write replacement root");
        let material =
            RetainedCustomRootCa::load_configured_alias_with_after_open(&configured, || {
                std::fs::remove_file(&configured).expect("remove root after opening it");
                symlink(&replacement, &configured).expect("replace root name with a symlink");
            })
            .expect("the already-opened CA identity remains authoritative");

        assert_eq!(material.captured_bytes(), valid_root);
        drop(material);
        std::fs::remove_dir_all(&directory).expect("remove custom-root swap directory");
    }

    #[cfg(unix)]
    #[test]
    fn configured_alias_retains_the_open_target_after_parent_replacement() {
        let directory = tempfile::tempdir().expect("create parent-swap test directory");
        let active = directory.path().join("active");
        let retained = directory.path().join("retained");
        std::fs::create_dir(&active).expect("create active CA parent");
        let configured = active.join("root.pem");
        let valid_root = include_bytes!("../tests/fixtures/valid-root.pem");
        std::fs::write(&configured, valid_root).expect("write original root");

        let material =
            RetainedCustomRootCa::load_configured_alias_with_after_open(&configured, || {
                std::fs::rename(&active, &retained).expect("retain opened CA parent");
                std::fs::create_dir(&active).expect("create replacement CA parent");
                std::fs::write(active.join("root.pem"), b"replacement is not a certificate")
                    .expect("write replacement root");
            })
            .expect("the already-opened CA identity remains authoritative");

        assert_eq!(material.captured_bytes(), valid_root);
    }

    #[cfg(unix)]
    #[test]
    fn custom_root_accepts_a_caller_alias_but_reports_the_original_path() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("create parent-symlink test directory");
        let actual_parent = directory.path().join("actual");
        let linked_parent = directory.path().join("linked");
        std::fs::create_dir(&actual_parent).expect("create actual CA parent");
        let configured = linked_parent.join("root.pem");
        std::fs::write(actual_parent.join("root.pem"), b"not a certificate")
            .expect("write invalid CA behind parent symlink");
        symlink(&actual_parent, &linked_parent).expect("create parent symlink");

        let error = RetainedCustomRootCa::load_configured_alias(&configured).unwrap_err();
        assert!(matches!(
            error,
            TlsConfigurationError::CustomRootCaInvalidPem { path } if path == configured
        ));

        std::fs::write(
            actual_parent.join("root.pem"),
            include_bytes!("../tests/fixtures/valid-root.pem"),
        )
        .expect("replace alias target with a valid CA");
        RetainedCustomRootCa::load_configured_alias(&configured)
            .expect("caller path alias resolves once and validates");
    }

    #[cfg(unix)]
    #[test]
    fn physical_custom_root_rejects_symlink_components() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("create physical-path test directory");
        let actual_parent = directory.path().join("actual");
        let linked_parent = directory.path().join("linked");
        std::fs::create_dir(&actual_parent).expect("create actual CA parent");
        let target = actual_parent.join("root.pem");
        std::fs::write(&target, include_bytes!("../tests/fixtures/valid-root.pem"))
            .expect("write physical CA");
        symlink(&actual_parent, &linked_parent).expect("create parent alias");

        let through_parent_alias = linked_parent.join("root.pem");
        assert!(matches!(
            RetainedCustomRootCa::load(&through_parent_alias),
            Err(TlsConfigurationError::CustomRootCaUnreadable { path })
                if path == through_parent_alias
        ));

        let final_alias = actual_parent.join("root-alias.pem");
        symlink(&target, &final_alias).expect("create final-component alias");
        assert!(matches!(
            RetainedCustomRootCa::load(&final_alias),
            Err(TlsConfigurationError::CustomRootCaUnreadable { path }) if path == final_alias
        ));
    }

    #[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
    #[test]
    fn custom_root_fifo_is_rejected_within_a_bounded_deadline() {
        use std::sync::mpsc::{self, RecvTimeoutError};
        use std::time::Duration;

        let directory = tempfile::tempdir().expect("create FIFO test directory");
        let configured = directory.path().join("root.pem");
        rustix::fs::mkfifoat(
            rustix::fs::CWD,
            configured.as_path(),
            rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        )
        .expect("create root-CA FIFO");

        let worker_path = configured.clone();
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = sender.send(validate_custom_root_ca(&worker_path));
        });
        let result = match receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => {
                // Release an implementation that accidentally performed a
                // blocking FIFO open so the test can join before failing.
                let _writer = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&configured)
                    .expect("open FIFO writer to release blocked reader");
                worker.join().expect("join released FIFO reader");
                panic!("custom-root validation blocked while opening a FIFO");
            }
            Err(RecvTimeoutError::Disconnected) => {
                worker.join().expect("surface FIFO validation panic");
                panic!("FIFO validation worker disconnected without a result");
            }
        };
        worker.join().expect("join FIFO validation worker");

        assert!(matches!(
            result,
            Err(TlsConfigurationError::CustomRootCaNotFile { path }) if path == configured
        ));
    }

    // -- parse_version_response ----------------------------------------------

    #[test]
    fn parse_response_ok() {
        let json = r#"{"distribution":"TypeDB CE","version":"3.12.1"}"#;
        let v = parse_version_response(json).unwrap();
        assert_eq!(v, Version::new(3, 12, 1));
    }

    #[test]
    fn parse_response_missing_version_field() {
        let json = r#"{"distribution":"TypeDB CE"}"#;
        assert!(parse_version_response(json).is_err());
    }

    #[test]
    fn parse_response_invalid_json() {
        assert!(parse_version_response("not json").is_err());
    }

    #[test]
    fn parse_response_bad_version_string() {
        let json = r#"{"version":"latest"}"#;
        assert!(parse_version_response(json).is_err());
    }

    // -- feature gate ----------------------------------------------------------

    #[test]
    fn feature_gate_rejects_pre_312_server() {
        let server = Version::new(3, 11, 5);
        let err = check_feature_supported(Feature::SchemaAnnotations, &server).unwrap_err();
        match &err {
            VersionError::FeatureUnsupported {
                feature,
                server,
                required,
                remediation,
            } => {
                assert_eq!(*feature, "schema annotations (@doc/@meta)");
                assert_eq!(*server, Version::new(3, 11, 5));
                assert_eq!(*required, Version::new(3, 12, 0));
                assert_eq!(*remediation, "remove the annotations from the schema");
            }
            other => panic!("expected FeatureUnsupported, got {other:?}"),
        }
        let message = err.to_string();
        assert!(message.contains("3.12"));
        assert!(message.contains("3.11.5"));
        assert!(message.contains("upgrade the server"));
    }

    #[test]
    fn feature_gate_accepts_312_and_newer() {
        assert!(
            check_feature_supported(Feature::SchemaAnnotations, &Version::new(3, 12, 0)).is_ok()
        );
        assert!(
            check_feature_supported(Feature::SchemaAnnotations, &Version::new(3, 12, 7)).is_ok()
        );
        assert!(
            check_feature_supported(Feature::SchemaAnnotations, &Version::new(4, 0, 0)).is_ok()
        );
    }
}
