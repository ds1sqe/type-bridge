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
//! | [`MIN_SUPPORTED`] | Floor of the support window (`3.8.0`) |
//! | [`MAX_SUPPORTED_LINE`] | Ceiling of the support window as `(major, minor)` (`3.12`) |
//! | [`window_contains`] | Range predicate combining both bounds |
//! | [`band`] | Band a driver natively speaks (data; measured against live servers) |
//! | [`server_accepted_bands`] | Bands a server accepts connections from (data; measured) |
//! | [`negotiate_server_band`] | Pick the embedded band to connect a server with |
//! | [`check_supported`] | Installed-driver gate: window + driver band ∈ server's accepted set |
//! | [`check_server_supported`] | Embedded-runtime gate: window + accepted ∩ embedded ≠ ∅ |
//! | [`support_status`] | Pure supported/deprecated/unsupported server-line classification |
//! | [`known_server_deprecation_notice`] | Core-owned notice for a known deprecated server version |
//! | [`unknown_legacy_fallback_deprecation_notice`] | Core-owned notice for an unknown legacy fallback |
//! | [`server_version`] | Released Boolean adapter for the HTTP version probe |
//! | [`server_version_plaintext`] | Explicit plaintext HTTP version probe |
//! | [`server_version_native_roots`] | HTTPS version probe using native trust roots |
//! | [`server_version_custom_root_ca`] | HTTPS version probe using one custom CA bundle |
//! | [`VersionError`] | Typed error for all failure modes |
//!
//! ## Two gates, two questions
//!
//! [`check_supported`] answers *"is the **installed** single-band Python driver
//! protocol-compatible with this server?"* — it tests whether the server accepts
//! the one driver band the user has installed ([`server_accepted_bands`]).
//!
//! [`check_server_supported`] answers *"does **this build** of type-bridge embed a
//! driver that can serve this server?"* — the embedded runtime can carry several
//! bands at once (the default build embeds them all), so it tests whether the
//! server's accepted band set intersects the embedded set.

use std::ffi::OsString;
use std::fmt;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};

#[cfg(unix)]
use cap_fs_ext::OpenOptionsSyncExt as _;
use cap_fs_ext::{DirExt as _, FollowSymlinks, OpenOptionsFollowExt as _};
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

// ---------------------------------------------------------------------------
// Window constants (SSOT — declared exactly once, here)
// ---------------------------------------------------------------------------

/// Floor of the TypeDB support window (`3.8.0`, inclusive).
///
/// Anything below this version is unsupported and must fail fast at connect.
pub const MIN_SUPPORTED: Version = Version::new(3, 8, 0);

/// Ceiling of the TypeDB support window as `(major, minor)`, inclusive.
///
/// Any patch release on this line is in-window (`3.12.0`, `3.12.1`, …).
/// The first version on the **next** minor (`3.13.0`) is out of window.
pub const MAX_SUPPORTED_LINE: (u32, u32) = (3, 12);

// ---------------------------------------------------------------------------
// Window predicate
// ---------------------------------------------------------------------------

/// Return `true` when `v` falls within the support window.
///
/// A version is in-window when:
/// - `v >= MIN_SUPPORTED` (i.e. at least `3.8.0`), **and**
/// - `(v.major, v.minor) <= MAX_SUPPORTED_LINE` (i.e. at most `3.12.x`).
pub fn window_contains(v: &Version) -> bool {
    *v >= MIN_SUPPORTED && (v.major, v.minor) <= MAX_SUPPORTED_LINE
}

// ---------------------------------------------------------------------------
// Protocol band map (SSOT — measured live)
// ---------------------------------------------------------------------------

