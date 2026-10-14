//! Select the SDK conformance identity a live producer runs under.
//!
//! Each `tests/contracts/sdk_conformance/sdk-vN/profiles-vN.json` lists, per
//! semantic profile, the exact live TypeDB server and (for sdk-v1..v3) the
//! expected semantic and per-binding projection fingerprints. Producers pick
//! the profile from `TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE`, defaulting to
//! the catalog's own profile, and emit that entry instead of literals.

use std::path::{Path, PathBuf};

use serde_json::Value;

pub const SEMANTIC_PROFILE_ENV: &str = "TYPE_BRIDGE_ACCEPTANCE_SEMANTIC_PROFILE";

/// One conformance set's expected identity on one semantic profile.
#[derive(Clone, Debug)]
pub struct SdkProfile {
    pub semantic_profile: String,
    pub live_server_version: String,
    /// `{"semantic": .., "projections": {..}}`; `Value::Null` for sdk-v4..v6.
    pub expected_fingerprints: Value,
}

impl SdkProfile {
    /// Fail unless `detected` is this profile's exact live server.
    pub fn require_server_version(&self, detected: Option<&str>) {
        assert_eq!(
            detected,
            Some(self.live_server_version.as_str()),
            "{} SDK reports require the actual detected TypeDB server version {}",
            self.semantic_profile,
            self.live_server_version
        );
    }
}

/// Repository root: the nearest ancestor of the including crate that holds
/// the conformance contracts.
pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|dir| dir.join("tests/contracts/sdk_conformance").is_dir())
        .expect("the repository root holds tests/contracts/sdk_conformance")
        .to_path_buf()
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("{} is JSON: {error}", path.display()))
}

/// The entry of `sdk-v{set}` for `profile`; panics on an unsupported profile.
pub fn select_from(root: &Path, set: u8, profile: &str) -> SdkProfile {
    let directory = root.join(format!("tests/contracts/sdk_conformance/sdk-v{set}"));
    let matrix = read_json(&directory.join(format!("profiles-v{set}.json")));
    let entry = matrix["profiles"]
        .get(profile)
        .unwrap_or_else(|| panic!("sdk-v{set} has no matrix entry for profile {profile:?}"));
    SdkProfile {
        semantic_profile: profile.to_owned(),
        live_server_version: entry["live_server_version"]
            .as_str()
            .unwrap_or_else(|| panic!("sdk-v{set} {profile} names its live server"))
            .to_owned(),
        expected_fingerprints: entry
            .get("expected_fingerprints")
            .cloned()
            .unwrap_or(Value::Null),
    }
}

/// The profile `sdk-v{set}`'s catalog was frozen on.
pub fn catalog_profile(root: &Path, set: u8) -> String {
    let catalog = read_json(&root.join(format!(
        "tests/contracts/sdk_conformance/sdk-v{set}/catalog-v{set}.json"
    )));
    catalog["fixture"]["semantic_profile"]
        .as_str()
        .unwrap_or_else(|| panic!("sdk-v{set} catalog declares its semantic profile"))
        .to_owned()
}

/// Profile named by `requested` (the environment value), else the catalog's.
pub fn requested_from(root: &Path, set: u8, requested: Option<&str>) -> SdkProfile {
    match requested.filter(|value| !value.is_empty()) {
        Some(profile) => select_from(root, set, profile),
        None => select_from(root, set, &catalog_profile(root, set)),
    }
}

/// The entry a producer of `sdk-v{set}` reports targets in this process.
pub fn requested(set: u8) -> SdkProfile {
    let environment = std::env::var(SEMANTIC_PROFILE_ENV).ok();
    requested_from(&repository_root(), set, environment.as_deref())
}
