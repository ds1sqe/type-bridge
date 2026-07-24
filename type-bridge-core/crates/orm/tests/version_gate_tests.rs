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
use type_bridge_orm::session::real_driver::{
    PINNED_DRIVER_VERSION, PINNED_DRIVER_VERSION_B7, PINNED_DRIVER_VERSION_B9,
};

// ── 1. Cargo.lock pin assertion ──────────────────────────────────────────────

/// Assert that `PINNED_DRIVER_VERSION` matches the
/// `type-bridge-typedb-driver-b8` entry in `Cargo.lock`, and that the pinned
/// version falls in the expected protocol band.
///
/// If this test breaks after a dependency bump, update `PINNED_DRIVER_VERSION`
/// in `crates/orm/src/session/real_driver.rs` to the new value.
#[test]
fn cargo_lock_pin() {
    // Read Cargo.lock at test time (relative to this crate's manifest dir).
    let lock_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock");
    let lock_contents =
        std::fs::read_to_string(lock_path).expect("Cargo.lock not found relative to crate root");

    // Find the band-8 driver fork package block and extract its version line.
    // The block looks like:
    //   [[package]]
    //   name = "type-bridge-typedb-driver-b8"
    //   version = "3.11.5"
    let lock_version = lock_contents
        .split("[[package]]")
        .find(|block| block.contains("name = \"type-bridge-typedb-driver-b8\""))
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
        .expect("could not parse type-bridge-typedb-driver-b8 version from Cargo.lock");

    assert_eq!(
        lock_version, PINNED_DRIVER_VERSION,
        "PINNED_DRIVER_VERSION ({PINNED_DRIVER_VERSION}) does not match \
         Cargo.lock type-bridge-typedb-driver-b8 version ({lock_version}); \
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

/// Assert that `PINNED_DRIVER_VERSION_B7` matches the
/// `type-bridge-typedb-driver-b7` entry in `Cargo.lock`, and that the pinned
/// fork version falls in the expected protocol band (7).
///
/// If this test breaks after a fork refresh, update `PINNED_DRIVER_VERSION_B7`
/// in `crates/orm/src/session/real_driver.rs` to the new value.
#[test]
fn cargo_lock_pin_b7() {
    let lock_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock");
    let lock_contents =
        std::fs::read_to_string(lock_path).expect("Cargo.lock not found relative to crate root");

    // Find the type-bridge-typedb-driver-b7 package block and extract its version.
    let lock_version = lock_contents
        .split("[[package]]")
        .find(|block| block.contains("name = \"type-bridge-typedb-driver-b7\""))
        .and_then(|block| {
            block
                .lines()
                .find(|l| l.trim_start().starts_with("version = "))
        })
        .and_then(|line| {
            let start = line.find('"')? + 1;
            let end = line.rfind('"')?;
            Some(&line[start..end])
        })
        .expect("could not parse type-bridge-typedb-driver-b7 version from Cargo.lock");

    assert_eq!(
        lock_version, PINNED_DRIVER_VERSION_B7,
        "PINNED_DRIVER_VERSION_B7 ({PINNED_DRIVER_VERSION_B7}) does not match \
         Cargo.lock type-bridge-typedb-driver-b7 version ({lock_version}); \
         update the constant in crates/orm/src/session/real_driver.rs"
    );

    // Assert the band-7 fork is in band 7.
    let pinned: core_version::Version = PINNED_DRIVER_VERSION_B7
        .parse()
        .expect("PINNED_DRIVER_VERSION_B7 must be a valid version string");
    assert_eq!(
        core_version::band(&pinned),
        Some(7),
        "pinned band-7 fork {PINNED_DRIVER_VERSION_B7} is no longer in band 7; \
         update PINNED_DRIVER_VERSION_B7 and review the compatibility window"
    );
}

/// Assert that `PINNED_DRIVER_VERSION_B9` matches the
/// upstream `typedb-driver` entry in `Cargo.lock`, and that the pinned version
/// falls in the expected protocol band (9).
///
/// If this test breaks after a driver refresh, update `PINNED_DRIVER_VERSION_B9`
/// in `crates/typedb-runtime/src/lib.rs` to the new value.
#[test]
fn cargo_lock_pin_b9() {
    let lock_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock");
    let lock_contents =
        std::fs::read_to_string(lock_path).expect("Cargo.lock not found relative to crate root");

    // Find the upstream typedb-driver package block and extract its version.
    let lock_version = lock_contents
        .split("[[package]]")
        .find(|block| block.contains("name = \"typedb-driver\""))
        .and_then(|block| {
            block
                .lines()
                .find(|l| l.trim_start().starts_with("version = "))
        })
        .and_then(|line| {
            let start = line.find('"')? + 1;
            let end = line.rfind('"')?;
            Some(&line[start..end])
        })
        .expect("could not parse typedb-driver version from Cargo.lock");

    assert_eq!(
        lock_version, PINNED_DRIVER_VERSION_B9,
        "PINNED_DRIVER_VERSION_B9 ({PINNED_DRIVER_VERSION_B9}) does not match \
         Cargo.lock typedb-driver version ({lock_version}); \
         update the constant in crates/typedb-runtime/src/lib.rs"
    );

    // Assert the upstream band-9 driver remains in band 9.
    let pinned: core_version::Version = PINNED_DRIVER_VERSION_B9
        .parse()
        .expect("PINNED_DRIVER_VERSION_B9 must be a valid version string");
    assert_eq!(
        core_version::band(&pinned),
        Some(9),
        "pinned band-9 driver {PINNED_DRIVER_VERSION_B9} is no longer in band 9; \
         update PINNED_DRIVER_VERSION_B9 and review the compatibility window"
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

    // Remediation stays interpreter-neutral while still telling users to
    // install a compatible driver/server combination.
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

// ── 5. ConnectOptions defaults ───────────────────────────────────────────────

use type_bridge_core_lib::version::DEFAULT_HTTP_PORT;
use type_bridge_orm::ConnectOptions;

/// `ConnectOptions::default()` must produce `{ http_port: DEFAULT_HTTP_PORT, tls: false }`.
///
/// Pins the default so a future change to `DEFAULT_HTTP_PORT` also fails
/// this test, keeping the constant and the default in sync.
#[test]
fn connect_options_default_equals_ssot() {
    let opts = ConnectOptions::default();
    assert_eq!(
        opts.http_port, DEFAULT_HTTP_PORT,
        "ConnectOptions::default().http_port must equal DEFAULT_HTTP_PORT ({DEFAULT_HTTP_PORT})"
    );
    assert!(!opts.tls, "ConnectOptions::default().tls must be false");
}

// ── Schema-annotation feature gate (TypeDB 3.12+) ────────────────────

mod common;

use type_bridge_core_lib::version::Version;
use type_bridge_orm::Database;
use type_bridge_orm::session::backend::{BoxFuture, DriverBackend, TransactionOps};

/// Mock backend that reports a fixed detected server version.
struct VersionedBackend {
    inner: common::MockBackend,
    server_version: Option<Version>,
    supports_given_rows: bool,
}

impl VersionedBackend {
    fn new(server_version: Option<Version>) -> Self {
        Self {
            inner: common::MockBackend::new(vec![]),
            server_version,
            supports_given_rows: false,
        }
    }

    fn with_given_rows(mut self) -> Self {
        self.supports_given_rows = true;
        self
    }
}

impl DriverBackend for VersionedBackend {
    fn open_transaction(
        &self,
        database: &str,
        tx_type: type_bridge_orm::TxType,
    ) -> BoxFuture<'_, std::result::Result<Box<dyn TransactionOps>, type_bridge_orm::OrmError>>
    {
        self.inner.open_transaction(database, tx_type)
    }

    fn is_open(&self) -> bool {
        true
    }

    fn server_version(&self) -> Option<Version> {
        self.server_version
    }

    fn supports_given_rows(&self) -> bool {
        self.supports_given_rows
    }
}

const ANNOTATED_DDL: &str = "define\nentity person @doc(\"A person.\"), owns name @key;";
const PLAIN_DDL: &str = "define\nentity person, owns name @key;";

#[test]
fn annotation_gate_rejects_annotated_ddl_on_pre_312_server() {
    let db = Database::with_backend(
        Box::new(VersionedBackend::new(Some(Version::new(3, 11, 5)))),
        "gate-test",
    );
    let error = db
        .check_schema_annotation_support(ANNOTATED_DDL)
        .expect_err("pre-3.12 server must reject annotated DDL");
    let message = error.to_string();
    assert!(
        message.contains("3.12"),
        "message must name 3.12: {message}"
    );
    assert!(
        message.contains("3.11.5"),
        "message must name the detected server: {message}"
    );
    assert!(
        message.contains("schema annotations"),
        "message must name the feature: {message}"
    );
}

#[test]
fn annotation_gate_passes_plain_ddl_on_pre_312_server() {
    let db = Database::with_backend(
        Box::new(VersionedBackend::new(Some(Version::new(3, 11, 5)))),
        "gate-test",
    );
    assert!(db.check_schema_annotation_support(PLAIN_DDL).is_ok());
}

#[test]
fn annotation_gate_passes_annotated_ddl_on_312_server() {
    let db = Database::with_backend(
        Box::new(VersionedBackend::new(Some(Version::new(3, 12, 0)))),
        "gate-test",
    );
    assert!(db.check_schema_annotation_support(ANNOTATED_DDL).is_ok());
}

#[test]
fn given_stage_gate_rejects_312_server_when_provider_remains_band8() {
    let db = Database::with_backend(
        Box::new(VersionedBackend::new(Some(Version::new(3, 12, 0)))),
        "gate-test",
    );
    let error = db
        .check_given_stage_support()
        .expect_err("band-8 fallback must not advertise given-row transport");
    assert!(
        error.to_string().contains("active band-9 provider"),
        "unexpected error: {error}"
    );
    assert!(!db.supports_given_stage());
}

#[test]
fn annotation_gate_defers_to_server_when_version_unknown() {
    // Band-7 gRPC fallback: version undetectable; the DDL is sent as-is.
    let db = Database::with_backend(Box::new(VersionedBackend::new(None)), "gate-test");
    assert!(db.check_schema_annotation_support(ANNOTATED_DDL).is_ok());
}

#[test]
fn mock_backend_reports_no_server_version_by_default() {
    let backend = common::MockBackend::new(vec![]);
    assert_eq!(backend.server_version(), None);
}

// ── Given-stage feature gate (TypeDB 3.12+) ──────────────────────────

use type_bridge_orm::{GivenRowsSpec, TxType};

fn empty_rows() -> GivenRowsSpec {
    GivenRowsSpec {
        variables: vec!["n".to_string()],
        rows: vec![],
    }
}

#[test]
fn given_stage_gate_rejects_pre_312_server() {
    let db = Database::with_backend(
        Box::new(VersionedBackend::new(Some(Version::new(3, 11, 5)))),
        "gate-test",
    );
    let error = db
        .check_given_stage_support()
        .expect_err("pre-3.12 server must reject given-stage queries");
    let message = error.to_string();
    assert!(
        message.contains("3.12"),
        "message must name 3.12: {message}"
    );
    assert!(
        message.contains("3.11.5"),
        "message must name the detected server: {message}"
    );
    assert!(
        message.contains("given-stage"),
        "message must name the feature: {message}"
    );
}

#[test]
fn given_stage_gate_passes_on_312_server() {
    let db = Database::with_backend(
        Box::new(VersionedBackend::new(Some(Version::new(3, 12, 0))).with_given_rows()),
        "gate-test",
    );
    assert!(db.check_given_stage_support().is_ok());
    assert!(db.supports_given_stage());
}

#[test]
fn supports_given_stage_is_false_pre_312_and_when_unknown() {
    let pre = Database::with_backend(
        Box::new(VersionedBackend::new(Some(Version::new(3, 11, 5)))),
        "gate-test",
    );
    assert!(!pre.supports_given_stage());

    let unknown = Database::with_backend(Box::new(VersionedBackend::new(None)), "gate-test");
    assert!(!unknown.supports_given_stage());
}

#[tokio::test]
async fn execute_with_rows_rejects_pre_312_before_opening_transaction() {
    let db = Database::with_backend(
        Box::new(VersionedBackend::new(Some(Version::new(3, 11, 5)))),
        "gate-test",
    );
    let error = db
        .execute_with_rows("given $n: string; insert ...;", TxType::Write, empty_rows())
        .await
        .expect_err("pre-3.12 server must reject execute_with_rows");
    assert!(
        matches!(error, OrmError::UnsupportedVersion(_)),
        "expected UnsupportedVersion, got {error:?}"
    );
}

#[tokio::test]
async fn execute_with_rows_unknown_version_fails_capability_preflight() {
    let db = Database::with_backend(Box::new(VersionedBackend::new(None)), "gate-test");
    let error = db
        .execute_with_rows("given $n: string; insert ...;", TxType::Write, empty_rows())
        .await
        .expect_err("backend without given-stage support must error");
    assert!(
        error.to_string().contains("server version is unknown"),
        "unexpected error: {error}"
    );
}
