//! Offline tests for the ORM version gate.
//!
//! All tests in this file run without a live TypeDB server.  They verify:
//!
//! 1. The compile-pinned driver version matches `Cargo.lock` exactly.
//! 2. `OrmError::UnsupportedVersion` preserves the core error message verbatim.
//! 3. The `band` predicate fires for out-of-window server versions.

// The pinned-version constant lives in real_driver, which is behind the
// `typedb` feature.  Gate the whole file accordingly.
#![cfg(feature = "typedb")]

use type_bridge_core_lib::version::{self as core_version, VersionError};
use type_bridge_orm::error::OrmError;
use type_bridge_orm::session::real_driver::PINNED_DRIVER_VERSION;

// ── 1. Cargo.lock pin assertion ──────────────────────────────────────────────

/// Assert that `PINNED_DRIVER_VERSION` matches the `typedb-driver` entry in
/// `Cargo.lock`, and that the pinned version falls in the expected protocol
/// band.
///
/// If this test breaks after a dependency bump, update `PINNED_DRIVER_VERSION`
/// in `crates/orm/src/session/real_driver.rs` to the new value.
#[test]
fn cargo_lock_pin() {
    // Read Cargo.lock at test time (relative to this crate's manifest dir).
    let lock_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock");
    let lock_contents =
        std::fs::read_to_string(lock_path).expect("Cargo.lock not found relative to crate root");

    // Find the typedb-driver package block and extract its version line.
    // The block looks like:
    //   [[package]]
    //   name = "typedb-driver"
    //   version = "3.8.1"
    let lock_version = lock_contents
        .split("[[package]]")
        .find(|block| block.contains("name = \"typedb-driver\""))
        .and_then(|block| {
            block
                .lines()
                .find(|l| l.trim_start().starts_with("version = "))
        })
        .and_then(|line| {
            // Extract the quoted value: version = "3.8.1"
            let start = line.find('"')? + 1;
            let end = line.rfind('"')?;
            Some(&line[start..end])
        })
        .expect("could not parse typedb-driver version from Cargo.lock");

    assert_eq!(
        lock_version, PINNED_DRIVER_VERSION,
        "PINNED_DRIVER_VERSION ({PINNED_DRIVER_VERSION}) does not match \
         Cargo.lock typedb-driver version ({lock_version}); \
         update the constant in crates/orm/src/session/real_driver.rs"
    );

    // Also assert the pinned driver is in the expected protocol band (8).
    // A dep bump that crosses a band boundary requires a conscious update here.
    let pinned: core_version::Version = PINNED_DRIVER_VERSION
        .parse()
        .expect("PINNED_DRIVER_VERSION must be a valid version string");
    assert_eq!(
        core_version::band(&pinned),
        Some(8),
        "pinned driver {PINNED_DRIVER_VERSION} is no longer in band 8; \
         update PINNED_DRIVER_VERSION and review the compatibility window"
    );
}

// ── 2. Error mapping — BandMismatch message preservation ────────────────────

/// `OrmError::UnsupportedVersion` must preserve the core message verbatim,
/// including both version strings and the remediation text.  It must NOT
/// contain "protocol" as a bare term (band numbers are not user-facing text).
#[test]
fn orm_error_band_mismatch_message_preserved() {
    // Driver 3.11.5 (band 8) vs server 3.10.4 (band 7) → BandMismatch.
    let core_err =
        core_version::check_supported(&"3.11.5".parse().unwrap(), &"3.10.4".parse().unwrap())
            .unwrap_err();

    // Wrap into OrmError via the #[from] impl.
    let orm_err: OrmError = core_err.into();

    let msg = orm_err.to_string();

    // Both version strings must appear.
    assert!(
        msg.contains("3.11.5"),
        "OrmError message missing driver version 3.11.5: {msg}"
    );
    assert!(
        msg.contains("3.10.4"),
        "OrmError message missing server version 3.10.4: {msg}"
    );

    // Remediation text: core says "install a driver matching the server line".
    assert!(
        msg.contains("install"),
        "OrmError message missing remediation text: {msg}"
    );

    // Protocol-band NUMBERS must never surface — the explanation stays in
    // human versions ("not protocol-compatible" prose is fine; "band 7" is not).
    assert!(
        !msg.contains("band 7") && !msg.contains("band 8"),
        "OrmError message exposes protocol band numbers: {msg}"
    );
    assert!(
        !msg.contains("0.0.0"),
        "OrmError message exposes the driver's bogus 0.0.0 self-report: {msg}"
    );
}

// ── 3. Window negative — Unsupported server via OrmError ────────────────────

/// Server 3.7.3 is below the support floor (3.8.0).  The error must name
/// 3.7.3 and describe the supported window.
#[test]
fn orm_error_window_negative_server_3_7_3() {
    // Use a 3.10.0 driver (in-window, band 7) against server 3.7.3 (below
    // the floor).  The gate must reject on the server window check.
    let core_err =
        core_version::check_supported(&"3.10.0".parse().unwrap(), &"3.7.3".parse().unwrap())
            .unwrap_err();

    let orm_err: OrmError = core_err.into();
    let msg = orm_err.to_string();

    // The detected out-of-range version must be named.
    assert!(
        msg.contains("3.7.3"),
        "OrmError message missing out-of-window server version 3.7.3: {msg}"
    );

    // The window boundary must be mentioned so the user knows what is
    // acceptable.
    assert!(
        msg.contains("3.8"),
        "OrmError message missing window floor 3.8: {msg}"
    );
}

// ── 4. Error mapping — Probe error passes through ───────────────────────────

/// `VersionError::Probe` (network failure) must also map cleanly into
/// `OrmError::UnsupportedVersion` with the original message intact.
#[test]
fn orm_error_probe_message_preserved() {
    let core_err = VersionError::Probe("connection refused at localhost:8000".to_string());
    let orm_err: OrmError = core_err.into();
    let msg = orm_err.to_string();

    assert!(
        msg.contains("connection refused"),
        "OrmError message lost probe detail: {msg}"
    );
}
