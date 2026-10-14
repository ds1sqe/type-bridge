//! Semantic profile selection for the connected-migration live suites.
//!
//! `TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE` names the workspace profile the
//! suite runs under (default `typedb-3.12.1/v1`). The expected server comes
//! from that profile's migration pairing, and every live connection is
//! checked against it before a test proceeds.

use type_bridge_contract::fingerprint::SemanticProfileId;
use type_bridge_core_lib::version::Version;
use type_bridge_orm::Database;
use type_bridge_schema_migration_typedb::migration_server_for_profile;

/// Environment variable naming the live suite's semantic profile.
pub const PROFILE_ENV: &str = "TYPE_BRIDGE_MIGRATION_SEMANTIC_PROFILE";

/// The semantic profile the live suite runs under.
pub fn live_profile() -> SemanticProfileId {
    let profile = std::env::var(PROFILE_ENV).unwrap_or_else(|_| "typedb-3.12.1/v1".to_owned());
    let profile = SemanticProfileId::new(&profile)
        .unwrap_or_else(|error| panic!("{PROFILE_ENV}={profile:?} is invalid: {error}"));
    live_server_for(&profile);
    profile
}

fn live_server_for(profile: &SemanticProfileId) -> Version {
    migration_server_for_profile(profile).unwrap_or_else(|| {
        panic!(
            "{PROFILE_ENV}={} cannot run connected migrations",
            profile.as_str()
        )
    })
}

/// Require that `database` negotiated the server the live profile pairs with.
pub fn assert_live_server(database: &Database) {
    let profile = live_profile();
    let expected = live_server_for(&profile);
    assert_eq!(
        database.server_version(),
        Some(expected),
        "{PROFILE_ENV}={} requires exactly TypeDB {expected}",
        profile.as_str()
    );
}