/// Return the protocol band a TypeDB version **natively speaks**, or `None`
/// if unmapped.
///
/// The band encodes which gRPC protocol revision a driver of this version
/// speaks — a driver connects successfully only to servers that accept its
/// band (see [`server_accepted_bands`]).  This map lets the gate detect the
/// mismatch **before** attempting a connection.
///
/// Measured by connecting every driver line (3.7.0, 3.8.1, 3.10.0, 3.11.5,
/// 3.12.0) against every server line (3.7.3, 3.8.3, 3.10.4, 3.11.5, 3.12.0):
///
/// | Minor line | Band |
/// |-----------|------|
/// | 3.7, 3.8, 3.10 | `7` (protocol 7) |
/// | 3.11 | `8` (protocol 8) |
/// | 3.12 | `9` (protocol 3.12.0-rc0) |
/// | anything else (incl. 3.9, 3.13, 2.x) | `None` |
///
/// When TypeDB ships a new minor line, measure it live and extend **both**
/// this map and [`server_accepted_bands`] — together they are the only data
/// the gate needs to cover the new version.
pub fn band(v: &Version) -> Option<u8> {
    // Major must be 3; 2.x and other majors are not mapped.
    if v.major != 3 {
        return None;
    }
    match v.minor {
        7 | 8 | 10 => Some(7),
        11 => Some(8),
        12 => Some(9),
        // 3.9 was never released; 3.13+ not yet mapped.
        _ => None,
    }
}

/// Return the protocol bands a TypeDB **server** of this version accepts
/// connections from, native band first.  Empty for unmapped versions.
///
/// Band acceptance is asymmetric starting with 3.12: server 3.12 retains
/// backward compatibility with band-8 drivers (a 3.11.5 driver completes a
/// full round trip against a 3.12.0 server), while a 3.12 driver's native
/// band-9 protocol is rejected by a 3.11 server at connect.  A single
/// per-version band cannot express that, so servers carry an accepted *set*
/// and drivers keep their single native [`band`].
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
/// | 3.7, 3.8, 3.10 | `[7]` |
/// | 3.11 | `[8]` |
/// | 3.12 | `[9, 8]` |
/// | anything else | `[]` |
pub fn server_accepted_bands(v: &Version) -> &'static [u8] {
    if v.major != 3 {
        return &[];
    }
    match v.minor {
        7 | 8 | 10 => &[7],
        11 => &[8],
        12 => &[9, 8],
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

/// Assert that `driver` and `server` are mutually compatible.
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

/// Assert that this build's **embedded** runtime can serve `server`.
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
/// `band7` / `band8` / `band9` features compiled into that crate — core
/// declares no band features and never hardcodes the set.
///
/// # Errors
///
/// - [`VersionError::Unsupported`] when `server` lies outside the support
///   window (below-window `3.7`, above-window `3.13`) **or** is in-window but
///   has no accepted bands (e.g. a future `3.9` line, only reachable if the
///   window ever widens past the band map).  This path names no band — it is
///   the same window-class rejection an out-of-range server gets today.
/// - [`VersionError::EmbeddedUnavailable`] when `server` is in-window with
///   accepted bands, none of which this build compiled in.  Reachable only in
///   a non-default single-band build; the default build embeds every band an
///   in-window server accepts, so such a server is always served.
pub fn check_server_supported(server: &Version, embedded_bands: &[u8]) -> Result<(), VersionError> {
    // Window check first — identical class to check_supported's server arm, so
    // below/above-window servers fail with the existing window message.
    if !window_contains(server) {
        return Err(VersionError::Unsupported {
            component: "server",
            found: *server,
        });
    }

    let accepted = server_accepted_bands(server);
    if accepted.is_empty() {
        // In-window but unmapped minor: defensive, only reachable if the window
        // widens past the band map (today the map covers the whole window).
        // Reject with the window class — naming a band here would be a lie, as
        // there is no band to name.
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
// Server support and deprecation status
// ---------------------------------------------------------------------------

/// Stable cross-binding code for TypeDB legacy-server deprecation notices.
///
/// Rust tracing, Python warnings, and Node process warnings use this exact
/// identifier so callers can filter the notice without matching prose.
pub const TYPEDB_LEGACY_SERVER_DEPRECATION_CODE: &str = "TYPE_BRIDGE_TYPEDB_LEGACY_SERVER";

/// TypeBridge release that removes active TypeDB 3.8/3.10 server support.
///
/// The legacy window closes in the 2.1.0 minor release: it is a scheduled,
/// deliberate exception to the ordinary major-version removal schedule, and
/// from 2.1.0 the wheel embeds only the band-8 and band-9 driver lines.
/// Every TypeBridge 2.0.x release continues to support these server lines.
pub const TYPEDB_LEGACY_SERVER_REMOVAL_RELEASE: &str = "2.1.0";

/// Support status for one exact, known TypeDB server version.
///
/// This classification is pure and does not emit a warning. Bindings decide
/// how to surface a [`Self::Deprecated`] result while sharing the stable code
/// and core-owned prose below.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ServerSupportStatus {
    /// The server is in the active window and has no scheduled compatibility
    /// removal.
    Supported,
    /// The server remains fully supported throughout TypeBridge 2.0.x but its
    /// active provider support is scheduled for removal in TypeBridge 2.1.0.
    Deprecated,
    /// The server is outside the active window or has no mapped accepted
    /// protocol path.
    Unsupported,
}

impl ServerSupportStatus {
    /// Return the stable warning code when this status is deprecated.
    #[must_use]
    pub const fn deprecation_code(self) -> Option<&'static str> {
        match self {
            Self::Deprecated => Some(TYPEDB_LEGACY_SERVER_DEPRECATION_CODE),
            Self::Supported | Self::Unsupported => None,
        }
    }

    /// Return the scheduled TypeBridge removal release when deprecated.
    #[must_use]
    pub const fn removal_release(self) -> Option<&'static str> {
        match self {
            Self::Deprecated => Some(TYPEDB_LEGACY_SERVER_REMOVAL_RELEASE),
            Self::Supported | Self::Unsupported => None,
        }
    }
}

