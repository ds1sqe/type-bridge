use typedb_driver::{Addresses, Credentials, DriverOptions, DriverTlsConfig, TypeDBDriver};

pub async fn ensure_database_exists(
    address: &str,
    database: &str,
    username: &str,
    password: &str,
    context: &str,
) {
    let options = DriverOptions::new(DriverTlsConfig::disabled());
    let addresses = Addresses::try_from_address_str(address)
        .unwrap_or_else(|error| panic!("{context}: invalid TypeDB address {address}: {error}"));
    let driver = TypeDBDriver::new(addresses, Credentials::new(username, password), options)
        .await
        .unwrap_or_else(|error| {
            panic!("{context}: failed to connect to TypeDB at {address}: {error}")
        });

    let databases = driver.databases();
    let exists = databases
        .contains(database)
        .await
        .unwrap_or_else(|error| panic!("{context}: database lookup failed: {error}"));
    if !exists {
        databases
            .create(database)
            .await
            .unwrap_or_else(|error| panic!("{context}: database create failed: {error}"));
    }
}
