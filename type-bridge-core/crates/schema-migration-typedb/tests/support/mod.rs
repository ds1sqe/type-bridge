//! Semantic profile selection for the connected-migration live suites.
//!
//! `TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE` names the workspace profile the
//! suite runs under (default `typedb-3.12.1/v1`). The admitted servers come
//! from that profile's migration pairing, and every live connection is
//! checked against them before a test proceeds.

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_core_lib::version::GeneratedPackageRelease;
use type_bridge_orm::Database;

/// Environment variable naming the live suite's semantic profile.
pub const PROFILE_ENV: &str = "TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE";

/// The semantic profile the live suite runs under.
pub fn live_profile() -> SemanticProfileId {
    let profile = std::env::var(PROFILE_ENV).unwrap_or_else(|_| "typedb-3.12.1/v1".to_owned());
    let profile = SemanticProfileId::new(&profile)
        .unwrap_or_else(|error| panic!("{PROFILE_ENV}={profile:?} is invalid: {error}"));
    live_release_for(&profile);
    profile
}

fn live_release_for(profile: &SemanticProfileId) -> GeneratedPackageRelease {
    GeneratedPackageRelease::for_profile(profile.as_str()).unwrap_or_else(|| {
        panic!(
            "{PROFILE_ENV}={} cannot run connected migrations",
            profile.as_str()
        )
    })
}

/// Require that `database` negotiated a server the live profile pairs with.
pub fn assert_live_server(database: &Database) {
    let profile = live_profile();
    let expected = live_release_for(&profile);
    let server = database
        .server_version()
        .expect("negotiated server version");
    assert!(
        expected.admits(&server),
        "{PROFILE_ENV}={} requires TypeDB {expected} (later patches on the same minor lines), got {server}",
        profile.as_str()
    );
}