/// Classify one exact, known TypeDB server version.
///
/// TypeDB 3.8.x and 3.10.x are deprecated but remain operational throughout
/// TypeBridge 2.x. TypeDB 3.11.x and 3.12.x are supported without a scheduled
/// removal. Versions rejected by the existing server gate, including the
/// unreleased 3.9 line, classify as [`ServerSupportStatus::Unsupported`].
#[must_use]
pub fn support_status(server: &Version) -> ServerSupportStatus {
    if !window_contains(server) || server_accepted_bands(server).is_empty() {
        return ServerSupportStatus::Unsupported;
    }

    if server.major == 3 && matches!(server.minor, 8 | 10) {
        ServerSupportStatus::Deprecated
    } else {
        ServerSupportStatus::Supported
    }
}

/// Return the shared notice for one exact deprecated server version.
///
/// Supported and unsupported versions return `None`; unsupported versions
/// remain the responsibility of the existing version gate.
#[must_use]
pub fn known_server_deprecation_notice(server: &Version) -> Option<String> {
    if support_status(server) != ServerSupportStatus::Deprecated {
        return None;
    }

    Some(format!(
        "TypeDB {server} is on the 3.8/3.10 line, which is deprecated in \
         type-bridge 2.0. Support for this server line will be removed in \
         type-bridge {TYPEDB_LEGACY_SERVER_REMOVAL_RELEASE}. Connections keep \
         working throughout 2.0.x. Upgrade the server to TypeDB 3.11 or 3.12, \
         or pin type-bridge>=2,<2.1."
    ))
}

/// Return the shared notice for a connection whose legacy fallback cannot
/// report an exact TypeDB version.
///
/// The wording names the compatibility path rather than claiming that an
/// unknown server is definitely on a particular semantic-version line.
#[must_use]
pub fn unknown_legacy_fallback_deprecation_notice() -> String {
    format!(
        "This connection uses the legacy TypeDB 3.8/3.10-compatible fallback \
         path, which is deprecated in type-bridge 2.0. Support for this path \
         will be removed in type-bridge {TYPEDB_LEGACY_SERVER_REMOVAL_RELEASE}. \
         Connections keep working throughout 2.0.x. Upgrade the server to TypeDB \
         3.11 or 3.12, or pin type-bridge>=2,<2.1. Configure an exact server \
         version for strict validation."
    )
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
/// `/dev/fd/<fd>` on Unix, or the current handle-derived path while Windows
/// no-write/no-delete handles keep both the snapshot parent and file pinned.
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
    #[cfg(windows)]
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
    }
}

