//! Move a database between two TypeDB servers for the live profile-upgrade
//! tests, with the official driver's database export and import.
//!
//! The pinned band-9 driver talks to both the TypeDB 3.12 and 3.13 lines, so
//! one driver exports from the old server and imports into the new one; no
//! TypeDB console is needed.

use std::fs;
use std::path::Path;

use typedb_driver::{Addresses, Credentials, DriverOptions, DriverTlsConfig, TypeDBDriver};

async fn connect(address: &str, credentials: &Credentials) -> TypeDBDriver {
    let addresses = Addresses::try_from_address_str(address)
        .unwrap_or_else(|error| panic!("{address} is not a TypeDB address: {error}"));
    TypeDBDriver::new(
        addresses,
        credentials.clone(),
        DriverOptions::new(DriverTlsConfig::disabled()),
    )
    .await
    .unwrap_or_else(|error| panic!("connect to {address}: {error}"))
}

/// Export database `name` from the server at `from` and import it under the
/// same name into the server at `to`, which must not hold that database yet.
/// Both servers accept `username`/`password` over plaintext; the exported
/// schema and data files are written under `files`.
pub async fn move_database(
    name: &str,
    from: &str,
    to: &str,
    username: &str,
    password: &str,
    files: &Path,
) {
    let credentials = &Credentials::new(username, password);
    let schema_path = files.join(format!("{name}.typeql"));
    let data_path = files.join(format!("{name}.typedb"));

    let source = connect(from, credentials).await;
    source
        .databases()
        .get(name)
        .await
        .unwrap_or_else(|error| panic!("open {name} on {from}: {error}"))
        .export_to_file(&schema_path, &data_path)
        .await
        .unwrap_or_else(|error| panic!("export {name} from {from}: {error}"));

    let schema = fs::read_to_string(&schema_path).expect("read the exported schema");
    connect(to, credentials)
        .await
        .databases()
        .import_from_file(name, schema, &data_path)
        .await
        .unwrap_or_else(|error| panic!("import {name} into {to}: {error}"));
}
