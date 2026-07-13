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
//! | [`server_version`] | HTTP probe → `GET /v1/version` |
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

use std::fmt;
use std::str::FromStr;

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
/// it accepts is present in `embedded_bands`.  A dual-band server (3.12
/// accepts `[9, 8]`) is thus served by a build that embeds only band 8.
///
/// `embedded_bands` is supplied by the caller and is cfg-derived from the
/// `band7` / `band8` features compiled into that crate — core declares no band
/// features and never hardcodes the set.
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

/// Query the TypeDB HTTP API for the server version.
///
/// Constructs the endpoint URL via [`version_endpoint`], issues a blocking
/// `GET`, and parses the response with [`parse_version_response`].
///
/// The gRPC `address` format is `"host:1729"` or bare `"host"`.  The HTTP
/// version endpoint always runs on a separate port (default `8000`); pass
/// `http_port` to override for non-standard deployments.
///
/// # Errors
///
/// Returns [`VersionError::Probe`] for any network, HTTP, or parse failure,
/// with an actionable message that names the URL tried.  The probe **never**
/// silently returns a default — it fails closed so an undetectable version
/// cannot be asserted compatible.
pub fn server_version(address: &str, http_port: u16, tls: bool) -> Result<Version, VersionError> {
    let url = version_endpoint(address, http_port, tls);

    let response_body = ureq::get(&url)
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
                // Derive which line the server is on so the remediation is actionable.
                let server_line = format!("{}.{}", server.major, server.minor);
                write!(
                    f,
                    "driver {driver} is not protocol-compatible with server {server}; \
                     install a driver matching the server line \
                     (e.g. typedb-driver ~{server_line})"
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
    fn negotiate_3_12_default_build_picks_band8() {
        // Default build embeds {7, 8}; server 3.12 accepts [9, 8] → band 8.
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