#[cfg(unix)]
fn create_custom_root_snapshot(
    configured_path: &Path,
    bytes: &[u8],
) -> Result<std::fs::File, TlsConfigurationError> {
    // `tempfile()` is anonymous on Unix. Only the retained descriptor can
    // reach this immutable-by-convention snapshot; configured-path writes,
    // renames, and parent swaps cannot change what a driver reads.
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

        #[cfg(unix)]
        let snapshot_file = create_custom_root_snapshot(configured_path, &bytes)?;
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
            #[cfg(windows)]
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

        #[cfg(unix)]
        let retained_path = {
            use std::os::fd::AsRawFd as _;
            PathBuf::from(format!("/dev/fd/{}", file.as_raw_fd()))
        };

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

    fn root_store(&self) -> rustls::RootCertStore {
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

fn native_root_store() -> Result<rustls::RootCertStore, TlsConfigurationError> {
    // Platform stores are often heterogeneous. Individual unreadable or
    // malformed entries are not fatal when the same load produced at least
    // one certificate rustls can use.
    native_root_store_from_loaded(rustls_native_certs::load_native_certs())
}

fn tls_agent(roots: rustls::RootCertStore) -> Result<ureq::Agent, TlsConfigurationError> {
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
    Ok(ureq::builder().tls_config(Arc::new(config)).build())
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
/// Expected shape: `{"distribution": "TypeDB CE", "version": "3.10.4"}`.
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

fn server_version_request(request: ureq::Request, url: &str) -> Result<Version, VersionError> {
    let response_body = request
        .call()
        .map_err(|e| {
            VersionError::Probe(format!(
                "could not reach TypeDB HTTP version endpoint at {url}; \
                 ensure the TypeDB HTTP endpoint is reachable and that the port is correct: {e}"
            ))
        })?
        .into_string()
        .map_err(|e| {
            VersionError::Probe(format!("failed to read response body from {url}: {e}"))
        })?;

    parse_version_response(&response_body).map_err(|e| match e {
        VersionError::Probe(msg) => VersionError::Probe(format!("{msg} (endpoint: {url})")),
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
    server_version_request(ureq::get(&url), &url).map_err(VersionProbeError::Probe)
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
    server_version_request(agent.get(&url), &url).map_err(VersionProbeError::Probe)
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
    server_version_request(agent.get(&url), &url).map_err(VersionProbeError::Probe)
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
    /// One endpoint lies outside the declared support window.
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

/// Human-readable window string used consistently in error messages.
const WINDOW_HUMAN: &str = "3.8.0–3.12.x";

impl fmt::Display for VersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VersionError::Unsupported { component, found } => write!(
                f,
                "{component} version {found} is outside the supported window ({WINDOW_HUMAN}); \
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
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TLS_TEMP_DIRECTORY: AtomicU64 = AtomicU64::new(0);

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
        assert!("3.10.4-alpha".parse::<Version>().is_err());
    }

    #[test]
    fn parse_error_on_single_component() {
        assert!("3".parse::<Version>().is_err());
    }

    #[test]
    fn display_round_trips() {
        let v = Version::new(3, 10, 4);
        assert_eq!(v.to_string(), "3.10.4");
        let v2 = Version::new(3, 11, 0);
        assert_eq!(v2.to_string(), "3.11.0");
    }

    // -- window_contains -----------------------------------------------------

    #[test]
    fn window_rejects_3_7_9() {
        assert!(!window_contains(&Version::new(3, 7, 9)));
    }

    #[test]
    fn window_accepts_3_8_0_floor() {
        assert!(window_contains(&Version::new(3, 8, 0)));
    }

    #[test]
    fn window_accepts_3_11_0() {
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
    fn window_rejects_3_13_0() {
        assert!(!window_contains(&Version::new(3, 13, 0)));
    }

    #[test]
    fn window_rejects_2_9_0() {
        assert!(!window_contains(&Version::new(2, 9, 0)));
    }

    // -- band ----------------------------------------------------------------

    #[test]
    fn band_3_7_is_7() {
        assert_eq!(band(&Version::new(3, 7, 0)), Some(7));
    }

    #[test]
    fn band_3_8_is_7() {
        assert_eq!(band(&Version::new(3, 8, 1)), Some(7));
    }

    #[test]
    fn band_3_10_is_7() {
        assert_eq!(band(&Version::new(3, 10, 4)), Some(7));
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
    fn band_3_13_is_none() {
        assert_eq!(band(&Version::new(3, 13, 0)), None);
    }

    #[test]
    fn band_2_x_is_none() {
        assert_eq!(band(&Version::new(2, 28, 0)), None);
    }

    // -- server_accepted_bands -------------------------------------------------

    #[test]
    fn server_accepts_band7_lines() {
        assert_eq!(server_accepted_bands(&Version::new(3, 8, 3)), &[7]);
        assert_eq!(server_accepted_bands(&Version::new(3, 10, 4)), &[7]);
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
    fn server_unmapped_accepts_nothing() {
        assert!(server_accepted_bands(&Version::new(3, 13, 0)).is_empty());
        assert!(server_accepted_bands(&Version::new(2, 28, 0)).is_empty());
    }

    // -- negotiate_server_band -------------------------------------------------

    #[test]
    fn negotiate_3_12_without_band9_picks_band8() {
        // A reduced {7, 8} build uses the server's compatible band-8 fallback.
        assert_eq!(
            negotiate_server_band(&Version::new(3, 12, 0), &[7, 8]),
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
    fn negotiate_none_when_no_overlap() {
        assert_eq!(negotiate_server_band(&Version::new(3, 11, 5), &[7]), None);
    }

    // -- check_supported -----------------------------------------------------

    #[test]
    fn check_driver_310_server_383_accept() {
        // Band A × Band A — cross-line intra-band interop must work.
        assert!(check_supported(&Version::new(3, 10, 0), &Version::new(3, 8, 3)).is_ok());
    }

    #[test]
    fn check_driver_381_server_104_accept() {
        // Band A × Band A — same direction.
        assert!(check_supported(&Version::new(3, 8, 1), &Version::new(3, 10, 4)).is_ok());
    }

    #[test]
    fn check_driver_3115_server_3104_reject_band_mismatch() {
        // Band B driver vs Band A server.
        let err = check_supported(&Version::new(3, 11, 5), &Version::new(3, 10, 4)).unwrap_err();
        assert!(
            matches!(err, VersionError::BandMismatch { .. }),
            "expected BandMismatch, got {err:?}"
        );
    }

    #[test]
    fn check_driver_3100_server_3115_reject_band_mismatch() {
        // Band A driver vs Band B server.
        let err = check_supported(&Version::new(3, 10, 0), &Version::new(3, 11, 5)).unwrap_err();
        assert!(
            matches!(err, VersionError::BandMismatch { .. }),
            "expected BandMismatch, got {err:?}"
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
        let err = check_supported(&Version::new(3, 10, 0), &Version::new(2, 28, 0)).unwrap_err();
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
    fn server_383_both_bands_accept() {
        // In-window band-7 server, both bands embedded → served.
        assert!(check_server_supported(&Version::new(3, 8, 3), &[7, 8]).is_ok());
    }

    #[test]
    fn server_3115_both_bands_accept() {
        // In-window band-8 server, both bands embedded → served.
        assert!(check_server_supported(&Version::new(3, 11, 5), &[7, 8]).is_ok());
    }

    #[test]
    fn server_373_reject_window() {
        // Below the floor — window class, never a band/embedded message.
        let err = check_server_supported(&Version::new(3, 7, 3), &[7, 8]).unwrap_err();
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
            err.to_string().contains("outside the supported window"),
            "expected window message, got {err}"
        );
    }

    #[test]
    fn server_3120_both_bands_accept() {
        // Dual-band server: accepts [9, 8]; the default build's band-8 driver
        // serves it (measured live).
        assert!(check_server_supported(&Version::new(3, 12, 0), &[7, 8]).is_ok());
    }

    #[test]
    fn server_3120_band7_only_unavailable() {
        // Server 3.12 accepts [9, 8]; a band-7-only build embeds neither.
        let err = check_server_supported(&Version::new(3, 12, 0), &[7]).unwrap_err();
        assert!(
            matches!(err, VersionError::EmbeddedUnavailable { .. }),
            "expected EmbeddedUnavailable, got {err:?}"
        );
    }

    #[test]
    fn server_3130_reject_window() {
        // Above the ceiling — window class.
        let err = check_server_supported(&Version::new(3, 13, 0), &[7, 8]).unwrap_err();
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
            err.to_string().contains("outside the supported window"),
            "expected window message, got {err}"
        );
    }

    #[test]
    fn server_383_band8_only_unavailable() {
        // Band-7 server, only band-8 embedded → embedded-unavailable.
        let err = check_server_supported(&Version::new(3, 8, 3), &[8]).unwrap_err();
        assert!(
            matches!(err, VersionError::EmbeddedUnavailable { .. }),
            "expected EmbeddedUnavailable, got {err:?}"
        );
    }

    #[test]
    fn server_3115_band7_only_unavailable() {
        // Band-8 server, only band-7 embedded → embedded-unavailable.
        let err = check_server_supported(&Version::new(3, 11, 5), &[7]).unwrap_err();
        assert!(
            matches!(err, VersionError::EmbeddedUnavailable { .. }),
            "expected EmbeddedUnavailable, got {err:?}"
        );
    }

    #[test]
    fn embedded_unavailable_message_names_server_no_forbidden_tokens() {
        let err = VersionError::EmbeddedUnavailable {
            server: Version::new(3, 8, 3),
        };
        let msg = err.to_string();
        assert!(msg.contains("3.8.3"), "missing server version: {msg}");
        // The forbidden tokens must never appear in any gate message.
        for forbidden in ["band 7", "band 8", "0.0.0"] {
            assert!(
                !msg.contains(forbidden),
                "embedded-unavailable message leaked forbidden token {forbidden:?}: {msg}"
            );
        }
    }

    #[test]
    fn check_server_supported_window_reject_no_forbidden_tokens() {
        // The window-class rejection from the embedded gate must also be clean.
        let err = check_server_supported(&Version::new(3, 7, 3), &[7, 8]).unwrap_err();
        let msg = err.to_string();
        for forbidden in ["band 7", "band 8", "0.0.0"] {
            assert!(
                !msg.contains(forbidden),
                "window rejection leaked forbidden token {forbidden:?}: {msg}"
            );
        }
    }

    // -- server support status / deprecation prose ---------------------------

    #[test]
    fn support_status_classifies_every_current_server_line() {
        for version in [
            Version::new(3, 8, 0),
            Version::new(3, 8, 99),
            Version::new(3, 10, 0),
            Version::new(3, 10, 4),
        ] {
            assert_eq!(
                support_status(&version),
                ServerSupportStatus::Deprecated,
                "{version}"
            );
        }

        for version in [Version::new(3, 11, 5), Version::new(3, 12, 1)] {
            assert_eq!(
                support_status(&version),
                ServerSupportStatus::Supported,
                "{version}"
            );
        }
    }

    #[test]
    fn support_status_does_not_reclassify_rejected_versions_as_supported() {
        for version in [
            Version::new(3, 7, 3),
            Version::new(3, 9, 0),
            Version::new(3, 13, 0),
            Version::new(4, 0, 0),
        ] {
            assert_eq!(
                support_status(&version),
                ServerSupportStatus::Unsupported,
                "{version}"
            );
        }
    }

    #[test]
    fn deprecated_status_owns_the_stable_code_and_legacy_window_removal_release() {
        assert_eq!(
            ServerSupportStatus::Deprecated.deprecation_code(),
            Some("TYPE_BRIDGE_TYPEDB_LEGACY_SERVER")
        );
        assert_eq!(
            ServerSupportStatus::Deprecated.removal_release(),
            Some("2.1.0")
        );
        assert_eq!(
            TYPEDB_LEGACY_SERVER_DEPRECATION_CODE,
            "TYPE_BRIDGE_TYPEDB_LEGACY_SERVER"
        );
        assert_eq!(TYPEDB_LEGACY_SERVER_REMOVAL_RELEASE, "2.1.0");

        for status in [
            ServerSupportStatus::Supported,
            ServerSupportStatus::Unsupported,
        ] {
            assert_eq!(status.deprecation_code(), None);
            assert_eq!(status.removal_release(), None);
        }
    }

    #[test]
    fn known_server_deprecation_notice_is_exact_and_core_owned() {
        let notice = known_server_deprecation_notice(&Version::new(3, 10, 4))
            .expect("3.10 must carry the shared deprecation notice");
        assert_eq!(
            notice,
            "TypeDB 3.10.4 is on the 3.8/3.10 line, which is deprecated in \
             type-bridge 2.0. Support for this server line will be removed in \
             type-bridge 2.1.0. Connections keep working throughout 2.0.x. \
             Upgrade the server to TypeDB 3.11 or 3.12, or pin \
             type-bridge>=2,<2.1."
        );

        for forbidden in ["band 7", "band 8", "band 9", "3.0.0", ">=2,<3"] {
            assert!(!notice.contains(forbidden), "{forbidden}: {notice}");
        }
    }

    #[test]
    fn known_server_notice_only_exists_for_deprecated_supported_lines() {
        assert!(
            known_server_deprecation_notice(&Version::new(3, 8, 3))
                .expect("3.8 must carry a notice")
                .contains("TypeDB 3.8.3")
        );
        assert!(known_server_deprecation_notice(&Version::new(3, 11, 5)).is_none());
        assert!(known_server_deprecation_notice(&Version::new(3, 12, 1)).is_none());
        assert!(known_server_deprecation_notice(&Version::new(3, 7, 3)).is_none());
        assert!(known_server_deprecation_notice(&Version::new(3, 13, 0)).is_none());
    }

    #[test]
    fn unknown_legacy_fallback_notice_is_exact_without_claiming_a_server_version() {
        let notice = unknown_legacy_fallback_deprecation_notice();
        assert_eq!(
            notice,
            "This connection uses the legacy TypeDB 3.8/3.10-compatible \
             fallback path, which is deprecated in type-bridge 2.0. Support \
             for this path will be removed in type-bridge 2.1.0. Connections \
             keep working throughout 2.0.x. Upgrade the server to TypeDB 3.11 \
             or 3.12, or pin type-bridge>=2,<2.1. Configure an exact server \
             version for strict validation."
        );

        for forbidden in ["band 7", "band 8", "band 9", "3.0.0", ">=2,<3"] {
            assert!(!notice.contains(forbidden), "{forbidden}: {notice}");
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
        assert!(msg.contains("3.8.0"), "missing window floor: {msg}");
        assert!(msg.contains("3.12.x"), "missing window ceiling: {msg}");
    }

    #[test]
    fn band_mismatch_message_contains_versions_and_remediation() {
        let err = VersionError::BandMismatch {
            driver: Version::new(3, 11, 5),
            server: Version::new(3, 10, 4),
            driver_band: 8,
            server_band: 7,
        };
        let msg = err.to_string();
        assert!(msg.contains("3.11.5"), "missing driver version: {msg}");
        assert!(msg.contains("3.10.4"), "missing server version: {msg}");
        // Remediation must mention the server line.
        assert!(
            msg.contains("3.10"),
            "missing server line in remediation: {msg}"
        );
        // Must NOT expose bare band numbers (7/8) as the primary explanation.
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

        let oversized = std::env::temp_dir().join(format!(
            "type-bridge-oversized-root-{}-{}.pem",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let file = std::fs::File::create(&oversized).expect("create oversized root fixture");
        file.set_len(MAX_CUSTOM_ROOT_CA_BYTES + 1)
            .expect("extend oversized root fixture");
        let oversized_error = validate_custom_root_ca(&oversized).unwrap_err();
        std::fs::remove_file(&oversized).expect("remove oversized root fixture");
        assert_eq!(oversized_error.code(), "tls_custom_root_ca_too_large");
        assert!(matches!(
            oversized_error,
            TlsConfigurationError::CustomRootCaTooLarge { path } if path == oversized
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
        let json = r#"{"distribution":"TypeDB CE","version":"3.10.4"}"#;
        let v = parse_version_response(json).unwrap();
        assert_eq!(v, Version::new(3, 10, 4));
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
